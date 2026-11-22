//! Compares per-chunk read strategies: prefix scan (`World::chunk`) versus
//! point lookups for the known record keys.

use std::time::Instant;

use bedrock_world::world::{OpenOptions, World};
use bedrock_world::{ChunkKey, ChunkPos, ChunkRecordTag, Dimension};
use bytes::Bytes;

fn keys_for(pos: ChunkPos) -> Vec<Bytes> {
    let mut keys = Vec::with_capacity(32);
    for tag in [
        ChunkRecordTag::Version,
        ChunkRecordTag::VersionOld,
        ChunkRecordTag::Data3D,
        ChunkRecordTag::Data2D,
        ChunkRecordTag::Data2DLegacy,
        ChunkRecordTag::LegacyTerrain,
        ChunkRecordTag::FinalizedState,
        ChunkRecordTag::BlockEntity,
        ChunkRecordTag::PendingTicks,
    ] {
        keys.push(ChunkKey::new(pos, tag).encode());
    }
    for y in -4_i8..=19 {
        keys.push(ChunkKey::subchunk(pos, y).encode());
    }
    keys
}

fn main() {
    let path = std::env::args().nth(1).expect("world path");
    let limit: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(1000);
    let mut open = OpenOptions::default();
    open.read_only = true;
    let world = World::open(&path, open).expect("open world");
    let mut positions: Vec<_> = world
        .chunk_positions(Default::default())
        .expect("list chunks")
        .into_iter()
        .filter(|pos| pos.dimension == Dimension::Overworld)
        .collect();
    positions.sort_by_key(|pos| (pos.x, pos.z));
    positions.truncate(limit);
    println!("chunks {}", positions.len());

    let started = Instant::now();
    let mut records = 0_usize;
    for pos in &positions {
        records += world.chunk(*pos).expect("read chunk").records.len();
    }
    println!("prefix scan: {:?} ({records} records)", started.elapsed());

    let started = Instant::now();
    let mut hits = 0_usize;
    for pos in &positions {
        for key in keys_for(*pos) {
            if world.storage().get(&key).expect("get").is_some() {
                hits += 1;
            }
        }
    }
    println!("single gets: {:?} ({hits} hits)", started.elapsed());

    let started = Instant::now();
    let mut hits = 0_usize;
    for pos in &positions {
        let keys = keys_for(*pos);
        for value in world.storage().get_many(&keys).expect("get_many") {
            if value.is_some() {
                hits += 1;
            }
        }
    }
    println!("batched gets: {:?} ({hits} hits)", started.elapsed());

    let started = Instant::now();
    let mut hits = 0_usize;
    for pos in &positions {
        let keys = keys_for(*pos);
        for value in world
            .storage()
            .get_many_ordered(&keys, Default::default())
            .expect("get_many_ordered")
        {
            if value.is_some() {
                hits += 1;
            }
        }
    }
    println!("ordered gets: {:?} ({hits} hits)", started.elapsed());
}
