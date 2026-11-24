//! Matrix contracts for every Java catalog bundled with WorldPort.
//! Synthetic NBT/Region fixtures are not substitutes for real saved-world tests.

use std::collections::BTreeMap;
use std::path::Path;

use bedrock_nbt::NbtTag;
use flate2::Compression;
use worldport_core::compare::compare_java;
use worldport_core::convert::{self, ConversionOptions, ConversionPlan, Features};
use worldport_core::item::{ItemStack, read_java_item, write_java_item};
use worldport_core::je::anvil::write_region_file;
use worldport_core::je::nbt;
use worldport_core::je::world::JavaWorld;
use worldport_core::je::{JeChunkPos, JeDimension};
use worldport_core::mapping::{BlockMapper, JavaCatalog, bedrock_catalog};
use worldport_core::nbt::{NbtExt, compound, empty_compound, tag_list, tag_string};

const MODERN_LAYOUT: i32 = 4786;
const CHUNK_X: i32 = -33;
const CHUNK_Z: i32 = 34;

fn manifest() -> BTreeMap<String, i32> {
    serde_json::from_str(include_str!("../../mappings/je_versions.json"))
        .expect("valid mappings/je_versions.json")
}

#[test]
fn every_manifest_version_is_present_and_schema_complete() {
    let declared = manifest();
    assert!(!declared.is_empty(), "empty version manifest");
    let loaded: BTreeMap<_, _> = JavaCatalog::bundled_versions()
        .into_iter().map(|(v, d)| (v.to_string(), d)).collect();
    assert_eq!(loaded, declared, "missing/broken catalogs must fail CI, not disappear");
    let mut last = i32::MIN;
    for (version, data_version) in JavaCatalog::bundled_versions() {
        assert!(data_version > last, "catalog DataVersions must be strictly ordered");
        last = data_version;
        let catalog = JavaCatalog::bundled(version).expect("bundled version");
        assert_eq!(catalog.data_version(), data_version, "{version}");
        assert_eq!(JavaCatalog::for_data_version(data_version).unwrap().version(), version);
        for name in ["air", "stone", "dirt", "oak_planks", "oak_log", "water"] {
            assert!(catalog.catalog().contains(name), "{version}: missing {name}");
        }
        let axis = catalog.catalog().block("oak_log")
            .and_then(|block| block.property("axis")).expect("oak_log.axis");
        for value in ["x", "y", "z"] {
            assert!(axis.accepts(value), "{version}: oak_log.axis={value}");
        }
        let level = catalog.catalog().block("water")
            .and_then(|block| block.property("level")).expect("water.level");
        for value in ["0", "7", "15"] {
            assert!(level.accepts(value), "{version}: water.level={value}");
        }
    }
}

#[test]
fn stable_block_mappings_roundtrip_in_both_directions_for_all_versions() {
    for (version, _) in JavaCatalog::bundled_versions() {
        let mapper = BlockMapper::new(JavaCatalog::bundled(version).unwrap().to_catalog(), bedrock_catalog());
        for name in ["minecraft:stone", "minecraft:oak_planks"] {
            let mapped = mapper.je_to_be(name, &BTreeMap::new())
                .unwrap_or_else(|| panic!("{version}: cannot convert {name} JE -> BE"));
            assert_eq!(mapped.name, name, "{version}");
            let back = mapper.be_to_je(&mapped.name, &mapped.properties)
                .unwrap_or_else(|| panic!("{version}: cannot convert {name} BE -> JE"));
            assert_eq!(back.name, name, "{version}");
        }
        for axis in ["x", "y", "z"] {
            let source = BTreeMap::from([("axis".to_string(), axis.to_string())]);
            let mapped = mapper.je_to_be("minecraft:oak_log", &source)
                .unwrap_or_else(|| panic!("{version}: oak_log axis={axis} JE -> BE"));
            assert_eq!(mapped.properties.get("pillar_axis").map(String::as_str), Some(axis), "{version}");
            let back = mapper.be_to_je(&mapped.name, &mapped.properties)
                .unwrap_or_else(|| panic!("{version}: oak_log axis={axis} BE -> JE"));
            assert_eq!(back.name, "minecraft:oak_log", "{version}");
            assert_eq!(back.properties.get("axis").map(String::as_str), Some(axis), "{version}");
        }
    }
    assert!(!JavaCatalog::bundled("1.20.6").unwrap().catalog().contains("creaking_heart"));
    assert!(JavaCatalog::bundled("1.21.4").unwrap().catalog().contains("creaking_heart"));
}

fn palette_chunk(data_version: i32) -> NbtTag {
    // 4096 Java block indices, 4-bit non-straddling long-array layout.
    let mut indices = [0_u16; 4096];
    for x in 0..16 {
        for z in 0..16 {
            indices[(z << 4) | x] = 1;
        }
    }
    indices[(1 << 8) | (4 << 4) | 3] = 2;
    indices[(2 << 8) | (4 << 4) | 3] = 2;
    indices[(1 << 8) | (6 << 4) | 6] = 3;
    let mut words = vec![0_i64; 256];
    for (index, value) in indices.into_iter().enumerate() {
        words[index / 16] |= i64::from(value) << ((index % 16) * 4);
    }
    let blocks = compound([
        ("palette", tag_list(vec![
            compound([("Name", tag_string("minecraft:air"))]),
            compound([("Name", tag_string("minecraft:stone"))]),
            compound([
                ("Name", tag_string("minecraft:oak_log")),
                ("Properties", compound([("axis", tag_string("x"))])),
            ]),
            compound([
                ("Name", tag_string("minecraft:water")),
                ("Properties", compound([("level", tag_string("7"))])),
            ]),
        ])),
        ("data", NbtTag::LongArray(words)),
    ]);
    let section = compound([
        ("Y", NbtTag::Byte(-4)),
        ("block_states", blocks),
        ("biomes", compound([("palette", tag_list(vec![tag_string("minecraft:plains")]))])),
    ]);
    compound([
        ("DataVersion", NbtTag::Int(data_version)),
        ("xPos", NbtTag::Int(CHUNK_X)),
        ("zPos", NbtTag::Int(CHUNK_Z)),
        ("yPos", NbtTag::Int(-4)),
        ("Status", tag_string(if data_version >= 3578 { "minecraft:full" } else { "full" })),
        ("sections", tag_list(vec![section])),
        ("Heightmaps", compound([
            ("WORLD_SURFACE", NbtTag::LongArray(vec![0; 37])),
            ("MOTION_BLOCKING", NbtTag::LongArray(vec![0; 37])),
        ])),
        ("isLightOn", NbtTag::Byte(0)),
        ("structures", compound([("starts", empty_compound()), ("References", empty_compound())])),
        ("block_entities", tag_list(Vec::new())),
        ("block_ticks", tag_list(Vec::new())),
        ("fluid_ticks", tag_list(Vec::new())),
        ("LastUpdate", NbtTag::Long(0)),
        ("InhabitedTime", NbtTag::Long(0)),
    ])
}

fn make_world(root: &Path, data_version: i32) {
    std::fs::create_dir_all(root).expect("Java world directory");
    let level = compound([("Data", compound([
        ("DataVersion", NbtTag::Int(data_version)),
        ("LevelName", tag_string("Version Matrix")),
        ("RandomSeed", NbtTag::Long(7)),
        ("SpawnX", NbtTag::Int(0)),
        ("SpawnY", NbtTag::Int(64)),
        ("SpawnZ", NbtTag::Int(0)),
        ("GameType", NbtTag::Int(0)),
        ("Difficulty", NbtTag::Byte(2)),
        ("Time", NbtTag::Long(1234)),
        ("DayTime", NbtTag::Long(5678)),
        ("LastPlayed", NbtTag::Long(1_700_000_000_000)),
    ]))]);
    nbt::write_gzip(&root.join("level.dat"), "", &level).expect("Java level.dat");
    let region = if data_version >= MODERN_LAYOUT {
        root.join("dimensions/minecraft/overworld/region/r.-2.1.mca")
    } else {
        root.join("region/r.-2.1.mca")
    };
    let payload = nbt::encode("", &palette_chunk(data_version)).expect("Java NBT encode");
    write_region_file(&region, vec![(31, 2, payload)], Compression::fast())
        .expect("Java Anvil write");
}

#[test]
fn exact_palette_nbt_types_and_item_components_for_every_version() {
    for (version, data_version) in JavaCatalog::bundled_versions() {
        let source = palette_chunk(data_version);
        let decoded = nbt::decode(&nbt::encode("", &source).expect("encode")).expect("decode");
        assert_eq!(decoded.tag, source, "{version}: Java NBT changed after encode/decode");
        assert_eq!(decoded.tag.get("DataVersion"), Some(&NbtTag::Int(data_version)), "{version}");
        let section = &decoded.tag.get_list("sections").unwrap()[0];
        assert_eq!(section.get("Y"), Some(&NbtTag::Byte(-4)), "{version}");
        let blocks = section.get("block_states").unwrap();
        assert_eq!(blocks.get_list("palette").unwrap().len(), 4);
        assert!(matches!(blocks.get("data"), Some(NbtTag::LongArray(words)) if words.len() == 256), "{version}");
        assert!(matches!(decoded.tag.get("Heightmaps").and_then(|tag| tag.get("WORLD_SURFACE")),
            Some(NbtTag::LongArray(words)) if words.len() == 37), "{version}");
        let source_item = ItemStack {
            id: "minecraft:diamond_sword".to_string(), count: 1, damage: Some(3),
            ..ItemStack::default()
        };
        // Items carry `components` only from 1.20.5; older releases use `Count`
        // and `tag`, so the shape assertion below applies from there on.
        let item = write_java_item(&source_item, data_version);
        if data_version < 3837 {
            continue;
        }
        let nbt_root = compound([("item", item.clone())]);
        assert_eq!(nbt::decode(&nbt::encode("", &nbt_root).unwrap()).unwrap().tag, nbt_root, "{version}");
        assert_eq!(item.get("count"), Some(&NbtTag::Int(1)), "{version}: item count TAG_Int");
        assert!(matches!(item.get("components"), Some(NbtTag::Compound(_))), "{version}: components TAG_Compound");
        assert!(item.get("Count").is_none(), "{version}: obsolete item Count");
        let back = read_java_item(&item).expect("Java item");
        assert_eq!(back.id, source_item.id, "{version}");
        assert_eq!(back.count, source_item.count, "{version}");
        assert_eq!(back.damage, source_item.damage, "{version}");
    }
}

#[test]
fn every_version_converts_java_to_bedrock_and_back_without_palette_differences() {
    assert_eq!(JavaCatalog::bundled_versions().len(), manifest().len());
    for (version, data_version) in JavaCatalog::bundled_versions() {
        // The converter supports 1.18+ for full conversion; older catalogs exist so
        // a world can still be inspected, and their block/item shapes differ too
        // much for this roundtrip to mean anything.
        if data_version < 2860 {
            continue;
        }
        let dir = tempfile::tempdir().expect("fixture directory");
        let source = dir.path().join("source-je");
        let bedrock = dir.path().join("bedrock");
        let restored = dir.path().join("restored-je");
        make_world(&source, data_version);

        let mut features = Features::none();
        features.blocks = true;
        features.biomes = true;
        features.level_dat = true;
        let mut forward = ConversionPlan::new(&source, &bedrock);
        forward.jobs = 1;
        forward.compression_level = 1;
        forward.features = features;
        convert::run(&forward, &ConversionOptions::default())
            .unwrap_or_else(|error| panic!("{version}: JE -> BE: {error}"));

        let mut reverse = ConversionPlan::new(&bedrock, &restored);
        reverse.jobs = 1;
        reverse.compression_level = 1;
        reverse.features = features;
        reverse.target_version = Some(version.to_string());
        convert::run(&reverse, &ConversionOptions::default())
            .unwrap_or_else(|error| panic!("{version}: BE -> JE: {error}"));

        let actual_region = if data_version >= MODERN_LAYOUT {
            restored.join("dimensions/minecraft/overworld/region/r.-2.1.mca")
        } else {
            restored.join("region/r.-2.1.mca")
        };
        assert!(actual_region.is_file(), "{version}: incorrect target region directory");
        let a = JavaWorld::open(&source).expect("source Java world");
        let b = JavaWorld::open(&restored).expect("converted Java world");
        assert_eq!(b.data().get_int("LastPlayed"), Some(1_700_000_000_000), "{version}");
        assert_eq!(b.total_ticks(), 1234, "{version}: simulation clock");
        assert_eq!(b.day_ticks(), 5678, "{version}: daylight clock");
        let diff = compare_java(&a, &b, JeDimension::Overworld)
            .unwrap_or_else(|error| panic!("{version}: compare: {error}"));
        assert_eq!(diff.first_chunks, 1, "{version}");
        assert_eq!(diff.second_chunks, 1, "{version}");
        assert_eq!(diff.compared_chunks, 1, "{version}");
        assert_eq!(diff.only_first, 0, "{version}");
        assert_eq!(diff.only_second, 0, "{version}");
        assert_eq!(diff.block_mismatches, 0,
            "{version}: block mismatch: {:?} / {:?}", diff.block_totals, diff.worst_chunks);
        assert_eq!(diff.biome_mismatches, 0, "{version}: biome mismatch: {:?}", diff.worst_chunks);
        assert!(diff.compared_non_air_positions > 0, "{version}: fixture became empty");
        let chunk = b.read_chunk(JeDimension::Overworld, JeChunkPos::new(CHUNK_X, CHUNK_Z))
            .unwrap().expect("reconstructed chunk");
        assert_eq!(chunk.get("DataVersion"), Some(&NbtTag::Int(data_version)), "{version}");
        assert_eq!(chunk.get("xPos"), Some(&NbtTag::Int(CHUNK_X)), "{version}");
        assert_eq!(chunk.get("zPos"), Some(&NbtTag::Int(CHUNK_Z)), "{version}");
        assert_eq!(chunk.get("yPos"), Some(&NbtTag::Int(-4)), "{version}");
        // Exact independent heightmap check: the floor is at Y=-64,
        // while the two-block log reaches Y=-62 at local (3,4).
        let heights = chunk.get("Heightmaps").and_then(|tag| tag.get("WORLD_SURFACE"))
            .expect("world surface heightmap");
        let NbtTag::LongArray(words) = heights else { panic!("heightmap must be a long array") };
        assert_eq!(words[0] & 511, 1, "{version}: negative-Y floor");
        let column = 4 * 16 + 3;
        assert_eq!((words[column / 7] >> ((column % 7) * 9)) & 511, 3,
            "{version}: asymmetric column and Y position");
        let section = &chunk.get_list("sections").unwrap()[0];
        assert_eq!(section.get("Y"), Some(&NbtTag::Byte(-4)), "{version}");
        assert!(matches!(section.get("block_states").and_then(|state| state.get("data")),
            Some(NbtTag::LongArray(_))), "{version}: packed long-array missing");
        assert!(matches!(section.get("biomes").and_then(|biomes| biomes.get("palette")),
            Some(NbtTag::List(_))), "{version}: biome NBT missing");
        assert_eq!(
            b.level_dat().tag.get("Data").and_then(|data| data.get("DataVersion")),
            Some(&NbtTag::Int(data_version)), "{version}: level.dat DataVersion"
        );
    }
}
