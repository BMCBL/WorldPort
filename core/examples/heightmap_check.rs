//! Decodes a real Java chunk's heightmaps and compares them with the blocks in
//! the same chunk, to establish the encoding the converter must reproduce.

use worldport_core::je::world::JavaWorld;
use worldport_core::je::JeDimension;
use bedrock_nbt::NbtTag;
use worldport_core::nbt::NbtExt;

fn main() {
    let path = std::env::args().nth(1).expect("java world path");
    let world = JavaWorld::open(&path).expect("open world");
    let positions = world.chunk_positions(JeDimension::Overworld).expect("positions");
    for pos in positions.iter().take(400) {
        let Ok(Some(chunk)) = world.read_chunk(JeDimension::Overworld, *pos) else {
            continue;
        };
        let Some(hm) = chunk.get("Heightmaps") else { continue };
        let Some(NbtTag::LongArray(words)) = hm.get("WORLD_SURFACE") else { continue };
        if words.is_empty() {
            continue;
        }
        // 9 bits per entry, 7 per long, z*16+x order.
        let mut heights = Vec::with_capacity(256);
        for index in 0..256usize {
            let word = words[index / 7] as u64;
            let shift = (index % 7) as u32 * 9;
            heights.push(((word >> shift) & 0x1FF) as i32);
        }
        let max = heights.iter().copied().max().unwrap_or(0);
        let min = heights.iter().copied().min().unwrap_or(0);
        // What the terrain actually reaches in this chunk.
        let mut top_block = i32::MIN;
        if let Some(sections) = chunk.get_list("sections") {
            for section in sections {
                let Some(y) = section.get_int("Y") else { continue };
                let Some(states) = section.get("block_states") else { continue };
                let Some(palette) = states.get_list("palette") else { continue };
                let has_solid = palette.iter().any(|entry| {
                    entry
                        .get_str("Name")
                        .is_some_and(|name| name != "minecraft:air" && name != "minecraft:cave_air")
                });
                if has_solid {
                    top_block = top_block.max((y as i32) * 16 + 15);
                }
            }
        }
        // Decode one column's real top block and compare with the encoded value.
        let column = 0usize; // z=0,x=0
        let mut real_top: Option<i32> = None;
        if let Some(sections) = chunk.get_list("sections") {
            for section in sections {
                let Some(y) = section.get_int("Y") else { continue };
                let Some(states) = section.get("block_states") else { continue };
                let Some(palette) = states.get_list("palette") else { continue };
                let solid: Vec<bool> = palette
                    .iter()
                    .map(|entry| {
                        entry.get_str("Name").is_some_and(|name| {
                            name != "minecraft:air" && name != "minecraft:cave_air"
                        })
                    })
                    .collect();
                // Single-entry palettes have no data array: uniform section.
                let indices: Vec<u16> = match states.get_i64_array("data") {
                    Some(data) => {
                        let bits = {
                            let needed = usize::BITS - palette.len().saturating_sub(1).leading_zeros();
                            needed.max(4)
                        };
                        let per_long = (64 / bits) as usize;
                        (0..4096)
                            .map(|i| {
                                let word = data.get(i / per_long).copied().unwrap_or(0) as u64;
                                ((word >> ((i % per_long) as u32 * bits)) & ((1 << bits) - 1)) as u16
                            })
                            .collect()
                    }
                    None => vec![0; 4096],
                };
                for local_y in 0..16usize {
                    let idx = (local_y << 8) | (0 << 4) | 0;
                    let p = *indices.get(idx).unwrap_or(&0) as usize;
                    if solid.get(p).copied().unwrap_or(false) {
                        real_top = Some((y as i32) * 16 + local_y as i32);
                    }
                }
            }
        }
        println!(
            "chunk {:?}: encoded heightmap {max} (min {min}) | real top at x0,z0 = {real_top:?}",
            (pos.x, pos.z)
        );
        println!(
            "  if min_y=-64 then encoded = real+1+64: {:?}",
            real_top.map(|t| t + 1 + 64)
        );
        return;
    }
}
