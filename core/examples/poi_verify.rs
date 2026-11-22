//! Reads a converted world's POI regions and summarizes them, so a conversion
//! can be checked against the source village records.

use std::collections::BTreeMap;

fn main() {
    let dir = std::env::args().nth(1).expect("poi directory");
    let dir = std::path::Path::new(&dir);
    let mut chunks = 0_usize;
    let mut records = 0_usize;
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut tickets: BTreeMap<i32, usize> = BTreeMap::new();
    let mut positions = std::collections::BTreeSet::new();
    let mut duplicates = 0_usize;
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|value| value.to_str()) != Some("mca") {
            continue;
        }
        for (_, _, chunk) in worldport_core::poi::read_region(&path).expect("read region") {
            chunks += 1;
            for (_, record) in chunk.iter() {
                records += 1;
                *kinds.entry(record.kind.clone()).or_default() += 1;
                *tickets.entry(record.free_tickets).or_default() += 1;
                if !positions.insert(record.position) {
                    duplicates += 1;
                }
            }
        }
    }
    println!("chunks {chunks} records {records} duplicate positions {duplicates}");
    for (kind, count) in kinds {
        println!("  {count:5} {kind}");
    }
    println!("free_tickets: {tickets:?}");
    // Per-kind ticket detail for meeting points (the only multi-holder kind).
    let mut meeting_tickets: BTreeMap<i32, usize> = BTreeMap::new();
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|value| value.to_str()) != Some("mca") {
            continue;
        }
        for (_, _, chunk) in worldport_core::poi::read_region(&path).expect("read region") {
            for (_, record) in chunk.iter() {
                if record.kind == "minecraft:meeting" {
                    *meeting_tickets.entry(record.free_tickets).or_default() += 1;
                }
            }
        }
    }
    println!("meeting free_tickets: {meeting_tickets:?}");
}
