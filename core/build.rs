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
