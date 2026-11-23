//! Semantic world comparison used to verify conversions.
//!
//! Compares two Java worlds chunk by chunk: block-state histograms, biome
//! histograms and the headline `level.dat` fields. Designed for round-trip
//! checks (`Java → Bedrock → Java` against the original).

use std::collections::{BTreeMap, BTreeSet};

use bedrock_nbt::NbtTag;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::je::chunk::summarize;
use crate::je::world::JavaWorld;
use crate::je::{JeChunkPos, JeDimension};
use crate::nbt::{NbtExt, TagValue};

mod palette;

/// One differing block state inside a chunk.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct StateDiff {
    /// Block state rendered as `name[prop=value,...]`.
    pub state: String,
    /// Occurrences in the first world.
    pub first: u64,
    /// Occurrences in the second world.
    pub second: u64,
}

/// Per-chunk comparison result.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ChunkDiff {
    /// Chunk position.
    pub chunk: String,
    /// Block differences (capped).
    pub blocks: Vec<StateDiff>,
    /// Biome differences (capped).
    pub biomes: Vec<StateDiff>,
    /// Total block-state entries that differ.
    pub block_mismatches: u64,
    /// Total biome cells that differ.
    pub biome_mismatches: u64,
    /// Entity type counts that differ.
    pub entities: Vec<StateDiff>,
    /// Block entity id counts that differ.
    pub block_entities: Vec<StateDiff>,
    /// Absolute difference in scheduled tick count.
    pub ticks: u64,
}

/// Whole-world comparison summary.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CompareReport {
    /// World names.
    pub first_name: String,
    /// World names.
    pub second_name: String,
    /// Chunks in the first world.
    pub first_chunks: u64,
    /// Chunks in the second world.
    pub second_chunks: u64,
    /// Chunks present in both.
    pub matched_chunks: u64,
    /// Chunks without a counterpart.
    pub only_first: u64,
    /// Chunks without a counterpart.
    pub only_second: u64,
    /// Chunks with at least one difference.
    pub differing_chunks: u64,
    /// Total differing block states across all matched chunks.
    pub block_mismatches: u64,
    /// Total differing biome cells across all matched chunks.
    pub biome_mismatches: u64,
    /// Total entity type or position mismatches across all matched chunks.
    pub entity_mismatches: u64,
    /// Total block entity type or position mismatches across all matched chunks.
    pub block_entity_mismatches: u64,
    /// Fully generated chunks compared for block and biome content.
    pub compared_chunks: u64,
    /// Positions occupied by a non-air block in either world.
    pub compared_non_air_positions: u64,
    /// Different non-air block states present at compared positions in the first world.
    pub first_distinct_block_states: u64,
    /// Player files in each world.
    pub first_players: u64,
    /// Player files in the second world.
    pub second_players: u64,
    /// Player files with different semantic data, plus unmatched files.
    pub player_mismatches: u64,
    /// Map files in each world.
    pub first_maps: u64,
    pub second_maps: u64,
    /// Maps whose pixels or metadata differ.
    pub map_mismatches: u64,
    /// Scheduled ticks in each world.
    pub first_ticks: u64,
    /// Scheduled ticks in each world.
    pub second_ticks: u64,
    /// Matched chunks whose scheduled tick count differs.
    pub tick_mismatches: u64,
    /// Village points of interest in the selected dimension.
    pub first_poi: u64,
    /// Village points of interest in the second world.
    pub second_poi: u64,
    /// Village points of interest with different positions, types or free tickets.
    pub poi_mismatches: u64,
    /// Chunk positions with different structure starts or references.
    pub structure_mismatches: u64,
    /// Structure starts the source declared that the target does not carry.
    ///
    /// Bedrock has no `StructureStart` equivalent for most structures, so a
    /// roundtrip reports them here rather than as a mismatch.
    pub structure_starts_dropped: u64,
    /// Largest per-chunk differences (capped).
    pub worst_chunks: Vec<ChunkDiff>,
    /// Global block state differences (capped).
    pub block_totals: Vec<StateDiff>,
    /// `level.dat` field differences as `(field, first, second)`.
    pub level_dat: Vec<(String, String, String)>,
}

impl CompareReport {
    /// True when both worlds match on every compared dimension.
    #[must_use]
    pub fn is_identical(&self) -> bool {
        self.only_first == 0
            && self.only_second == 0
            && self.block_mismatches == 0
            && self.biome_mismatches == 0
            && self.entity_mismatches == 0
            && self.block_entity_mismatches == 0
            && self.tick_mismatches == 0
            && self.map_mismatches == 0
            && self.player_mismatches == 0
            && self.poi_mismatches == 0
            && self.structure_mismatches == 0
            && self.level_dat.is_empty()
    }

    /// Fraction of matched block-state entries that agree, `0.0..=1.0` (1.0 when empty).
    #[must_use]
    pub fn block_match_ratio(&self, total_blocks: u64) -> f64 {
        if total_blocks == 0 {
            return 1.0;
        }
        1.0 - self.block_mismatches.min(total_blocks) as f64 / total_blocks as f64
    }
}

/// Compares two Java worlds.
pub fn compare_java(
    first: &JavaWorld,
    second: &JavaWorld,
    dimension: JeDimension,
) -> Result<CompareReport> {
    let mut report = CompareReport {
        first_name: first.level_name(),
        second_name: second.level_name(),
        ..CompareReport::default()
    };
    let first_positions = first.chunk_positions(dimension)?;
    let second_positions = second.chunk_positions(dimension)?;
    let second_set: BTreeSet<JeChunkPos> = second_positions.iter().copied().collect();
    let first_set: BTreeSet<JeChunkPos> = first_positions.iter().copied().collect();
    report.first_chunks = first_positions.len() as u64;
    report.second_chunks = second_positions.len() as u64;
    report.only_first = first_set.difference(&second_set).count() as u64;
    report.only_second = second_set.difference(&first_set).count() as u64;

    let mut totals: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    let mut worst: Vec<ChunkDiff> = Vec::new();
    let mut first_states = BTreeSet::new();
    let mut first_cache = crate::je::world::RegionCache::default();
    let mut second_cache = crate::je::world::RegionCache::default();

    let mut common: Vec<_> = first_set.intersection(&second_set).copied().collect();
    common.sort_by_key(|pos| (pos.region(), pos.local()));
    for pos in common {
        report.matched_chunks += 1;
        let Some(first_chunk) = first.read_chunk_cached(&mut first_cache, dimension, pos)? else {
            continue;
        };
        let Some(second_chunk) = second.read_chunk_cached(&mut second_cache, dimension, pos)?
        else {
            continue;
        };
        let first_summary = summarize(&first_chunk);
        let second_summary = summarize(&second_chunk);
        // Status strings differ per version ("full" vs "minecraft:full"), so
        // compare only fully generated chunks instead of requiring equality.
        if !first_summary.status.ends_with("full") || !second_summary.status.ends_with("full") {
            continue;
        }
        report.compared_chunks += 1;
        let blocks = palette::compare(&first_chunk, &second_chunk, palette::Cells::Blocks);
        let biomes = palette::compare(&first_chunk, &second_chunk, palette::Cells::Biomes);
        report.compared_non_air_positions += blocks.occupied;
        first_states.extend(blocks.first_states);
        for row in &blocks.rows {
            let counts = totals.entry(row.state.clone()).or_default();
            counts.0 += row.first;
            counts.1 += row.second;
        }
        let mut diff = ChunkDiff {
            chunk: pos.label(),
            blocks: blocks.rows.into_iter().take(64).collect(),
            biomes: biomes.rows.into_iter().take(8).collect(),
            block_mismatches: blocks.mismatches,
            biome_mismatches: biomes.mismatches,
            ..ChunkDiff::default()
        };
        let first_entities = entity_histogram(
            &first_chunk,
            &first.read_entity_chunk_cached(&mut first_cache, dimension, pos)?,
        );
        let second_entities = entity_histogram(
            &second_chunk,
            &second.read_entity_chunk_cached(&mut second_cache, dimension, pos)?,
        );
        diff.entities = histogram_diff(&first_entities, &second_entities);
        diff.block_entities = histogram_diff(
            &block_entity_histogram(&first_chunk),
            &block_entity_histogram(&second_chunk),
        );
        for row in &diff.entities {
            report.entity_mismatches += row.first.abs_diff(row.second);
        }
        for row in &diff.block_entities {
            report.block_entity_mismatches += row.first.abs_diff(row.second);
        }
        let first_ticks = ticks(&first_chunk);
        let second_ticks = ticks(&second_chunk);
        report.first_ticks += first_ticks.values().sum::<u64>();
        report.second_ticks += second_ticks.values().sum::<u64>();
        diff.ticks = histogram_diff(&first_ticks, &second_ticks)
            .iter()
            .map(|row| row.first.abs_diff(row.second))
            .sum();
        report.tick_mismatches += diff.ticks;
        // Structure starts are only comparable when both sides carry them. A
        // Bedrock->Java roundtrip deliberately writes no `StructureStart` (a
        // fabricated piece list would break `/locate` and later generation), so
        // the expected shape is "source has starts, roundtrip has none"; that is
        // counted separately instead of as a mismatch.
        let first_starts = structure_start_count(&first_chunk);
        let second_starts = structure_start_count(&second_chunk);
        let structure_diff = first_starts == second_starts && second_starts > 0
            && first_chunk.get("structures") != second_chunk.get("structures");
        report.structure_mismatches += u64::from(structure_diff);
        if first_starts > 0 && second_starts == 0 {
            report.structure_starts_dropped += u64::from(first_starts);
        }
        report.block_mismatches += diff.block_mismatches;
        report.biome_mismatches += diff.biome_mismatches;
        if diff.block_mismatches != 0
            || diff.biome_mismatches != 0
            || !diff.entities.is_empty()
            || !diff.block_entities.is_empty()
            || diff.ticks != 0
            || structure_diff
        {
            report.differing_chunks += 1;
            worst.push(diff);
            worst.sort_by_key(|chunk| std::cmp::Reverse(chunk.block_mismatches));
            worst.truncate(20);
        }
    }

    report.worst_chunks = worst;
    report.block_totals = totals
        .into_iter()
        .map(|(state, (first, second))| StateDiff {
            state,
            first,
            second,
        })
        .collect();
    report.block_totals.sort_by(|left, right| {
        right
            .first
            .saturating_add(right.second)
            .cmp(&left.first.saturating_add(left.second))
    });
    report.block_totals.truncate(40);
    report.first_distinct_block_states = first_states.len() as u64;
    compare_maps(first, second, &mut report)?;
    compare_players(first, second, &mut report)?;
    compare_poi(first, second, dimension, &mut report)?;
    report.level_dat = level_dat_diff(first, second);
    Ok(report)
}

fn ticks(chunk: &NbtTag) -> BTreeMap<String, u64> {
    use crate::tick::TickTarget;
    let mut ticks = BTreeMap::new();
    for tick in crate::tick::read_java_chunk(chunk) {
        let target = match &tick.target {
            TickTarget::Block { name, .. } => name.as_str(),
            TickTarget::Fluid { fluid } => fluid.java_id(),
        };
        let key = format!(
            "{target} {:?} {} {}",
            tick.position, tick.delay, tick.priority
        );
        *ticks.entry(key).or_default() += 1;
    }
    ticks
}

fn compare_poi(
    first: &JavaWorld,
    second: &JavaWorld,
    dimension: JeDimension,
    report: &mut CompareReport,
) -> Result<()> {
    let count = |world: &JavaWorld| -> Result<BTreeMap<String, u64>> {
        let path = world.dimension_dir(dimension).join("poi");
        let mut records = BTreeMap::new();
        let entries = match path.read_dir() {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(records),
            Err(error) => return Err(crate::error::WorldPortError::io(path, error)),
        };
        for entry in entries {
            let entry = entry.map_err(|error| crate::error::WorldPortError::io(&path, error))?;
            if entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "mca")
            {
                continue;
            }
            for (_, _, chunk) in crate::poi::java::read_region(&entry.path())? {
                for (_, record) in chunk.iter() {
                    let key = format!(
                        "{} {:?} {}",
                        record.kind, record.position, record.free_tickets
                    );
                    *records.entry(key).or_default() += 1;
                }
            }
        }
        Ok(records)
    };
    let first = count(first)?;
    let second = count(second)?;
    report.first_poi = first.values().sum();
    report.second_poi = second.values().sum();
    report.poi_mismatches = histogram_diff(&first, &second)
        .iter()
        .map(|row| row.first.abs_diff(row.second))
        .sum();
    Ok(())
}

fn compare_maps(first: &JavaWorld, second: &JavaWorld, report: &mut CompareReport) -> Result<()> {
    use crate::mapping::map_colors::shaded_palette;
    let palette = shaded_palette();
    let first_maps = first.map_files()?;
    let second_maps = second.map_files()?;
    report.first_maps = first_maps.len() as u64;
    report.second_maps = second_maps.len() as u64;
    let second_by_id: BTreeMap<i64, _> = second_maps.iter().cloned().collect();
    let first_ids: BTreeSet<_> = first_maps.iter().map(|(id, _)| *id).collect();
    report.map_mismatches += second_by_id
        .keys()
        .filter(|id| !first_ids.contains(*id))
        .count() as u64;
    for (id, path) in &first_maps {
        let Some(other_path) = second_by_id.get(id) else {
            report.map_mismatches += 1;
            continue;
        };
        let Ok(left) = crate::map_item::read_java_map(path, *id) else {
            report.map_mismatches += 1;
            continue;
        };
        let Ok(right) = crate::map_item::read_java_map(other_path, *id) else {
            report.map_mismatches += 1;
            continue;
        };
        let same_meta = left.scale == right.scale
            && left.dimension == right.dimension
            && left.x_center == right.x_center
            && left.z_center == right.z_center
            && left.locked == right.locked;
        let same_pixels = crate::map_item::java_colors_to_rgba(&left.colors, &palette)
            == crate::map_item::java_colors_to_rgba(&right.colors, &palette);
        if !same_meta || !same_pixels {
            report.map_mismatches += 1;
        }
    }
    Ok(())
}

fn compare_players(
    first: &JavaWorld,
    second: &JavaWorld,
    report: &mut CompareReport,
) -> Result<()> {
    // Compare the local player as a concept, not as a file: a world that keeps
    // it in `level.dat.Data.Player` and one that keeps it in
    // `playerdata/<uuid>.dat` are the same world with the same player, and a
    // conversion legitimately moves it between the two layouts.
    let first_player = first.local_player();
    let second_player = second.local_player();
    report.first_players = u64::from(first_player.is_some());
    report.second_players = u64::from(second_player.is_some());
    if player_semantically_equal(&first_player, &second_player) {
        return Ok(());
    }
    report.player_mismatches += 1;
    Ok(())
}

fn level_dat_diff(first: &JavaWorld, second: &JavaWorld) -> Vec<(String, String, String)> {
    let mut rows = Vec::new();
    let checks: &[(&str, fn(&JavaWorld) -> String)] = &[
        ("seed", |world| world.seed().to_string()),
        ("level_name", |world| world.level_name()),
        ("data_version", |world| world.data_version().to_string()),
        ("time", |world| world.total_ticks().to_string()),
        ("raining", |world| world.raining().to_string()),
        ("thundering", |world| world.thundering().to_string()),
        ("game_rules", |world| world.game_rules().len().to_string()),
    ];
    for (field, read) in checks {
        let left = read(first);
        let right = read(second);
        if left != right {
            rows.push(((*field).to_string(), left, right));
        }
    }
    rows
}

fn histogram_diff(first: &BTreeMap<String, u64>, second: &BTreeMap<String, u64>) -> Vec<StateDiff> {
    let mut rows = Vec::new();
    for (key, count) in first {
        let other = second.get(key).copied().unwrap_or(0);
        if other != *count {
            rows.push(StateDiff {
                state: key.clone(),
                first: *count,
                second: other,
            });
        }
    }
    for (key, count) in second {
        if !first.contains_key(key) {
            rows.push(StateDiff {
                state: key.clone(),
                first: 0,
                second: *count,
            });
        }
    }
    rows
}

fn entity_histogram(chunk: &NbtTag, external: &[NbtTag]) -> BTreeMap<String, u64> {
    let mut histogram = BTreeMap::new();
    let inline = chunk.get_list("Entities").unwrap_or_default();
    for entity in inline.iter().chain(external.iter()) {
        if let Some(id) = entity.get_str("id") {
            let key = format!("{id} {:?}", entity.get("Pos"));
            *histogram.entry(key).or_insert(0) += 1;
        }
    }
    histogram
}

fn block_entity_histogram(chunk: &NbtTag) -> BTreeMap<String, u64> {
    let mut histogram = BTreeMap::new();
    for entity in chunk.get_list("block_entities").unwrap_or_default() {
        if let Some(id) = entity.get_str("id") {
            let position = ["x", "y", "z"].map(|axis| entity.get_int(axis));
            *histogram.entry(format!("{id} {position:?}")).or_insert(0) += 1;
        }
    }
    histogram
}

/// Compares two player payloads up to the precision the target format stores.
///
/// Bedrock keeps `Pos` and `Motion` as Float while Java uses Double, so a
/// converted player's coordinates differ in the low mantissa bits no matter how
/// careful the conversion is. Normalising both sides through f32 expresses
/// exactly that: anything coarser than that still counts as a mismatch.
fn player_semantically_equal(
    first: &Option<NbtTag>,
    second: &Option<NbtTag>,
) -> bool {
    let (Some(first), Some(second)) = (first, second) else {
        return first.is_none() && second.is_none();
    };
    normalize_player(first) == normalize_player(second)
}

fn normalize_player(player: &NbtTag) -> NbtTag {
    let Some(fields) = player.as_compound() else {
        return player.clone();
    };
    let mut out = fields.clone();
    for key in ["Pos", "Motion", "Rotation"] {
        let Some(NbtTag::List(values)) = fields.get(key) else {
            continue;
        };
        let rounded: Vec<NbtTag> = values
            .iter()
            .map(|value| match value {
                NbtTag::Double(value) => NbtTag::Double(f64::from(*value as f32)),
                NbtTag::Float(value) => NbtTag::Double(f64::from(*value)),
                other => other.clone(),
            })
            .collect();
        out.insert(key.to_string(), NbtTag::List(rounded));
    }
    NbtTag::Compound(out)
}

/// Number of `structures.starts` entries in one chunk.
fn structure_start_count(chunk: &NbtTag) -> u64 {
    chunk
        .get("structures")
        .and_then(|value| value.get("starts"))
        .and_then(|value| value.as_compound())
        .map_or(0, |starts| u64::try_from(starts.len()).unwrap_or(u64::MAX))
}

fn render_state(entry: &NbtTag) -> String {
    let name = entry.get_str("Name").unwrap_or("minecraft:air");
    let Some(properties) = entry
        .get("Properties")
        .and_then(|value| value.as_compound())
    else {
        return name.to_string();
    };
    if properties.is_empty() {
        return name.to_string();
    }
    // Property maps have no canonical order in NBT, so sort for comparison.
    let mut pairs: Vec<(&String, &NbtTag)> = properties.iter().collect();
    pairs.sort_by(|left, right| left.0.cmp(right.0));
    let body = pairs
        .into_iter()
        .map(|(key, value)| format!("{key}={}", value.as_str_value().unwrap_or("?")))
        .collect::<Vec<_>>()
        .join(",");
    format!("{name}[{body}]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nbt::compound;

    #[test]
    fn renders_states_with_properties() {
        let entry = compound([
            ("Name", NbtTag::String("minecraft:oak_stairs".to_string())),
            (
                "Properties",
                compound([
                    ("facing", NbtTag::String("north".to_string())),
                    ("half", NbtTag::String("top".to_string())),
                ]),
            ),
        ]);
        assert_eq!(
            render_state(&entry),
            "minecraft:oak_stairs[facing=north,half=top]"
        );
    }
}
