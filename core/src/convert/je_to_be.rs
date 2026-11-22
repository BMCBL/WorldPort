//! Java → Bedrock conversion.
//!
//! Milestone M1 covers terrain, biomes and chunk records. Block entities,
//! entities, players, maps and ticks follow in later milestones and are counted
//! as skipped for now.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicI64, Ordering};

use bedrock_nbt::NbtTag;
use bedrock_world::block::BlockEntity;
use bedrock_world::{ActorUid, ChunkKey, ChunkPos, ChunkRecordTag};
use bedrock_world::chunk::palette::BlockState;
use bedrock_world::world::CreateOptions;
use bedrock_world::{Dimension, Spawn};
use bytes::Bytes;
use indexmap::IndexMap;
use rayon::prelude::*;

use crate::be::write::{
    BedrockOutput, CHUNK_VERSION_VALUE, FINALIZED_STATE_POPULATED, SECTION_BLOCKS, SectionWrite,
    chunk_keys, dimension_of, encode_biomes, encode_section, to_block_state,
};
use crate::convert::{ConversionOptions, ConversionPlan, Edition, Progress};
use crate::error::{Result, WorldPortError};
use crate::je::chunk::summarize;
use crate::je::nbt as je_nbt;
use crate::je::world::JavaWorld;
use crate::je::{JeChunkPos, JeDimension};
use crate::mapping::biomes::bedrock_biome_id;
use crate::mapping::schema::{JavaCatalog, bedrock_catalog};
use crate::mapping::{BlockMapper, MappedState};
use crate::nbt::{NbtExt, TagValue};
use crate::report::{ReportBuilder, WarningCode};

/// Converts a Java world into a Bedrock world.
pub fn convert(plan: &ConversionPlan, options: &ConversionOptions) -> Result<crate::report::ConversionReport> {
    let mut world = JavaWorld::open(&plan.source)?;
    let mut report = ReportBuilder::new(
        "java",
        "bedrock",
        world.level_name(),
        plan.target.display().to_string(),
    );
    // Pick the bundled catalog closest to the source DataVersion so property
    // domains match what the world actually contains.
    let java_catalog = JavaCatalog::for_data_version(world.data_version())?;
    let source_version = if java_catalog.data_version() == world.data_version() {
        format!("java {}", java_catalog.version())
    } else {
        format!("java DataVersion {}", world.data_version())
    };
    report.set_versions(
        source_version,
        format!(
            "bedrock 1.26.60 (block state {})",
            crate::be::write::target_block_state_version()
        ),
    );
    if java_catalog.data_version() != world.data_version() {
        report.warn(
            WarningCode::Lossy,
            "java-version",
            format!(
                "source DataVersion {} has no bundled catalog; mapping with {} ({})",
                world.data_version(),
                java_catalog.version(),
                java_catalog.data_version()
            ),
        );
    }
    let mapper = BlockMapper::new(java_catalog.to_catalog(), bedrock_catalog());
    if let Some(requested) = plan.target_version.as_deref()
        && !requested.contains("1.26.60")
    {
        report.warn(
            WarningCode::Lossy,
            "bedrock-target",
            format!(
                "Bedrock target {requested} is not supported yet; writing 1.26.60 block states"
            ),
        );
    }

    report.begin_stage("create-world");
    let be_options = create_options(&world)?;
    let mut output = BedrockOutput::create(&plan.target, be_options)?;
    report.end_stage();

    report.begin_stage("terrain");
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(plan.jobs)
        .build()
        .map_err(|error| WorldPortError::other(format!("cannot start worker pool: {error}")))?;
    let mut converted_chunks = 0_u64;
    let uid_counter = AtomicI64::new(1);
    let current_tick = world.total_ticks();
    for dimension in [JeDimension::Overworld, JeDimension::Nether, JeDimension::End] {
        let positions = world.chunk_positions(dimension)?;
        if positions.is_empty() {
            continue;
        }
        options.report(crate::convert::Progress::new(
            "terrain",
            converted_chunks,
            None,
        ));
        for window in positions.chunks(256) {
            options.check_cancelled()?;
            let outcomes: Vec<Result<ChunkOutcome>> = pool.install(|| {
                window
                    .par_iter()
                    .map(|pos| {
                        options.check_cancelled()?;
                        convert_position(
                            &world,
                            &mapper,
                            dimension,
                            *pos,
                            &uid_counter,
                            current_tick,
                            plan.pruning.blocks,
                        )
                    })
                    .collect()
            });
            for outcome in outcomes {
                let outcome = outcome?;
                for (key, value) in outcome.raw {
                    output.put_raw(key, value);
                }
                for (pos, uid, payload) in outcome.actors {
                    output.put_actor(pos, uid, payload);
                }
                report.merge(outcome.report);
                if outcome.converted {
                    converted_chunks += 1;
                    report.counters().chunks += 1;
                    output.finish_chunk()?;
                }
            }
            options.report(crate::convert::Progress::new(
                "terrain",
                converted_chunks,
                None,
            ));
        }
    }
    output.flush()?;
    report.end_stage();

    report.begin_stage("maps");
    options.report(Progress::new("maps", 0, None));
    if plan.features.maps {
        write_maps(&mut output, &world, &mut report)?;
    }
    report.end_stage();

    report.begin_stage("players");
    options.report(Progress::new("players", 0, None));
    if plan.features.players {
        write_player(&mut output, &world, &mut report)?;
    }
    report.end_stage();

    report.begin_stage("level.dat");
    options.report(Progress::new("level.dat", 0, None));
    if plan.features.level_dat {
        let root = level_dat_root(&world, &mut report)?;
        output.write_level_dat(root)?;
    }
    report.end_stage();

    report.begin_stage("finalize");
    options.report(Progress::new("finalize", 0, None));
    output.finish()?;
    report.end_stage();
    Ok(report.finish())
}

/// Converts Java map files into Bedrock `map_<id>` records.
fn write_maps(
    output: &mut BedrockOutput,
    world: &JavaWorld,
    report: &mut ReportBuilder,
) -> Result<()> {
    for (id, path) in world.map_files()? {
        let map = match crate::map_item::read_java_map(&path, id) {
            Ok(map) => map,
            Err(error) => {
                report.warn(
                    WarningCode::Skipped,
                    format!("map_{id}"),
                    format!("map could not be read: {error}"),
                );
                continue;
            }
        };
        let nbt = crate::map_item::write_bedrock_map(&map);
        let bytes = bedrock_nbt::serialize_root(&nbt)
            .map_err(|error| WorldPortError::nbt(format!("map {id}: {error}")))?;
        output.put_raw(
            bytes::Bytes::from(format!("map_{id}")),
            bytes::Bytes::from(bytes),
        );
        report.counters().maps += 1;
    }
    Ok(())
}

/// Converts the local player into Bedrock's `~local_player` record.
fn write_player(
    output: &mut BedrockOutput,
    world: &JavaWorld,
    report: &mut ReportBuilder,
) -> Result<()> {
    // Modern worlds keep `playerdata/<uuid>.dat`; classic ones embed `Data.Player`.
    let player = if let Some((_, path)) = world.player_files()?.into_iter().next() {
        je_nbt::read_gzip(&path).ok()
    } else {
        world
            .data()
            .get("Player")
            .map(|player| je_nbt::NamedTag {
                name: String::new(),
                tag: player.clone(),
            })
    };
    let Some(player) = player else {
        report.warn(
            WarningCode::Skipped,
            "player",
            "no local player data was found in the source world",
        );
        return Ok(());
    };
    let be = crate::player::java_to_bedrock(&player.tag);
    let bytes = bedrock_nbt::serialize_root(&be)
        .map_err(|error| WorldPortError::nbt(format!("player: {error}")))?;
    output.put_raw(
        bytes::Bytes::from_static(b"~local_player"),
        bytes::Bytes::from(bytes),
    );
    report.counters().players += 1;
    Ok(())
}

fn create_options(world: &JavaWorld) -> Result<CreateOptions> {
    let data = world.data();
    let spawn = spawn_of(world);
    Ok(CreateOptions {
        level_name: world.level_name(),
        seed: world.seed(),
        spawn,
        network_version: None,
        storage_version: 9,
        level_dat_version: 10,
        game_mode: match data.get_int("GameType").unwrap_or(0) {
            1 => bedrock_world::world::BedrockGameMode::Creative,
            2 => bedrock_world::world::BedrockGameMode::Adventure,
            _ => bedrock_world::world::BedrockGameMode::Survival,
        },
        difficulty: match difficulty_name(data) {
            "peaceful" => bedrock_world::world::BedrockDifficulty::Peaceful,
            "easy" => bedrock_world::world::BedrockDifficulty::Easy,
            "hard" => bedrock_world::world::BedrockDifficulty::Hard,
            _ => bedrock_world::world::BedrockDifficulty::Normal,
        },
        commands_enabled: data.get_bool("allowCommands").unwrap_or(false),
        daylight_cycle: true,
        last_opened_with_version: None,
    })
}

fn difficulty_name(data: &NbtTag) -> &'static str {
    if let Some(settings) = data.get("difficulty_settings")
        && let Some(name) = settings.get_str("difficulty")
    {
        return match name {
            "peaceful" => "peaceful",
            "easy" => "easy",
            "normal" => "normal",
            "hard" => "hard",
            _ => "normal",
        };
    }
    match data.get_int("Difficulty") {
        Some(0) => "peaceful",
        Some(1) => "easy",
        Some(2) => "normal",
        _ => "hard",
    }
}

fn spawn_of(world: &JavaWorld) -> Spawn {
    let data = world.data();
    // 26.x stores `spawn: { pos: [x,y,z], yaw, pitch, dimension }`.
    if let Some(spawn) = data.get("spawn")
        && let Some(NbtTag::IntArray(pos)) = spawn.get("pos")
        && pos.len() >= 3
    {
        return Spawn {
            x: pos[0],
            y: pos[1],
            z: pos[2],
        };
    }
    Spawn {
        x: data.get_int("SpawnX").and_then(|v| i32::try_from(v).ok()).unwrap_or(0),
        y: data.get_int("SpawnY").and_then(|v| i32::try_from(v).ok()).unwrap_or(64),
        z: data.get_int("SpawnZ").and_then(|v| i32::try_from(v).ok()).unwrap_or(0),
    }
}

/// Records one converted chunk contributes, committed in input order.
struct ChunkOutcome {
    raw: Vec<(Bytes, Bytes)>,
    actors: Vec<(ChunkPos, ActorUid, NbtTag)>,
    report: ReportBuilder,
    converted: bool,
}

/// Per-chunk write buffer; the driver replays it into [`BedrockOutput`].
#[derive(Default)]
struct ChunkSink {
    raw: Vec<(Bytes, Bytes)>,
    actors: Vec<(ChunkPos, ActorUid, NbtTag)>,
}

impl ChunkSink {
    fn put_raw(&mut self, key: Bytes, value: Bytes) {
        self.raw.push((key, value));
    }

    fn put_actor(&mut self, pos: ChunkPos, uid: ActorUid, payload: NbtTag) {
        self.actors.push((pos, uid, payload));
    }
}

thread_local! {
    /// One region cache per worker thread: no locking, handles stay private.
    static REGIONS: std::cell::RefCell<crate::je::RegionCache> =
        std::cell::RefCell::new(crate::je::RegionCache::default());
}

/// Reads and converts one Java chunk on a worker thread.
fn convert_position(
    world: &JavaWorld,
    mapper: &BlockMapper,
    dimension: JeDimension,
    pos: JeChunkPos,
    uid_counter: &AtomicI64,
    current_tick: i64,
    policy: crate::convert::UnknownDataPolicy,
) -> Result<ChunkOutcome> {
    let mut report = ReportBuilder::new("java", "bedrock", "", "");
    let chunk = REGIONS.with_borrow_mut(|cache| world.read_chunk_cached(cache, dimension, pos));
    let Some(chunk) = chunk? else {
        report.warn(WarningCode::Skipped, pos.label(), "chunk vanished while reading");
        return Ok(ChunkOutcome {
            raw: Vec::new(),
            actors: Vec::new(),
            report,
            converted: false,
        });
    };
    let summary = summarize(&chunk);
    if !summary.status.ends_with("full") {
        report.warn(
            WarningCode::Skipped,
            summary.status.clone(),
            format!("chunk {} is not fully generated and was skipped", pos.label()),
        );
        return Ok(ChunkOutcome {
            raw: Vec::new(),
            actors: Vec::new(),
            report,
            converted: false,
        });
    }
    let mut sink = ChunkSink::default();
    convert_chunk(
        &mut sink,
        &mut report,
        mapper,
        world,
        &chunk,
        pos,
        dimension,
        uid_counter,
        current_tick,
        policy,
    )?;
    Ok(ChunkOutcome {
        raw: sink.raw,
        actors: sink.actors,
        report,
        converted: true,
    })
}

fn convert_chunk(
    sink: &mut ChunkSink,
    report: &mut ReportBuilder,
    mapper: &BlockMapper,
    world: &JavaWorld,
    chunk: &NbtTag,
    pos: JeChunkPos,
    dimension: JeDimension,
    uid_counter: &AtomicI64,
    current_tick: i64,
    policy: crate::convert::UnknownDataPolicy,
) -> Result<()> {
    let be_pos = ChunkPos {
        x: pos.x,
        z: pos.z,
        dimension: dimension_of(dimension),
    };
    let keys = chunk_keys(be_pos);
    let mut mapped_cache: BTreeMap<(String, Vec<(String, String)>), MappedState> = BTreeMap::new();

    // Sections, in the Bedrock subchunk grid.
    let mut sections: Vec<SectionWrite> = Vec::new();
    if let Some(list) = chunk.get_list("sections") {
        for section in list {
            let y = section
                .get_int("Y")
                .and_then(|value| i8::try_from(value).ok())
                .unwrap_or(0);
            if let Some(write) = convert_section(
                section,
                y,
                mapper,
                &mut mapped_cache,
                report,
                policy,
            )? {
                sections.push(write);
            } else {
                // Still record biomes for sections without blocks.
                if let Some(biome) = convert_biomes(section) {
                    sections.push(SectionWrite {
                        y,
                        primary: vec![0; SECTION_BLOCKS],
                        primary_palette: vec![air_state()],
                        liquid: None,
                        liquid_palette: Vec::new(),
                        biomes: biome,
                    });
                }
            }
        }
    }

    let mut height_map = [0_i16; 256];
    let mut quart_biomes: Vec<[u32; 64]> = Vec::new();
    let min_y = sections.iter().map(|section| section.y).min().unwrap_or(-4);
    let max_y = sections.iter().map(|section| section.y).max().unwrap_or(19);
    for y in min_y..=max_y {
        let biome = sections
            .iter()
            .find(|section| section.y == y)
            .map(|section| section.biomes)
            .unwrap_or([1_u32; 64]);
        quart_biomes.push(biome);
    }

    for section in &sections {
        let base_y = i32::from(section.y) * 16;
        for x in 0..16_usize {
            for z in 0..16_usize {
                let mut top = 0_i32;
                for y in 0..16_usize {
                    let index = x * 256 + z * 16 + y;
                    if section.primary.get(index).copied().unwrap_or(0) != 0 {
                        top = base_y + y as i32 + 1;
                    }
                }
                let column = z * 16 + x;
                height_map[column] = height_map[column].max(i16::try_from(top).unwrap_or(i16::MAX));
            }
        }
        let bytes = encode_section(section)?;
        sink.put_raw(keys.subchunk(section.y).encode(), bytes);
        report.counters().sections += 1;
        for index in &section.primary {
            if *index != 0 {
                report.counters().blocks += 1;
            }
        }
    }

    if !sections.is_empty() {
        let bytes = encode_biomes(&height_map, &quart_biomes)?;
        sink.put_raw(keys.data3d.encode(), bytes);
    }
    sink.put_raw(keys.version.encode(), bytes::Bytes::from(vec![CHUNK_VERSION_VALUE]));
    sink.put_raw(
        keys.finalized.encode(),
        bytes::Bytes::from(FINALIZED_STATE_POPULATED.to_le_bytes().to_vec()),
    );

    // Block entities: translate payloads into one `BlockEntity` record.
    let mut block_entities: Vec<NbtTag> = Vec::new();
    if let Some(entities) = chunk.get_list("block_entities") {
        for entity in entities {
            if let Some(converted) = crate::block_entity::java_to_bedrock(entity, report) {
                block_entities.push(NbtTag::Compound(converted));
            }
        }
    }
    if !block_entities.is_empty() {
        let bytes = bedrock_nbt::serialize_consecutive_roots(&block_entities)
            .map_err(|error| WorldPortError::nbt(format!("block entities: {error}")))?;
        sink.put_raw(
            ChunkKey::new(be_pos, ChunkRecordTag::BlockEntity).encode(),
            bytes::Bytes::from(bytes),
        );
    }

    // Entities: legacy inline `Entities` plus the 1.17+ `entities/` region file.
    let mut je_entities: Vec<NbtTag> = chunk
        .get_list("Entities")
        .map(<[NbtTag]>::to_vec)
        .unwrap_or_default();
    je_entities.extend(REGIONS.with_borrow_mut(|cache| {
        world.read_entity_chunk_cached(cache, dimension, pos)
    })?);
    if !je_entities.is_empty() {
        for entity in &je_entities {
            let uid = uid_counter.fetch_add(1, Ordering::Relaxed);
            let Some(payload) = crate::entity::java_to_bedrock(entity, uid, report) else {
                continue;
            };
            let actor_pos = crate::entity::chunk_of(&payload).unwrap_or((be_pos.x, be_pos.z));
            sink.put_actor(
                ChunkPos {
                    x: actor_pos.0,
                    z: actor_pos.1,
                    dimension: be_pos.dimension,
                },
                ActorUid::from_unique_id(uid),
                payload,
            );
        }
    }

    // Scheduled ticks: read the unified model, then write Bedrock's record.
    let ticks = crate::tick::read_java_chunk(chunk);
    if !ticks.is_empty() {
        let state_at = |x: i32, y: i32, z: i32| -> Option<(String, BTreeMap<String, NbtTag>)> {
            let section_y = i8::try_from(y.div_euclid(16)).ok()?;
            let section = sections.iter().find(|section| section.y == section_y)?;
            let lx = usize::try_from(x.rem_euclid(16)).ok()?;
            let ly = usize::try_from(y.rem_euclid(16)).ok()?;
            let lz = usize::try_from(z.rem_euclid(16)).ok()?;
            let index = section.primary.get(lx * 256 + lz * 16 + ly).copied()?;
            let state = section.primary_palette.get(usize::from(index))?;
            Some((state.name.clone(), state.states.clone()))
        };
        let (record, stats) = crate::tick::write_bedrock_pending(
            &ticks,
            current_tick,
            crate::be::write::target_block_state_version(),
            state_at,
        );
        report.counters().ticks += stats.written as u64;
        if stats.pruned > 0 {
            report.warn(
                WarningCode::Pruned,
                "ticks",
                format!("{} scheduled ticks referenced non-vanilla blocks", stats.pruned),
            );
        }
        if let Some(record) = record {
            let bytes = bedrock_nbt::serialize_root(&record)
                .map_err(|error| WorldPortError::nbt(format!("ticks: {error}")))?;
            sink.put_raw(
                ChunkKey::new(be_pos, ChunkRecordTag::PendingTicks).encode(),
                bytes::Bytes::from(bytes),
            );
        }
    }

    Ok(())
}

fn air_state() -> BlockState {
    BlockState {
        name: "minecraft:air".to_string(),
        states: BTreeMap::new(),
        version: Some(crate::be::write::target_block_state_version()),
    }
}

fn convert_section(
    section: &NbtTag,
    y: i8,
    mapper: &BlockMapper,
    cache: &mut BTreeMap<(String, Vec<(String, String)>), MappedState>,
    report: &mut ReportBuilder,
    policy: crate::convert::UnknownDataPolicy,
) -> Result<Option<SectionWrite>> {
    let Some(block_states) = section.get("block_states") else {
        return Ok(None);
    };
    let Some(palette) = block_states.get_list("palette") else {
        return Ok(None);
    };
    if palette.is_empty() {
        return Ok(None);
    }
    let entries: Vec<(String, BTreeMap<String, String>)> = palette
        .iter()
        .map(|entry| {
            let name = entry.get_str("Name").unwrap_or("minecraft:air").to_string();
            let properties = entry
                .get("Properties")
                .and_then(|properties| properties.as_compound())
                .map(|properties| {
                    properties
                        .iter()
                        .filter_map(|(key, value)| {
                            value.as_str_value().map(|value| (key.clone(), value.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            (name, properties)
        })
        .collect();

    let data = block_states.get_i64_array("data");
    let bits = palette_bits(entries.len());
    let indices = decode_indices(data, SECTION_BLOCKS, bits);

    let mut primary_palette: Vec<BlockState> = vec![air_state()];
    let mut primary_index_map: BTreeMap<u16, u16> = BTreeMap::new();
    let mut primary = vec![0_u16; SECTION_BLOCKS];

    let mut liquid_palette: Vec<BlockState> = vec![air_state(), water_state()];
    let mut liquid = vec![0_u16; SECTION_BLOCKS];
    let mut has_liquid = false;

    let mut any_block = false;
    for x in 0..16_usize {
        for z in 0..16_usize {
            for local_y in 0..16_usize {
                let je_index = (local_y << 8) | (z << 4) | x;
                let palette_index = indices.get(je_index).copied().unwrap_or(0) as usize;
                let Some((name, properties)) = entries.get(palette_index) else {
                    continue;
                };
                let be_offset = x * 256 + z * 16 + local_y;

                if !crate::mapping::is_vanilla(name) {
                    // Mod/addon block: prune instead of writing an unknown id.
                    report.warn(
                        WarningCode::Pruned,
                        name.clone(),
                        format!("non-vanilla block {name} was pruned"),
                    );
                    continue;
                }
                let short = crate::mapping::short_name(name);
                if short == "air" || short == "cave_air" || short == "void_air" {
                    continue;
                }
                if short == "water" || short == "lava" {
                    has_liquid = true;
                    liquid[be_offset] = if short == "water" { 1 } else { 2 };
                    if short == "lava" && liquid_palette.len() == 2 {
                        liquid_palette.push(lava_state());
                    }
                    continue;
                }

                let waterlogged = properties.get("waterlogged").is_some_and(|value| value == "true");
                let cache_key = (
                    name.clone(),
                    properties
                        .iter()
                        .filter(|(key, _)| key.as_str() != "waterlogged")
                        .map(|(key, value)| (key.clone(), value.clone()))
                        .collect::<Vec<_>>(),
                );
                let mapped = if let Some(mapped) = cache.get(&cache_key) {
                    mapped.clone()
                } else {
                    let mapped = match mapper.je_to_be(name, &properties) {
                        Some(mapped) => mapped,
                        None => {
                            if policy == crate::convert::UnknownDataPolicy::Error {
                                return Err(WorldPortError::unsupported(format!(
                                    "block {name}{properties:?} has no Bedrock mapping"
                                )));
                            }
                            report.unmapped_block(
                                format!("{name}{properties:?}"),
                                format!("block {name} has no Bedrock mapping; air was written"),
                            );
                            MappedState {
                                name: "minecraft:air".to_string(),
                                properties: BTreeMap::new(),
                                lossy: true,
                            }
                        }
                    };
                    cache.insert(cache_key, mapped.clone());
                    mapped
                };
                if mapped.name == "minecraft:air" {
                    continue;
                }
                if mapped.lossy {
                    report.warn(
                        WarningCode::Lossy,
                        mapped.name.clone(),
                        format!("{} lost states the Bedrock block cannot express", mapped.name),
                    );
                }
                let state = to_block_state(&mapped, mapper.bedrock());
                let palette_index = *primary_index_map.entry(palette_index as u16).or_insert_with(|| {
                    primary_palette.push(state.clone());
                    (primary_palette.len() - 1) as u16
                });
                any_block = true;
                primary[be_offset] = palette_index;
                if waterlogged {
                    has_liquid = true;
                    liquid[be_offset] = 1;
                }
            }
        }
    }

    if !any_block && !has_liquid {
        return Ok(None);
    }
    let Some(biomes) = convert_biomes(section) else {
        return Ok(None);
    };
    Ok(Some(SectionWrite {
        y,
        primary,
        primary_palette,
        liquid: has_liquid.then_some(liquid),
        liquid_palette,
        biomes,
    }))
}

fn convert_biomes(section: &NbtTag) -> Option<[u32; 64]> {
    let biomes = section.get("biomes")?;
    let palette = biomes.get_list("palette")?;
    let names: Vec<u32> = palette
        .iter()
        .map(|entry| {
            let name = entry.as_str_value().unwrap_or("minecraft:plains");
            bedrock_biome_id(name).unwrap_or(1)
        })
        .collect();
    let mut result = [names.first().copied().unwrap_or(1); 64];
    if names.len() > 1 {
        let bits = biomes_bits(names.len());
        if let Some(data) = biomes.get_i64_array("data") {
            for (index, value) in decode_indices(Some(data), 64, bits).iter().enumerate() {
                result[index] = names.get(*value as usize).copied().unwrap_or(1);
            }
        }
    }
    Some(result)
}

/// Java palette index width: at least 4 bits for blocks.
fn palette_bits(len: usize) -> u32 {
    let needed = usize::BITS - len.saturating_sub(1).leading_zeros();
    needed.max(4)
}

/// Java biome palette index width: at least 1 bit when more than one entry.
fn biomes_bits(len: usize) -> u32 {
    let needed = usize::BITS - len.saturating_sub(1).leading_zeros();
    needed.max(1)
}

/// Decodes Java's non-straddling packed long array.
fn decode_indices(data: Option<&[i64]>, count: usize, bits: u32) -> Vec<u16> {
    if bits == 0 || data.is_none() {
        return vec![0; count];
    }
    let data = data.unwrap();
    let per_long = (64 / bits) as usize;
    let mask = (1_u64 << bits) - 1;
    let mut values = vec![0_u16; count];
    // Mirror of the writer: unpack one source word into its lanes instead of
    // re-deriving the word index and shift for every single entry.
    for (word_index, word) in data.iter().enumerate() {
        let base = word_index * per_long;
        let word = *word as u64;
        for lane in 0..per_long {
            let Some(slot) = values.get_mut(base + lane) else {
                break;
            };
            *slot = ((word >> (lane as u32 * bits)) & mask) as u16;
        }
    }
    values
}

fn water_state() -> BlockState {
    let mut states = BTreeMap::new();
    states.insert("liquid_depth".to_string(), NbtTag::Int(0));
    BlockState {
        name: "minecraft:water".to_string(),
        states,
        version: Some(crate::be::write::target_block_state_version()),
    }
}

fn lava_state() -> BlockState {
    let mut states = BTreeMap::new();
    states.insert("liquid_depth".to_string(), NbtTag::Int(0));
    BlockState {
        name: "minecraft:lava".to_string(),
        states,
        version: Some(crate::be::write::target_block_state_version()),
    }
}

fn level_dat_root(world: &JavaWorld, report: &mut ReportBuilder) -> Result<NbtTag> {
    let data = world.data();
    let spawn = spawn_of(world);
    let mut root = IndexMap::new();
    root.insert(
        "LevelName".to_string(),
        NbtTag::String(world.level_name()),
    );
    root.insert("RandomSeed".to_string(), NbtTag::Long(world.seed()));
    root.insert("SpawnX".to_string(), NbtTag::Int(spawn.x));
    root.insert("SpawnY".to_string(), NbtTag::Int(spawn.y));
    root.insert("SpawnZ".to_string(), NbtTag::Int(spawn.z));
    root.insert(
        "GameType".to_string(),
        NbtTag::Int(i32::try_from(data.get_int("GameType").unwrap_or(0)).unwrap_or(0)),
    );
    root.insert(
        "Difficulty".to_string(),
        NbtTag::Int(match difficulty_name(data) {
            "peaceful" => 0,
            "easy" => 1,
            "normal" => 2,
            _ => 3,
        }),
    );
    root.insert(
        "Time".to_string(),
        NbtTag::Long(data.get_int("Time").unwrap_or(0)),
    );
    root.insert(
        "currentTick".to_string(),
        NbtTag::Long(world.total_ticks()),
    );
    root.insert(
        "commandsEnabled".to_string(),
        NbtTag::Byte(i8::from(data.get_bool("allowCommands").unwrap_or(false))),
    );
    root.insert(
        "NetworkVersion".to_string(),
        NbtTag::Int(1_260_600),
    );
    root.insert("StorageVersion".to_string(), NbtTag::Int(9));
    root.insert("WorldVersion".to_string(), NbtTag::Int(1));
    root.insert("MultiplayerGame".to_string(), NbtTag::Byte(1));
    root.insert("LANBroadcast".to_string(), NbtTag::Byte(1));
    root.insert("PlatformBroadcastIntent".to_string(), NbtTag::Int(3));
    root.insert("XBLBroadcastIntent".to_string(), NbtTag::Int(3));
    root.insert("isFromWorldTemplate".to_string(), NbtTag::Byte(0));
    root.insert("isWorldTemplateOptionLocked".to_string(), NbtTag::Byte(0));
    root.insert(
        "LightningLevel".to_string(),
        NbtTag::Float(if world.thundering() { 1.0 } else { 0.0 }),
    );
    root.insert(
        "RainLevel".to_string(),
        NbtTag::Float(if world.raining() { 1.0 } else { 0.0 }),
    );
    root.insert(
        "LightningTime".to_string(),
        NbtTag::Int(i32::try_from(world.thunder_time()).unwrap_or(i32::MAX)),
    );
    root.insert(
        "RainTime".to_string(),
        NbtTag::Int(i32::try_from(world.rain_time()).unwrap_or(i32::MAX)),
    );
    root.insert(
        "lastOpenedWithVersion".to_string(),
        NbtTag::List(vec![
            NbtTag::Int(1),
            NbtTag::Int(26),
            NbtTag::Int(60),
            NbtTag::Int(0),
            NbtTag::Int(0),
        ]),
    );
    for (name, value) in world.game_rules() {
        let row = crate::mapping::game_rules::find_by_java(&name);
        let key = crate::mapping::game_rules::bedrock_key(&name);
        let Some(tag) = gamerule_tag(&value) else {
            report.warn(
                WarningCode::DroppedField,
                key.clone(),
                "gamerule value is not a boolean or integer",
            );
            continue;
        };
        // Inverted rules (elytra movement check) flip between editions.
        let tag = match (row, tag) {
            (Some((_, _, _, _, true)), NbtTag::Byte(value)) => NbtTag::Byte(i8::from(value == 0)),
            (_, tag) => tag,
        };
        root.insert(key, tag);
    }
    Ok(NbtTag::Compound(root))
}

/// Normalises a Java gamerule value to a Bedrock flat tag.
fn gamerule_tag(value: &NbtTag) -> Option<NbtTag> {
    match value {
        NbtTag::String(text) => {
            if text == "true" || text == "false" {
                Some(NbtTag::Byte(i8::from(text == "true")))
            } else {
                text.parse::<i32>().ok().map(NbtTag::Int)
            }
        }
        NbtTag::Byte(value) => Some(NbtTag::Byte(*value)),
        NbtTag::Short(value) => Some(NbtTag::Int(i32::from(*value))),
        NbtTag::Int(value) => Some(NbtTag::Int(*value)),
        NbtTag::Long(value) => Some(NbtTag::Int(i32::try_from(*value).ok()?)),
        _ => None,
    }
}

/// Result of mapping one Java chunk status to a Bedrock dimension.
#[must_use]
pub const fn bedrock_dimension(dimension: JeDimension) -> Dimension {
    dimension_of(dimension)
}

/// Builds a Bedrock block entity from converted data (used by later milestones).
#[must_use]
pub fn passthrough_block_entity(id: &str, x: i32, y: i32, z: i32) -> BlockEntity {
    crate::be::write::block_entity(id, x, y, z, IndexMap::new())
}

/// Placeholder to keep `je_nbt` in scope for later milestones.
#[allow(dead_code)]
fn _read_raw(path: &std::path::Path) -> Result<je_nbt::NamedTag> {
    je_nbt::read_gzip(path)
}

/// Reports the target edition of this module.
#[must_use]
pub const fn target_edition() -> Edition {
    Edition::Bedrock
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_java_packed_indices() {
        // 4 bits per entry, 16 per long, first long holds indices 0x0..0xF.
        let word = 0x0123_4567_89AB_CDEF_i64;
        let values = decode_indices(Some(&[word]), 16, 4);
        assert_eq!(values[0], 0xF);
        assert_eq!(values[1], 0xE);
        assert_eq!(values[15], 0x0);
    }

    #[test]
    fn rounds_palette_bits_up() {
        assert_eq!(palette_bits(1), 4);
        assert_eq!(palette_bits(2), 4);
        assert_eq!(palette_bits(17), 5);
        assert_eq!(biomes_bits(2), 1);
        assert_eq!(biomes_bits(5), 3);
    }
}
