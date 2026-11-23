//! Legacy Bedrock block families.
//!
//! Worlds older than the flattened block ids store variants in a single id plus
//! a type property (`planks` + `wood_type`, `monster_egg` + `monster_egg_stone_type`,
//! ...). These rules map them onto the modern Java names.

use super::{
    MappedState, bool_str, button_to_java, cardinal_legacy_from_number, namespaced,
    weirdo_direction_from_number,
};
use std::collections::BTreeMap;

/// Wood value used by the legacy `planks`/`log`/`leaves` families.
fn legacy_wood(value: &str) -> Option<&'static str> {
    match value {
        "oak" => Some("oak"),
        "spruce" => Some("spruce"),
        "birch" => Some("birch"),
        "jungle" => Some("jungle"),
        "acacia" => Some("acacia"),
        "dark_oak" | "big_oak" => Some("dark_oak"),
        _ => None,
    }
}

/// Maps a legacy Bedrock id to a Java state, or `None` when the id is not a
/// legacy family.
pub(super) fn legacy_be_family(
    short: &str,
    properties: &BTreeMap<String, String>,
) -> Option<MappedState> {
    let get = |key: &str| properties.get(key).map(String::as_str);
    let wood = || {
        get("wood_type")
            .or_else(|| get("old_log_type"))
            .or_else(|| get("new_log_type"))
            .or_else(|| get("old_leaf_type"))
            .or_else(|| get("new_leaf_type"))
            .or_else(|| get("sapling_type"))
            .and_then(legacy_wood)
    };
    let half = || {
        if get("upper_block_bit") == Some("1") || get("minecraft:vertical_half") == Some("top") {
            "upper"
        } else {
            "lower"
        }
    };
    match short {
        "planks" => wood().map(|wood| MappedState::new(namespaced(&format!("{wood}_planks")))),
        "wood" => wood().map(|wood| MappedState::new(namespaced(&format!("{wood}_wood")))),
        "log" | "log2" => {
            let mut mapped = MappedState::new(namespaced(&format!("{}_log", wood()?)));
            if let Some(axis) = get("pillar_axis") {
                mapped = mapped.with("axis", axis);
            }
            Some(mapped)
        }
        "leaves" | "leaves2" => {
            let mut mapped = MappedState::new(namespaced(&format!("{}_leaves", wood()?)));
            let persistent =
                get("persistent_bit") == Some("1") || get("persistent_bit") == Some("true");
            mapped = mapped.with("persistent", bool_str(persistent));
            // Bedrock has no decay distance; 1 keeps converted leaves stable.
            mapped = mapped.with("distance", "1");
            mapped = mapped.with("waterlogged", "false");
            Some(mapped.lossy())
        }
        "sapling" => wood().map(|wood| MappedState::new(namespaced(&format!("{wood}_sapling")))),
        "wooden_slab" => {
            let mut mapped = MappedState::new(namespaced(&format!("{}_slab", wood()?)));
            mapped = mapped.with(
                "type",
                if get("minecraft:vertical_half") == Some("top") || get("top_slot_bit") == Some("1")
                {
                    "top"
                } else {
                    "bottom"
                },
            );
            Some(mapped)
        }
        "double_wooden_slab" => Some(
            MappedState::new(namespaced(&format!("{}_slab", wood()?))).with("type", "double"),
        ),
        "wooden_stairs" => {
            let mut mapped = MappedState::new(namespaced(&format!("{}_stairs", wood()?)));
            if let Some(facing) = get("weirdo_direction").and_then(weirdo_direction_from_number) {
                mapped = mapped.with("facing", facing);
            }
            mapped = mapped.with(
                "half",
                if get("upside_down_bit") == Some("1") {
                    "top"
                } else {
                    "bottom"
                },
            );
            mapped = mapped.with("shape", "straight");
            mapped = mapped.with("waterlogged", "false");
            Some(mapped.lossy())
        }
        "fence" => wood().map(|wood| MappedState::new(namespaced(&format!("{wood}_fence")))),
        "fence_gate" if get("wood_type").is_some() => {
            let mut mapped = MappedState::new(namespaced(&format!("{}_fence_gate", wood()?)));
            if let Some(cardinal) = get("minecraft:cardinal_direction") {
                mapped = mapped.with("facing", cardinal);
            } else if let Some(facing) = get("direction").and_then(cardinal_legacy_from_number) {
                mapped = mapped.with("facing", facing);
            }
            mapped = mapped.with("in_wall", bool_str(get("in_wall_bit") == Some("1")));
            mapped = mapped.with("open", bool_str(get("open_bit") == Some("1")));
            mapped = mapped.with("powered", "false");
            Some(mapped.lossy())
        }
        "trapdoor" if get("wood_type").is_some() => {
            let mut mapped = MappedState::new(namespaced(&format!("{}_trapdoor", wood()?)));
            if let Some(facing) = get("direction").and_then(weirdo_direction_from_number) {
                mapped = mapped.with("facing", facing);
            }
            mapped = mapped.with(
                "half",
                if get("upside_down_bit") == Some("1") {
                    "top"
                } else {
                    "bottom"
                },
            );
            mapped = mapped.with("open", bool_str(get("open_bit") == Some("1")));
            mapped = mapped.with("powered", "false");
            Some(mapped.lossy())
        }
        "wooden_button" if get("wood_type").is_some() => {
            let mut mapped = MappedState::new(namespaced(&format!("{}_button", wood()?)));
            button_to_java(
                &mut mapped,
                get("facing_direction"),
                get("button_pressed_bit"),
            );
            Some(mapped)
        }
        "wooden_pressure_plate" if get("wood_type").is_some() => {
            let mut mapped = MappedState::new(namespaced(&format!("{}_pressure_plate", wood()?)));
            mapped = mapped.with(
                "powered",
                bool_str(get("redstone_signal").is_some_and(|value| value != "0")),
            );
            Some(mapped)
        }
        "stone" => {
            let target = match get("stone_type") {
                Some("granite") => "granite",
                Some("granite_smooth") => "polished_granite",
                Some("diorite") => "diorite",
                Some("diorite_smooth") => "polished_diorite",
                Some("andesite") => "andesite",
                Some("andesite_smooth") => "polished_andesite",
                _ => "stone",
            };
            Some(MappedState::new(namespaced(target)))
        }
        "stonebrick" => {
            let target = match get("stone_brick_type") {
                Some("mossy") => "mossy_stone_bricks",
                Some("cracked") => "cracked_stone_bricks",
                Some("chiseled") => "chiseled_stone_bricks",
                Some("smooth") => "smooth_stone",
                _ => "stone_bricks",
            };
            Some(MappedState::new(namespaced(target)))
        }
        "sand" => Some(MappedState::new(namespaced(match get("sand_type") {
            Some("red") => "red_sand",
            _ => "sand",
        }))),
        "dirt" => Some(MappedState::new(namespaced(match get("dirt_type") {
            Some("coarse") => "coarse_dirt",
            _ => "dirt",
        }))),
        "prismarine" => Some(MappedState::new(namespaced(
            match get("prismarine_block_type") {
                Some("dark") => "dark_prismarine",
                Some("bricks") => "prismarine_bricks",
                _ => "prismarine",
            },
        ))),
        "quartz_block" => {
            let mut mapped = MappedState::new(namespaced(match get("chisel_type") {
                Some("chiseled") => "chiseled_quartz_block",
                Some("lines") => "quartz_pillar",
                Some("smooth") => "smooth_quartz",
                _ => "quartz_block",
            }));
            if let Some(axis) = get("pillar_axis") {
                mapped = mapped.with("axis", axis);
            }
            Some(mapped)
        }
        "sandstone" | "red_sandstone" => {
            let base = if short == "sandstone" {
                "sandstone"
            } else {
                "red_sandstone"
            };
            let target = match get("sand_stone_type") {
                Some("chiseled") => format!("chiseled_{base}"),
                Some("cut") => format!("cut_{base}"),
                Some("smooth") => format!("smooth_{base}"),
                _ => base.to_string(),
            };
            Some(MappedState::new(namespaced(&target)))
        }
        "monster_egg" => {
            let target = match get("monster_egg_stone_type") {
                Some("cobblestone") => "infested_cobblestone",
                Some("stone_brick") => "infested_stone_bricks",
                Some("mossy_stone_brick") => "infested_mossy_stone_bricks",
                Some("cracked_stone_brick") => "infested_cracked_stone_bricks",
                Some("chiseled_stone_brick") => "infested_chiseled_stone_bricks",
                Some("deepslate") => "infested_deepslate",
                _ => "infested_stone",
            };
            Some(MappedState::new(namespaced(target)))
        }
        "grass" => Some(MappedState::new("minecraft:grass_block").with("snowy", "false")),
        "tallgrass" => {
            let target = match get("tall_grass_type") {
                Some("fern") => "fern",
                Some("tall") => "tall_grass",
                _ => "short_grass",
            };
            let mut mapped = MappedState::new(namespaced(target));
            if target == "tall_grass" {
                mapped = mapped.with("half", "lower");
            }
            Some(mapped)
        }
        "double_plant" => {
            let target = match get("double_plant_type") {
                Some("sunflower") => "sunflower",
                Some("syringa") => "lilac",
                Some("grass") => "tall_grass",
                Some("fern") => "large_fern",
                Some("rose") => "rose_bush",
                Some("paeonia") => "peony",
                _ => "tall_grass",
            };
            Some(MappedState::new(namespaced(target)).with("half", half()))
        }
        "yellow_flower" => Some(MappedState::new("minecraft:dandelion")),
        "red_flower" => {
            let target = match get("flower_type") {
                Some("blue_orchid") => "blue_orchid",
                Some("allium") => "allium",
                Some("houstonia") => "azure_bluet",
                Some("tulip_red") => "red_tulip",
                Some("tulip_orange") => "orange_tulip",
                Some("tulip_white") => "white_tulip",
                Some("tulip_pink") => "pink_tulip",
                Some("oxeye") => "oxeye_daisy",
                Some("cornflower") => "cornflower",
                Some("lily_of_the_valley") => "lily_of_the_valley",
                _ => "poppy",
            };
            Some(MappedState::new(namespaced(target)))
        }
        "wool" | "carpet" | "concrete" | "concretePowder" | "stained_glass"
        | "stained_glass_pane" | "stained_hardened_clay" | "shulker_box" => {
            let color = get("color")?;
            let suffix = match short {
                "wool" => "wool",
                "carpet" => "carpet",
                "concrete" => "concrete",
                "concretePowder" => "concrete_powder",
                "stained_glass" => "stained_glass",
                "stained_glass_pane" => "stained_glass_pane",
                "stained_hardened_clay" => "terracotta",
                _ => "shulker_box",
            };
            Some(MappedState::new(namespaced(&format!("{color}_{suffix}"))))
        }
        // Bedrock keeps `undyed_shulker_box`; Java's plain name is `shulker_box`.
        // The reverse rule has to exist too, or the state maps to a block that
        // cannot be mapped back and disappears from the audit as unstable.
        "undyed_shulker_box" => Some(MappedState::new("minecraft:shulker_box").lossy()),
        "chain" => {
            let mut mapped = MappedState::new("minecraft:chain");
            if let Some(axis) = get("pillar_axis") {
                mapped = mapped.with("axis", axis);
            }
            mapped = mapped.with("waterlogged", "false");
            Some(mapped)
        }
        "anvil" => {
            let mut mapped = MappedState::new(namespaced(match get("damage") {
                Some("slightly_damaged") => "chipped_anvil",
                Some("very_damaged") => "damaged_anvil",
                _ => "anvil",
            }));
            if let Some(facing) = get("minecraft:cardinal_direction").or_else(|| get("direction")) {
                mapped = mapped.with("facing", facing);
            }
            Some(mapped.lossy())
        }
        "lit_furnace" => {
            let mut mapped = MappedState::new("minecraft:furnace");
            mapped = mapped.with("lit", "true");
            if let Some(facing) = get("minecraft:cardinal_direction").or_else(|| get("direction")) {
                mapped = mapped.with("facing", facing);
            }
            Some(mapped.lossy())
        }
        "skull" => {
            // The variant lives in block-entity data, not the state.
            Some(
                MappedState::new("minecraft:skeleton_skull")
                    .with("rotation", "0")
                    .lossy(),
            )
        }
        _ => None,
    }
}
