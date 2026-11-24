//! Read-only Bedrock world summaries used by `worldport inspect` and the UI.

use std::path::Path;

use bedrock_world::world::{OpenOptions, World};
use bedrock_world::{ChunkPos, Dimension};
use serde::{Deserialize, Serialize};

use crate::error::{Result, WorldPortError};
use crate::nbt::{NbtExt, TagValue};

/// High-level facts about a Bedrock world.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct BedrockWorldSummary {
    /// `LevelName`.
    pub level_name: String,
    /// `RandomSeed`.
    pub seed: i64,
    /// `lastOpenedWithVersion` rendered as `1.21.60.1`.
    pub version_label: String,
    /// `StorageVersion`.
    pub storage_version: i64,
    /// `GameType`.
    pub game_type: i64,
    /// `Difficulty`.
    pub difficulty: i64,
    /// Chunk totals per dimension.
    pub overworld_chunks: usize,
    /// Nether chunk total.
    pub nether_chunks: usize,
    /// End chunk total.
    pub end_chunks: usize,
    /// Record-kind counts from a key classification scan.
    pub record_counts: Vec<(String, usize)>,
    /// Players stored in the world.
    pub players: usize,
}

impl BedrockWorldSummary {
    /// Total chunks across dimensions.
    #[must_use]
    pub const fn total_chunks(&self) -> usize {
        self.overworld_chunks + self.nether_chunks + self.end_chunks
    }
}

/// Reads a summary of a Bedrock world directory.
pub fn summarize(path: &Path) -> Result<BedrockWorldSummary> {
    let mut options = OpenOptions::default();
    options.read_only = true;
    let world = World::open(path, options).map_err(|error| {
        WorldPortError::invalid(format!("cannot open Bedrock world {}: {error}", path.display()))
    })?;
    let level_dat = world.read_level_dat()?;
    let mut summary = metadata(&level_dat)?;
    let positions = world.chunk_positions(Default::default())?;
    for pos in positions {
        match pos.dimension {
            Dimension::Overworld => summary.overworld_chunks += 1,
            Dimension::Nether => summary.nether_chunks += 1,
            Dimension::End => summary.end_chunks += 1,
            Dimension::Unknown(_) => {}
        }
    }

    let mut counts: Vec<(String, usize)> = world
        .classify_keys(Default::default())?
        .into_iter()
        .collect();
    counts.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    summary.record_counts = counts;
    summary.players = world.players()?.len();
    Ok(summary)
}

/// Reads only level.dat metadata, without opening or scanning LevelDB.
pub fn read_metadata(path: &Path) -> Result<BedrockWorldSummary> {
    let document = bedrock_world::level::read(&path.join("level.dat"))?;
    metadata(&document)
}

fn metadata(level_dat: &bedrock_world::LevelDatDocument) -> Result<BedrockWorldSummary> {
    let root = &level_dat.root;

    Ok(BedrockWorldSummary {
        level_name: root.get_str("LevelName").unwrap_or_default().to_string(),
        seed: level_dat.random_seed()?.unwrap_or(0),
        version_label: version_label(root),
        storage_version: root.get_int("StorageVersion").unwrap_or(0),
        game_type: root.get_int("GameType").unwrap_or(0),
        difficulty: root.get_int("Difficulty").unwrap_or(0),
        ..BedrockWorldSummary::default()
    })
}

pub(crate) fn version_label(root: &bedrock_nbt::NbtTag) -> String {
    root.get_list("lastOpenedWithVersion")
        .unwrap_or_default()
        .iter()
        .filter_map(TagValue::as_i64_value)
        .map(|version| version.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

/// Checks whether a chunk column is present.
pub fn has_chunk(world: &World, pos: ChunkPos) -> bool {
    world.check_chunk_saved(pos).unwrap_or(false)
}
