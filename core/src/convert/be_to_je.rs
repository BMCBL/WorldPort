//! Bedrock → Java conversion.
//!
//! Milestone M2 covers terrain, biomes, block entities (raw passthrough) and
//! `level.dat`. Entities, players, maps and ticks follow in M3.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bedrock_nbt::NbtTag;
use bedrock_world::world::{OpenOptions, World};
use bedrock_world::{
    ChunkKey, ChunkPos, ChunkRecordTag, Dimension, SubChunkDecodeMode, SubChunkFormat,
};
use flate2::Compression;
use indexmap::IndexMap;
use rayon::prelude::*;

use crate::convert::{ConversionOptions, ConversionPlan, Progress};
use crate::error::{Result, WorldPortError, io_result};
use crate::je::anvil::write_region_file;
use crate::je::nbt as je_nbt;
use crate::je::{JeBlockPos, JeChunkPos, JeDimension};
use crate::mapping::biomes::java_biome_name;
use crate::mapping::schema::bedrock_catalog;
use crate::mapping::{BlockMapper, MappedState};
use crate::nbt::{NbtExt, TagValue, compound, empty_compound, tag_list, tag_string};
use crate::report::{ReportBuilder, WarningCode};

/// Oldest Java target whose chunk layout this writer emits (1.18 sections).
pub const MIN_TARGET_DATA_VERSION: i32 = 2860;
/// 1.21 moved vanilla dimensions into `dimensions/<namespace>/<path>/`.
const DIMENSIONS_LAYOUT_DATA_VERSION: i32 = 3953;
/// 26.1 split `level.dat` into `data/minecraft/*.dat`, `players/data` and `data/minecraft/maps`.
const SPLIT_DATA_VERSION: i32 = 4786;
/// 1.16 moved map `dimension` from a byte to a string id.
const MAP_DIMENSION_STRING_DATA_VERSION: i32 = 2586;

/// Everything that depends on the Java target version.
struct JavaTarget {
    data_version: i32,
    catalog: &'static crate::mapping::schema::JavaCatalog,
    dimensions_layout: bool,
    split_data: bool,
}

impl JavaTarget {
    fn resolve(plan: &ConversionPlan) -> Result<Self> {
        let catalog = match plan.target_version.as_deref() {
            Some(value) => crate::mapping::schema::JavaCatalog::for_version_or_data_version(value)?,
            None => crate::mapping::schema::JavaCatalog::for_data_version(i32::MAX)?,
        };
        let data_version = catalog.data_version();
        if data_version < MIN_TARGET_DATA_VERSION {
            return Err(WorldPortError::unsupported(format!(
                "Java target {} is too old: only 1.18+ chunk layouts are written",
                catalog.version()
            )));
        }
        Ok(Self {
            data_version,
            catalog,
            dimensions_layout: data_version >= DIMENSIONS_LAYOUT_DATA_VERSION,
            split_data: data_version >= SPLIT_DATA_VERSION,
        })
    }

    fn dimension_dir(&self, root: &Path, dimension: JeDimension) -> PathBuf {
        if self.dimensions_layout {
            root.join("dimensions")
                .join("minecraft")
                .join(dimension.folder_name())
        } else {
            let legacy = dimension.legacy_folder_name();
            if legacy.is_empty() {
                root.to_path_buf()
            } else {
                root.join(legacy)
            }
        }
    }

    fn status(&self) -> &'static str {
        if self.data_version >= DIMENSIONS_LAYOUT_DATA_VERSION {
            "minecraft:full"
        } else {
            "full"
        }
    }

    fn players_dir(&self, root: &Path) -> PathBuf {
        if self.split_data {
            root.join("players").join("data")
        } else {
            root.join("playerdata")
        }
    }

    fn maps_dir(&self, root: &Path) -> PathBuf {
        if self.split_data {
            root.join("data").join("minecraft").join("maps")
        } else {
            root.join("data")
        }
    }

    fn map_dimension_string(&self) -> bool {
        self.data_version >= MAP_DIMENSION_STRING_DATA_VERSION
    }

    /// Java stores POI regions under `<dimension>/poi/` for every layout.
    fn poi_dir(&self, root: &Path, dimension: JeDimension) -> PathBuf {
        self.dimension_dir(root, dimension).join("poi")
    }
}

/// Converts a Bedrock world into a Java world.
pub fn convert(
    plan: &ConversionPlan,
    options: &ConversionOptions,
) -> Result<crate::report::ConversionReport> {
    let mut open = OpenOptions::default();
    open.read_only = true;
    let world = World::open(&plan.source, open).map_err(|error| {
        WorldPortError::invalid(format!(
            "cannot open Bedrock world {}: {error}",
            plan.source.display()
        ))
    })?;
    let level_dat = world.read_level_dat()?;
    let level_name = level_dat
        .root
        .get_str("LevelName")
        .unwrap_or("world")
        .to_string();

    let mut report = ReportBuilder::new(
        "bedrock",
        "java",
        level_name.clone(),
        plan.target.display().to_string(),
    );
    let target = JavaTarget::resolve(plan)?;
    let mapper = BlockMapper::new(target.catalog.to_catalog(), bedrock_catalog());
    report.set_versions(
        "bedrock 1.26.60",
        format!("java {} ({})", target.catalog.version(), target.data_version),
    );

    report.begin_stage("maps");
    options.report(Progress::new("maps", 0, None));
    if plan.features.maps {
        write_maps(&plan.target, &world, &target, &mut report)?;
    }
    report.end_stage();

    report.begin_stage("players");
    options.report(Progress::new("players", 0, None));
    let player_uuid = write_players(&plan.target, &world, &target, &mut report)?;
    report.end_stage();

    report.begin_stage("poi");
    options.report(Progress::new("poi", 0, None));
    if plan.features.village_interests {
        write_poi(&plan.target, &world, &target, plan, &mut report)?;
    }
    report.end_stage();

    report.begin_stage("level.dat");
    options.report(Progress::new("level.dat", 0, None));
    write_java_level(
        &plan.target,
        &world,
        &level_name,
        player_uuid,
        plan,
        &target,
        &mut report,
    )?;
    report.end_stage();

    report.begin_stage("terrain");
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(plan.jobs)
        .build()
        .map_err(|error| WorldPortError::other(format!("cannot start worker pool: {error}")))?;
    let mut converted = 0_u64;
    // One key scan for every dimension; filtering three times would walk the
    // whole database three times.
    let all_positions = world.chunk_positions(Default::default())?;
    for (dimension, je_dimension) in [
        (Dimension::Overworld, JeDimension::Overworld),
        (Dimension::Nether, JeDimension::Nether),
        (Dimension::End, JeDimension::End),
    ] {
        let mut positions: Vec<ChunkPos> = all_positions
            .iter()
            .filter(|pos| pos.dimension == dimension)
            .copied()
            .collect();
        positions.sort_by_key(|pos| (pos.x, pos.z));
        if let Some(limit) = plan.chunk_limit {
            positions.truncate(limit.saturating_sub(usize::try_from(converted).unwrap_or(0)));
        }
        if positions.is_empty() {
            continue;
        }
        options.report(Progress::new("terrain", converted, Some(0)));
        // Group the dimension by region file and write each region as soon as
        // its chunks are converted. Holding every region's encoded payloads
        // until the dimension finished kept ~700 MB resident on a large world;
        // one region is at most 1024 chunks (~7 MB).
        let mut by_region: BTreeMap<(i32, i32), Vec<ChunkPos>> = BTreeMap::new();
        for pos in positions {
            by_region
                .entry((pos.x.div_euclid(32), pos.z.div_euclid(32)))
                .or_default()
                .push(pos);
        }
        let dimension_dir = target.dimension_dir(&plan.target, je_dimension);
        let region_dir = dimension_dir.join("region");
        let entity_dir = dimension_dir.join("entities");
        for ((region_x, region_z), region_positions) in by_region {
            options.check_cancelled()?;
            let outcomes: Vec<Result<(Option<ConvertedChunk>, ReportBuilder)>> = pool.install(|| {
                region_positions
                    .par_iter()
                    .map(|pos| {
                        options.check_cancelled()?;
                        convert_position(&world, *pos, &mapper, &target, plan.pruning.blocks)
                    })
                    .collect()
            });
            let mut chunks: Vec<(u8, u8, Vec<u8>)> = Vec::with_capacity(region_positions.len());
            let mut entities: Vec<(u8, u8, Vec<u8>)> = Vec::new();
            for (pos, outcome) in region_positions.iter().zip(outcomes) {
                let (chunk, worker) = outcome?;
                report.merge(worker);
                let je_pos = JeChunkPos::new(pos.x, pos.z);
                let Some(chunk) = chunk else {
                    report.warn(
                        WarningCode::Skipped,
                        je_pos.label(),
                        "chunk has no convertible section",
                    );
                    continue;
                };
                let (lx, lz) = je_pos.local();
                // 1.17+ stores entities in their own region files.
                if let Some(entity_payload) = chunk.entities {
                    entities.push((lx, lz, entity_payload));
                }
                chunks.push((lx, lz, chunk.payload));
                converted += 1;
                report.counters().chunks += 1;
            }
            // Compression inside the writer runs on this pool as well.
            pool.install(|| -> Result<()> {
                if !chunks.is_empty() {
                    let path = region_dir.join(format!("r.{region_x}.{region_z}.mca"));
                    write_region_file(&path, chunks, compression_level(plan))?;
                }
                if !entities.is_empty() {
                    let path = entity_dir.join(format!("r.{region_x}.{region_z}.mca"));
                    write_region_file(&path, entities, compression_level(plan))?;
                }
                Ok(())
            })?;
            options.report(Progress::new("terrain", converted, None));
        }
    }
    report.end_stage();

    report.begin_stage("structures");
    options.report(Progress::new("structures", 0, None));
    if plan.features.chunk_metadata {
        write_structures(&world, &mut report)?;
    }
    report.end_stage();
    Ok(report.finish())
}

thread_local! {
    /// Per-worker NBT encode buffer, swapped in and out per chunk.
    static SCRATCH: std::cell::RefCell<Vec<u8>> =
        std::cell::RefCell::new(Vec::with_capacity(64 * 1024));
}

/// One chunk encoded for the Java region writer.
struct ConvertedChunk {
    /// NBT payload (moved into the region entry, so no copy on the way out).
    payload: Vec<u8>,
    entities: Option<Vec<u8>>,
}

/// Reads and encodes one Bedrock chunk on a worker thread.
fn convert_position(
    world: &World,
    pos: ChunkPos,
    mapper: &BlockMapper,
    target: &JavaTarget,
    policy: crate::convert::UnknownDataPolicy,
) -> Result<(Option<ConvertedChunk>, ReportBuilder)> {
    let mut report = ReportBuilder::new("", "", "", "");
    let Some((nbt, entities)) = read_chunk(world, pos, mapper, target, policy, &mut report)? else {
        return Ok((None, report));
    };
    // Reuse one encode buffer per worker; a chunk payload is a few hundred KiB,
    // so allocating it per chunk dominated the allocator for a whole world.
    let mut scratch = SCRATCH.with_borrow_mut(std::mem::take);
    je_nbt::encode_into(&mut scratch, "", &nbt)?;
    let payload = std::mem::replace(&mut scratch, Vec::with_capacity(64 * 1024));
    let entities = if entities.is_empty() {
        None
    } else {
        let entity_chunk = compound([
            ("Entities", tag_list(entities)),
            ("DataVersion", NbtTag::Int(target.data_version)),
            ("Position", NbtTag::IntArray(vec![pos.x, pos.z])),
        ]);
        je_nbt::encode_into(&mut scratch, "", &entity_chunk)?;
        Some(std::mem::replace(&mut scratch, Vec::with_capacity(16 * 1024)))
    };
    SCRATCH.with_borrow_mut(|slot| *slot = scratch);
    Ok((Some(ConvertedChunk { payload, entities }), report))
}

fn compression_level(plan: &ConversionPlan) -> Compression {
    Compression::new(plan.compression_level.min(9))
}

/// Reads one Bedrock chunk and builds the Java chunk NBT.
fn read_chunk(
    world: &World,
    pos: ChunkPos,
    mapper: &BlockMapper,
    target: &JavaTarget,
    policy: crate::convert::UnknownDataPolicy,
    report: &mut ReportBuilder,
) -> Result<Option<(NbtTag, Vec<NbtTag>)>> {
    let (chunk, biome_data) = world.chunk_snapshot(pos)?;
    let biomes = biome_data.map(|data| data.storages);
    let mut sections: Vec<NbtTag> = Vec::new();
    let mut height_map_blocking = [0_i16; 256];
    let mut height_map_surface = [0_i16; 256];
    let mut any_blocks = false;

    // Union of subchunk layers and biome storages decides which sections to emit.
    let mut section_ys: Vec<i8> = Vec::new();
    for y in -4_i8..=19 {
        if let Ok(Some(_)) = chunk.subchunk(y) {
            section_ys.push(y);
        }
    }
    if let Some(biomes) = &biomes {
        for storage in biomes {
            if let Some(y) = storage.y {
                let section_y = i8::try_from(y >> 4).unwrap_or(0);
                if !section_ys.contains(&section_y) {
                    section_ys.push(section_y);
                }
            }
        }
    }
    section_ys.sort_unstable();

    for section_y in section_ys {
        let subchunk = chunk.subchunk(section_y)?;
        let mut primary: Option<(&Vec<bedrock_world::BlockState>, &Vec<u16>)> = None;
        let mut liquid: Option<(&Vec<bedrock_world::BlockState>, &Vec<u16>)> = None;
        if let Some(subchunk) = &subchunk
            && let SubChunkFormat::Paletted { storages, .. } = &subchunk.format
        {
            if let Some(storage) = storages.first()
                && let Some(indices) = storage.indices.as_ref()
            {
                primary = Some((&storage.states, indices));
            }
            if let Some(storage) = storages.get(1)
                && let Some(indices) = storage.indices.as_ref()
            {
                liquid = Some((&storage.states, indices));
            }
        }

        let mut palette: Vec<NbtTag> = vec![compound([("Name", tag_string("minecraft:air"))])];
        let mut indices = vec![0_u16; 4096];
        let mut section_has_blocks = false;

        if let Some((states, packed)) = primary {
            // A section repeats a handful of palette entries: map each unique
            // (state, waterlogged) pair once instead of cloning the name and
            // properties for every one of the 4096 blocks. Slots hold
            // `u32::MAX` until resolved and `palette index + 1` afterwards
            // (`0` = air or dropped), so the hot loop only touches a small vec.
            let liquid_len = liquid.map_or(0, |(states, _)| states.len());
            let stride = liquid_len + 1;
            let mut mapped_slots = vec![u32::MAX; states.len() * stride];
            for x in 0..16_usize {
                for z in 0..16_usize {
                    for local_y in 0..16_usize {
                        let offset = x * 256 + z * 16 + local_y;
                        let Some(state_index) = packed.get(offset).copied() else {
                            continue;
                        };
                        let Some(state) = states.get(usize::from(state_index)) else {
                            continue;
                        };
                        let liquid_index = liquid
                            .and_then(|(liquid_states, liquid_packed)| {
                                let index = liquid_packed.get(offset).copied()?;
                                let liquid_state = liquid_states.get(usize::from(index))?;
                                (liquid_state.name == "minecraft:water").then_some(index)
                            });
                        let slot = usize::from(state_index) * stride
                            + liquid_index.map_or(0, |index| usize::from(index) + 1);
                        let mut mapped = mapped_slots[slot];
                        if mapped == u32::MAX {
                            mapped = map_palette_state(
                                mapper,
                                report,
                                &mut palette,
                                state,
                                liquid_index.is_some(),
                                policy,
                            )?;
                            mapped_slots[slot] = mapped;
                        }
                        if mapped == 0 {
                            continue;
                        }
                        indices[(local_y << 8) | (z << 4) | x] =
                            u16::try_from(mapped - 1).unwrap_or(0);
                        section_has_blocks = true;
                        report.counters().blocks += 1;
                        let world_y = i32::from(section_y) * 16 + local_y as i32;
                        let column = z * 16 + x;
                        height_map_blocking[column] =
                            height_map_blocking[column].max(i16::try_from(world_y + 1).unwrap_or(0));
                        height_map_surface[column] =
                            height_map_surface[column].max(i16::try_from(world_y + 1).unwrap_or(0));
                    }
                }
            }
        }

        // Standalone fluids (no primary block) come from the liquid layer.
        if let Some((liquid_states, liquid_packed)) = liquid {
            let mut water_palette: Option<u16> = None;
            let mut lava_palette: Option<u16> = None;
            for x in 0..16_usize {
                for z in 0..16_usize {
                    for local_y in 0..16_usize {
                        let offset = x * 256 + z * 16 + local_y;
                        let je_index = (local_y << 8) | (z << 4) | x;
                        if indices[je_index] != 0 {
                            continue;
                        }
                        let Some(liquid_index) = liquid_packed.get(offset).copied() else {
                            continue;
                        };
                        let Some(state) = liquid_states.get(usize::from(liquid_index)) else {
                            continue;
                        };
                        let (slot, target) = match state.name.as_str() {
                            "minecraft:water" => (&mut water_palette, "minecraft:water"),
                            "minecraft:lava" => (&mut lava_palette, "minecraft:lava"),
                            _ => continue,
                        };
                        let index = *slot.get_or_insert_with(|| {
                            palette.push(compound([
                                ("Name", tag_string(target)),
                                (
                                    "Properties",
                                    compound([(
                                        "level",
                                        tag_string(state_depth(state)),
                                    )]),
                                ),
                            ]));
                            (palette.len() - 1) as u16
                        });
                        indices[je_index] = index;
                        section_has_blocks = true;
                    }
                }
            }
        }

        any_blocks |= section_has_blocks;
        let mut section = IndexMap::new();
        section.insert("Y".to_string(), NbtTag::Byte(section_y));
        section.insert("block_states".to_string(), block_states_nbt(&palette, &indices));
        if let Some(biome) = section_biomes(biomes.as_deref(), section_y) {
            section.insert("biomes".to_string(), biome);
        }
        sections.push(NbtTag::Compound(section));
    }

    if !any_blocks {
        return Ok(None);
    }

    let mut root = IndexMap::new();
    root.insert(
        "DataVersion".to_string(),
        NbtTag::Int(target.data_version),
    );
    root.insert("xPos".to_string(), NbtTag::Int(pos.x));
    root.insert("zPos".to_string(), NbtTag::Int(pos.z));
    root.insert("yPos".to_string(), NbtTag::Int(-4));
    root.insert(
        "Status".to_string(),
        NbtTag::String(target.status().to_string()),
    );
    root.insert("sections".to_string(), tag_list(sections));
    root.insert(
        "Heightmaps".to_string(),
        compound([
            ("MOTION_BLOCKING", packed_heightmap(&height_map_blocking)),
            ("WORLD_SURFACE", packed_heightmap(&height_map_surface)),
        ]),
    );
    root.insert("isLightOn".to_string(), NbtTag::Byte(0));
    root.insert("structures".to_string(), compound([
        ("starts", empty_compound()),
        ("References", empty_compound()),
    ]));
    // Scheduled ticks: Bedrock record -> unified model -> Java lists.
    let mut block_ticks = tag_list(Vec::new());
    let mut fluid_ticks = tag_list(Vec::new());
    if let Some(record) = chunk
        .records
        .iter()
        .find(|record| record.key.tag == ChunkRecordTag::PendingTicks)
        && let Ok(root_tag) = bedrock_nbt::parse_root(&record.value)
    {
        let (ticks, stats) = crate::tick::read_bedrock_pending(&root_tag);
        report.counters().ticks += stats.written as u64;
        if stats.pruned > 0 {
            report.warn(
                WarningCode::Pruned,
                "ticks",
                format!("{} scheduled ticks referenced non-vanilla blocks", stats.pruned),
            );
        }
        let (blocks, fluids) = crate::tick::write_java_chunk(&ticks);
        block_ticks = blocks;
        fluid_ticks = fluids;
    }
    root.insert("block_ticks".to_string(), block_ticks);
    root.insert("fluid_ticks".to_string(), fluid_ticks);
    root.insert("PostProcessing".to_string(), tag_list(Vec::new()));
    root.insert("InhabitedTime".to_string(), NbtTag::Long(0));
    root.insert("LastUpdate".to_string(), NbtTag::Long(0));

    // Block entities: translate payloads into Java shape.
    let mut block_entities: Vec<NbtTag> = Vec::new();
    if let Some(record) = chunk
        .records
        .iter()
        .find(|record| record.key.tag == ChunkRecordTag::BlockEntity)
        && let Ok(entities) = bedrock_nbt::parse_consecutive_roots(&record.value)
    {
        for entity in &entities {
            if let Some(converted) =
                crate::block_entity::bedrock_to_java(entity, report, target.data_version)
            {
                block_entities.push(converted);
            }
        }
    }
    root.insert("block_entities".to_string(), tag_list(block_entities));

    // Entities from modern actor storage (digp + actorprefix). They are written
    // to the separate `entities/` region file by the caller, matching 1.17+.
    let mut java_entities: Vec<NbtTag> = Vec::new();
    for record in world.actors(pos)? {
        if let Some(entity) =
            crate::entity::bedrock_to_java(&record.entity.nbt, report, target.data_version)
        {
            java_entities.push(entity);
        }
    }

    Ok(Some((NbtTag::Compound(root), java_entities)))
}

/// Maps one Bedrock palette entry to a Java palette slot.
///
/// Returns `palette index + 1`, or `0` when the block is air, non-vanilla or
/// unmapped. Warnings are reported once per unique state instead of per block.
fn map_palette_state(
    mapper: &BlockMapper,
    report: &mut ReportBuilder,
    palette: &mut Vec<NbtTag>,
    state: &bedrock_world::BlockState,
    waterlogged: bool,
    policy: crate::convert::UnknownDataPolicy,
) -> Result<u32> {
    if state.name == "minecraft:air" {
        return Ok(0);
    }
    if !crate::mapping::is_vanilla(&state.name) {
        report.warn(
            WarningCode::Pruned,
            state.name.clone(),
            format!("non-vanilla block {} was pruned", state.name),
        );
        return Ok(0);
    }
    // Merge the liquid layer into `waterlogged` where Java has it.
    let mut properties = state_properties(state);
    if waterlogged {
        properties.insert("waterlogged".to_string(), "true".to_string());
    }
    let mapped = map_state(mapper, report, &state.name, &properties, policy)?;
    Ok(push_palette(palette, mapped))
}

fn push_palette(palette: &mut Vec<NbtTag>, mapped: Option<MappedState>) -> u32 {
    let Some(je) = mapped else {
        return 0;
    };
    palette.push(palette_entry(&je));
    u32::try_from(palette.len()).unwrap_or(u32::MAX)
}

fn map_state(
    mapper: &BlockMapper,
    report: &mut ReportBuilder,
    name: &str,
    properties: &BTreeMap<String, String>,
    policy: crate::convert::UnknownDataPolicy,
) -> Result<Option<MappedState>> {
    let mapped = mapper.be_to_je(name, properties);
    if mapped.is_none() {
        if policy == crate::convert::UnknownDataPolicy::Error {
            return Err(WorldPortError::unsupported(format!(
                "block {name}{properties:?} has no Java mapping"
            )));
        }
        report.unmapped_block(
            format!("{name}{properties:?}"),
            format!("block {name} has no Java mapping; air was written"),
        );
    }
    Ok(mapped)
}

fn state_properties(state: &bedrock_world::BlockState) -> BTreeMap<String, String> {
    state
        .states
        .iter()
        .filter_map(|(key, value)| {
            let value = match value {
                NbtTag::Byte(value) => {
                    if *value == 0 || *value == 1 {
                        (*value == 1).to_string()
                    } else {
                        value.to_string()
                    }
                }
                NbtTag::Int(value) => value.to_string(),
                NbtTag::Short(value) => value.to_string(),
                NbtTag::Long(value) => value.to_string(),
                NbtTag::String(value) => value.clone(),
                _ => return None,
            };
            Some((key.clone(), value))
        })
        .collect()
}

fn state_depth(state: &bedrock_world::BlockState) -> String {
    state
        .states
        .get("liquid_depth")
        .and_then(TagValue::as_i64_value)
        .unwrap_or(0)
        .to_string()
}

fn palette_entry(state: &MappedState) -> NbtTag {
    let mut entry = IndexMap::new();
    entry.insert("Name".to_string(), NbtTag::String(state.name.clone()));
    if !state.properties.is_empty() {
        entry.insert(
            "Properties".to_string(),
            NbtTag::Compound(
                state
                    .properties
                    .iter()
                    .map(|(key, value)| (key.clone(), NbtTag::String(value.clone())))
                    .collect(),
            ),
        );
    }
    NbtTag::Compound(entry)
}

fn block_states_nbt(palette: &[NbtTag], indices: &[u16]) -> NbtTag {
    let mut states = IndexMap::new();
    states.insert("palette".to_string(), tag_list(palette.to_vec()));
    if palette.len() > 1 {
        let bits = palette_bits(palette.len());
        states.insert("data".to_string(), pack_indices(indices, bits));
    }
    NbtTag::Compound(states)
}

fn palette_bits(len: usize) -> u32 {
    let needed = usize::BITS - len.saturating_sub(1).leading_zeros();
    needed.max(4)
}

fn pack_indices(indices: &[u16], bits: u32) -> NbtTag {
    let per_long = (64 / bits) as usize;
    let mask = (1_u64 << bits) - 1;
    let longs = indices.len().div_ceil(per_long);
    let mut words = vec![0_i64; longs];
    // Fill one output word from its `per_long` lanes. Building each word in a
    // register avoids the read-modify-write of the scattered form (measured
    // ~2.7x faster over 4096 indices; every section of every chunk packs).
    for (word_index, word) in words.iter_mut().enumerate() {
        let base = word_index * per_long;
        let mut packed = 0_u64;
        for lane in 0..per_long {
            let Some(value) = indices.get(base + lane) else {
                break;
            };
            packed |= (u64::from(*value) & mask) << (lane as u32 * bits);
        }
        *word = packed as i64;
    }
    NbtTag::LongArray(words)
}

fn packed_heightmap(values: &[i16; 256]) -> NbtTag {
    // 9 bits per entry, seven values per long, `z * 16 + x` order.
    const BITS: u32 = 9;
    const PER_LONG: usize = 7;
    let mut words = vec![0_i64; 256_usize.div_ceil(PER_LONG)];
    for (word_index, word) in words.iter_mut().enumerate() {
        let base = word_index * PER_LONG;
        let mut packed = 0_u64;
        for lane in 0..PER_LONG {
            let Some(value) = values.get(base + lane) else {
                break;
            };
            packed |= (u64::from(*value as u16) & ((1 << BITS) - 1)) << (lane as u32 * BITS);
        }
        *word = packed as i64;
    }
    NbtTag::LongArray(words)
}

fn section_biomes(
    storages: Option<&[bedrock_world::BiomeStorage]>,
    section_y: i8,
) -> Option<NbtTag> {
    let storages = storages?;
    let base_y = i32::from(section_y) * 16;
    let storage = storages
        .iter()
        .find(|storage| storage.y.is_some_and(|y| y >= base_y && y < base_y + 16))
        .or_else(|| storages.iter().find(|storage| storage.y.is_none()))?;
    let mut palette: Vec<String> = Vec::new();
    let mut index_map: BTreeMap<u32, u16> = BTreeMap::new();
    let mut indices = vec![0_u16; 64];
    for qy in 0..4_usize {
        for qz in 0..4_usize {
            for qx in 0..4_usize {
                let x = (qx * 4 + 2) as u8;
                let y = (qy * 4 + 2) as u8;
                let z = (qz * 4 + 2) as u8;
                let id = storage.biome_id_at(x, y, z).unwrap_or(1);
                let index = *index_map.entry(id).or_insert_with(|| {
                    let name = java_biome_name(id)
                        .map_or_else(|| "minecraft:plains".to_string(), |name| {
                            format!("minecraft:{name}")
                        });
                    palette.push(name);
                    (palette.len() - 1) as u16
                });
                indices[(qy * 4 + qz) * 4 + qx] = index;
            }
        }
    }
    if palette.is_empty() {
        return None;
    }
    let palette_len = palette.len();
    let mut nbt = IndexMap::new();
    nbt.insert(
        "palette".to_string(),
        tag_list(palette.into_iter().map(NbtTag::String).collect()),
    );
    if palette_len > 1 {
        let bits = (usize::BITS - (palette_len - 1).leading_zeros()).max(1);
        nbt.insert("data".to_string(), pack_indices(&indices, bits));
    }
    Some(NbtTag::Compound(nbt))
}

fn block_entity_id(id: &str) -> String {
    let short = id.strip_prefix("minecraft:").unwrap_or(id);
    let mapped = match short {
        "Chest" => "chest",
        "TrappedChest" => "trapped_chest",
        "EnderChest" => "ender_chest",
        "Furnace" => "furnace",
        "BlastFurnace" => "blast_furnace",
        "Smoker" => "smoker",
        "Sign" => "sign",
        "HangingSign" => "hanging_sign",
        "Spawner" | "MobSpawner" => "mob_spawner",
        "Skull" => "skull",
        "FlowerPot" => "flower_pot",
        "BrewingStand" => "brewing_stand",
        "Noteblock" => "noteblock",
        "Jukebox" => "jukebox",
        "Comparator" => "comparator",
        "Cauldron" => "cauldron",
        "PistonArm" => "piston",
        "Beacon" => "beacon",
        "Banner" => "banner",
        "StructureBlock" => "structure_block",
        "JigsawBlock" => "jigsaw",
        "ShulkerBox" => "shulker_box",
        "CommandBlock" => "command_block",
        "Bed" => "bed",
        "Lectern" => "lectern",
        "Campfire" => "campfire",
        "Lodestone" => "lodestone",
        "Beehive" => "beehive",
        other => {
            // Most Bedrock ids are the Java path in CamelCase; fall back to the
            // original so the field stays readable in the report.
            let _ = other;
            short
        }
    };
    if mapped.contains(':') {
        mapped.to_string()
    } else {
        format!("minecraft:{mapped}")
    }
}

/// Converts Bedrock players into `playerdata/<uuid>.dat` files.
///
/// Returns the UUID of the first real player so `level.dat` can point at it.
fn write_players(
    root: &Path,
    world: &World,
    target: &JavaTarget,
    report: &mut ReportBuilder,
) -> Result<Option<[i32; 4]>> {
    let ids = world.players()?;
    let mut chosen: Option<[i32; 4]> = None;
    for id in &ids {
        let Some(player) = world.player(id)? else {
            continue;
        };
        // Server identity stubs have no position and no inventory.
        if player.nbt.get_list("Pos").is_none() {
            continue;
        }
        let key = id
            .storage_key()
            .map(|key| key.to_vec())
            .unwrap_or_default();
        let uuid = crate::player::offline_uuid(&key);
        let java = crate::player::bedrock_to_java(&player.nbt, uuid, target.data_version);
        let path = target
            .players_dir(root)
            .join(format!("{}.dat", uuid_string(uuid)));
        je_nbt::write_gzip(&path, "", &java)?;
        report.counters().players += 1;
        chosen.get_or_insert(uuid);
    }
    if chosen.is_none() && !ids.is_empty() {
        report.warn(
            WarningCode::Skipped,
            "players",
            "Bedrock player records contain no gameplay data (server stubs)",
        );
    }
    Ok(chosen)
}

/// Converts Bedrock `map_<id>` records into `data/minecraft/maps/map_<id>.dat`.
fn write_maps(
    root: &Path,
    world: &World,
    target: &JavaTarget,
    report: &mut ReportBuilder,
) -> Result<()> {
    use bedrock_world::storage::{StorageReadOptions, StorageVisitorControl};
    let mut maps = Vec::new();
    world.storage().for_each_prefix(
        b"map_",
        StorageReadOptions::default(),
        &mut |key, value| {
            if let Some(id) = key
                .get(4..)
                .and_then(|suffix| std::str::from_utf8(suffix).ok())
                .and_then(|suffix| suffix.parse::<i64>().ok())
                && let Ok(root) = bedrock_nbt::parse_root(value)
                && let Some(map) = crate::map_item::read_bedrock_map(&root, id)
            {
                maps.push(map);
            }
            Ok(StorageVisitorControl::Continue)
        },
    )?;
    if maps.is_empty() {
        return Ok(());
    }
    let dir = target.maps_dir(root);
    // Java map ids are 32-bit; Bedrock ids are 64-bit, so ids that do not fit are
    // renumbered densely. Items referencing a renumbered map keep their old id.
    let mut used: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
    let mut next_id: i32 = 0;
    let mut max_id: i32 = 0;
    let mut renumbered = 0_u64;
    for map in &mut maps {
        let id = i32::try_from(map.id).ok().filter(|id| *id >= 0 && !used.contains(id));
        let new_id = match id {
            Some(id) => id,
            None => {
                while used.contains(&next_id) {
                    next_id += 1;
                }
                next_id
            }
        };
        if i64::from(new_id) != map.id {
            renumbered += 1;
            map.id = i64::from(new_id);
        }
        used.insert(new_id);
        max_id = max_id.max(new_id);
        crate::map_item::write_java_map(
            &dir,
            map,
            target.data_version,
            target.map_dimension_string(),
        )?;
        report.counters().maps += 1;
    }
    if renumbered > 0 {
        report.warn(
            WarningCode::Lossy,
            "map-ids",
            format!(
                "{renumbered} maps used 64-bit Bedrock ids and were renumbered; item references were not rewritten"
            ),
        );
    }
    crate::map_item::write_idcounts(&dir, i64::from(max_id))?;
    Ok(())
}

fn uuid_string(uuid: [i32; 4]) -> String {
    let bytes: Vec<u8> = uuid
        .iter()
        .flat_map(|value| value.to_be_bytes())
        .collect();
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// Collects Bedrock hardcoded spawn areas and reports them.
///
/// Bedrock's only structure record is this binary box list; Java's equivalent
/// (`structures.starts`) needs piece lists that Bedrock does not store, so the
/// areas are reported rather than turned into fabricated structure starts. The
/// reader walks chunk records directly instead of going through the terrain
/// pass, because a monument or outpost can sit in a chunk with no converted
/// blocks of its own.
fn write_structures(world: &World, report: &mut ReportBuilder) -> Result<()> {
    let positions = world.chunk_positions(Default::default())?;
    // One batched lookup per 4096 chunks instead of a per-chunk prefix scan:
    // the record key is fixed, so a point read is enough and the scan cost stays
    // proportional to the number of chunks rather than to the table layout.
    let mut areas = Vec::new();
    for window in positions.chunks(4096) {
        let keys: Vec<bytes::Bytes> = window
            .iter()
            .map(|pos| ChunkKey::new(*pos, ChunkRecordTag::HardcodedSpawners).encode())
            .collect();
        let values = world.storage().get_many(&keys)?;
        for (pos, value) in window.iter().zip(values) {
            let Some(value) = value else {
                continue;
            };
            match crate::structure::read_bedrock_chunk(&value) {
                Ok(found) => areas.extend(found),
                Err(error) => report.warn(
                    WarningCode::Skipped,
                    format!("hardcoded-spawners:{},{}", pos.x, pos.z),
                    format!("hardcoded spawn areas could not be decoded: {error}"),
                ),
            }
        }
    }
    crate::structure::report_bedrock_areas(&areas, report);
    Ok(())
}

/// Converts Bedrock village records into Java `poi/` region files.
///
/// Bedrock keeps interests per villager, Java per block position, so the
/// conversion regroups and deduplicates them; see `poi::bedrock`.
fn write_poi(
    root: &Path,
    world: &World,
    target: &JavaTarget,
    plan: &ConversionPlan,
    report: &mut ReportBuilder,
) -> Result<()> {
    let villages = world.villages(Default::default())?;
    if villages.is_empty() {
        return Ok(());
    }
    let conversion = crate::poi::convert(&villages, plan.pruning.blocks, report)?;
    if conversion.records() == 0 {
        if conversion.unsupported > 0 {
            report.warn(
                WarningCode::Lossy,
                "village-interests",
                format!(
                    "{} Bedrock interests have no Java POI type; no POI records were written",
                    conversion.unsupported
                ),
            );
        }
        return Ok(());
    }

    // Bedrock village records do not carry a dimension: village keys are global
    // and the interests inside a village are all in one dimension. The world's
    // village data here is overworld-only, which matches where the records came
    // from; interests outside the overworld would need a per-village dimension
    // lookup that the Bedrock format does not store.
    let poi_dir = target.poi_dir(root, JeDimension::Overworld);
    crate::poi::java::ensure_dir(&poi_dir)?;

    // Group chunk positions by region file so each region is written once.
    let mut by_region: BTreeMap<(i32, i32), Vec<(u8, u8, crate::poi::PoiChunk)>> = BTreeMap::new();
    for ((chunk_x, chunk_z), chunk) in &conversion.chunks {
        let region_x = chunk_x.div_euclid(32);
        let region_z = chunk_z.div_euclid(32);
        let local_x = u8::try_from(chunk_x.rem_euclid(32)).unwrap_or(0);
        let local_z = u8::try_from(chunk_z.rem_euclid(32)).unwrap_or(0);
        by_region
            .entry((region_x, region_z))
            .or_default()
            .push((local_x, local_z, chunk.clone()));
    }
    let compression = Compression::new(plan.compression_level.min(9));
    for ((region_x, region_z), entries) in by_region {
        crate::poi::java::validate_region_entries(&entries)?;
        let path = crate::poi::java::region_path(&poi_dir, region_x, region_z);
        crate::poi::java::write_region(&path, entries, target.data_version, compression)?;
    }
    report.counters().village_interests = conversion.written;
    if conversion.unsupported > 0 {
        report.warn(
            WarningCode::Lossy,
            "village-interests-unsupported",
            format!(
                "{} Bedrock interests (beehives, lodestones, lightning rods, portals) have no Java POI type",
                conversion.unsupported
            ),
        );
    }
    if conversion.malformed > 0 {
        report.warn(
            WarningCode::Skipped,
            "village-interests-malformed",
            format!("{} village POI records could not be parsed", conversion.malformed),
        );
    }
    Ok(())
}

fn write_java_level(
    world_root: &Path,
    world: &World,
    level_name: &str,
    player_uuid: Option<[i32; 4]>,
    plan: &ConversionPlan,
    target: &JavaTarget,
    report: &mut ReportBuilder,
) -> Result<()> {
    let level = world.read_level_dat()?;
    let data_dir = world_root.join("data").join("minecraft");
    if target.split_data {
        io_result(&data_dir, std::fs::create_dir_all(&data_dir))?;
    }

    let spawn = [
        level.root.get_int("SpawnX").unwrap_or(0) as i32,
        level.root.get_int("SpawnY").unwrap_or(64) as i32,
        level.root.get_int("SpawnZ").unwrap_or(0) as i32,
    ];
    let difficulty = match level.root.get_int("Difficulty").unwrap_or(2) {
        0 => "peaceful",
        1 => "easy",
        3 => "hard",
        _ => "normal",
    };
    let rain = level.root.get_int("RainLevel").unwrap_or(0) != 0;
    let thunder = level.root.get_int("LightningLevel").unwrap_or(0) != 0;
    let rain_time = level.root.get_int("RainTime").unwrap_or(0) as i32;
    let thunder_time = level.root.get_int("LightningTime").unwrap_or(0) as i32;
    let current_tick = level.root.get_int("currentTick").unwrap_or(0);

    let mut root = IndexMap::new();
    root.insert(
        "DataVersion".to_string(),
        NbtTag::Int(target.data_version),
    );
    root.insert("LevelName".to_string(), NbtTag::String(level_name.to_string()));
    root.insert(
        "Version".to_string(),
        compound([
            ("Id", NbtTag::Int(target.data_version)),
            ("Name", tag_string(target.catalog.version())),
            ("Series", tag_string("main")),
            ("Snapshot", NbtTag::Byte(0)),
        ]),
    );
    root.insert(
        "GameType".to_string(),
        NbtTag::Int(level.root.get_int("GameType").unwrap_or(0) as i32),
    );
    root.insert(
        "Time".to_string(),
        NbtTag::Long(level.root.get_int("Time").unwrap_or(0)),
    );
    root.insert(
        "allowCommands".to_string(),
        NbtTag::Byte(level.root.get_int("commandsEnabled").unwrap_or(0) as i8),
    );
    root.insert("initialized".to_string(), NbtTag::Byte(1));
    root.insert("WasModded".to_string(), NbtTag::Byte(0));
    root.insert("ServerBrands".to_string(), tag_list(vec![tag_string("vanilla")]));
    root.insert("LastPlayed".to_string(), NbtTag::Long(0));
    root.insert(
        "singleplayer_uuid".to_string(),
        NbtTag::IntArray(player_uuid.unwrap_or([0, 0, 0, 0]).to_vec()),
    );
    root.insert("version".to_string(), NbtTag::Int(19133));

    // Gamerules: Bedrock flat keys -> Java names (legacy names before 26.1,
    // namespaced ids with typed values from 26.1 on).
    let mut rules = IndexMap::new();
    if let Some(entries) = level.root.as_compound() {
        for (key, value) in entries {
            if let Some(row) = crate::mapping::game_rules::find_by_bedrock(key) {
                let (_, legacy, namespaced, kind, inverted) = *row;
                let truth = match value {
                    NbtTag::Byte(byte) => Some(*byte != 0),
                    NbtTag::Int(int) => Some(*int != 0),
                    _ => None,
                };
                let number = match value {
                    NbtTag::Int(int) => Some(*int),
                    NbtTag::Byte(byte) => Some(i32::from(*byte)),
                    _ => None,
                };
                if target.split_data {
                    let converted = match (kind, truth, number) {
                        (crate::mapping::game_rules::GameRuleKind::Bool, Some(flag), _) => {
                            NbtTag::Byte(i8::from(if inverted { !flag } else { flag }))
                        }
                        (crate::mapping::game_rules::GameRuleKind::Int, _, Some(number)) => {
                            NbtTag::Int(number)
                        }
                        _ => continue,
                    };
                    rules.insert(namespaced.to_string(), converted);
                } else {
                    let text = match (kind, truth, number) {
                        (crate::mapping::game_rules::GameRuleKind::Bool, Some(flag), _) => {
                            (if inverted { !flag } else { flag }).to_string()
                        }
                        (crate::mapping::game_rules::GameRuleKind::Int, _, Some(number)) => {
                            number.to_string()
                        }
                        _ => continue,
                    };
                    rules.insert(legacy.to_string(), tag_string(text));
                }
            }
        }
    }
    if rules.is_empty() {
        report.warn(
            WarningCode::DroppedField,
            "gamerules",
            "no Bedrock gamerules were found in level.dat",
        );
    }

    if target.split_data {
        root.insert(
            "spawn".to_string(),
            compound([
                ("pos", NbtTag::IntArray(spawn.to_vec())),
                ("pitch", NbtTag::Float(0.0)),
                ("yaw", NbtTag::Float(0.0)),
                ("dimension", tag_string("minecraft:overworld")),
            ]),
        );
        root.insert(
            "difficulty_settings".to_string(),
            compound([
                ("difficulty", tag_string(difficulty)),
                ("hardcore", NbtTag::Byte(0)),
                ("locked", NbtTag::Byte(0)),
            ]),
        );
    } else {
        root.insert("SpawnX".to_string(), NbtTag::Int(spawn[0]));
        root.insert("SpawnY".to_string(), NbtTag::Int(spawn[1]));
        root.insert("SpawnZ".to_string(), NbtTag::Int(spawn[2]));
        root.insert("SpawnAngle".to_string(), NbtTag::Float(0.0));
        root.insert(
            "Difficulty".to_string(),
            NbtTag::Int(match difficulty {
                "peaceful" => 0,
                "easy" => 1,
                "hard" => 3,
                _ => 2,
            }),
        );
        root.insert("hardcore".to_string(), NbtTag::Byte(0));
        root.insert("DifficultyLocked".to_string(), NbtTag::Byte(0));
        root.insert("GameRules".to_string(), NbtTag::Compound(rules.clone()));
        root.insert("raining".to_string(), NbtTag::Byte(i8::from(rain)));
        root.insert("thundering".to_string(), NbtTag::Byte(i8::from(thunder)));
        root.insert("rainTime".to_string(), NbtTag::Int(rain_time));
        root.insert("thunderTime".to_string(), NbtTag::Int(thunder_time));
        root.insert("clearWeatherTime".to_string(), NbtTag::Int(0));
        root.insert("DayTime".to_string(), NbtTag::Long(current_tick));
    }

    let level_dat_path = world_root.join("level.dat");
    je_nbt::write_gzip(&level_dat_path, "", &compound([("Data", NbtTag::Compound(root))]))?;

    if target.split_data {
        // 26.x split world data files.
        let mut world_gen = IndexMap::new();
        world_gen.insert(
            "seed".to_string(),
            NbtTag::Long(level.root.get_int("RandomSeed").unwrap_or(0)),
        );
        world_gen.insert("generate_structures".to_string(), NbtTag::Byte(1));
        world_gen.insert("bonus_chest".to_string(), NbtTag::Byte(0));
        world_gen.insert(
            "dimensions".to_string(),
            compound([
                (
                    "minecraft:overworld",
                    compound([
                        ("type", tag_string("minecraft:overworld")),
                        (
                            "generator",
                            compound([("type", tag_string("minecraft:noise"))]),
                        ),
                    ]),
                ),
                (
                    "minecraft:the_nether",
                    compound([
                        ("type", tag_string("minecraft:the_nether")),
                        (
                            "generator",
                            compound([
                                ("type", tag_string("minecraft:noise")),
                                ("settings", tag_string("minecraft:nether")),
                            ]),
                        ),
                    ]),
                ),
                (
                    "minecraft:the_end",
                    compound([
                        ("type", tag_string("minecraft:the_end")),
                        (
                            "generator",
                            compound([
                                ("type", tag_string("minecraft:noise")),
                                ("settings", tag_string("minecraft:end")),
                            ]),
                        ),
                    ]),
                ),
            ]),
        );
        write_split(&data_dir, "world_gen_settings", world_gen, target.data_version)?;

        // Bedrock omits `dofiretick` unless it was toggled; a missing key means
        // fire spreads, which matches the Java default, so only an explicit
        // value is carried over (it would otherwise show up as an added rule).
        if let Some(fire_allowed) = level
            .root
            .get("dofiretick")
            .and_then(TagValue::as_i64_value)
        {
            rules.insert(
                "minecraft:fire_spread_radius_around_player".to_string(),
                NbtTag::Int(if fire_allowed != 0 { 128 } else { 0 }),
            );
        }
        write_split(&data_dir, "game_rules", rules, target.data_version)?;

        let mut weather = IndexMap::new();
        weather.insert("raining".to_string(), tag_string(rain.to_string()));
        weather.insert("thundering".to_string(), tag_string(thunder.to_string()));
        weather.insert("rain_time".to_string(), NbtTag::Int(rain_time));
        weather.insert("thunder_time".to_string(), NbtTag::Int(thunder_time));
        weather.insert("clear_weather_time".to_string(), NbtTag::Int(0));
        write_split(&data_dir, "weather", weather, target.data_version)?;

        let mut clocks = IndexMap::new();
        clocks.insert(
            "minecraft:overworld".to_string(),
            compound([
                ("total_ticks", NbtTag::Long(current_tick)),
                ("partial_tick", NbtTag::Float(0.0)),
                ("rate", NbtTag::Float(1.0)),
            ]),
        );
        write_split(&data_dir, "world_clocks", clocks, target.data_version)?;
    }

    let _ = plan;
    Ok(())
}

fn write_split(
    dir: &Path,
    name: &str,
    data: IndexMap<String, NbtTag>,
    data_version: i32,
) -> Result<()> {
    let mut root = IndexMap::new();
    root.insert("data".to_string(), NbtTag::Compound(data));
    root.insert("DataVersion".to_string(), NbtTag::Int(data_version));
    let path = dir.join(format!("{name}.dat"));
    je_nbt::write_gzip(&path, "", &NbtTag::Compound(root))
}

/// Builds a Bedrock chunk key for tests and later milestones.
#[must_use]
pub fn chunk_key(pos: ChunkPos, tag: ChunkRecordTag) -> ChunkKey {
    ChunkKey::new(pos, tag)
}

/// Block position helper kept for the entity/block-entity milestones.
#[must_use]
pub const fn block_pos(x: i32, y: i32, z: i32) -> JeBlockPos {
    JeBlockPos::new(x, y, z)
}

/// Default decode mode used when reading subchunks.
#[must_use]
pub const fn decode_mode() -> SubChunkDecodeMode {
    SubChunkDecodeMode::FullIndices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_palette_indices() {
        let mut indices = vec![0_u16; 4096];
        indices[0] = 1;
        indices[4095] = 2;
        let NbtTag::LongArray(words) = pack_indices(&indices, 4) else {
            panic!("long array");
        };
        assert_eq!(words.len(), 256);
        assert_eq!(words[0] & 0xF, 1);
        assert_eq!((words[255] as u64 >> 60) & 0xF, 2);
    }

    #[test]
    fn packs_heightmap_seven_per_long() {
        let mut heights = [0_i16; 256];
        heights[0] = 70;
        heights[6] = 100;
        let NbtTag::LongArray(words) = packed_heightmap(&heights) else {
            panic!("long array");
        };
        assert_eq!(words.len(), 37);
        assert_eq!(words[0] & 0x1FF, 70);
        assert_eq!((words[0] as u64 >> 54) & 0x1FF, 100);
    }

    #[test]
    fn bedrock_block_entity_ids_map_to_java_paths() {
        assert_eq!(block_entity_id("Chest"), "minecraft:chest");
        assert_eq!(block_entity_id("minecraft:Furnace"), "minecraft:furnace");
    }
}
