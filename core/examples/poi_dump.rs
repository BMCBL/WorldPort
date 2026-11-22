//! Dumps Java POI region chunks as SNBT so the record format can be calibrated
//! against a real world.

use worldport_core::je::anvil::RegionFile;
use worldport_core::je::nbt as je_nbt;
use worldport_core::nbt::NbtExt;

fn main() {
    let path = std::env::args().nth(1).expect("poi region path");
    // Exercise the shipped reader first so the dump validates real code.
    match worldport_core::poi::read_region(std::path::Path::new(&path)) {
        Ok(chunks) => {
            let records: usize = chunks.iter().map(|(_, _, chunk)| chunk.len()).sum();
            let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
            let mut tickets: std::collections::BTreeMap<String, usize> = Default::default();
            for (_, _, chunk) in &chunks {
                for (_, record) in chunk.iter() {
                    *kinds.entry(record.kind.clone()).or_default() += 1;
                    *tickets.entry(record.free_tickets.to_string()).or_default() += 1;
                }
            }
            println!("poi::read_region: {} chunks, {records} records", chunks.len());
            for (kind, count) in &kinds {
                println!("  {count:5} {kind}");
            }
            println!("  free_tickets: {tickets:?}");
        }
        Err(error) => println!("poi::read_region failed: {error}"),
    }
    let limit: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(3);
    let mut region = RegionFile::open(std::path::Path::new(&path), false).expect("open region");
    let slots = region.occupied_slots();
    println!("{path}: {} chunks", slots.len());
    let mut shown = 0;
    let mut total_records = 0_usize;
    let mut types: std::collections::BTreeMap<String, usize> = Default::default();
    for (local_x, local_z) in slots {
        let Some(payload) = region.read_chunk(local_x, local_z).expect("read") else {
            continue;
        };
        let Ok(decoded) = je_nbt::decode(&payload) else {
            continue;
        };
        // Java stores records per section: `Sections.<y>.Records[]`.
        if let Some(sections) = decoded.tag.get("Sections").and_then(|value| value.as_compound()) {
            for (section_y, section) in sections {
                let Some(records) = section.get_list("Records") else {
                    continue;
                };
                total_records += records.len();
                for record in records {
                    let kind = record.get_str("type").unwrap_or("?");
                    let tickets = record
                        .get_int("free_tickets")
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "-".to_string());
                    *types
                        .entry(format!("{kind} free_tickets={tickets} (section {section_y})"))
                        .or_default() += 1;
                }
            }
        }
        if shown < limit {
            println!(
                "--- chunk {local_x},{local_z} ---\n{}",
                bedrock_nbt::snbt::to_string(&decoded.tag).expect("snbt")
            );
            shown += 1;
        }
    }
    println!("\nrecords {total_records}");
    for (kind, count) in types {
        println!("  {count:6} {kind}");
    }
}
