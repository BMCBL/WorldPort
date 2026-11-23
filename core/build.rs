//! Bakes every bundled Java block catalog into a compact binary at compile time.
//!
//! `mappings/je_versions.json` lists the supported versions and their
//! DataVersions. For each one this script reads
//! `mappings/data/java/<version>/blocks.json.gz`, writes a length-prefixed
//! binary, compresses it with LZ4 and emits a Rust table that embeds the blobs
//! with `include_bytes!`. The runtime never parses JSON (see `mapping::schema`).

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let root = manifest.parent().expect("workspace root");
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let versions_file = root.join("mappings/je_versions.json");
    println!("cargo:rerun-if-changed={}", versions_file.display());
    println!("cargo:rerun-if-changed=build.rs");

    let versions: BTreeMap<String, i32> = match fs::read_to_string(&versions_file)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
    {
        Some(versions) => versions,
        None => {
            println!(
                "cargo:warning=WorldPort: cannot read {}; Java catalogs will be unavailable",
                versions_file.display()
            );
            BTreeMap::new()
        }
    };

    let mut table = String::from(
        "/// `(version label, DataVersion, baked LZ4 blob)` for every bundled catalog.\n\
         pub const JAVA_CATALOGS: &[(&str, i32, &[u8])] = &[\n",
    );
    let mut baked = 0_usize;
    for (version, data_version) in &versions {
        let source = root
            .join("mappings/data/java")
            .join(version)
            .join("blocks.json.gz");
        println!("cargo:rerun-if-changed={}", source.display());
        let Some(binary) = bake(&source) else {
            continue;
        };
        let compressed = lz4_flex::block::compress_prepend_size(&binary);
        let slug = version.replace('.', "_");
        let file = format!("java_catalog_{slug}.bin.lz4");
        fs::write(out_dir.join(&file), &compressed).expect("write baked catalog");
        table.push_str(&format!(
            "    (\"{version}\", {data_version}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/{file}\"))),\n"
        ));
        baked += 1;
        println!(
            "cargo:warning=WorldPort: baked Java {version} -> {} bytes ({} compressed)",
            binary.len(),
            compressed.len()
        );
    }
    table.push_str("];\n");
    fs::write(out_dir.join("java_catalogs.rs"), table).expect("write catalog table");
    if baked == 0 {
        println!("cargo:warning=WorldPort: no Java block catalogs were baked");
    }

    bake_bedrock(root, &out_dir);
}

fn bake(source: &Path) -> Option<Vec<u8>> {
    let payload = fs::read(source).ok()?;
    let mut json = String::new();
    flate2::read::GzDecoder::new(payload.as_slice())
        .read_to_string(&mut json)
        .ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&json).ok()?;
    let blocks = parsed.as_object()?;
    let mut binary = Vec::with_capacity(256 * 1024);
    binary.extend_from_slice(b"WPCB");
    binary.extend_from_slice(&u32::try_from(blocks.len()).unwrap_or(u32::MAX).to_le_bytes());
    for (name, block) in blocks {
        write_str(&mut binary, name);
        let properties = block
            .get("properties")
            .and_then(serde_json::Value::as_object);
        let Some(properties) = properties else {
            binary.extend_from_slice(&0_u16.to_le_bytes());
            continue;
        };
        binary.extend_from_slice(&u16::try_from(properties.len()).unwrap_or(u16::MAX).to_le_bytes());
        for (property, values) in properties {
            write_str(&mut binary, property);
            let values: Vec<String> = values
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(|value| match value {
                            serde_json::Value::String(text) => Some(text.clone()),
                            serde_json::Value::Bool(flag) => Some(flag.to_string()),
                            serde_json::Value::Number(number) => Some(number.to_string()),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            let kind = if values.len() == 2
                && values.iter().any(|value| value == "true")
                && values.iter().any(|value| value == "false")
            {
                0_u8
            } else if !values.is_empty() && values.iter().all(|value| value.parse::<i32>().is_ok())
            {
                1
            } else {
                2
            };
            binary.push(kind);
            binary.extend_from_slice(&u16::try_from(values.len()).unwrap_or(u16::MAX).to_le_bytes());
            for value in values {
                write_str(&mut binary, &value);
            }
        }
    }
    Some(binary)
}

fn write_str(out: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    out.extend_from_slice(&u16::try_from(bytes.len()).unwrap_or(u16::MAX).to_le_bytes());
    out.extend_from_slice(bytes);
}

/// Bakes every Bedrock catalog into ONE blob with a shared string pool.
///
/// 59 catalogs share almost all of their block, property and value names, so
/// storing each one separately paid for that vocabulary once per catalog and LZ4
/// could not help across blobs. Interning the strings once and referencing them by
/// index makes the whole set a single compressible stream, which is what the size
/// of the embedded data depends on.
///
/// Format (`WPCB` version 2):
///   u16 version, u32 pool size, then each string as u16 length + bytes,
///   u32 catalog count, then per catalog: u32 version index, u32 block count,
///   then per block: u32 name index, u16 property count,
///   then per property: u32 name index, u8 kind, u16 value count,
///   then value indices as u32.
fn bake_bedrock(root: &Path, out_dir: &Path) {
    let manifest = root.join("mappings/bedrock_versions.json");
    println!("cargo:rerun-if-changed={}", manifest.display());
    let versions: BTreeMap<String, String> = match fs::read_to_string(&manifest)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
    {
        Some(versions) => versions,
        None => {
            println!(
                "cargo:warning=WorldPort: cannot read {}; per-version Bedrock catalogs are unavailable",
                manifest.display()
            );
            return;
        }
    };

    let mut catalogs: Vec<(String, Vec<(String, Vec<(String, u8, Vec<String>)>)>)> = Vec::new();
    for (version, relative) in &versions {
        let source = root.join(relative);
        println!("cargo:rerun-if-changed={}", source.display());
        let Some(blocks) = decode_catalog(&source) else {
            continue;
        };
        catalogs.push((version.clone(), blocks));
    }
    if catalogs.is_empty() {
        println!("cargo:warning=WorldPort: no Bedrock block catalogs were baked");
        return;
    }

    // One pass to intern every string the set uses.
    let mut index: BTreeMap<&str, u32> = BTreeMap::new();
    let mut symbols: Vec<&str> = Vec::new();
    for (version, blocks) in &catalogs {
        let mut intern = |value: &'static str| {
            let _ = value;
        };
        let _ = &mut intern;
        for candidate in std::iter::once(version.as_str())
            .chain(blocks.iter().map(|(name, _)| name.as_str()))
            .chain(blocks.iter().flat_map(|(_, properties)| {
                properties.iter().flat_map(|(key, _, values)| {
                    std::iter::once(key.as_str()).chain(values.iter().map(String::as_str))
                })
            }))
        {
            if !index.contains_key(candidate) {
                index.insert(candidate, u32::try_from(symbols.len()).unwrap_or(u32::MAX));
                symbols.push(candidate);
            }
        }
    }
    let symbol = |value: &str| index.get(value).copied().unwrap_or(0);

    let mut out = Vec::with_capacity(4 * 1024 * 1024);
    out.extend_from_slice(b"WPCB");
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&u32::try_from(symbols.len()).unwrap_or(u32::MAX).to_le_bytes());
    for text in &symbols {
        let bytes = text.as_bytes();
        out.extend_from_slice(&u16::try_from(bytes.len()).unwrap_or(u16::MAX).to_le_bytes());
        out.extend_from_slice(bytes);
    }
    out.extend_from_slice(&u32::try_from(catalogs.len()).unwrap_or(u32::MAX).to_le_bytes());
    for (version, blocks) in &catalogs {
        out.extend_from_slice(&symbol(version).to_le_bytes());
        out.extend_from_slice(&u32::try_from(blocks.len()).unwrap_or(u32::MAX).to_le_bytes());
        for (name, properties) in blocks {
            out.extend_from_slice(&symbol(name).to_le_bytes());
            out.extend_from_slice(&u16::try_from(properties.len()).unwrap_or(u16::MAX).to_le_bytes());
            for (key, kind, values) in properties {
                out.extend_from_slice(&symbol(key).to_le_bytes());
                out.push(*kind);
                out.extend_from_slice(&u16::try_from(values.len()).unwrap_or(u16::MAX).to_le_bytes());
                for value in values {
                    out.extend_from_slice(&symbol(value).to_le_bytes());
                }
            }
        }
    }

    let compressed = lz4_flex::block::compress_prepend_size(&out);
    fs::write(out_dir.join("bedrock_catalogs.bin.lz4"), &compressed)
        .expect("write baked bedrock catalogs");
    println!(
        "cargo:warning=WorldPort: baked {} Bedrock catalogs, {} symbols, {} bytes ({} compressed)",
        catalogs.len(),
        symbols.len(),
        out.len(),
        compressed.len()
    );
}

/// Reads one generated catalog file into `(name, [(property, kind, values)])`.
fn decode_catalog(source: &Path) -> Option<Vec<(String, Vec<(String, u8, Vec<String>)>)>> {
    let payload = fs::read(source).ok()?;
    let mut json = String::new();
    flate2::read::GzDecoder::new(payload.as_slice())
        .read_to_string(&mut json)
        .ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&json).ok()?;
    let blocks = parsed.as_object()?;
    let mut out = Vec::with_capacity(blocks.len());
    for (name, block) in blocks {
        let mut properties = Vec::new();
        if let Some(map) = block.get("properties").and_then(serde_json::Value::as_object) {
            for (key, values) in map {
                let values: Vec<String> = values
                    .as_array()
                    .map(|values| {
                        values
                            .iter()
                            .filter_map(|value| match value {
                                serde_json::Value::String(text) => Some(text.clone()),
                                serde_json::Value::Bool(flag) => Some(flag.to_string()),
                                serde_json::Value::Number(number) => Some(number.to_string()),
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let kind = if values.len() == 2
                    && values.iter().any(|value| value == "true")
                    && values.iter().any(|value| value == "false")
                {
                    0_u8
                } else if values.iter().all(|value| value.parse::<i64>().is_ok()) {
                    1
                } else {
                    2
                };
                properties.push((key.clone(), kind, values));
            }
        }
        out.push((name.clone(), properties));
    }
    Some(out)
}
