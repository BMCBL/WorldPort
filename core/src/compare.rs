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
            && self.level_dat.is_empty()
    }

    /// Fraction of matched block-state entries that agree, `0.0..=1.0` (1.0 when empty).
    #[must_use]
    pub fn block_match_ratio(&self, total_blocks: u64) -> f64 {
        if total_blocks == 0 {
            return 1.0;
        }
        1.0 - self.block_mismatches as f64 / total_blocks as f64
    }
}

/// Compares two Java worlds.
pub fn compare_java(first: &JavaWorld, second: &JavaWorld, dimension: JeDimension) -> Result<CompareReport> {
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

    for pos in first_set.intersection(&second_set) {
        report.matched_chunks += 1;
        let Some(first_chunk) = first.read_chunk(dimension, *pos)? else {
            continue;
        };
        let Some(second_chunk) = second.read_chunk(dimension, *pos)? else {
            continue;
        };
        let first_summary = summarize(&first_chunk);
        let second_summary = summarize(&second_chunk);
        if first_summary.status != second_summary.status {
            continue;
        }
        let first_hist = state_histogram(&first_chunk);
        let second_hist = state_histogram(&second_chunk);
        let mut diff = ChunkDiff {
            chunk: pos.label(),
            ..ChunkDiff::default()
        };
        for (state, count) in &first_hist {
            let other = second_hist.get(state).copied().unwrap_or(0);
            if other != *count {
                let entry = totals.entry(state.clone()).or_insert((0, 0));
                entry.0 += *count;
                entry.1 += other;
                let mismatch = count.abs_diff(other);
                diff.block_mismatches += mismatch;
                if diff.blocks.len() < 64 {
                    diff.blocks.push(StateDiff {
                        state: state.clone(),
                        first: *count,
                        second: other,
                    });
                }
            }
        }
        for (state, count) in &second_hist {
            if !first_hist.contains_key(state) {
                let entry = totals.entry(state.clone()).or_insert((0, 0));
                entry.0 += 0;
                entry.1 += *count;
                diff.block_mismatches += *count;
                if diff.blocks.len() < 64 {
                    diff.blocks.push(StateDiff {
                        state: state.clone(),
                        first: 0,
                        second: *count,
                    });
                }
            }
        }
        let first_biomes = biome_histogram(&first_chunk);
        let second_biomes = biome_histogram(&second_chunk);
        for (biome, count) in &first_biomes {
            let other = second_biomes.get(biome).copied().unwrap_or(0);
            if other != *count {
                diff.biome_mismatches += count.abs_diff(other);
                if diff.biomes.len() < 8 {
                    diff.biomes.push(StateDiff {
                        state: biome.clone(),
                        first: *count,
                        second: other,
                    });
                }
            }
        }
        report.block_mismatches += diff.block_mismatches;
        report.biome_mismatches += diff.biome_mismatches;
        if diff.block_mismatches != 0 || diff.biome_mismatches != 0 {
            report.differing_chunks += 1;
            worst.push(diff);
        }
    }

    worst.sort_by(|left, right| right.block_mismatches.cmp(&left.block_mismatches));
    worst.truncate(20);
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
            .abs_diff(right.second)
            .cmp(&left.first.abs_diff(left.second))
    });
    report.block_totals.truncate(40);
    report.level_dat = level_dat_diff(first, second);
    Ok(report)
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

fn state_histogram(chunk: &NbtTag) -> BTreeMap<String, u64> {
    let mut histogram = BTreeMap::new();
    let Some(sections) = chunk.get_list("sections") else {
        return histogram;
    };
    for section in sections {
        let Some(block_states) = section.get("block_states") else {
            continue;
        };
        let Some(palette) = block_states.get_list("palette") else {
            continue;
        };
        let palette: Vec<String> = palette.iter().map(render_state).collect();
        if palette.len() == 1 {
            // A single-entry palette is 4096 identical blocks; air contributes nothing.
            if palette[0] != "minecraft:air" {
                *histogram.entry(palette[0].clone()).or_insert(0) += 4096;
            }
            continue;
        }
        let Some(data) = block_states.get_i64_array("data") else {
            continue;
        };
        let bits = (usize::BITS - (palette.len() - 1).leading_zeros()).max(4);
        let per_long = (64 / bits) as usize;
        let mask = (1_u64 << bits) - 1;
        for index in 0..4096_usize {
            let long_index = index / per_long;
            let offset = (index % per_long) as u32 * bits;
            let Some(word) = data.get(long_index) else {
                break;
            };
            let palette_index = (((*word as u64) >> offset) & mask) as usize;
            if let Some(state) = palette.get(palette_index)
                && state != "minecraft:air"
            {
                *histogram.entry(state.clone()).or_insert(0) += 1;
            }
        }
    }
    histogram
}

fn biome_histogram(chunk: &NbtTag) -> BTreeMap<String, u64> {
    let mut histogram = BTreeMap::new();
    let Some(sections) = chunk.get_list("sections") else {
        return histogram;
    };
    for section in sections {
        let Some(biomes) = section.get("biomes") else {
            continue;
        };
        let Some(palette) = biomes.get_list("palette") else {
            continue;
        };
        let palette: Vec<String> = palette
            .iter()
            .map(|entry| entry.as_str_value().unwrap_or("unknown").to_string())
            .collect();
        if palette.len() == 1 {
            *histogram.entry(palette[0].clone()).or_insert(0) += 64;
            continue;
        }
        let Some(data) = biomes.get_i64_array("data") else {
            continue;
        };
        let bits = (usize::BITS - (palette.len() - 1).leading_zeros()).max(1);
        let per_long = (64 / bits) as usize;
        let mask = (1_u64 << bits) - 1;
        for index in 0..64_usize {
            let long_index = index / per_long;
            let offset = (index % per_long) as u32 * bits;
            let Some(word) = data.get(long_index) else {
                break;
            };
            let palette_index = (((*word as u64) >> offset) & mask) as usize;
            if let Some(biome) = palette.get(palette_index) {
                *histogram.entry(biome.clone()).or_insert(0) += 1;
            }
        }
    }
    histogram
}

fn render_state(entry: &NbtTag) -> String {
    let name = entry.get_str("Name").unwrap_or("minecraft:air");
    let Some(properties) = entry.get("Properties").and_then(|value| value.as_compound()) else {
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
            (
                "Name",
                NbtTag::String("minecraft:oak_stairs".to_string()),
            ),
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
