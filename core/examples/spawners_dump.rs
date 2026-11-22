//! Dumps Bedrock `HardcodedSpawners` records, the only structure-like metadata a
//! Bedrock chunk carries.

use bedrock_world::world::{OpenOptions, World};
use bedrock_world::{ChunkRecordTag, Dimension};

fn main() {
    let path = std::env::args().nth(1).expect("bedrock world path");
    let limit: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(2);
    let mut open = OpenOptions::default();
    open.read_only = true;
    let world = World::open(&path, open).expect("open world");
    let positions: Vec<_> = world
        .chunk_positions(Default::default())
        .expect("list")
        .into_iter()
        .filter(|pos| pos.dimension == Dimension::Overworld)
        .collect();
    println!("chunks {}", positions.len());
    let mut shown = 0;
    let mut total = 0_usize;
    let mut key_names: std::collections::BTreeMap<String, usize> = Default::default();
    for pos in &positions {
        let chunk = world.chunk(*pos).expect("chunk");
        let Some(record) = chunk
            .records
            .iter()
            .find(|record| record.key.tag == ChunkRecordTag::HardcodedSpawners)
        else {
            continue;
        };
        total += 1;
        match bedrock_world::scan::decode_hardcoded_spawn_areas(&record.value) {
            Ok(areas) => {
                for area in &areas {
                    *key_names.entry(format!("{:?}", area.kind)).or_default() += 1;
                }
                if shown < limit {
                    println!("--- {},{} ---", pos.x, pos.z);
                    for area in &areas {
                        println!("  {:?} {:?}..{:?}", area.kind, area.min, area.max);
                    }
                    shown += 1;
                }
            }
            Err(error) => {
                *key_names.entry(format!("decode error: {error}")).or_default() += 1;
            }
        }
    }
    println!("\n{total} chunks with HardcodedSpawners");
    println!("keys: {key_names:?}");
}
