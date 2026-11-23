//! Lightweight world summaries for the plugin's detection card.

use std::path::Path;

use worldport_core::convert::{Edition, detect_edition};
use worldport_core::je::world::JavaWorld;
use worldport_core::je::JeDimension;
use worldport_core::map_item::preview;
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
        map_preview: java_map_preview(&world),
    })
}

/// Renders the world's first map as a preview, or an empty string when it has
/// none.
///
/// Detection has to stay cheap: this reads one map file, and a world with no
/// maps or an unreadable one simply yields no preview so the page keeps its
/// placeholder.
fn java_map_preview(world: &JavaWorld) -> String {
    let Ok(files) = world.map_files() else {
        return String::new();
    };
    let Some((_, path)) = files.first() else {
        return String::new();
    };
    // The map id is in the file name Java uses (`map_<id>.dat`); 0 is a safe
    // stand-in when a world renamed it, because the preview only needs pixels.
    let id = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(|stem| stem.strip_prefix("map_"))
        .and_then(|id| id.parse::<i64>().ok())
        .unwrap_or(0);
    match worldport_core::map_item::read_java_map(path, id) {
        Ok(map) => preview::preview_data_url(&map, &worldport_core::map_item::palette()),
        Err(_) => String::new(),
    }
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
        // Bedrock maps are palette images too, but reading one needs Bedrock's
        // own map record path; until that exists this stays empty and the page
        // shows its placeholder rather than a wrong picture.
        map_preview: String::new(),
    })
}
