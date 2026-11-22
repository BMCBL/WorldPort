//! Java point-of-interest storage (`poi/r.X.Z.mca`).
//!
//! Each chunk holds `{ Sections: { "<sectionY>": { Valid, Records: [...] } },
//! DataVersion }`, where a record is `{ pos: [x,y,z], type, free_tickets }`.
//! Bedrock keeps the same interests per villager instead (see
//! `bedrock_world::village::poi`), so this module is the Java side of the
//! shared interest model.
//!
//! Section keys are **section Y** (`block_y >> 4`), not chunk Y, and a section
//! only counts when `Valid` is set — Java writes an empty `Records` list with
//! `Valid: 0` for sections it has scanned but found nothing in.

use std::path::Path;

use bedrock_nbt::NbtTag;
use indexmap::IndexMap;

use crate::error::{Result, WorldPortError, io_result};
use crate::je::JeChunkPos;
use crate::je::anvil::{RegionFile, write_region_file};
use crate::je::nbt as je_nbt;
use crate::nbt::{NbtExt, compound, tag_list};

/// One Java POI record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PoiRecord {
    /// Block position.
    pub position: [i32; 3],
    /// POI type id (`minecraft:home`, `minecraft:farmer`, ...).
    pub kind: String,
    /// Remaining tickets; a record with 0 is still valid and can be reclaimed.
    pub free_tickets: i32,
}

/// One chunk's POI payload.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PoiChunk {
    /// Records grouped by section Y, matching the on-disk layout.
    pub sections: Vec<(i8, Vec<PoiRecord>)>,
    /// `DataVersion` read from the source chunk (0 when written fresh).
    pub data_version: i32,
}

impl PoiChunk {
    /// Total records across sections.
    #[must_use]
    pub fn len(&self) -> usize {
        self.sections.iter().map(|(_, records)| records.len()).sum()
    }

    /// True when no section holds a record.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Every record with its section Y, in storage order.
    pub fn iter(&self) -> impl Iterator<Item = (i8, &PoiRecord)> {
        self.sections
            .iter()
            .flat_map(|(section_y, records)| records.iter().map(move |record| (*section_y, record)))
    }
}

/// Section Y for a block Y, matching Java's `block_y >> 4`.
#[must_use]
pub fn section_of(block_y: i32) -> i8 {
    i8::try_from(block_y.div_euclid(16)).unwrap_or(0)
}

/// Reads one POI chunk, or `None` when the region or slot is absent.
pub fn read_chunk(region: &mut RegionFile, local_x: u8, local_z: u8) -> Result<Option<PoiChunk>> {
    let Some(payload) = region.read_chunk(local_x, local_z)? else {
        return Ok(None);
    };
    let decoded = je_nbt::decode(&payload)?;
    let data_version = decoded
        .tag
        .get_int("DataVersion")
        .and_then(|value| i32::try_from(value).ok())
        .unwrap_or(0);
    let mut sections = Vec::new();
    let Some(entries) = decoded
        .tag
        .get("Sections")
        .and_then(|value| value.as_compound())
    else {
        return Ok(Some(PoiChunk {
            sections,
            data_version,
        }));
    };
    for (key, section) in entries {
        let Ok(section_y) = key.parse::<i8>() else {
            continue;
        };
        // `Valid: 0` marks a scanned-but-empty section; its records (if any)
        // are not authoritative.
        if !section.get_bool("Valid").unwrap_or(false) {
            continue;
        }
        let mut records = Vec::new();
        if let Some(list) = section.get_list("Records") {
            for record in list {
                let Some(kind) = record.get_str("type") else {
                    continue;
                };
                let Some(NbtTag::IntArray(position)) = record.get("pos") else {
                    continue;
                };
                if position.len() < 3 {
                    continue;
                }
                records.push(PoiRecord {
                    position: [position[0], position[1], position[2]],
                    kind: kind.to_string(),
                    free_tickets: record
                        .get_int("free_tickets")
                        .and_then(|value| i32::try_from(value).ok())
                        .unwrap_or(0),
                });
            }
        }
        if !records.is_empty() {
            sections.push((section_y, records));
        }
    }
    sections.sort_by_key(|(section_y, _)| *section_y);
    Ok(Some(PoiChunk {
        sections,
        data_version,
    }))
}

/// Encodes one POI chunk for the region writer.
///
/// `data_version` is the target world's DataVersion, so the record format always
/// matches the world it is written into.
pub fn encode_chunk(chunk: &PoiChunk, data_version: i32) -> Result<Vec<u8>> {
    let mut sections: IndexMap<String, NbtTag> = IndexMap::new();
    for (section_y, records) in &chunk.sections {
        let records: Vec<NbtTag> = records
            .iter()
            .map(|record| {
                compound([
                    ("pos", NbtTag::IntArray(record.position.to_vec())),
                    ("type", NbtTag::String(record.kind.clone())),
                    ("free_tickets", NbtTag::Int(record.free_tickets)),
                ])
            })
            .collect();
        sections.insert(
            section_y.to_string(),
            compound([
                ("Valid", NbtTag::Byte(1)),
                ("Records", tag_list(records)),
            ]),
        );
    }
    let root = compound([
        ("Sections", NbtTag::Compound(sections)),
        ("DataVersion", NbtTag::Int(data_version)),
    ]);
    je_nbt::encode("", &root)
}

/// Reads every chunk of one POI region file.
///
/// Returns `(local_x, local_z, chunk)` in slot order.
pub fn read_region(path: &Path) -> Result<Vec<(u8, u8, PoiChunk)>> {
    let mut region = RegionFile::open(path, false)?;
    let mut chunks = Vec::new();
    for (local_x, local_z) in region.occupied_slots() {
        if let Some(chunk) = read_chunk(&mut region, local_x, local_z)? {
            chunks.push((local_x, local_z, chunk));
        }
    }
    Ok(chunks)
}

/// Writes one POI region file from `(local_x, local_z, chunk)` entries.
pub fn write_region(
    path: &Path,
    entries: Vec<(u8, u8, PoiChunk)>,
    data_version: i32,
    compression: flate2::Compression,
) -> Result<()> {
    let mut encoded = Vec::with_capacity(entries.len());
    for (local_x, local_z, chunk) in entries {
        encoded.push((local_x, local_z, encode_chunk(&chunk, data_version)?));
    }
    write_region_file(path, encoded, compression)
}

/// Collects records per chunk position from a chunk-relative record list.
///
/// Java groups records by the section of their own Y coordinate, so callers can
/// build a [`PoiChunk`] without tracking sections themselves.
#[must_use]
pub fn group_by_section(records: impl IntoIterator<Item = PoiRecord>) -> PoiChunk {
    let mut by_section: std::collections::BTreeMap<i8, Vec<PoiRecord>> =
        std::collections::BTreeMap::new();
    for record in records {
        by_section
            .entry(section_of(record.position[1]))
            .or_default()
            .push(record);
    }
    PoiChunk {
        sections: by_section.into_iter().collect(),
        data_version: 0,
    }
}

/// Chunk position of a POI record, for grouping into region files.
#[must_use]
pub fn chunk_of(record: &PoiRecord) -> JeChunkPos {
    JeChunkPos::new(record.position[0].div_euclid(16), record.position[2].div_euclid(16))
}

/// Ensures a chunk position is inside one region file's slot range.
pub fn validate_region_entries(entries: &[(u8, u8, PoiChunk)]) -> Result<()> {
    for (local_x, local_z, _) in entries {
        if *local_x >= 32 || *local_z >= 32 {
            return Err(WorldPortError::other(format!(
                "POI local chunk position out of range: {local_x},{local_z}"
            )));
        }
    }
    Ok(())
}

/// Region directory for a dimension, used by callers building POI paths.
#[must_use]
pub fn region_path(poi_dir: &Path, region_x: i32, region_z: i32) -> std::path::PathBuf {
    poi_dir.join(format!("r.{region_x}.{region_z}.mca"))
}

/// Creates the POI directory when writing a fresh world.
pub fn ensure_dir(path: &Path) -> Result<()> {
    io_result(path, std::fs::create_dir_all(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(x: i32, y: i32, z: i32, kind: &str, tickets: i32) -> PoiRecord {
        PoiRecord {
            position: [x, y, z],
            kind: kind.to_string(),
            free_tickets: tickets,
        }
    }

    #[test]
    fn section_of_uses_floor_division_for_negative_y() {
        // Java's `block_y >> 4` floors, so -1 belongs to section -1, not 0.
        assert_eq!(section_of(0), 0);
        assert_eq!(section_of(15), 0);
        assert_eq!(section_of(16), 1);
        assert_eq!(section_of(-1), -1);
        assert_eq!(section_of(-16), -1);
        assert_eq!(section_of(-17), -2);
    }

    #[test]
    fn group_by_section_buckets_records_by_their_own_y() {
        let chunk = group_by_section(vec![
            record(1, 70, 1, "minecraft:home", 1),
            record(2, 71, 2, "minecraft:farmer", 1),
            record(3, 100, 3, "minecraft:home", 1),
        ]);
        assert_eq!(chunk.sections.len(), 2);
        assert_eq!(chunk.sections[0].0, 4);
        assert_eq!(chunk.sections[0].1.len(), 2);
        assert_eq!(chunk.sections[1].0, 6);
        assert_eq!(chunk.len(), 3);
        assert!(!chunk.is_empty());
    }

    #[test]
    fn encode_sets_valid_and_the_target_data_version() {
        let chunk = group_by_section(vec![
            record(23, 70, 7, "minecraft:home", 1),
            record(25, 70, 15, "minecraft:meeting", 32),
            record(-3, -60, 9, "minecraft:farmer", 0),
        ]);
        // y=70 twice and y=-60 once: sections 4 and -4.
        assert_eq!(chunk.sections.len(), 2);
        let payload = encode_chunk(&chunk, 4903).expect("encode");
        let decoded = je_nbt::decode(&payload).expect("decode");
        assert_eq!(decoded.tag.get_int("DataVersion"), Some(4903));
        let sections = decoded
            .tag
            .get("Sections")
            .and_then(|value| value.as_compound())
            .expect("sections");
        assert_eq!(sections.len(), 2);
        for (_, section) in sections {
            // `Valid` must be set or a reader discards the section.
            assert!(section.get_bool("Valid").unwrap_or(false));
        }
    }

    #[test]
    fn region_file_round_trips_chunk_records() {
        let dir = std::env::temp_dir().join("worldport-poi-roundtrip");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join("r.0.0.mca");
        let source = group_by_section(vec![
            record(23, 70, 7, "minecraft:home", 1),
            record(23, 70, 3, "minecraft:home", 1),
            record(25, 70, 11, "minecraft:farmer", 0),
        ]);
        write_region(
            &path,
            vec![(5, 6, source.clone())],
            4903,
            flate2::Compression::new(1),
        )
        .expect("write region");

        let chunks = read_region(&path).expect("read region");
        assert_eq!(chunks.len(), 1);
        let (local_x, local_z, chunk) = &chunks[0];
        assert_eq!((*local_x, *local_z), (5, 6));
        assert_eq!(chunk.data_version, 4903);
        assert_eq!(chunk.sections, source.sections);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reader_skips_invalid_sections_and_missing_positions() {
        // Mirrors what Java writes for a scanned section with no interests, and
        // for a record whose `pos` is not an int array.
        let root = compound([
            (
                "Sections",
                NbtTag::Compound(indexmap::indexmap! {
                    "4".to_string() => compound([
                        ("Valid", NbtTag::Byte(0)),
                        ("Records", tag_list(vec![compound([
                            ("pos", NbtTag::IntArray(vec![1, 70, 1])),
                            ("type", NbtTag::String("minecraft:home".to_string())),
                        ])])),
                    ]),
                    "5".to_string() => compound([
                        ("Valid", NbtTag::Byte(1)),
                        ("Records", tag_list(vec![
                            compound([
                                ("pos", NbtTag::String("broken".to_string())),
                                ("type", NbtTag::String("minecraft:home".to_string())),
                            ]),
                            compound([
                                ("pos", NbtTag::IntArray(vec![2, 80, 2])),
                                ("type", NbtTag::String("minecraft:farmer".to_string())),
                                ("free_tickets", NbtTag::Int(3)),
                            ]),
                        ])),
                    ]),
                }),
            ),
            ("DataVersion", NbtTag::Int(4903)),
        ]);
        let payload = je_nbt::encode("", &root).expect("encode");
        // Reuse the reader's section walk through a region file.
        let dir = std::env::temp_dir().join("worldport-poi-invalid");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create dir");
        let path = dir.join("r.0.0.mca");
        write_region_file(&path, vec![(0, 0, payload)], flate2::Compression::new(1))
            .expect("write");
        let chunks = read_region(&path).expect("read");
        assert_eq!(chunks.len(), 1);
        let chunk = &chunks[0].2;
        // Section 4 was invalid, and the broken record has no usable position.
        assert_eq!(chunk.sections.len(), 1);
        assert_eq!(chunk.sections[0].0, 5);
        assert_eq!(chunk.sections[0].1.len(), 1);
        assert_eq!(chunk.sections[0].1[0].kind, "minecraft:farmer");
        assert_eq!(chunk.sections[0].1[0].free_tickets, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_chunk_encodes_with_no_sections() {
        let chunk = PoiChunk::default();
        assert!(chunk.is_empty());
        let payload = encode_chunk(&chunk, 4903).expect("encode");
        let decoded = je_nbt::decode(&payload).expect("decode");
        let sections = decoded
            .tag
            .get("Sections")
            .and_then(|value| value.as_compound())
            .expect("sections");
        assert!(sections.is_empty());
    }

    #[test]
    fn chunk_of_groups_negative_coordinates_correctly() {
        let pos = chunk_of(&record(-1, 64, -17, "minecraft:home", 1));
        assert_eq!((pos.x, pos.z), (-1, -2));
        let pos = chunk_of(&record(15, 64, 16, "minecraft:home", 1));
        assert_eq!((pos.x, pos.z), (0, 1));
    }
}
