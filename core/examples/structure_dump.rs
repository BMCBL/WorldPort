//! Finds Java chunks with generated-structure metadata and prints it.

use worldport_core::je::world::JavaWorld;
use worldport_core::je::JeDimension;
use worldport_core::nbt::NbtExt;

fn main() {
    let path = std::env::args().nth(1).expect("java world path");
    let limit: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(2);
    let world = JavaWorld::open(&path).expect("open world");
    let positions = world.chunk_positions(JeDimension::Overworld).expect("positions");
    println!("chunks {}", positions.len());
    let mut found = 0_usize;
    let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
    for pos in &positions {
        let Ok(Some(chunk)) = world.read_chunk(JeDimension::Overworld, *pos) else {
            continue;
        };
        let Some(structures) = chunk.get("structures") else {
            continue;
        };
        let starts = structures.get("starts").and_then(|value| value.as_compound());
        let references = structures.get("References").and_then(|value| value.as_compound());
        let start_count = starts.map_or(0, |map| map.len());
        let ref_count = references.map_or(0, |map| map.len());
        if start_count == 0 && ref_count == 0 {
            continue;
        }
        if let Some(starts) = starts {
            for (kind, value) in starts {
                *kinds.entry(format!("start:{kind} ({:?})", value.get_str("id"))).or_default() += 1;
            }
        }
        if let Some(references) = references {
            for (kind, value) in references {
                *kinds.entry(format!("ref:{kind}")).or_default() += 1;
            }
        }
        if found < limit {
            println!(
                "--- {} starts={start_count} refs={ref_count} ---",
                pos.label()
            );
            if let Some(starts) = starts {
                for (kind, value) in starts {
                    println!(
                        "{kind}: {}",
                        bedrock_nbt::snbt::to_string(value).unwrap_or_default()
                    );
                }
            }
            if let Some(references) = references {
                println!(
                    "References: {}",
                    bedrock_nbt::snbt::to_string(&bedrock_nbt::NbtTag::Compound(references.clone()))
                        .unwrap_or_default()
                );
            }
            found += 1;
        }
    }
    println!("\nstructure kinds:");
    for (kind, count) in kinds {
        println!("  {count:5} {kind}");
    }
}
