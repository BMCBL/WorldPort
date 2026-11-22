//! Compares per-chunk batched reads against grouping several chunks into one
//! `get_many` call, which is what a multi-chunk snapshot would save.

use std::time::Instant;

use bedrock_world::world::{OpenOptions, World};
use bedrock_world::{ChunkKey, ChunkPos, ChunkRecordTag, Dimension};
use bytes::Bytes;

const TAGS: [ChunkRecordTag; 13] = [
    ChunkRecordTag::Version,
    ChunkRecordTag::VersionOld,
    ChunkRecordTag::Data3D,
    ChunkRecordTag::Data2D,
    ChunkRecordTag::Data2DLegacy,
    ChunkRecordTag::LegacyTerrain,
    ChunkRecordTag::FinalizedState,
    ChunkRecordTag::BlockEntity,
    ChunkRecordTag::Entity,
    ChunkRecordTag::PendingTicks,
    ChunkRecordTag::BlockExtraData,
    ChunkRecordTag::BiomeState,
    ChunkRecordTag::HardcodedSpawners,
];

fn keys_for(pos: ChunkPos, out: &mut Vec<Bytes>) {
    for tag in TAGS {
        out.push(ChunkKey::new(pos, tag).encode());
    }
    for offset in 0..24_i8 {
        out.push(ChunkKey::subchunk(pos, -4 + offset).encode());
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("world path");
    let limit: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(4000);
    let mut open = OpenOptions::default();
    open.read_only = true;
    let world = World::open(&path, open).expect("open world");
    let mut positions: Vec<_> = world
        .chunk_positions(Default::default())
        .expect("list")
        .into_iter()
        .filter(|pos| pos.dimension == Dimension::Overworld)
        .collect();
    positions.sort_by_key(|pos| (pos.x, pos.z));
    positions.truncate(limit);
    println!("chunks {}", positions.len());

    let started = Instant::now();
    let mut hits = 0_usize;
    for pos in &positions {
        let mut keys = Vec::with_capacity(37);
        keys_for(*pos, &mut keys);
        for value in world.storage().get_many(&keys).expect("get_many") {
            if value.is_some() {
                hits += 1;
            }
        }
    }
    println!("per chunk: {:?} ({hits} records)", started.elapsed());

    for batch in [8usize, 16, 32] {
        let started = Instant::now();
        let mut hits = 0_usize;
        for window in positions.chunks(batch) {
            let mut keys = Vec::with_capacity(window.len() * 37);
            for pos in window {
                keys_for(*pos, &mut keys);
            }
            for value in world.storage().get_many(&keys).expect("get_many") {
                if value.is_some() {
                    hits += 1;
                }
            }
        }
        println!("batch {batch:>2}: {:?} ({hits} records)", started.elapsed());
    }
}
