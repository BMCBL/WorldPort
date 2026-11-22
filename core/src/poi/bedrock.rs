//! Bedrock village records → Java POI chunks.
//!
//! Bedrock and Java store the same interests with different ownership models:
//!
//! - Bedrock: one `VILLAGE_*_POI` record per village, a list keyed by
//!   `VillagerID`, where each entry lists the interests that villager may claim
//!   (`OwnerCount` says how many villagers currently hold it).
//! - Java: one record per **block position**, with `free_tickets` counting the
//!   remaining claims.
//!
//! So the conversion is a regroup, not a copy: every villager's live instances
//! are flattened, deduplicated by position, and each unique position becomes one
//! Java record whose ticket count is derived from Bedrock's `OwnerCount` and
//! `Capacity`.
//!
//! Java's ticket semantics are inverted relative to Bedrock's ownership:
//! `free_tickets` counts *remaining* claims, so a fully claimed interest has 0.

use std::collections::BTreeMap;

use bedrock_world::village::{
    InterestInstance, InterestKind, VillagerInterests, java_interest_type,
};
use bedrock_world::village::{Entry as VillageEntry, VillageRecordKind};

use crate::convert::UnknownDataPolicy;
use crate::poi::java::{PoiChunk, PoiRecord, group_by_section};
use crate::report::{ReportBuilder, WarningCode};

/// Interests that exist in Bedrock but have no Java POI registry entry.
///
/// Bedrock tracks beehives, lodestones, lightning rods and portals as village
/// interests; Java's POI registry does not, so they are dropped rather than
/// written as unknown types.
const NO_JAVA_EQUIVALENT: &[&str] = &[
    "bee_nest",
    "beehive",
    "lightning_rod",
    "lodestone",
    "nether_portal",
];

/// One deduplicated interest position.
#[derive(Debug)]
struct Claim {
    kind: String,
    /// Villagers currently holding it.
    owners: i64,
    /// Maximum holders Bedrock reported.
    capacity: i64,
}

/// Result of converting one world's village records.
#[derive(Debug, Default)]
pub struct PoiConversion {
    /// POI chunks keyed by chunk position.
    pub chunks: BTreeMap<(i32, i32), PoiChunk>,
    /// Unique interest positions written.
    pub written: u64,
    /// Instances dropped because Java has no equivalent POI type.
    pub unsupported: u64,
    /// Instances dropped because the record could not be parsed.
    pub malformed: u64,
}

impl PoiConversion {
    /// Total records across every chunk.
    #[must_use]
    pub fn records(&self) -> usize {
        self.chunks.values().map(PoiChunk::len).sum()
    }
}

/// Converts Bedrock `VILLAGE_*_POI` records into Java POI chunks.
///
/// `policy` controls what happens to an interest whose Bedrock name has no Java
/// POI type: `Error` aborts, `Drop`/`Fallback` count it in
/// [`PoiConversion::unsupported`].
///
/// # Errors
///
/// Returns an error only when `policy` is [`UnknownDataPolicy::Error`] and an
/// interest cannot be represented in Java.
pub fn convert(
    villages: &[VillageEntry],
    policy: UnknownDataPolicy,
    report: &mut ReportBuilder,
) -> crate::error::Result<PoiConversion> {
    let mut conversion = PoiConversion::default();
    // Key by position: the same bed is listed by every villager that may use it.
    let mut claims: BTreeMap<[i32; 3], Claim> = BTreeMap::new();

    for entry in villages {
        if entry.key.kind != VillageRecordKind::Poi {
            continue;
        }
        for root in &entry.roots {
            let villagers = match bedrock_world::village::parse_poi_record(root) {
                Ok(villagers) => villagers,
                Err(error) => {
                    conversion.malformed += 1;
                    report.warn(
                        WarningCode::Skipped,
                        entry.key.raw.clone(),
                        format!("village POI record could not be parsed: {error}"),
                    );
                    continue;
                }
            };
            for villager in &villagers {
                absorb(villager, &mut claims, &mut conversion, policy, report)?;
            }
        }
    }

    // Group the surviving claims into per-chunk record lists.
    let mut by_chunk: BTreeMap<(i32, i32), Vec<PoiRecord>> = BTreeMap::new();
    for (position, claim) in claims {
        let record = PoiRecord {
            position,
            kind: claim.kind,
            free_tickets: free_tickets(claim.owners, claim.capacity),
        };
        by_chunk
            .entry((
                position[0].div_euclid(16),
                position[2].div_euclid(16),
            ))
            .or_default()
            .push(record);
    }
    for (chunk_pos, records) in by_chunk {
        conversion.written += records.len() as u64;
        conversion.chunks.insert(chunk_pos, group_by_section(records));
    }
    Ok(conversion)
}

/// Folds one villager's live instances into the shared claim table.
fn absorb(
    villager: &VillagerInterests,
    claims: &mut BTreeMap<[i32; 3], Claim>,
    conversion: &mut PoiConversion,
    policy: UnknownDataPolicy,
    report: &mut ReportBuilder,
) -> crate::error::Result<()> {
    for instance in villager.live() {
        let Some(kind) = instance.kind else {
            // Unrecognised `Type`: report rather than guess a Java POI type.
            conversion.unsupported += 1;
            report.warn(
                WarningCode::Lossy,
                instance.name.clone(),
                format!(
                    "interest {} at {:?} has an unrecognised Bedrock Type",
                    instance.name, instance.position
                ),
            );
            continue;
        };
        let Some(java_kind) = java_interest_type(&instance.name, kind) else {
            if NO_JAVA_EQUIVALENT.contains(&instance.name.as_str()) {
                // Known Bedrock-only interest: count it, but do not warn per
                // position (a world can hold thousands of beehives).
                conversion.unsupported += 1;
                continue;
            }
            if policy == UnknownDataPolicy::Error {
                return Err(crate::error::WorldPortError::unsupported(format!(
                    "interest {} at {:?} has no Java POI type",
                    instance.name, instance.position
                )));
            }
            conversion.unsupported += 1;
            report.warn(
                WarningCode::UnmappedBlock,
                format!("poi:{}", instance.name),
                format!(
                    "interest {} has no Java POI type; the record was dropped",
                    instance.name
                ),
            );
            continue;
        };
        merge(claims, instance, java_kind);
    }
    Ok(())
}

fn merge(claims: &mut BTreeMap<[i32; 3], Claim>, instance: &InterestInstance, java_kind: &str) {
    let entry = claims.entry(instance.position).or_insert_with(|| Claim {
        kind: java_kind.to_string(),
        owners: 0,
        capacity: 0,
    });
    // Every villager lists the same position; keep the largest observed
    // ownership so a partially-listed record does not shrink the count.
    entry.owners = entry.owners.max(instance.owner_count);
    entry.capacity = entry.capacity.max(instance.capacity);
    if entry.kind != java_kind {
        // Two Bedrock names claimed one position with different Java types.
        // Keep the first and let the caller's report surface it.
        entry.kind = java_kind.to_string();
    }
}

/// Java `free_tickets` from Bedrock ownership.
///
/// Bedrock's `Capacity` is how many villagers may hold the interest and
/// `OwnerCount` how many do, so the remaining claims are the difference. A
/// record Bedrock marked with zero capacity (older saves) is treated as a
/// single-holder interest, which is what vanilla writes for beds and
/// workstations.
#[must_use]
pub fn free_tickets(owners: i64, capacity: i64) -> i32 {
    let capacity = if capacity <= 0 { 1 } else { capacity };
    let remaining = capacity.saturating_sub(owners).max(0);
    i32::try_from(remaining).unwrap_or(i32::MAX)
}

/// Interest kind recorded for a Java POI type, used when reading Java back.
#[must_use]
pub fn kind_of_java_type(kind: &str) -> Option<InterestKind> {
    let short = kind.strip_prefix("minecraft:").unwrap_or(kind);
    match short {
        "home" => Some(InterestKind::Bed),
        "meeting" => Some(InterestKind::MeetingPoint),
        _ => Some(InterestKind::Workstation),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_world::village::{Entry as VillageEntry, VillageKey, VillageRecordKind};
    use bytes::Bytes;
    use crate::nbt::{compound, tag_list};
    use bedrock_nbt::NbtTag;
    use indexmap::IndexMap;

    fn instance(name: &str, kind: i32, pos: [i32; 3], owners: i64, capacity: i64) -> NbtTag {
        let mut map = IndexMap::new();
        map.insert("Name".to_string(), NbtTag::String(name.to_string()));
        map.insert("Type".to_string(), NbtTag::Int(kind));
        map.insert("X".to_string(), NbtTag::Int(pos[0]));
        map.insert("Y".to_string(), NbtTag::Int(pos[1]));
        map.insert("Z".to_string(), NbtTag::Int(pos[2]));
        map.insert("OwnerCount".to_string(), NbtTag::Long(owners));
        map.insert("Capacity".to_string(), NbtTag::Long(capacity));
        NbtTag::Compound(map)
    }

    fn poi_entry(villagers: Vec<(i64, Vec<NbtTag>)>) -> VillageEntry {
        let list = villagers
            .into_iter()
            .map(|(id, instances)| {
                compound([
                    ("VillagerID", NbtTag::Long(id)),
                    ("instances", tag_list(instances)),
                ])
            })
            .collect::<Vec<_>>();
        let root = compound([("POI", tag_list(list))]);
        VillageEntry {
            key: VillageKey {
                raw: "VILLAGE_test_POI".to_string(),
                dimension: None,
                uuid: "test".to_string(),
                kind: VillageRecordKind::Poi,
            },
            roots: vec![root],
            raw: Bytes::new(),
        }
    }

    fn report() -> ReportBuilder {
        ReportBuilder::new("bedrock", "java", "test", "out")
    }

    #[test]
    fn one_bed_claimed_by_one_villager_becomes_a_single_record() {
        let entries = vec![poi_entry(vec![(
            1,
            vec![instance("villager", 0, [23, 70, 7], 1, 1)],
        )])];
        let mut report = report();
        let conversion = convert(&entries, UnknownDataPolicy::Drop, &mut report).expect("convert");
        assert_eq!(conversion.records(), 1);
        assert_eq!(conversion.written, 1);
        let chunk = conversion.chunks.values().next().expect("chunk");
        let (_, record) = chunk.iter().next().expect("record");
        assert_eq!(record.kind, "minecraft:home");
        assert_eq!(record.position, [23, 70, 7]);
        // Bedrock: 1 of 1 owned -> Java: no free tickets.
        assert_eq!(record.free_tickets, 0);
    }

    #[test]
    fn the_same_position_listed_by_two_villagers_is_written_once() {
        // A shared bed: both villagers list it, Java must hold one record.
        let entries = vec![poi_entry(vec![
            (1, vec![instance("villager", 0, [5, 70, 5], 1, 2)]),
            (2, vec![instance("villager", 0, [5, 70, 5], 1, 2)]),
        ])];
        let mut report = report();
        let conversion = convert(&entries, UnknownDataPolicy::Drop, &mut report).expect("convert");
        assert_eq!(conversion.records(), 1);
        let chunk = conversion.chunks.values().next().expect("chunk");
        let (_, record) = chunk.iter().next().expect("record");
        // Capacity 2, one holder -> one ticket remains.
        assert_eq!(record.free_tickets, 1);
    }

    #[test]
    fn meeting_points_map_to_meeting_not_home() {
        let entries = vec![poi_entry(vec![(
            1,
            vec![instance("villager", 1, [9, 70, 9], 0, 32)],
        )])];
        let mut report = report();
        let conversion = convert(&entries, UnknownDataPolicy::Drop, &mut report).expect("convert");
        let chunk = conversion.chunks.values().next().expect("chunk");
        let (_, record) = chunk.iter().next().expect("record");
        assert_eq!(record.kind, "minecraft:meeting");
        assert_eq!(record.free_tickets, 32);
    }

    #[test]
    fn workstation_maps_by_profession() {
        let entries = vec![poi_entry(vec![(
            1,
            vec![instance("farmer", 2, [1, 64, 1], 1, 1)],
        )])];
        let mut report = report();
        let conversion = convert(&entries, UnknownDataPolicy::Drop, &mut report).expect("convert");
        let chunk = conversion.chunks.values().next().expect("chunk");
        let (_, record) = chunk.iter().next().expect("record");
        assert_eq!(record.kind, "minecraft:farmer");
    }

    #[test]
    fn bedrock_only_interests_are_counted_not_written() {
        let entries = vec![poi_entry(vec![(
            1,
            vec![
                instance("beehive", 2, [1, 64, 1], 1, 1),
                instance("lightning_rod", 2, [2, 64, 2], 1, 1),
                instance("villager", 0, [3, 70, 3], 1, 1),
            ],
        )])];
        let mut report = report();
        let conversion = convert(&entries, UnknownDataPolicy::Drop, &mut report).expect("convert");
        assert_eq!(conversion.records(), 1);
        assert_eq!(conversion.unsupported, 2);
    }

    #[test]
    fn error_policy_rejects_an_unmappable_interest() {
        let entries = vec![poi_entry(vec![(
            1,
            vec![instance("modded_thing", 2, [1, 64, 1], 1, 1)],
        )])];
        let mut report = report();
        let error = convert(&entries, UnknownDataPolicy::Error, &mut report).expect_err("must fail");
        assert!(error.to_string().contains("no Java POI type"));
    }

    #[test]
    fn records_land_in_the_chunk_that_contains_them() {
        let entries = vec![poi_entry(vec![(
            1,
            vec![
                instance("villager", 0, [15, 70, 15], 1, 1),
                instance("villager", 0, [16, 70, 16], 1, 1),
                instance("villager", 0, [-1, 70, -1], 1, 1),
            ],
        )])];
        let mut report = report();
        let conversion = convert(&entries, UnknownDataPolicy::Drop, &mut report).expect("convert");
        assert_eq!(conversion.chunks.len(), 3);
        assert!(conversion.chunks.contains_key(&(0, 0)));
        assert!(conversion.chunks.contains_key(&(1, 1)));
        assert!(conversion.chunks.contains_key(&(-1, -1)));
    }

    #[test]
    fn free_tickets_never_goes_negative() {
        assert_eq!(free_tickets(0, 1), 1);
        assert_eq!(free_tickets(1, 1), 0);
        // More holders than capacity: clamp instead of underflowing.
        assert_eq!(free_tickets(5, 1), 0);
        // Zero capacity (older saves) means a single-holder interest.
        assert_eq!(free_tickets(0, 0), 1);
    }

    #[test]
    fn java_type_round_trips_to_interest_kind() {
        assert_eq!(kind_of_java_type("minecraft:home"), Some(InterestKind::Bed));
        assert_eq!(
            kind_of_java_type("minecraft:meeting"),
            Some(InterestKind::MeetingPoint)
        );
        assert_eq!(
            kind_of_java_type("minecraft:farmer"),
            Some(InterestKind::Workstation)
        );
    }
}
