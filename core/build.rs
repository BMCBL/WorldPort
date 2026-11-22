//! Bakes the Java block catalog into a compact binary at compile time.
//!
//! The source of truth stays `mappings/data/java/<version>/blocks.json.gz`, but
//! the runtime never parses JSON: this build script reads it once, writes a
//! length-prefixed binary, compresses it with LZ4 (block format, size prefix),
//! and the crate embeds the result with `include_bytes!`. At runtime the blob is
//! decompressed once into memory (see `mapping::schema`).

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let root = manifest.parent().expect("workspace root");
    let source = root.join("mappings/data/java/26.2/blocks.json.gz");
    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed=build.rs");

    let payload = match fs::read(&source) {
        Ok(bytes) => bytes,
        Err(error) => {
            println!(
                "cargo:warning=WorldPort: cannot read {} ({error}); Java catalogs will be unavailable",
                source.display()
            );
            write_placeholder();
            return;
        }
    };
    let mut json = String::new();
    if let Err(error) = flate2::read::GzDecoder::new(payload.as_slice()).read_to_string(&mut json) {
        println!("cargo:warning=WorldPort: gzip decode failed: {error}");
        write_placeholder();
        return;
    }
    let parsed: serde_json::Value = match serde_json::from_str(&json) {
        Ok(value) => value,
        Err(error) => {
            println!("cargo:warning=WorldPort: blocks.json parse failed: {error}");
            write_placeholder();
            return;
        }
    };
    let Some(blocks) = parsed.as_object() else {
        println!("cargo:warning=WorldPort: blocks.json is not an object");
        write_placeholder();
        return;
    };

    let mut binary = Vec::with_capacity(512 * 1024);
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

    let compressed = lz4_flex::block::compress_prepend_size(&binary);
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    fs::write(out_dir.join("java_catalog_26_2.bin.lz4"), &compressed).expect("write baked catalog");
    println!(
        "cargo:warning=WorldPort: baked {} blocks -> {} bytes ({} compressed)",
        blocks.len(),
        binary.len(),
        compressed.len()
    );
}

fn write_str(out: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    out.extend_from_slice(&u16::try_from(bytes.len()).unwrap_or(u16::MAX).to_le_bytes());
    out.extend_from_slice(bytes);
}

fn write_placeholder() {
    // Empty catalog: `WPCB` + zero blocks, still LZ4 framed so the reader path is identical.
    let mut binary = Vec::new();
    binary.extend_from_slice(b"WPCB");
    binary.extend_from_slice(&0_u32.to_le_bytes());
    let compressed = lz4_flex::block::compress_prepend_size(&binary);
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR"));
    let _ = fs::create_dir_all(&out_dir);
    fs::write(out_dir.join("java_catalog_26_2.bin.lz4"), compressed)
        .expect("write placeholder catalog");
    let _ = Path::new("");
}
