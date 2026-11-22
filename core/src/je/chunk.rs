//! Java chunk views and summaries.

use bedrock_nbt::NbtTag;

use crate::nbt::NbtExt;

/// Per-chunk statistics gathered without converting anything.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChunkSummary {
    /// Number of `sections` entries.
    pub sections: u32,
    /// Sections with a non-empty block palette.
    pub non_empty_sections: u32,
    /// `block_entities` entries.
    pub block_entities: u32,
    /// `Entities` entries.
    pub entities: u32,
    /// Chunk `DataVersion`.
    pub data_version: i32,
    /// Chunk status string (`minecraft:full`, `full`, ...).
    pub status: String,
    /// Lowest section Y.
    pub min_section_y: i8,
    /// Highest section Y.
    pub max_section_y: i8,
}

/// Summarizes one decoded Java chunk.
#[must_use]
pub fn summarize(chunk: &NbtTag) -> ChunkSummary {
    let mut summary = ChunkSummary {
        data_version: chunk
            .get_int("DataVersion")
            .and_then(|value| i32::try_from(value).ok())
            .unwrap_or(0),
        status: chunk.get_str("Status").unwrap_or_default().to_string(),
        ..ChunkSummary::default()
    };
    let mut min_y = i8::MAX;
    let mut max_y = i8::MIN;
    if let Some(sections) = chunk.get_list("sections") {
        for section in sections {
            summary.sections += 1;
            if let Some(y) = section.get_int("Y") {
                let y = i8::try_from(y).unwrap_or(0);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
            let palette_is_non_air = |palette: Option<&[NbtTag]>| {
                palette.is_some_and(|palette| {
                    palette.len() > 1
                        || palette
                            .first()
                            .is_some_and(|entry| entry.get_str("Name") != Some("minecraft:air"))
                })
            };
            // 1.18+ `block_states.palette`, 1.13..1.17 `Palette`, older `Blocks`.
            let has_blocks = section
                .get("block_states")
                .is_some_and(|states| palette_is_non_air(states.get_list("palette")))
                || palette_is_non_air(section.get_list("Palette"))
                || match section.get("Blocks") {
                    Some(NbtTag::ByteArray(blocks)) => blocks.iter().any(|value| *value != 0),
                    _ => false,
                };
            if has_blocks {
                summary.non_empty_sections += 1;
            }
        }
    }
    if min_y > max_y {
        min_y = 0;
        max_y = 0;
    }
    summary.min_section_y = min_y;
    summary.max_section_y = max_y;
    summary.block_entities = chunk
        .get_list("block_entities")
        .map_or(0, |values| u32::try_from(values.len()).unwrap_or(u32::MAX));
    summary.entities = chunk
        .get_list("Entities")
        .map_or(0, |values| u32::try_from(values.len()).unwrap_or(u32::MAX));
    summary
}
