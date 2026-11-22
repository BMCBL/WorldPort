//! Measures Bedrock chunk read/decode throughput (levels: raw record scan vs
//! full subchunk decode) so conversion speed can be attributed correctly.

use std::time::Instant;

use bedrock_world::world::{OpenOptions, World};
use bedrock_world::Dimension;
use rayon::prelude::*;

fn main() {
    let path = std::env::args().nth(1).expect("world path");
    let limit: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(5000);
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
    println!(
        "records only: {:?} ({records} records)",
        started.elapsed()
    );

    let started = Instant::now();
    let decoded: usize = positions
        .iter()
        .map(|pos| {
            let chunk = world.chunk(*pos).expect("read chunk");
            let mut sections = 0;
            for y in -4_i8..=19 {
                if chunk.subchunk(y).expect("decode subchunk").is_some() {
                    sections += 1;
                }
            }
            sections
        })
        .sum();
    println!(
        "sequential decode: {:?} ({decoded} sections)",
        started.elapsed()
    );

    let started = Instant::now();
    let decoded: usize = positions
        .par_iter()
        .map(|pos| {
            let chunk = world.chunk(*pos).expect("read chunk");
            let mut sections = 0;
            for y in -4_i8..=19 {
                if chunk.subchunk(y).expect("decode subchunk").is_some() {
                    sections += 1;
                }
            }
            sections
        })
        .sum();
    println!(
        "parallel decode: {:?} ({decoded} sections)",
        started.elapsed()
    );
}
