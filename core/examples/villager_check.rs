//! Counts converted villagers and their professions, to verify the SkinID
//! mapping against the source save.

use std::collections::BTreeMap;

use worldport_core::je::JeDimension;
use worldport_core::je::world::JavaWorld;
use worldport_core::nbt::NbtExt;

fn main() {
    let path = std::env::args().nth(1).expect("java world path");
    let world = JavaWorld::open(&path).expect("open world");
    let positions = world.chunk_positions(JeDimension::Overworld).expect("positions");
    let mut villagers = 0_usize;
    let mut professions: BTreeMap<String, usize> = BTreeMap::new();
    let mut types: BTreeMap<String, usize> = BTreeMap::new();
    let mut levels: BTreeMap<i64, usize> = BTreeMap::new();
    let mut missing_ids: BTreeMap<String, usize> = BTreeMap::new();
    for pos in &positions {
        let Ok(entities) = world.read_entity_chunk(JeDimension::Overworld, *pos) else {
            continue;
        };
        for entity in &entities {
            // `villager_golem` and `zombie_villager` also carry "villager" in
            // their id but are not plain villagers.
            let is_villager = entity.get_str("id").is_some_and(|id| {
                matches!(id, "minecraft:villager" | "minecraft:wandering_trader")
            });
            if !is_villager {
                continue;
            }
            villagers += 1;
            if let Some(data) = entity.get("VillagerData") {
                *professions
                    .entry(data.get_str("profession").unwrap_or("?").to_string())
                    .or_default() += 1;
                *types
                    .entry(data.get_str("type").unwrap_or("?").to_string())
                    .or_default() += 1;
                *levels.entry(data.get_int("level").unwrap_or(0)).or_default() += 1;
            } else {
                *professions.entry("(no VillagerData)".to_string()).or_default() += 1;
                *missing_ids
                    .entry(entity.get_str("id").unwrap_or("?").to_string())
                    .or_default() += 1;
            }
        }
    }
    println!("villagers {villagers}");
    println!("professions: {professions:?}");
    println!("types: {types:?}");
    println!("levels: {levels:?}");
    println!("missing VillagerData by id: {missing_ids:?}");
}
