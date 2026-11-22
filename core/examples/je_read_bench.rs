//! Measures the Java region read path: reopening the region file per chunk
//! (header re-read) versus reusing one open handle per region.

use std::time::Instant;

use worldport_core::je::anvil::RegionFile;
use worldport_core::je::world::JavaWorld;
use worldport_core::je::JeDimension;
use worldport_core::nbt::NbtExt;

fn main() {
    let path = std::env::args().nth(1).expect("java world path");
    let world = JavaWorld::open(&path).expect("open world");
    let positions = world.chunk_positions(JeDimension::Overworld).expect("positions");
    println!("chunks {}", positions.len());

    let started = Instant::now();
    let mut decoded = 0_usize;
    for pos in &positions {
        if let Some(chunk) = world.read_chunk(JeDimension::Overworld, *pos).expect("read") {
            decoded += chunk.as_compound().map_or(0, |c| c.len());
        }
    }
    println!("reopen per chunk: {:?} ({decoded} keys)", started.elapsed());

    // Reuse one handle per region file.
    let started = Instant::now();
    let mut decoded = 0_usize;
    let mut by_region: std::collections::BTreeMap<(i32, i32), Vec<(u8, u8)>> = Default::default();
    for pos in &positions {
        by_region
            .entry(pos.region())
            .or_default()
            .push(pos.local());
    }
    let mut open_count = 0_usize;
    for ((region_x, region_z), locals) in &by_region {
        let file = world
            .region_dir(JeDimension::Overworld)
            .join(format!("r.{region_x}.{region_z}.mca"));
        if !file.is_file() {
            continue;
        }
        let mut region = RegionFile::open(&file, false).expect("open region");
        open_count += 1;
        for (local_x, local_z) in locals {
            if let Some(payload) = region.read_chunk(*local_x, *local_z).expect("read") {
                if let Ok(decoded_tag) = worldport_core::je::nbt::decode(&payload) {
                    decoded += decoded_tag.tag.as_compound().map_or(0, |c| c.len());
                }
            }
        }
    }
    println!(
        "reuse per region: {:?} ({decoded} keys, {open_count} opens)",
        started.elapsed()
    );
}
