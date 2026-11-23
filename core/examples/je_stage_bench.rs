//! Breaks down the Java chunk read path: region read, zlib inflate, NBT parse,
//! palette unpack and block mapping, so a library swap can be aimed at the part
//! that actually costs time.
//!
//! Run with a Java world path and a chunk count.

use std::time::Instant;

use worldport_core::je::anvil::RegionFile;
use worldport_core::je::nbt as je_nbt;
use worldport_core::je::world::JavaWorld;
use worldport_core::je::JeDimension;
use worldport_core::nbt::NbtExt;

fn main() {
    let path = std::env::args().nth(1).expect("java world path");
    let limit: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(2000);
    let world = JavaWorld::open(&path).expect("open world");
    let mut positions = world
        .chunk_positions(JeDimension::Overworld)
        .expect("positions");
    positions.truncate(limit);
    let count = positions.len();
    println!("chunks {count}");

    // Group by region so the file handle is opened once, matching the converter.
    let mut by_region: std::collections::BTreeMap<(i32, i32), Vec<(u8, u8)>> = Default::default();
    for pos in &positions {
        by_region.entry(pos.region()).or_default().push(pos.local());
    }

    let mut payloads: Vec<Vec<u8>> = Vec::with_capacity(count);
    let started = Instant::now();
    for ((region_x, region_z), locals) in &by_region {
        let file = world
            .region_dir(JeDimension::Overworld)
            .join(format!("r.{region_x}.{region_z}.mca"));
        let mut region = RegionFile::open_read_only(&file).expect("open region");
        for (local_x, local_z) in locals {
            if let Some(payload) = region.read_chunk(*local_x, *local_z).expect("read") {
                payloads.push(payload);
            }
        }
    }
    let region_read = started.elapsed();
    let bytes: usize = payloads.iter().map(Vec::len).sum();
    println!(
        "region read + inflate: {:?} ({:.3} ms/chunk, {} MiB)",
        region_read,
        region_read.as_secs_f64() * 1000.0 / count as f64,
        bytes / (1024 * 1024)
    );

    let started = Instant::now();
    let mut decoded = Vec::with_capacity(payloads.len());
    for payload in &payloads {
        if let Ok(named) = je_nbt::decode(payload) {
            decoded.push(named.tag);
        }
    }
    let nbt_parse = started.elapsed();
    println!(
        "nbt parse: {:?} ({:.3} ms/chunk, {} chunks)",
        nbt_parse,
        nbt_parse.as_secs_f64() * 1000.0 / count as f64,
        decoded.len()
    );

    // Palette unpack + mapping over every section of every chunk.
    let started = Instant::now();
    let mut sections_seen = 0_usize;
    let mut blocks_seen = 0_u64;
    for chunk in &decoded {
        let Some(list) = chunk.get_list("sections") else {
            continue;
        };
        for section in list {
            let Some(states) = section.get("block_states") else {
                continue;
            };
            let Some(palette) = states.get_list("palette") else {
                continue;
            };
            sections_seen += 1;
            let data = states.get_i64_array("data");
            let bits = {
                let needed = usize::BITS - palette.len().saturating_sub(1).leading_zeros();
                needed.max(4)
            };
            let per_long = (64 / bits) as usize;
            let mask = (1_u64 << bits) - 1;
            let mut unpacked = vec![0_u16; 4096];
            for (word_index, word) in data.unwrap_or(&[]).iter().enumerate() {
                let base = word_index * per_long;
                let word = *word as u64;
                for lane in 0..per_long {
                    let Some(slot) = unpacked.get_mut(base + lane) else {
                        break;
                    };
                    *slot = ((word >> (lane as u32 * bits)) & mask) as u16;
                }
            }
            blocks_seen += unpacked.iter().filter(|value| **value != 0).count() as u64;
        }
    }
    let unpack = started.elapsed();
    println!(
        "palette unpack: {:?} ({:.3} ms/chunk, {sections_seen} sections, {blocks_seen} blocks)",
        unpack,
        unpack.as_secs_f64() * 1000.0 / count as f64
    );

    let total = region_read + nbt_parse + unpack;
    println!(
        "\ntotal {:.3} ms/chunk | read {:.0}% nbt {:.0}% unpack {:.0}%",
        total.as_secs_f64() * 1000.0 / count as f64,
        region_read.as_secs_f64() * 100.0 / total.as_secs_f64(),
        nbt_parse.as_secs_f64() * 100.0 / total.as_secs_f64(),
        unpack.as_secs_f64() * 100.0 / total.as_secs_f64(),
    );
}
