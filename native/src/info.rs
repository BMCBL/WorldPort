//! Lightweight world summaries for the plugin's detection card.

use std::path::Path;

use worldport_core::convert::{Edition, detect_edition};
use worldport_core::je::world::JavaWorld;
use worldport_core::je::JeDimension;
use worldport_core::mapping::schema::JavaCatalog;
use worldport_protocol::WorldInfo;

/// Detects the edition of `path` and summarizes it without decoding chunks.
pub fn world_info(path: &Path) -> worldport_core::error::Result<WorldInfo> {
    match detect_edition(path)? {
        Edition::Java => java_info(path),
        Edition::Bedrock => bedrock_info(path),
    }
}

fn java_info(path: &Path) -> worldport_core::error::Result<WorldInfo> {
    let world = JavaWorld::open(path)?;
    let mut dimensions = Vec::new();
    for (dimension, label) in [
        (JeDimension::Overworld, "overworld"),
        (JeDimension::Nether, "the_nether"),
        (JeDimension::End, "the_end"),
    ] {
        let count = world.chunk_positions(dimension)?.len();
        dimensions.push((label.to_string(), u64::try_from(count).unwrap_or(u64::MAX)));
    }
    let data_version = world.data_version();
    let version = JavaCatalog::for_data_version(data_version)
        .map(|catalog| catalog.version().to_string())
        .unwrap_or_else(|_| format!("DataVersion {data_version}"));
    Ok(WorldInfo {
        edition: "java".to_string(),
        level_name: world.level_name(),
        seed: world.seed(),
        version,
        dimensions,
        players: u64::try_from(world.player_files()?.len()).unwrap_or(u64::MAX),
        maps: u64::try_from(world.map_files()?.len()).unwrap_or(u64::MAX),
    })
}

fn bedrock_info(path: &Path) -> worldport_core::error::Result<WorldInfo> {
    let summary = worldport_core::be::inspect::summarize(path)?;
    Ok(WorldInfo {
        edition: "bedrock".to_string(),
        level_name: summary.level_name,
        seed: summary.seed,
        version: summary.version_label,
        dimensions: vec![
            (
                "overworld".to_string(),
                u64::try_from(summary.overworld_chunks).unwrap_or(u64::MAX),
            ),
            (
                "the_nether".to_string(),
                u64::try_from(summary.nether_chunks).unwrap_or(u64::MAX),
            ),
            (
                "the_end".to_string(),
                u64::try_from(summary.end_chunks).unwrap_or(u64::MAX),
            ),
        ],
        players: u64::try_from(summary.players).unwrap_or(u64::MAX),
        maps: 0,
    })
}
