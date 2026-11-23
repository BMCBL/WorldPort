//! Synthetic Java worlds used to exercise the converter end to end.
//!
//! `worldport fixtures <dir>` writes a small 26.2 world containing a stone
//! platform, container/sign/spawner block entities, a spread of entities, a
//! local player and scheduled ticks. Converting it to Bedrock and back is the
//! M3 acceptance check.

use std::path::Path;

use bedrock_nbt::NbtTag;
use indexmap::IndexMap;

use crate::error::{Result, io_result};
use crate::item::ItemStack;
use crate::je::JeChunkPos;
use crate::je::anvil::write_region_file;
use crate::je::nbt as je_nbt;
use crate::nbt::{NbtExt, compound, empty_compound, tag_list, tag_string};

/// Data version written into the fixture world (Java 26.2).
pub const FIXTURE_DATA_VERSION: i32 = 4903;

/// Writes the fixture world under `root`.
pub fn write_world(root: &Path) -> Result<()> {
    io_result(root, std::fs::create_dir_all(root))?;
    write_level(root)?;
    write_chunk(root)?;
    write_maps(root)?;
    write_player(root)?;
    Ok(())
}

/// Writes a Java world with `side * side` chunks of solid stone.
///
/// Used to check that JE->BE memory stays bounded as the world grows: the
/// LevelDB writer keeps writes in its WAL overlay until an explicit flush, so a
/// large world is the only way to observe whether the write path is bounded.
pub fn write_flat_world(root: &Path, side: i32) -> Result<()> {
    io_result(root, std::fs::create_dir_all(root))?;
    write_level(root)?;
    let region_dir = root.join("dimensions/minecraft/overworld/region");
    io_result(&region_dir, std::fs::create_dir_all(&region_dir))?;

    // One section at y=-4: a single stone layer, so chunk payloads stay small.
    let mut section = IndexMap::new();
    section.insert("Y".to_string(), NbtTag::Byte(-4));
    section.insert(
        "block_states".to_string(),
        block_states_palette(
            &[("minecraft:air", None), ("minecraft:stone", None)],
            |_, _, _| 1_u16,
        ),
    );
    section.insert(
        "biomes".to_string(),
        compound([(
            "palette",
            tag_list(vec![tag_string("minecraft:plains")]),
        )]),
    );

    let mut by_region: std::collections::BTreeMap<(i32, i32), Vec<(u8, u8, Vec<u8>)>> =
        Default::default();
    for chunk_x in 0..side {
        for chunk_z in 0..side {
            let chunk = compound([
                ("DataVersion", NbtTag::Int(FIXTURE_DATA_VERSION)),
                ("xPos", NbtTag::Int(chunk_x)),
                ("zPos", NbtTag::Int(chunk_z)),
                ("yPos", NbtTag::Int(-4)),
                ("Status", tag_string("minecraft:full")),
                ("sections", tag_list(vec![NbtTag::Compound(section.clone())])),
                (
                    "Heightmaps",
                    compound([(
                        "WORLD_SURFACE",
                        NbtTag::LongArray(vec![0_i64; 37]),
                    )]),
                ),
                ("isLightOn", NbtTag::Byte(0)),
                (
                    "structures",
                    compound([("starts", empty_compound()), ("References", empty_compound())]),
                ),
                ("block_entities", tag_list(Vec::new())),
                ("block_ticks", tag_list(Vec::new())),
                ("fluid_ticks", tag_list(Vec::new())),
            ]);
            let payload = je_nbt::encode("", &chunk)?;
            let local_x = u8::try_from(chunk_x.rem_euclid(32)).unwrap_or(0);
            let local_z = u8::try_from(chunk_z.rem_euclid(32)).unwrap_or(0);
            by_region
                .entry((chunk_x.div_euclid(32), chunk_z.div_euclid(32)))
                .or_default()
                .push((local_x, local_z, payload));
        }
    }
    for ((region_x, region_z), entries) in by_region {
        let path = region_dir.join(format!("r.{region_x}.{region_z}.mca"));
        write_region_file(&path, entries, flate2::Compression::new(1))?;
    }
    Ok(())
}

fn write_level(root: &Path) -> Result<()> {
    let data = compound([
        ("DataVersion", NbtTag::Int(FIXTURE_DATA_VERSION)),
        ("LevelName", tag_string("WorldPort Fixture")),
        (
            "Version",
            compound([
                ("Id", NbtTag::Int(FIXTURE_DATA_VERSION)),
                ("Name", tag_string("26.2")),
                ("Series", tag_string("main")),
                ("Snapshot", NbtTag::Byte(0)),
            ]),
        ),
        (
            "spawn",
            compound([
                ("pos", NbtTag::IntArray(vec![0, -59, 0])),
                ("pitch", NbtTag::Float(0.0)),
                ("yaw", NbtTag::Float(0.0)),
                ("dimension", tag_string("minecraft:overworld")),
            ]),
        ),
        (
            "difficulty_settings",
            compound([
                ("difficulty", tag_string("normal")),
                ("hardcore", NbtTag::Byte(0)),
                ("locked", NbtTag::Byte(0)),
            ]),
        ),
        ("GameType", NbtTag::Int(1)),
        ("Time", NbtTag::Long(1200)),
        ("allowCommands", NbtTag::Byte(1)),
        ("initialized", NbtTag::Byte(1)),
        ("WasModded", NbtTag::Byte(0)),
        ("ServerBrands", tag_list(vec![tag_string("vanilla")])),
        ("LastPlayed", NbtTag::Long(0)),
        ("singleplayer_uuid", NbtTag::IntArray(vec![1, 2, 3, 4])),
        ("version", NbtTag::Int(19133)),
    ]);
    je_nbt::write_gzip(&root.join("level.dat"), "", &compound([("Data", data)]))?;

    let data_dir = root.join("data").join("minecraft");
    io_result(&data_dir, std::fs::create_dir_all(&data_dir))?;
    let world_gen = compound([
        ("seed", NbtTag::Long(123456789)),
        ("generate_structures", NbtTag::Byte(1)),
        ("bonus_chest", NbtTag::Byte(0)),
        (
            "dimensions",
            compound([(
                "minecraft:overworld",
                compound([
                    ("type", tag_string("minecraft:overworld")),
                    (
                        "generator",
                        compound([("type", tag_string("minecraft:noise"))]),
                    ),
                ]),
            )]),
        ),
    ]);
    write_split(&data_dir, "world_gen_settings", world_gen)?;
    let rules = compound([
        ("minecraft:advance_time", NbtTag::Byte(1)),
        ("minecraft:block_drops", NbtTag::Byte(1)),
        ("minecraft:keep_inventory", NbtTag::Byte(1)),
        ("minecraft:random_tick_speed", NbtTag::Int(3)),
    ]);
    write_split(&data_dir, "game_rules", rules)?;
    let weather = compound([
        ("raining", tag_string("false")),
        ("thundering", tag_string("false")),
        ("rain_time", NbtTag::Int(0)),
        ("thunder_time", NbtTag::Int(0)),
        ("clear_weather_time", NbtTag::Int(0)),
    ]);
    write_split(&data_dir, "weather", weather)?;
    let clocks = compound([(
        "minecraft:overworld",
        compound([
            ("total_ticks", NbtTag::Long(1200)),
            ("partial_tick", NbtTag::Float(0.0)),
            ("rate", NbtTag::Float(1.0)),
        ]),
    )]);
    write_split(&data_dir, "world_clocks", clocks)?;
    Ok(())
}

fn write_split(dir: &Path, name: &str, data: NbtTag) -> Result<()> {
    let root = compound([
        ("data", data),
        ("DataVersion", NbtTag::Int(FIXTURE_DATA_VERSION)),
    ]);
    je_nbt::write_gzip(&dir.join(format!("{name}.dat")), "", &root)
}

fn write_chunk(root: &Path) -> Result<()> {
    let mut section = IndexMap::new();
    section.insert("Y".to_string(), NbtTag::Byte(-4));
    section.insert(
        "block_states".to_string(),
        block_states_palette(
            &[
                ("minecraft:air", None),
                ("minecraft:stone", None),
                ("minecraft:oak_planks", None),
                // Introduced in 1.21.4: used to check version downgrade pruning.
                (
                    "minecraft:creaking_heart",
                    Some(&[
                        ("axis", "y"),
                        ("creaking_heart_state", "dormant"),
                        ("natural", "false"),
                    ]),
                ),
                // Fence line end points and middle, with Java's own connection
                // values spelled out.
                (
                    "minecraft:oak_fence",
                    Some(&[
                        ("north", "false"),
                        ("east", "true"),
                        ("south", "false"),
                        ("west", "false"),
                        ("waterlogged", "false"),
                    ]),
                ),
                (
                    "minecraft:oak_fence",
                    Some(&[
                        ("north", "false"),
                        ("east", "true"),
                        ("south", "false"),
                        ("west", "true"),
                        ("waterlogged", "false"),
                    ]),
                ),
                (
                    "minecraft:oak_fence",
                    Some(&[
                        ("north", "false"),
                        ("east", "false"),
                        ("south", "false"),
                        ("west", "true"),
                        ("waterlogged", "false"),
                    ]),
                ),
                // The same shapes for a glass-pane line.
                (
                    "minecraft:glass_pane",
                    Some(&[
                        ("north", "false"),
                        ("east", "true"),
                        ("south", "false"),
                        ("west", "false"),
                        ("waterlogged", "false"),
                    ]),
                ),
                (
                    "minecraft:glass_pane",
                    Some(&[
                        ("north", "false"),
                        ("east", "true"),
                        ("south", "false"),
                        ("west", "true"),
                        ("waterlogged", "false"),
                    ]),
                ),
                (
                    "minecraft:glass_pane",
                    Some(&[
                        ("north", "false"),
                        ("east", "false"),
                        ("south", "false"),
                        ("west", "true"),
                        ("waterlogged", "false"),
                    ]),
                ),
            ],
            // Bottom layer stone, a 3x3 plank platform above it, then a fence
            // line along x and a glass-pane line along z.
            |x, y, z| {
                if y == 0 {
                    1
                } else if y == 1 && x == 14 && z == 14 {
                    3
                } else if y == 1 && (1..=3).contains(&x) && (1..=3).contains(&z) {
                    2
                } else if y == 2 && z == 1 && (1..=3).contains(&x) {
                    // A three-fence line: the ends connect inward, the middle
                    // connects both ways. These values are the ground truth a
                    // roundtrip has to reproduce, because Bedrock stores no fence
                    // connections at all and they can only come back from
                    // neighbour derivation.
                    match x {
                        1 => 4,
                        2 => 5,
                        _ => 6,
                    }
                } else if y == 2 && z == 3 && (1..=3).contains(&x) {
                    match x {
                        1 => 7,
                        2 => 8,
                        _ => 9,
                    }
                } else {
                    0
                }
            },
        ),
    );
    section.insert(
        "biomes".to_string(),
        compound([("palette", tag_list(vec![tag_string("minecraft:plains")]))]),
    );
    let chunk = compound([
        ("DataVersion", NbtTag::Int(FIXTURE_DATA_VERSION)),
        ("xPos", NbtTag::Int(0)),
        ("zPos", NbtTag::Int(0)),
        ("yPos", NbtTag::Int(-4)),
        ("Status", tag_string("minecraft:full")),
        ("sections", tag_list(vec![NbtTag::Compound(section)])),
        (
            "Heightmaps",
            compound([
                ("MOTION_BLOCKING", heightmap(2)),
                ("WORLD_SURFACE", heightmap(2)),
            ]),
        ),
        ("isLightOn", NbtTag::Byte(0)),
        (
            "structures",
            // One ocean monument start with a single piece, so the
            // Java->Bedrock hardcoded-spawn-area path has something to convert.
            compound([
                (
                    "starts",
                    compound([(
                        "minecraft:monument",
                        compound([
                            ("id", tag_string("minecraft:monument")),
                            ("ChunkX", NbtTag::Int(0)),
                            ("ChunkZ", NbtTag::Int(0)),
                            (
                                "Children",
                                tag_list(vec![compound([(
                                    "BB",
                                    NbtTag::IntArray(vec![-8, 40, -8, 8, 60, 8]),
                                )])]),
                            ),
                            ("References", empty_compound()),
                        ]),
                    )]),
                ),
                ("References", empty_compound()),
            ]),
        ),
        (
            "block_ticks",
            tag_list(vec![compound([
                ("i", tag_string("minecraft:oak_planks")),
                ("x", NbtTag::Int(2)),
                // The plank platform is the section's local y=1, i.e. world
                // y=-63. A tick at -62 sits on air, and the converter
                // (correctly) writes the state it actually finds there.
                ("y", NbtTag::Int(-63)),
                ("z", NbtTag::Int(2)),
                ("t", NbtTag::Int(40)),
                ("p", NbtTag::Int(0)),
            ])]),
        ),
        (
            "fluid_ticks",
            tag_list(vec![compound([
                ("i", tag_string("minecraft:water")),
                ("x", NbtTag::Int(5)),
                ("y", NbtTag::Int(-63)),
                ("z", NbtTag::Int(5)),
                ("t", NbtTag::Int(5)),
                ("p", NbtTag::Int(0)),
            ])]),
        ),
        ("PostProcessing", tag_list(Vec::new())),
        ("InhabitedTime", NbtTag::Long(0)),
        ("LastUpdate", NbtTag::Long(0)),
        ("block_entities", tag_list(block_entities())),
    ]);
    let payload = je_nbt::encode("", &chunk)?;
    let region_dir = root
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("region");
    write_region_file(
        &region_dir.join("r.0.0.mca"),
        vec![(0, 0, payload)],
        flate2::Compression::default(),
    )?;

    // 1.17+ entity storage.
    let entity_chunk = compound([
        ("Entities", tag_list(entities())),
        ("DataVersion", NbtTag::Int(FIXTURE_DATA_VERSION)),
        ("Position", NbtTag::IntArray(vec![0, 0])),
    ]);
    let entity_payload = je_nbt::encode("", &entity_chunk)?;
    let entity_dir = root
        .join("dimensions")
        .join("minecraft")
        .join("overworld")
        .join("entities");
    write_region_file(
        &entity_dir.join("r.0.0.mca"),
        vec![(0, 0, entity_payload)],
        flate2::Compression::default(),
    )?;
    Ok(())
}

fn block_states_palette(
    palette: &[(&str, Option<&[(&str, &str)]>)],
    index_at: impl Fn(usize, usize, usize) -> u16,
) -> NbtTag {
    let entries: Vec<NbtTag> = palette
        .iter()
        .map(|(name, properties)| {
            let mut entry = IndexMap::new();
            entry.insert("Name".to_string(), tag_string(*name));
            if let Some(properties) = properties {
                entry.insert(
                    "Properties".to_string(),
                    NbtTag::Compound(
                        properties
                            .iter()
                            .map(|(key, value)| ((*key).to_string(), tag_string(*value)))
                            .collect(),
                    ),
                );
            }
            NbtTag::Compound(entry)
        })
        .collect();
    let mut indices = vec![0_u16; 4096];
    for x in 0..16 {
        for z in 0..16 {
            for y in 0..16 {
                indices[(y << 8) | (z << 4) | x] = index_at(x, y, z);
            }
        }
    }
    let bits = (usize::BITS - (palette.len() - 1).leading_zeros()).max(4);
    let per_long = (64 / bits) as usize;
    let mut words = vec![0_i64; indices.len().div_ceil(per_long)];
    let mask = (1_u64 << bits) - 1;
    for (index, value) in indices.iter().enumerate() {
        let word = index / per_long;
        let offset = (index % per_long) as u32 * bits;
        words[word] |= ((u64::from(*value) & mask) << offset) as i64;
    }
    compound([
        ("palette", tag_list(entries)),
        ("data", NbtTag::LongArray(words)),
    ])
}

fn heightmap(top: i16) -> NbtTag {
    // 9 bits per entry, seven per long.
    let mut words = vec![0_i64; 37];
    for index in 0..256_usize {
        let word = index / 7;
        let offset = (index % 7) as u32 * 9;
        words[word] |= ((u64::from(top as u16) & 0x1FF) << offset) as i64;
    }
    NbtTag::LongArray(words)
}

fn block_entities() -> Vec<NbtTag> {
    let mut chest_items = IndexMap::new();
    chest_items.insert(
        "Items".to_string(),
        tag_list(vec![java_item(3, "minecraft:diamond", 5, None)]),
    );
    let mut sign_front = IndexMap::new();
    sign_front.insert(
        "messages".to_string(),
        tag_list(vec![
            tag_string("\"Hello\""),
            tag_string("\"World\""),
            tag_string("\"\""),
            tag_string("\"\""),
        ]),
    );
    vec![
        compound([
            ("id", tag_string("minecraft:chest")),
            ("x", NbtTag::Int(2)),
            ("y", NbtTag::Int(-62)),
            ("z", NbtTag::Int(2)),
            ("CustomName", tag_string("\"Fixture Chest\"")),
            ("Items", tag_list(vec![java_item(3, "minecraft:diamond", 5, None)])),
        ]),
        compound([
            ("id", tag_string("minecraft:furnace")),
            ("x", NbtTag::Int(6)),
            ("y", NbtTag::Int(-62)),
            ("z", NbtTag::Int(2)),
            ("BurnTime", NbtTag::Short(200)),
            ("CookTime", NbtTag::Short(40)),
            (
                "Items",
                tag_list(vec![
                    java_item(0, "minecraft:iron_ore", 3, None),
                    java_item(1, "minecraft:coal", 7, None),
                ]),
            ),
        ]),
        compound([
            ("id", tag_string("minecraft:sign")),
            ("x", NbtTag::Int(8)),
            ("y", NbtTag::Int(-62)),
            ("z", NbtTag::Int(2)),
            ("front_text", compound([("messages", sign_front.get("messages").cloned().unwrap())])),
        ]),
        compound([
            ("id", tag_string("minecraft:mob_spawner")),
            ("x", NbtTag::Int(10)),
            ("y", NbtTag::Int(-62)),
            ("z", NbtTag::Int(2)),
            ("Delay", NbtTag::Short(20)),
            (
                "SpawnData",
                compound([("entity", compound([("id", tag_string("minecraft:zombie"))]))]),
            ),
        ]),
    ]
}

fn java_item(slot: i8, id: &str, count: i32, name: Option<&str>) -> NbtTag {
    let mut item = match crate::item::write_java_item(
        &ItemStack {
            id: id.to_string(),
            count,
            custom_name: name.map(str::to_string),
            ..ItemStack::default()
        },
        FIXTURE_DATA_VERSION,
    ) {
        NbtTag::Compound(entries) => entries,
        _ => IndexMap::new(),
    };
    item.insert("Slot".to_string(), NbtTag::Byte(slot));
    NbtTag::Compound(item)
}

fn entities() -> Vec<NbtTag> {
    let mut zombie_hands = Vec::new();
    zombie_hands.push(crate::item::write_java_item(
        &ItemStack {
            id: "minecraft:iron_sword".to_string(),
            count: 1,
            custom_name: Some("Fixture Blade".to_string()),
            enchantments: vec![("minecraft:sharpness".to_string(), 5)],
            ..ItemStack::default()
        },
        FIXTURE_DATA_VERSION,
    ));
    zombie_hands.push(empty_compound());
    let mut armor = vec![crate::item::write_java_item(
        &ItemStack {
            id: "minecraft:iron_helmet".to_string(),
            count: 1,
            ..ItemStack::default()
        },
        FIXTURE_DATA_VERSION,
    )];
    armor.extend(std::iter::repeat_n(empty_compound(), 3));

    vec![
        compound([
            ("id", tag_string("minecraft:zombie")),
            (
                "Pos",
                tag_list(vec![
                    NbtTag::Double(4.5),
                    NbtTag::Double(-61.0),
                    NbtTag::Double(4.5),
                ]),
            ),
            (
                "Motion",
                tag_list(vec![
                    NbtTag::Double(0.0),
                    NbtTag::Double(0.0),
                    NbtTag::Double(0.0),
                ]),
            ),
            ("Rotation", tag_list(vec![NbtTag::Float(90.0), NbtTag::Float(0.0)])),
            ("Health", NbtTag::Float(20.0)),
            ("Age", NbtTag::Int(0)),
            ("HandItems", tag_list(zombie_hands)),
            ("ArmorItems", tag_list(armor)),
            (
                "active_effects",
                tag_list(vec![compound([
                    ("id", tag_string("minecraft:speed")),
                    ("amplifier", NbtTag::Int(1)),
                    ("duration", NbtTag::Int(600)),
                ])]),
            ),
        ]),
        compound([
            ("id", tag_string("minecraft:item")),
            (
                "Pos",
                tag_list(vec![
                    NbtTag::Double(5.5),
                    NbtTag::Double(-61.0),
                    NbtTag::Double(5.5),
                ]),
            ),
            (
                "Item",
                crate::item::write_java_item(
                    &ItemStack {
                        id: "minecraft:apple".to_string(),
                        count: 3,
                        ..ItemStack::default()
                    },
                    FIXTURE_DATA_VERSION,
                ),
            ),
        ]),
        compound([
            ("id", tag_string("minecraft:experience_orb")),
            (
                "Pos",
                tag_list(vec![
                    NbtTag::Double(6.5),
                    NbtTag::Double(-61.0),
                    NbtTag::Double(5.5),
                ]),
            ),
            ("Value", NbtTag::Short(7)),
        ]),
        compound([
            ("id", tag_string("minecraft:villager")),
            (
                "Pos",
                tag_list(vec![
                    NbtTag::Double(7.5),
                    NbtTag::Double(-61.0),
                    NbtTag::Double(5.5),
                ]),
            ),
            (
                "VillagerData",
                compound([
                    ("profession", tag_string("minecraft:farmer")),
                    ("level", NbtTag::Int(2)),
                    ("type", tag_string("minecraft:plains")),
                ]),
            ),
        ]),
        compound([
            ("id", tag_string("minecraft:item_frame")),
            (
                "Pos",
                tag_list(vec![
                    NbtTag::Double(3.5),
                    NbtTag::Double(-60.0),
                    NbtTag::Double(3.5),
                ]),
            ),
            ("Facing", NbtTag::Byte(1)),
            ("ItemRotation", NbtTag::Byte(0)),
            (
                "Item",
                crate::item::write_java_item(
                    &ItemStack {
                        id: "minecraft:filled_map".to_string(),
                        count: 1,
                        ..ItemStack::default()
                    },
                    FIXTURE_DATA_VERSION,
                ),
            ),
        ]),
        compound([
            ("id", tag_string("minecraft:painting")),
            (
                "Pos",
                tag_list(vec![
                    NbtTag::Double(9.5),
                    NbtTag::Double(-60.0),
                    NbtTag::Double(3.5),
                ]),
            ),
            ("Facing", NbtTag::Byte(2)),
            ("Motive", tag_string("minecraft:kebab")),
        ]),
    ]
}

fn write_maps(root: &Path) -> Result<()> {
    let dir = root.join("data").join("minecraft").join("maps");
    let mut colors = vec![0_u8; 128 * 128];
    for (index, pixel) in colors.iter_mut().enumerate() {
        let x = index % 128;
        let z = index / 128;
        // A simple checkerboard with a few distinct palette entries.
        *pixel = if (x / 16 + z / 16) % 2 == 0 { 4 } else { 8 };
    }
    let map = crate::map_item::MapData {
        id: 3,
        scale: 1,
        dimension: "minecraft:overworld".to_string(),
        x_center: 0,
        z_center: 0,
        width: 128,
        height: 128,
        locked: false,
        tracking: true,
        colors,
    };
    crate::map_item::write_java_map(&dir, &map, FIXTURE_DATA_VERSION, true)?;
    crate::map_item::write_idcounts(&dir, map.id, FIXTURE_DATA_VERSION)?;
    Ok(())
}

fn write_player(root: &Path) -> Result<()> {
    let player = compound([
        ("DataVersion", NbtTag::Int(FIXTURE_DATA_VERSION)),
        ("UUID", NbtTag::IntArray(vec![1, 2, 3, 4])),
        (
            "Pos",
            tag_list(vec![
                NbtTag::Double(2.5),
                NbtTag::Double(-61.0),
                NbtTag::Double(2.5),
            ]),
        ),
        ("Rotation", tag_list(vec![NbtTag::Float(0.0), NbtTag::Float(0.0)])),
        ("Health", NbtTag::Float(20.0)),
        ("foodLevel", NbtTag::Int(20)),
        ("foodSaturationLevel", NbtTag::Float(5.0)),
        ("XpLevel", NbtTag::Int(12)),
        ("XpP", NbtTag::Float(0.25)),
        ("XpTotal", NbtTag::Int(500)),
        ("playerGameType", NbtTag::Int(1)),
        ("Dimension", tag_string("minecraft:overworld")),
        ("SelectedItemSlot", NbtTag::Int(0)),
        (
            "Inventory",
            tag_list(vec![
                java_item(0, "minecraft:diamond_pickaxe", 1, Some("Fixture Pick")),
                java_item(9, "minecraft:bread", 12, None),
                java_item(-106, "minecraft:shield", 1, None),
            ]),
        ),
        (
            "EnderItems",
            tag_list(vec![java_item(0, "minecraft:ender_pearl", 4, None)]),
        ),
        (
            "active_effects",
            tag_list(vec![compound([
                ("id", tag_string("minecraft:night_vision")),
                ("amplifier", NbtTag::Int(0)),
                ("duration", NbtTag::Int(1200)),
            ])]),
        ),
        ("SpawnX", NbtTag::Int(0)),
        ("SpawnY", NbtTag::Int(-59)),
        ("SpawnZ", NbtTag::Int(0)),
        ("SpawnDimension", tag_string("minecraft:overworld")),
    ]);
    let path = root
        .join("playerdata")
        // The file name must be the player's own UUID: Java names the file
        // after `UUID` in the payload, and a mismatch makes the identity
        // ambiguous when a tool preserves one and derives the other.
        .join("00000001-0000-0002-0000-000300000004.dat");
    je_nbt::write_gzip(&path, "", &player)
}

/// Chunk position used by the fixture world.
#[must_use]
pub const fn fixture_chunk() -> JeChunkPos {
    JeChunkPos::new(0, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_world_round_trips_through_the_reader() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("world");
        write_world(&root).expect("write fixture");
        let world = crate::je::world::JavaWorld::open(&root).expect("open fixture");
        assert_eq!(world.level_name(), "WorldPort Fixture");
        assert_eq!(world.seed(), 123456789);
        let positions = world.chunk_positions(crate::je::JeDimension::Overworld).expect("chunks");
        assert_eq!(positions.len(), 1);
        let chunk = world
            .read_chunk(crate::je::JeDimension::Overworld, fixture_chunk())
            .expect("read chunk")
            .expect("chunk");
        assert_eq!(chunk.get_list("block_entities").map(|v| v.len()), Some(4));
        let entities = world
            .read_entity_chunk(crate::je::JeDimension::Overworld, fixture_chunk())
            .expect("entities");
        assert_eq!(entities.len(), 6);
        assert_eq!(world.player_files().expect("players").len(), 1);
    }
}
