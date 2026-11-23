//! Dumps a catalog as JSON so it can be diffed against a reference source
//! (Chunker's per-version registries) outside the build.
//!
//! `cargo run --example dump_catalog -- bedrock` or a Java version label.

use std::collections::BTreeMap;

use worldport_core::mapping::schema::bedrock_catalog;
use worldport_core::mapping::JavaCatalog;

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "bedrock".to_string());
    let catalog = if which == "bedrock" {
        bedrock_catalog()
    } else {
        JavaCatalog::bundled(&which)
            .unwrap_or_else(|error| panic!("unknown Java version {which}: {error}"))
            .to_catalog()
    };
    let mut out: BTreeMap<&str, BTreeMap<&str, Vec<&str>>> = BTreeMap::new();
    for (name, schema) in catalog.blocks() {
        let properties: BTreeMap<&str, Vec<&str>> = schema
            .properties()
            .iter()
            .map(|(key, property)| {
                (
                    key.as_str(),
                    property.values().iter().map(String::as_str).collect(),
                )
            })
            .collect();
        out.insert(name.as_str(), properties);
    }
    println!("{}", serde_json::to_string(&out).expect("serialize"));
}
