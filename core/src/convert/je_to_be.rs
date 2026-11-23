//! Java → Bedrock conversion.
//!
//! Milestone M1 covers terrain, biomes and chunk records. Block entities,
//! entities, players, maps and ticks follow in later milestones and are counted
//! as skipped for now.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
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
    let world = JavaWorld::open(&plan.source)?;
    if world.data_version() < super::be_to_je::MIN_TARGET_DATA_VERSION {
        return Err(WorldPortError::unsupported(format!(
            "Java source DataVersion {} can be inspected, but full conversion requires Minecraft Java 1.18 or newer",
            world.data_version()
        )));
    }
    super::prepare_target(&plan.target, plan.overwrite)?;
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
    let mut interests = crate::poi::InterestInventory::default();
    // Structure areas collected during the terrain pass; no second world read.
    let mut structure_areas: Vec<(i32, i32, crate::structure::StructureArea)> = Vec::new();
    let profile_totals = crate::convert::profile::Totals::default();
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
                window.par_iter().map(|pos| {
                    options.check_cancelled()?;
                    convert_position(
                        &world, &mapper, dimension, *pos, &uid_counter,
                        current_tick, &plan.features, plan.pruning.blocks,
                    )
                }).collect()
            });
            for outcome in outcomes {
                let outcome = outcome?;
                for (key, value) in outcome.raw {
                    output.put_raw(key, value);
                }
                for (pos, uid, payload) in outcome.actors {
                    output.put_actor(pos, uid, payload)?;
                }
                report.merge(outcome.report);
                interests.total += outcome.interests.total;
                for (name, count) in outcome.interests.by_name {
                    *interests.by_name.entry(name).or_default() += count;
                }
                for (kind, count) in outcome.interests.by_kind {
                    *interests.by_kind.entry(kind).or_default() += count;
                }
                structure_areas.extend(outcome.structures);
                profile_totals.add(&outcome.timings);
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
    crate::convert::profile::report(&profile_totals.snapshot());

    report.begin_stage("structures");
    options.report(Progress::new("structures", 0, None));
    if plan.features.chunk_metadata {
        write_structures(&mut output, &structure_areas, &mut report)?;
    }
    report.end_stage();

    report.begin_stage("village-interests");
    options.report(Progress::new("village-interests", 0, None));
    if plan.features.village_interests {
        crate::poi::report_inventory(&interests, &mut report);
    }
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
        write_player(&mut output, &world, plan.player.as_deref(), &mut report)?;
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

/// Writes Bedrock hardcoded spawn areas for Java structures that have one.
///
/// Bedrock records only the spawn box, so a Java structure becomes a
/// `HardcodedSpawners` record covering its piece bounds. Structures Bedrock has
/// no hardcoded area for (villages, ancient cities, ...) keep their blocks and
/// are reported instead of being given a fabricated area.
/// Writes Bedrock hardcoded spawn areas for the Java structures found during
/// the terrain pass.
///
/// The areas come from chunks the converter already decoded: a separate pass
/// would re-open every region, re-read and re-inflate every chunk and rebuild
/// its NBT tree only to read `structures.starts`.
fn write_structures(
    output: &mut BedrockOutput,
    areas: &[(i32, i32, crate::structure::StructureArea)],
    report: &mut ReportBuilder,
) -> Result<()> {
    let mut convertible: std::collections::BTreeMap<&'static str, u64> = Default::default();
    let mut by_chunk: BTreeMap<(i32, i32), Vec<crate::structure::StructureArea>> = BTreeMap::new();
    for (chunk_x, chunk_z, area) in areas {
        *convertible.entry(area.kind.java_id()).or_default() += 1;
        by_chunk
            .entry((*chunk_x, *chunk_z))
            .or_default()
            .push(area.clone());
    }
    for ((chunk_x, chunk_z), chunk_areas) in by_chunk {
        let value = crate::structure::write_bedrock_chunk(&chunk_areas)?;
        output.put_raw(
            ChunkKey::new(
                ChunkPos {
                    x: chunk_x,
                    z: chunk_z,
                    dimension: Dimension::Overworld,
                },
                ChunkRecordTag::HardcodedSpawners,
            )
            .encode(),
            Bytes::from(value),
        );
        report.counters().structures += chunk_areas.len() as u64;
    }
    crate::structure::report_java_structures(&convertible, &BTreeMap::new(), report);
    Ok(())
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
    selector: Option<&str>,
    report: &mut ReportBuilder,
) -> Result<()> {
    // Modern worlds keep `playerdata/<uuid>.dat`; classic ones embed `Data.Player`.
    // Taking "the first file" would be arbitrary: a world played by several
    // accounts has several files and the choice would follow sort order. A
    // selector wins when given; otherwise the most recently played player is
    // used, and the decision is always reported so it is never silent.
    let chosen = choose_player(world, selector, report)?;
    let player = if let Some((stem, path)) = chosen {
        report.warn(
            WarningCode::Notice,
            "player",
            format!("converted player {stem} as the Bedrock local player"),
        );
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

/// Picks which Java player becomes Bedrock's local player.
///
/// The default is local-to-local: Java marks the single-player owner in
/// `level.dat` as `Data.SingleplayerUUID`, and that player is the one a Bedrock
/// `~local_player` record corresponds to. Only when that marker is missing or
/// does not resolve does this fall back, and any fallback is reported - a
/// conversion must never silently pick a different account.
fn choose_player(
    world: &JavaWorld,
    selector: Option<&str>,
    report: &mut ReportBuilder,
) -> Result<Option<(String, PathBuf)>> {
    let mut players = world.player_files()?;
    if players.is_empty() {
        return Ok(None);
    }
    if let Some(selector) = selector {
        let needle = selector.to_ascii_lowercase();
        let matched = players
            .iter()
            .position(|(stem, _)| {
                let stem = stem.to_ascii_lowercase();
                stem == needle || stem.starts_with(&needle)
            })
            .ok_or_else(|| {
                WorldPortError::unsupported(format!(
                    "no player matches {selector:?}; this world has {}",
                    players
                        .iter()
                        .map(|(stem, _)| stem.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            })?;
        return Ok(Some(players.remove(matched)));
    }
    // Local to local: the world itself names its single-player owner.
    if let Some(uuid) = singleplayer_uuid(world)
        && let Some(index) = players
            .iter()
            .position(|(stem, _)| stem.eq_ignore_ascii_case(&uuid))
    {
        if players.len() > 1 {
            report.warn(
                WarningCode::Notice,
                "player",
                format!(
                    "converted {uuid}, the world's single-player owner; {} other player                      file(s) were left alone. Pass --player <uuid> to convert a different one",
                    players.len() - 1
                ),
            );
        }
        return Ok(Some(players.remove(index)));
    }
    if players.len() == 1 {
        return Ok(players.into_iter().next());
    }
    // No marker and several accounts: prefer the most recently played file, and
    // say so. Sorting by write time is deterministic on a real save.
    players.sort_by_key(|(_, path)| {
        std::cmp::Reverse(
            std::fs::metadata(path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|since| since.as_millis())
                .unwrap_or(0),
        )
    });
    let alternatives: Vec<String> = players.iter().skip(1).map(|(stem, _)| stem.clone()).collect();
    report.warn(
        WarningCode::Notice,
        "player",
        format!(
            "this world names no single-player owner and has {} players; converted the most              recently played one. Pass --player <uuid> to choose another. Others: {}",
            players.len(),
            alternatives.join(", ")
        ),
    );
    Ok(Some(players.remove(0)))
}

/// The single-player owner Java records in `level.dat`, as a dashed UUID.
fn singleplayer_uuid(world: &JavaWorld) -> Option<String> {
    let raw = world.data().get_i32_array("SingleplayerUUID")?;
    let value = <[i32; 4]>::try_from(raw).ok()?;
    // Java stores the UUID as four big-endian ints.
    Some(format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        value[0] as u32,
        (value[1] >> 16) as u16,
        value[1] as u16,
        (value[2] >> 16) as u16,
        ((u64::from(value[2] as u32) & 0xFFFF) << 32) | u64::from(value[3] as u32)
    ))
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
    /// Village-interest blocks found in this chunk.
    interests: crate::poi::InterestInventory,
    /// Per-phase timings for this chunk.
    timings: crate::convert::profile::ChunkTimings,
    /// Structure areas this chunk declared, with the chunk position they came
    /// from.
    ///
    /// Extracted here, while the chunk NBT is already parsed: a separate pass
    /// would re-open every region, re-read and re-inflate every chunk, and
    /// rebuild the whole NBT tree just to read `structures.starts`.
    structures: Vec<(i32, i32, crate::structure::StructureArea)>,
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

    fn put_actor(&mut self, pos: ChunkPos, uid: ActorUid, payload: NbtTag) -> Result<()> {
        self.actors.push((pos, uid, payload));
        Ok(())
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
    features: &super::Features,
    policy: crate::convert::UnknownDataPolicy,
) -> Result<ChunkOutcome> {
    let mut profiler = crate::convert::profile::Profiler::new();
    let mut report = ReportBuilder::new("java", "bedrock", "", "");
    profiler.start();
    let chunk = REGIONS.with_borrow_mut(|cache| world.read_chunk_cached(cache, dimension, pos));
    let Some(chunk) = chunk? else {
        profiler.stop(crate::convert::profile::read);
        report.warn(WarningCode::Skipped, pos.label(), "chunk vanished while reading");
        return Ok(ChunkOutcome {
            raw: Vec::new(),
            actors: Vec::new(),
            report,
            converted: false,
            interests: Default::default(),
            structures: Vec::new(),
            timings: profiler.timings(),
        });
    };
    profiler.stop(crate::convert::profile::read);
    // `summarize` walks the parsed tree, so it is analysis, not I/O; count it
    // with the mapping phase it feeds.
    profiler.start();
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
            interests: Default::default(),
            structures: Vec::new(),
            timings: profiler.timings(),
        });
    }
    let mut sink = ChunkSink::default();
    let mut interests = crate::poi::InterestInventory::default();
    let mut structures = Vec::new();
    let mut unsupported_structures = BTreeMap::new();
    profiler.start();
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
        features,
        policy,
        &mut interests,
        &mut structures,
        &mut unsupported_structures,
    )?;
    profiler.stop(crate::convert::profile::map);
    profiler.count_chunk();
    let structures: Vec<(i32, i32, crate::structure::StructureArea)> = structures
        .into_iter()
        .map(|area| (pos.x, pos.z, area))
        .collect();
    if !unsupported_structures.is_empty() {
        for (id, count) in &unsupported_structures {
            report.warn(
                WarningCode::UnsupportedFeature,
                format!("structure:{id}"),
                format!("{count} Java structure(s) of type {id} have no Bedrock spawn area"),
            );
        }
    }
    Ok(ChunkOutcome {
        raw: sink.raw,
        actors: sink.actors,
        report,
        converted: true,
        interests,
        structures,
        timings: profiler.timings(),
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
    features: &super::Features,
    policy: crate::convert::UnknownDataPolicy,
    interests: &mut crate::poi::InterestInventory,
    structures: &mut Vec<crate::structure::StructureArea>,
    unsupported_structures: &mut BTreeMap<String, u64>,
) -> Result<()> {
    let be_pos = ChunkPos {
        x: pos.x,
        z: pos.z,
        dimension: dimension_of(dimension),
    };
    let keys = chunk_keys(be_pos);

    // Sections, in the Bedrock subchunk grid.
    let mut sections: Vec<SectionWrite> = Vec::new();
    if let Some(list) = chunk.get_list("sections") {
        for section in list {
            let y = section
                .get_int("Y")
                .and_then(|value| i8::try_from(value).ok())
                .unwrap_or(0);
            let outcome = convert_section(
                section,
                y,
                mapper,
                report,
                policy,
            )?;
            interests.total += outcome.interests.total;
            for (name, count) in outcome.interests.by_name {
                *interests.by_name.entry(name).or_default() += count;
            }
            for (kind, count) in outcome.interests.by_kind {
                *interests.by_kind.entry(kind).or_default() += count;
            }
            if let Some(write) = outcome.write {
                sections.push(write);
            } else {
                // Still record biomes for sections without blocks.
                if let Some(biome) = convert_biomes(section) {
                    sections.push(SectionWrite {
                        y,
                        // A section with only biome data needs a subchunk
                        // header, but no expanded block indices.
                        primary: Vec::new(),
                        primary_palette: Vec::new(),
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

    // Consume the sections: `encode_section` needs owned palettes and indices,
    // and cloning them per section copied 8 KiB of indices plus the whole
    // palette (24 sections per chunk). The heightmap pass runs first so nothing
    // needs the sections afterwards.
    // Scheduled ticks resolve their block state by position. Read them before
    // the sections are consumed, and only then snapshot the section data the
    // lookup needs (most chunks carry no ticks, so this copy is rare).
    let pending_ticks = if features.ticks {
        crate::tick::read_java_chunk(chunk)
    } else {
        Vec::new()
    };
    // Only sections that carry real block indices can answer a state lookup: a
    // biome-only section is stored with empty `primary`, and treating that as
    // "all air" made every scheduled tick in it resolve to air.
    let tick_sections: Vec<(i8, Vec<u16>, Vec<BlockState>)> = if pending_ticks.is_empty() {
        Vec::new()
    } else {
        sections
            .iter()
            .filter(|section| !section.primary.is_empty())
            .map(|section| (section.y, section.primary.clone(), section.primary_palette.clone()))
            .collect()
    };

    let had_sections = !sections.is_empty();
    for section in sections {
        let y = section.y;
        report.counters().sections += 1;
        if section.primary.is_empty() {
            sink.put_raw(keys.subchunk(y).encode(), encode_air_section(y)?);
            continue;
        }
        let base_y = i32::from(section.y) * 16;
        let mut blocks = 0;
        for x in 0..16_usize {
            for z in 0..16_usize {
                let mut top = 0_i32;
                for y in 0..16_usize {
                    let index = x * 256 + z * 16 + y;
                    if section.primary.get(index).copied().unwrap_or(0) != 0 {
                        top = base_y + y as i32 + 1;
                        blocks += 1;
                    }
                }
                let column = z * 16 + x;
                height_map[column] = height_map[column].max(i16::try_from(top).unwrap_or(i16::MAX));
            }
        }
        report.counters().blocks += blocks;
        let bytes = encode_section(section)?;
        sink.put_raw(keys.subchunk(y).encode(), bytes);
    }

    if had_sections {
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
    if features.block_entities && let Some(entities) = chunk.get_list("block_entities") {
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
    if features.entities {
        let inline = chunk.get_list("Entities").unwrap_or_default();
        let external = REGIONS.with_borrow_mut(|cache| {
            world.read_entity_chunk_cached(cache, dimension, pos)
        })?;
        for entity in inline.iter().chain(external.iter()) {
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
            )?;
        }
    }

    // Scheduled ticks: write Bedrock's record from the model read earlier.
    if !pending_ticks.is_empty() {
        let state_at = |x: i32, y: i32, z: i32| -> Option<(String, BTreeMap<String, NbtTag>)> {
            let section_y = i8::try_from(y.div_euclid(16)).ok()?;
            let (_, primary, palette) = tick_sections
                .iter()
                .find(|(section, _, _)| *section == section_y)?;
            let lx = usize::try_from(x.rem_euclid(16)).ok()?;
            let ly = usize::try_from(y.rem_euclid(16)).ok()?;
            let lz = usize::try_from(z.rem_euclid(16)).ok()?;
            let index = primary.get(lx * 256 + lz * 16 + ly).copied()?;
            let state = palette.get(usize::from(index))?;
            Some((state.name.clone(), state.states.clone()))
        };
        let (record, stats) = crate::tick::write_bedrock_pending(
            &pending_ticks,
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

    // Structure metadata from the Java starts already in hand. Only the four
    // structures Bedrock also tracks as hardcoded spawn areas become areas;
    // the rest keep their blocks and are counted by the caller.
    if features.chunk_metadata && let Some(starts) = chunk
        .get("structures")
        .and_then(|value| value.get("starts"))
        .and_then(|value| value.as_compound())
    {
        for (id, start) in starts {
            let Some(kind) = crate::structure::StructureKind::from_java_id(id) else {
                *unsupported_structures.entry(id.clone()).or_default() += 1;
                continue;
            };
            let Some((min, max)) = crate::structure::java_start_bounds(start) else {
                *unsupported_structures
                    .entry(format!("{id} (no pieces)"))
                    .or_default() += 1;
                continue;
            };
            structures.push(crate::structure::StructureArea { kind, min, max });
        }
    }

    Ok(())
}

/// Renders a Bedrock state as a comparable key for palette deduplication.
fn render_bedrock_state(state: &BlockState) -> String {
    if state.states.is_empty() {
        return state.name.clone();
    }
    let mut out = String::with_capacity(state.name.len() + 16 * state.states.len());
    out.push_str(&state.name);
    out.push('[');
    for (index, (key, value)) in state.states.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(key);
        out.push('=');
        out.push_str(&format!("{value:?}"));
    }
    out.push(']');
    out
}

/// Maps one Java palette entry to its Bedrock state.
///
/// `with_water` requests the waterlogged variant of a block that can hold
/// water; the returned state already includes that flag.
fn resolve_je_entry(
    mapper: &BlockMapper,
    report: &mut ReportBuilder,
    name: &str,
    properties: &BTreeMap<String, String>,
    with_water: bool,
    policy: crate::convert::UnknownDataPolicy,
) -> Result<MappedState> {
    let mut properties = properties.clone();
    if with_water {
        properties.insert("waterlogged".to_string(), "true".to_string());
    }
    let Some(mapped) = mapper.je_to_be(name, &properties) else {
        if policy == crate::convert::UnknownDataPolicy::Error {
            return Err(WorldPortError::unsupported(format!(
                "block {name}{properties:?} has no Bedrock mapping"
            )));
        }
        report.unmapped_block(
            format!("{name}{properties:?}"),
            format!("block {name} has no Bedrock mapping; air was written"),
        );
        return Ok(MappedState {
            name: "minecraft:air".to_string(),
            properties: BTreeMap::new(),
            lossy: true,
        });
    };
    Ok(mapped)
}

fn air_state() -> BlockState {
    BlockState {
        name: "minecraft:air".to_string(),
        states: BTreeMap::new(),
        version: Some(crate::be::write::target_block_state_version()),
    }
}

fn encode_air_section(y: i8) -> Result<bytes::Bytes> {
    static AIR: OnceLock<std::result::Result<[bytes::Bytes; 256], String>> = OnceLock::new();
    let sections = AIR.get_or_init(|| {
        let template = encode_section(SectionWrite {
            y: 0,
            primary: vec![0; SECTION_BLOCKS],
            primary_palette: vec![air_state()],
            liquid: None,
            liquid_palette: Vec::new(),
            biomes: [0; 64],
        })
        .map_err(|error| error.to_string())?;
        // The v9 header is version, storage count, then signed section Y.
        Ok(std::array::from_fn(|y| {
            let mut bytes = template.to_vec();
            bytes[2] = y as u8;
            bytes::Bytes::from(bytes)
        }))
    });
    match sections {
        Ok(sections) => Ok(sections[usize::from(y.to_ne_bytes()[0])].clone()),
        Err(error) => Err(WorldPortError::other(error.clone())),
    }
}

/// Section conversion result: the storage write (when non-empty) plus the
/// village-interest blocks found in the section.
struct SectionOutcome {
    write: Option<SectionWrite>,
    interests: crate::poi::InterestInventory,
}

fn convert_section(
    section: &NbtTag,
    y: i8,
    mapper: &BlockMapper,
    report: &mut ReportBuilder,
    policy: crate::convert::UnknownDataPolicy,
) -> Result<SectionOutcome> {
    let Some(block_states) = section.get("block_states") else {
        return Ok(SectionOutcome {
            write: None,
            interests: Default::default(),
        });
    };
    let Some(palette) = block_states.get_list("palette") else {
        return Ok(SectionOutcome {
            write: None,
            interests: Default::default(),
        });
    };
    if palette.is_empty() {
        return Ok(SectionOutcome {
            write: None,
            interests: Default::default(),
        });
    }
    let data = block_states.get_i64_array("data");
    if data.is_none_or(<[i64]>::is_empty)
        && let [entry] = palette
        && matches!(
            entry.get_str("Name").unwrap_or("minecraft:air"),
            "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
        )
    {
        return Ok(SectionOutcome {
            write: None,
            interests: Default::default(),
        });
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

    let bits = palette_bits(entries.len());
    let indices = decode_indices(data, SECTION_BLOCKS, bits);

    let mut primary_palette: Vec<BlockState> = vec![air_state()];
    // Keyed by the rendered target state: different Java variants that map to
    // the same Bedrock block share one palette slot.
    let mut primary_index_map: BTreeMap<String, u16> = BTreeMap::new();
    let mut primary = vec![0_u16; SECTION_BLOCKS];

    let mut liquid_palette: Vec<BlockState> = vec![air_state(), water_state()];
    let mut fluid_slots: BTreeMap<(&'static str, u8), u16> =
        BTreeMap::from([(("minecraft:water", 0), 1)]);
    let mut liquid = vec![0_u16; SECTION_BLOCKS];
    let mut has_liquid = false;

    // Village interests: classify each palette entry once, then weight by how
    // many blocks use it. Bedrock rebuilds villages from loaded chunks, so the
    // conversion only has to report what it left behind.
    let mut interests = crate::poi::InterestInventory::default();
    let mut interest_kinds: Vec<Option<crate::poi::BlockInterest>> =
        entries.iter().map(|(name, _)| crate::poi::classify(name)).collect();
    let mut palette_usage = vec![0_u32; entries.len()];

    // Resolve every palette entry once, before walking the 4096 blocks.
    //
    // The previous loop rebuilt a `(name, properties)` key and cloned a
    // `MappedState` per block, so a section with 30 entries paid 4096 key
    // builds, map lookups and clones for 30 distinct answers. Per entry the
    // work is now: map once, render once, store two slot values (plain and
    // waterlogged) plus the liquid slot.
    //
    // `slot_primary[i][v]` is `0` for "no block" and `palette index + 1`
    // otherwise; `slot_liquid[i]` is the liquid palette slot (`0` = none,
    // `1` = water, `2` = lava) that the entry's blocks contribute.
    let mut slot_primary: Vec<u32> = vec![0; entries.len()];
    let mut slot_liquid: Vec<u16> = vec![0; entries.len()];
    for (index, (name, properties)) in entries.iter().enumerate() {
        if !crate::mapping::is_vanilla(name) {
            // Mod/addon block: pruned, so both variants stay empty.
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
            // Each flowing/falling level is a different fluid palette state.
            // Preserve Java's level (0..=15) as Bedrock's liquid_depth.
            let fluid = if short == "water" {
                "minecraft:water"
            } else {
                "minecraft:lava"
            };
            let depth = properties
                .get("level")
                .and_then(|value| value.parse::<u8>().ok())
                .filter(|depth| *depth <= 15);
            if depth.is_none() && properties.contains_key("level") {
                report.warn(
                    WarningCode::Lossy,
                    name.clone(),
                    format!("invalid fluid level for {name}; defaulted to source level 0"),
                );
            }
            slot_liquid[index] = resolve_fluid_slot(
                fluid,
                depth.unwrap_or(0),
                &mut fluid_slots,
                &mut liquid_palette,
            )?;
            continue;
        }
        // A palette entry's waterlogged state is fixed by the entry itself, not
        // by the block's position, so exactly one variant is ever referenced.
        // Resolving both doubled the mapping work, allocated states no block
        // used, and could widen the packed palette for nothing.
        let waterlogged = properties
            .get("waterlogged")
            .is_some_and(|value| value == "true");
        let mapped = resolve_je_entry(mapper, report, name, properties, waterlogged, policy)?;
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
        let key = render_bedrock_state(&state);
        let target = *primary_index_map.entry(key).or_insert_with(|| {
            primary_palette.push(state);
            (primary_palette.len() - 1) as u16
        });
        slot_primary[index] = u32::from(target) + 1;
        if waterlogged {
            slot_liquid[index] = 1;
        }
    }
    let mut any_block = false;
    let mut out_of_range: u64 = 0;
    for x in 0..16_usize {
        for z in 0..16_usize {
            for local_y in 0..16_usize {
                let je_index = (local_y << 8) | (z << 4) | x;
                let entry_index = indices.get(je_index).copied().unwrap_or(0) as usize;
                let be_offset = x * 256 + z * 16 + local_y;
                if let Some(usage) = palette_usage.get_mut(entry_index) {
                    *usage = usage.saturating_add(1);
                }
                // A corrupt chunk can carry a palette index past the palette
                // length; indexing the slot tables directly would panic and
                // abort the whole conversion. Treat it as air and record it.
                let (Some(&mapped), Some(&fluid)) =
                    (slot_primary.get(entry_index), slot_liquid.get(entry_index))
                else {
                    out_of_range += 1;
                    continue;
                };
                if mapped != 0 {
                    any_block = true;
                    primary[be_offset] = u16::try_from(mapped - 1).unwrap_or(0);
                }
                if fluid != 0 {
                    has_liquid = true;
                    liquid[be_offset] = fluid;
                }
            }
        }
    }

    for (index, usage) in palette_usage.iter().enumerate() {
        let Some(interest) = interest_kinds.get_mut(index).and_then(Option::take) else {
            continue;
        };
        for _ in 0..*usage {
            interests.record(interest);
        }
    }

    if out_of_range > 0 {
        report.warn(
            WarningCode::Skipped,
            "palette-index",
            format!(
                "{out_of_range} block(s) in section {y} had a palette index past the palette; treated as air"
            ),
        );
    }

    if !any_block && !has_liquid {
        return Ok(SectionOutcome {
            write: None,
            interests,
        });
    }
    let Some(biomes) = convert_biomes(section) else {
        return Ok(SectionOutcome {
            write: None,
            interests,
        });
    };
    Ok(SectionOutcome {
        write: Some(SectionWrite {
            y,
            primary,
            primary_palette,
            liquid: has_liquid.then_some(liquid),
            liquid_palette,
            biomes,
        }),
        interests,
    })
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

/// Resolves a distinct fluid palette slot once per Java palette entry.
fn resolve_fluid_slot(
    name: &'static str,
    depth: u8,
    slots: &mut BTreeMap<(&'static str, u8), u16>,
    palette: &mut Vec<BlockState>,
) -> Result<u16> {
    if let Some(&existing) = slots.get(&(name, depth)) {
        return Ok(existing);
    }
    let slot = u16::try_from(palette.len())
        .map_err(|_| WorldPortError::other("Bedrock fluid palette exceeds u16"))?;
    let mut state = if name == "minecraft:lava" { lava_state() } else { water_state() };
    state.states.insert("liquid_depth".to_string(), NbtTag::Int(i32::from(depth)));
    palette.push(state);
    slots.insert((name, depth), slot);
    Ok(slot)
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
    fn cached_air_subchunk_matches_the_normal_encoder() {
        for y in [-4, 0, 19] {
            let expected = encode_section(SectionWrite {
                y,
                primary: vec![0; SECTION_BLOCKS],
                primary_palette: vec![air_state()],
                liquid: None,
                liquid_palette: Vec::new(),
                biomes: [0; 64],
            }).unwrap();
            assert_eq!(encode_air_section(y).unwrap(), expected);
        }
    }

    #[test]
    fn distinct_java_fluid_levels_survive_bedrock_palette() {
        let mut palette = vec![air_state(), water_state()];
        let mut slots = BTreeMap::from([(("minecraft:water", 0_u8), 1_u16)]);
        let source = resolve_fluid_slot("minecraft:water", 0, &mut slots, &mut palette).unwrap();
        let flowing = resolve_fluid_slot("minecraft:water", 7, &mut slots, &mut palette).unwrap();
        let falling = resolve_fluid_slot("minecraft:water", 8, &mut slots, &mut palette).unwrap();
        let lava = resolve_fluid_slot("minecraft:lava", 7, &mut slots, &mut palette).unwrap();
        assert_eq!(source, 1);
        assert_ne!(flowing, falling);
        assert_ne!(flowing, lava);
        assert_eq!(flowing, resolve_fluid_slot("minecraft:water", 7, &mut slots, &mut palette).unwrap());
        assert_eq!(palette[usize::from(flowing)].states.get("liquid_depth"), Some(&NbtTag::Int(7)));
        assert_eq!(palette[usize::from(falling)].states.get("liquid_depth"), Some(&NbtTag::Int(8)));
        assert_eq!(palette[usize::from(lava)].states.get("liquid_depth"), Some(&NbtTag::Int(7)));
    }

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
