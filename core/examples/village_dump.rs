//! Lists Bedrock village records and their NBT, to calibrate the village model.

use bedrock_world::village::VillageRecordKind;
use bedrock_world::world::{OpenOptions, World};
use worldport_core::nbt::NbtExt;

fn main() {
    let path = std::env::args().nth(1).expect("bedrock world path");
    let limit: usize = std::env::args()
        .nth(2)
        .and_then(|value| value.parse().ok())
        .unwrap_or(2);
    let mut open = OpenOptions::default();
    open.read_only = true;
    let world = World::open(&path, open).expect("open world");
    let villages = world.villages(Default::default()).expect("scan villages");
    println!("{} village records", villages.len());
    let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
    for entry in &villages {
        *kinds
            .entry(format!("{:?}", entry.key.kind))
            .or_default() += 1;
    }
    for (kind, count) in kinds {
        println!("  {count:4} {kind}");
    }
    // Distinct POI instance shapes across every village.
    let mut shapes: std::collections::BTreeMap<String, usize> = Default::default();
    for entry in &villages {
        if entry.key.kind != VillageRecordKind::Poi {
            continue;
        }
        for root in &entry.roots {
            let Some(list) = root.get_list("POI") else { continue };
            for poi in list {
                let Some(instances) = poi.get_list("instances") else { continue };
                for instance in instances {
                    if instance.get_bool("Skip").unwrap_or(false) {
                        *shapes.entry("(skipped)".to_string()).or_default() += 1;
                        continue;
                    }
                    let name = instance.get_str("Name").unwrap_or("?");
                    let kind = instance
                        .get("Type")
                        .map(|tag| format!("{tag:?}"))
                        .unwrap_or_else(|| "?".to_string());
                    let init = instance.get_str("InitEvent").unwrap_or("");
                    let sound = instance.get_str("SoundEvent").unwrap_or("");
                    *shapes
                        .entry(format!("Name={name} Type={kind} Init={init} Sound={sound}"))
                        .or_default() += 1;
                }
            }
        }
    }
    println!("
POI instance shapes:");
    for (shape, count) in shapes {
        println!("  {count:6} {shape}");
    }
    // Exercise the shared POI model against real records.
    let mut model_kinds: std::collections::BTreeMap<String, usize> = Default::default();
    let mut java_types: std::collections::BTreeMap<String, usize> = Default::default();
    let mut unmapped = 0_usize;
    let mut live = 0_usize;
    for entry in &villages {
        if entry.key.kind != VillageRecordKind::Poi {
            continue;
        }
        for root in &entry.roots {
            let Ok(villagers) = bedrock_world::village::parse_poi_record(root) else {
                continue;
            };
            for villager in &villagers {
                for instance in villager.live() {
                    live += 1;
                    *model_kinds
                        .entry(format!("{:?}", instance.kind))
                        .or_default() += 1;
                    match instance.kind.and_then(|kind| {
                        bedrock_world::village::java_interest_type(&instance.name, kind)
                    }) {
                        Some(name) => *java_types.entry(name.to_string()).or_default() += 1,
                        None => unmapped += 1,
                    }
                }
            }
        }
    }
    println!("
POI model: {live} live instances, {unmapped} unmapped");
    for (kind, count) in model_kinds {
        println!("  {count:6} {kind}");
    }
    for (name, count) in java_types {
        println!("  {count:6} -> {name}");
    }
    for entry in villages.iter().take(limit) {
        println!("--- {} ---", entry.key.raw);
        for root in &entry.roots {
            println!("{}", bedrock_nbt::snbt::to_string(root).unwrap_or_default());
        }
    }
}
