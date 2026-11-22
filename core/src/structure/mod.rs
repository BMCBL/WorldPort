//! Structure metadata shared by both editions.
//!
//! The two editions record generated structures very differently, and only a
//! narrow overlap can be converted honestly:
//!
//! - Bedrock: `HardcodedSpawners` (tag `0x39`) is a binary list of hardcoded
//!   spawn areas — nether fortress, swamp hut, ocean monument, pillager outpost
//!   — each an axis-aligned box. It is **not** NBT and carries no structure
//!   pieces, no loot state and no `/locate` key.
//! - Java: each chunk's `structures.starts` holds `StructureStart` compounds with
//!   a `Children` list of pieces, and `structures.References` maps structure ids
//!   to referenced pieces. `/locate structure` and structure-specific mob
//!   spawning read these.
//!
//! Converting one to the other is therefore asymmetric:
//!
//! - **Java → Bedrock**: a Java structure that Bedrock also tracks as a
//!   hardcoded spawn area can become one, because both are "a box where this
//!   structure's mobs spawn". Structures Bedrock has no hardcoded area for are
//!   reported, not invented.
//! - **Bedrock → Java**: writing a `StructureStart` requires valid `Children`
//!   pieces with palette data. Fabricating them would make `/locate` return a
//!   structure whose pieces do not exist and can break later chunk generation,
//!   so this direction records the areas in the report and leaves `starts`
//!   empty. The blocks and entities are already converted, so the buildings
//!   themselves survive; only the lookup metadata is absent.

use bedrock_world::scan::{HardcodedSpawnArea, HardcodedSpawnAreaKind};

use crate::report::{ReportBuilder, WarningCode};

/// A generated structure both editions can name.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub enum StructureKind {
    /// Nether fortress.
    NetherFortress,
    /// Swamp hut (witch hut).
    SwampHut,
    /// Ocean monument.
    OceanMonument,
    /// Pillager outpost.
    PillagerOutpost,
}

impl StructureKind {
    /// Java structure id.
    #[must_use]
    pub const fn java_id(self) -> &'static str {
        match self {
            Self::NetherFortress => "minecraft:fortress",
            Self::SwampHut => "minecraft:swamp_hut",
            Self::OceanMonument => "minecraft:monument",
            Self::PillagerOutpost => "minecraft:pillager_outpost",
        }
    }

    /// Bedrock hardcoded spawn area kind, when Bedrock tracks one.
    #[must_use]
    pub const fn bedrock_area(self) -> Option<HardcodedSpawnAreaKind> {
        Some(match self {
            Self::NetherFortress => HardcodedSpawnAreaKind::NetherFortress,
            Self::SwampHut => HardcodedSpawnAreaKind::SwampHut,
            Self::OceanMonument => HardcodedSpawnAreaKind::OceanMonument,
            Self::PillagerOutpost => HardcodedSpawnAreaKind::PillagerOutpost,
        })
    }

    /// Classifies a Bedrock hardcoded spawn area kind.
    #[must_use]
    pub const fn from_bedrock_area(kind: HardcodedSpawnAreaKind) -> Option<Self> {
        Some(match kind {
            HardcodedSpawnAreaKind::NetherFortress => Self::NetherFortress,
            HardcodedSpawnAreaKind::SwampHut => Self::SwampHut,
            HardcodedSpawnAreaKind::OceanMonument => Self::OceanMonument,
            HardcodedSpawnAreaKind::PillagerOutpost => Self::PillagerOutpost,
            HardcodedSpawnAreaKind::Unknown(_) => return None,
        })
    }

    /// Classifies a Java structure id.
    #[must_use]
    pub fn from_java_id(id: &str) -> Option<Self> {
        Some(match id {
            "minecraft:fortress" => Self::NetherFortress,
            "minecraft:swamp_hut" => Self::SwampHut,
            "minecraft:monument" => Self::OceanMonument,
            "minecraft:pillager_outpost" => Self::PillagerOutpost,
            _ => return None,
        })
    }
}

/// One structure area in world coordinates.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StructureArea {
    /// Structure this area belongs to.
    pub kind: StructureKind,
    /// Inclusive minimum corner.
    pub min: [i32; 3],
    /// Inclusive maximum corner.
    pub max: [i32; 3],
}

impl StructureArea {
    /// Converts to a Bedrock hardcoded spawn area.
    #[must_use]
    pub fn to_bedrock(&self) -> HardcodedSpawnArea {
        HardcodedSpawnArea {
            kind: self
                .kind
                .bedrock_area()
                .unwrap_or(HardcodedSpawnAreaKind::Unknown(0)),
            min: self.min,
            max: self.max,
        }
    }

    /// Builds an area from a Bedrock hardcoded spawn area, or `None` when the
    /// kind has no shared identity.
    #[must_use]
    pub fn from_bedrock(area: &HardcodedSpawnArea) -> Option<Self> {
        Some(Self {
            kind: StructureKind::from_bedrock_area(area.kind)?,
            min: area.min,
            max: area.max,
        })
    }

    /// True when the area covers the given block position.
    #[must_use]
    pub fn contains(&self, position: [i32; 3]) -> bool {
        (0..3).all(|axis| position[axis] >= self.min[axis] && position[axis] <= self.max[axis])
    }
}

/// Decodes a Bedrock chunk's `HardcodedSpawners` value into shared areas.
///
/// Returns `(areas, unknown_kinds)`; areas whose kind Bedrock knows but Java has
/// no structure id for are reported by the caller rather than dropped silently.
pub fn read_bedrock_chunk(value: &[u8]) -> crate::error::Result<Vec<StructureArea>> {
    let areas = bedrock_world::scan::decode_hardcoded_spawn_areas(value).map_err(|error| {
        crate::error::WorldPortError::nbt(format!("HardcodedSpawners: {error}"))
    })?;
    Ok(areas.iter().filter_map(StructureArea::from_bedrock).collect())
}

/// Encodes shared areas into a Bedrock `HardcodedSpawners` value.
pub fn write_bedrock_chunk(areas: &[StructureArea]) -> crate::error::Result<Vec<u8>> {
    let bedrock: Vec<HardcodedSpawnArea> = areas.iter().map(StructureArea::to_bedrock).collect();
    let bytes = bedrock_world::scan::encode_hardcoded_spawn_areas(&bedrock)
        .map_err(|error| crate::error::WorldPortError::nbt(format!("HardcodedSpawners: {error}")))?;
    Ok(bytes.to_vec())
}

/// Reports what a Bedrock→Java conversion did with hardcoded spawn areas.
///
/// `starts` stays empty on purpose: a Java `StructureStart` needs valid piece
/// lists, and inventing them would corrupt `/locate` and later generation.
pub fn report_bedrock_areas(areas: &[StructureArea], report: &mut ReportBuilder) {
    // Counters, not warnings: a large world produces thousands of block-level
    // warnings, and a warning row for every structure kind would be pushed out
    // by the report's row limit.
    report.counters().structures = areas.len() as u64;
    if areas.is_empty() {
        return;
    }
    let mut counts: std::collections::BTreeMap<&'static str, usize> = Default::default();
    for area in areas {
        *counts.entry(area.kind.java_id()).or_default() += 1;
    }
    let summary = counts
        .iter()
        .map(|(kind, count)| format!("{count} {kind}"))
        .collect::<Vec<_>>()
        .join(", ");
    report.warn(
        WarningCode::UnsupportedFeature,
        "structure-starts",
        format!(
            "Bedrock hardcoded spawn areas ({summary}) are not written as Java structure \
             starts: a StructureStart needs its piece list, and fabricating one would break \
             /locate and later generation. The structure blocks and entities themselves are converted."
        ),
    );
}

/// Reports which Java structures could become Bedrock hardcoded spawn areas.
pub fn report_java_structures(
    convertible: &std::collections::BTreeMap<&'static str, u64>,
    unsupported: &std::collections::BTreeMap<String, u64>,
    report: &mut ReportBuilder,
) {
    if convertible.is_empty() && unsupported.is_empty() {
        return;
    }
    let converted = convertible
        .iter()
        .map(|(kind, count)| format!("{count} {kind}"))
        .collect::<Vec<_>>()
        .join(", ");
    if !converted.is_empty() {
        report.warn(
            WarningCode::Lossy,
            "structure-areas",
            format!(
                "wrote Bedrock hardcoded spawn areas for {converted}; Bedrock keeps only the \
                 spawn box, not the Java piece list"
            ),
        );
    }
    if !unsupported.is_empty() {
        let names = unsupported
            .iter()
            .take(10)
            .map(|(kind, count)| format!("{count} {kind}"))
            .collect::<Vec<_>>()
            .join(", ");
        report.warn(
            WarningCode::UnsupportedFeature,
            "structure-unsupported",
            format!(
                "Java structures without a Bedrock hardcoded spawn area were left as blocks \
                 only: {names}"
            ),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area(kind: StructureKind) -> StructureArea {
        StructureArea {
            kind,
            min: [-10, 50, -10],
            max: [10, 90, 10],
        }
    }

    #[test]
    fn every_shared_kind_round_trips_through_bedrock() {
        for kind in [
            StructureKind::NetherFortress,
            StructureKind::SwampHut,
            StructureKind::OceanMonument,
            StructureKind::PillagerOutpost,
        ] {
            let source = area(kind);
            let bedrock = source.to_bedrock();
            let back = StructureArea::from_bedrock(&bedrock).expect("round trip");
            assert_eq!(back, source, "{kind:?}");
        }
    }

    #[test]
    fn unknown_bedrock_kind_has_no_shared_identity() {
        let bedrock = HardcodedSpawnArea {
            kind: HardcodedSpawnAreaKind::Unknown(9),
            min: [0, 0, 0],
            max: [1, 1, 1],
        };
        assert_eq!(StructureArea::from_bedrock(&bedrock), None);
    }

    #[test]
    fn java_ids_match_the_vanilla_registry() {
        assert_eq!(StructureKind::NetherFortress.java_id(), "minecraft:fortress");
        assert_eq!(StructureKind::SwampHut.java_id(), "minecraft:swamp_hut");
        assert_eq!(StructureKind::OceanMonument.java_id(), "minecraft:monument");
        assert_eq!(
            StructureKind::PillagerOutpost.java_id(),
            "minecraft:pillager_outpost"
        );
    }

    #[test]
    fn java_id_round_trips() {
        for kind in [
            StructureKind::NetherFortress,
            StructureKind::SwampHut,
            StructureKind::OceanMonument,
            StructureKind::PillagerOutpost,
        ] {
            assert_eq!(
                StructureKind::from_java_id(kind.java_id()),
                Some(kind),
                "{kind:?}"
            );
        }
        assert_eq!(StructureKind::from_java_id("minecraft:village"), None);
    }

    #[test]
    fn chunk_value_round_trips_through_the_bedrock_codec() {
        let areas = vec![
            area(StructureKind::NetherFortress),
            StructureArea {
                kind: StructureKind::PillagerOutpost,
                min: [100, 60, 100],
                max: [120, 80, 120],
            },
        ];
        let encoded = write_bedrock_chunk(&areas).expect("encode");
        let decoded = read_bedrock_chunk(&encoded).expect("decode");
        assert_eq!(decoded, areas);
    }

    #[test]
    fn empty_chunk_value_decodes_to_no_areas() {
        let encoded = write_bedrock_chunk(&[]).expect("encode");
        assert!(read_bedrock_chunk(&encoded).expect("decode").is_empty());
    }

    #[test]
    fn contains_uses_inclusive_bounds() {
        let area = area(StructureKind::SwampHut);
        assert!(area.contains([-10, 50, -10]));
        assert!(area.contains([10, 90, 10]));
        assert!(area.contains([0, 70, 0]));
        assert!(!area.contains([11, 70, 0]));
        assert!(!area.contains([0, 49, 0]));
    }
}
