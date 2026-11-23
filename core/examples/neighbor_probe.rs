//! Prints how the mapper handles the families whose state depends on neighbours:
//! fences, walls, glass panes, stairs and chests.
//!
//! Run before changing the conversion: the output is the "before" picture, and
//! it makes the missing connection data visible instead of theoretical.

use std::collections::BTreeMap;

use worldport_core::mapping::schema::bedrock_catalog;
use worldport_core::mapping::{BlockMapper, JavaCatalog};

fn props(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn show(mapper: &BlockMapper, name: &str, pairs: &[(&str, &str)]) {
    let properties = props(pairs);
    let java_to_be = mapper
        .je_to_be(name, &properties)
        .map(|state| {
            let mut out = state.name.clone();
            for (key, value) in &state.properties {
                out.push_str(&format!(" {key}={value}"));
            }
            out
        })
        .unwrap_or_else(|| "unmapped".to_string());
    println!("JE {name} {properties:?}\n   -> BE {java_to_be}");
}

fn show_be(mapper: &BlockMapper, name: &str, pairs: &[(&str, &str)]) {
    let properties = props(pairs);
    let be_to_je = mapper
        .be_to_je(name, &properties)
        .map(|state| {
            let mut out = state.name.clone();
            for (key, value) in &state.properties {
                out.push_str(&format!(" {key}={value}"));
            }
            out
        })
        .unwrap_or_else(|| "unmapped".to_string());
    println!("BE {name} {properties:?}\n   -> JE {be_to_je}");
}

fn main() {
    // 26.2 is the current target the converter writes.
    let java = JavaCatalog::for_data_version(4903).expect("java catalog");
    let mapper = BlockMapper::new(java.to_catalog(), bedrock_catalog());
    println!("== Java -> Bedrock ==");
    show(&mapper, "minecraft:oak_fence", &[
        ("north", "true"), ("east", "false"), ("south", "true"), ("west", "false"),
        ("waterlogged", "false"),
    ]);
    show(&mapper, "minecraft:cobblestone_wall", &[
        ("north", "tall"), ("east", "low"), ("south", "none"), ("west", "tall"),
        ("up", "true"), ("waterlogged", "false"),
    ]);
    show(&mapper, "minecraft:glass_pane", &[
        ("north", "true"), ("east", "false"), ("south", "false"), ("west", "true"),
        ("waterlogged", "false"),
    ]);
    show(&mapper, "minecraft:oak_stairs", &[
        ("facing", "north"), ("half", "bottom"), ("shape", "inner_left"),
        ("waterlogged", "false"),
    ]);
    show(&mapper, "minecraft:chest", &[
        ("facing", "north"), ("type", "left"), ("waterlogged", "false"),
    ]);

    println!("\n== Bedrock -> Java ==");
    show_be(&mapper, "minecraft:oak_fence", &[]);
    show_be(&mapper, "minecraft:oak_fence", &[("north", "1"), ("south", "1")]);
    show_be(&mapper, "minecraft:cobblestone_wall", &[
        ("wall_connection_type_north", "tall"),
        ("wall_connection_type_east", "short"),
        ("wall_connection_type_south", "none"),
        ("wall_connection_type_west", "tall"),
        ("wall_post_bit", "1"),
    ]);
    show_be(&mapper, "minecraft:glass_pane", &[]);
    show_be(&mapper, "minecraft:oak_stairs", &[
        ("weirdo_direction", "2"), ("upside_down_bit", "0"), ("corner", "outer_right"),
    ]);
    show_be(&mapper, "minecraft:chest", &[("minecraft:cardinal_direction", "north")]);
}
