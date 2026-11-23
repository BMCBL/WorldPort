//! Writes a synthetic Java world with many chunks, to test whether JE->BE
//! memory stays bounded as the world grows.

use std::path::Path;

fn main() {
    let path = std::env::args().nth(1).expect("destination");
    let side: i32 = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(20);
    let path = Path::new(&path);
    worldport_core::fixtures::write_flat_world(path, side).expect("write world");
    println!("wrote {side}x{side} = {} chunks to {}", side * side, path.display());
}
