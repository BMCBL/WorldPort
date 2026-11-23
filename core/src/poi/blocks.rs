//! Java blocks → Bedrock village interest inventory.
//!
//! Bedrock has no per-position POI storage: a `VILLAGE_*_POI` record belongs to a
//! village (UUID, bounds, dwellers) that only Bedrock's village manager can
//! create. Writing synthetic village records would give the game a village whose
//! bounds and residents do not match its own bookkeeping, so this module does not
//! do that.
//!
//! What it does instead: count the interests that exist in the converted terrain
//! and report them. Bedrock's village manager scans POIs when chunks load and
//! rebuilds villages from the blocks, which are already converted 1:1 — so the
//! behaviour is restored by the game itself rather than by fabricated metadata.

use std::collections::BTreeMap;

use bedrock_world::village::InterestKind;

use crate::report::{ReportBuilder, WarningCode};

/// A Java block that Bedrock treats as a village interest.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub enum BlockInterest {
    /// A bed (`minecraft:*_bed`), a villager home.
    Bed,
    /// A bell, a village gathering site.
    MeetingPoint,
    /// A workstation block; the value is the profession it serves.
    Workstation(&'static str),
}

impl BlockInterest {
    /// Bedrock interest kind this block provides.
    #[must_use]
    pub const fn kind(self) -> InterestKind {
        match self {
            Self::Bed => InterestKind::Bed,
            Self::MeetingPoint => InterestKind::MeetingPoint,
            Self::Workstation(_) => InterestKind::Workstation,
        }
    }

    /// Bedrock `Name` for the interest instance.
    #[must_use]
    pub const fn bedrock_name(self) -> &'static str {
        match self {
            Self::Bed | Self::MeetingPoint => "villager",
            Self::Workstation(profession) => profession,
        }
    }
}

/// Classifies a Java block name (without properties) as a village interest.
///
/// Only blocks Bedrock's village manager recognises are listed: beds, bells and
/// the profession workstations. Composters, barrels and the like are skipped
/// because they only matter through the profession they unlock.
#[must_use]
pub fn classify(block: &str) -> Option<BlockInterest> {
    let name = block.strip_prefix("minecraft:").unwrap_or(block);
    if name == "bell" {
        return Some(BlockInterest::MeetingPoint);
    }
    if name.ends_with("_bed") {
        return Some(BlockInterest::Bed);
    }
    Some(BlockInterest::Workstation(match name {
        "blast_furnace" => "armorer",
        "smoker" => "butcher",
        "cartography_table" => "cartographer",
        "brewing_stand" => "cleric",
        "composter" => "farmer",
        "barrel" => "fisherman",
        "fletching_table" => "fletcher",
        "cauldron" | "water_cauldron" | "lava_cauldron" | "powder_snow_cauldron" => "leatherworker",
        "lectern" => "librarian",
        "stonecutter" => "mason",
        "loom" => "shepherd",
        "smithing_table" => "toolsmith",
        "grindstone" => "weaponsmith",
        _ => return None,
    }))
}

/// Interests found in the converted terrain.
#[derive(Debug, Default, Clone, Eq, PartialEq)]
pub struct InterestInventory {
    /// Count per Bedrock interest name (`villager`, `farmer`, ...).
    pub by_name: BTreeMap<&'static str, u64>,
    /// Count per interest kind.
    pub by_kind: BTreeMap<InterestKind, u64>,
    /// Total interest blocks seen.
    pub total: u64,
}

impl InterestInventory {
    /// Records one interest block.
    pub fn record(&mut self, interest: BlockInterest) {
        self.record_many(interest, 1);
    }

    /// Accounts for an entire palette entry's usage in constant time.
    ///
    /// Avoids repeating two BTreeMap lookups for each of the 4096 block
    /// positions when a section has many village-interest blocks.
    pub fn record_many(&mut self, interest: BlockInterest, count: u64) {
        if count == 0 {
            return;
        }
        let by_name = self.by_name.entry(interest.bedrock_name()).or_default();
        *by_name = by_name.saturating_add(count);
        let by_kind = self.by_kind.entry(interest.kind()).or_default();
        *by_kind = by_kind.saturating_add(count);
        self.total = self.total.saturating_add(count);
    }

    /// True when the terrain holds no interest block.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }
}

/// Reports what the converted terrain contains and why no village records follow.
///
/// Bedrock rebuilds villages from loaded chunks, so the conversion's job is to
/// state what it left for the game to find instead of writing metadata the game
/// would have to reconcile.
pub fn report_inventory(inventory: &InterestInventory, report: &mut ReportBuilder) {
    if inventory.is_empty() {
        return;
    }
    report.counters().village_interests = inventory.total;
    let beds = inventory.by_kind.get(&InterestKind::Bed).copied().unwrap_or(0);
    let meetings = inventory
        .by_kind
        .get(&InterestKind::MeetingPoint)
        .copied()
        .unwrap_or(0);
    let workstations = inventory
        .by_kind
        .get(&InterestKind::Workstation)
        .copied()
        .unwrap_or(0);
    report.warn(
        WarningCode::Lossy,
        "village-records",
        format!(
            "converted {beds} beds, {meetings} bells and {workstations} workstations; \
             Bedrock village records are rebuilt by the game when the chunks load, \
             because a village (bounds, residents) cannot be derived from blocks alone"
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beds_and_bells_are_recognised() {
        assert_eq!(classify("minecraft:red_bed"), Some(BlockInterest::Bed));
        assert_eq!(classify("minecraft:light_blue_bed"), Some(BlockInterest::Bed));
        assert_eq!(classify("minecraft:bell"), Some(BlockInterest::MeetingPoint));
    }

    #[test]
    fn every_profession_workstation_maps_to_its_profession() {
        let cases = [
            ("minecraft:blast_furnace", "armorer"),
            ("minecraft:smoker", "butcher"),
            ("minecraft:cartography_table", "cartographer"),
            ("minecraft:brewing_stand", "cleric"),
            ("minecraft:composter", "farmer"),
            ("minecraft:barrel", "fisherman"),
            ("minecraft:fletching_table", "fletcher"),
            ("minecraft:cauldron", "leatherworker"),
            ("minecraft:lectern", "librarian"),
            ("minecraft:stonecutter", "mason"),
            ("minecraft:loom", "shepherd"),
            ("minecraft:smithing_table", "toolsmith"),
            ("minecraft:grindstone", "weaponsmith"),
        ];
        for (block, profession) in cases {
            assert_eq!(
                classify(block),
                Some(BlockInterest::Workstation(profession)),
                "{block}"
            );
        }
    }

    #[test]
    fn water_cauldron_variants_are_all_leatherworker() {
        for block in [
            "minecraft:cauldron",
            "minecraft:water_cauldron",
            "minecraft:lava_cauldron",
            "minecraft:powder_snow_cauldron",
        ] {
            assert_eq!(
                classify(block),
                Some(BlockInterest::Workstation("leatherworker")),
                "{block}"
            );
        }
    }

    #[test]
    fn unrelated_blocks_are_not_interests() {
        for block in [
            "minecraft:stone",
            "minecraft:chest",
            "minecraft:furnace",
            "minecraft:crafting_table",
        ] {
            assert_eq!(classify(block), None, "{block}");
        }
    }

    #[test]
    fn palette_bulk_accounting_matches_single_block_accounting() {
        let mut per_block = InterestInventory::default();
        let mut per_palette = InterestInventory::default();
        let interest = BlockInterest::Workstation("farmer");
        for _ in 0..4096 {
            per_block.record(interest);
        }
        per_palette.record_many(interest, 4096);
        per_palette.record_many(interest, 0);
        assert_eq!(per_block, per_palette);
    }

    #[test]
    fn inventory_counts_by_name_and_kind() {
        let mut inventory = InterestInventory::default();
        inventory.record(BlockInterest::Bed);
        inventory.record(BlockInterest::Bed);
        inventory.record(BlockInterest::MeetingPoint);
        inventory.record(BlockInterest::Workstation("farmer"));
        assert_eq!(inventory.total, 4);
        assert_eq!(inventory.by_name.get("villager"), Some(&3));
        assert_eq!(inventory.by_name.get("farmer"), Some(&1));
        assert_eq!(inventory.by_kind.get(&InterestKind::Bed), Some(&2));
        assert_eq!(
            inventory.by_kind.get(&InterestKind::MeetingPoint),
            Some(&1)
        );
        assert_eq!(
            inventory.by_kind.get(&InterestKind::Workstation),
            Some(&1)
        );
    }

    #[test]
    fn bedrock_names_match_the_interest_model() {
        // The names must be the ones Bedrock writes into VILLAGE_*_POI records.
        assert_eq!(BlockInterest::Bed.bedrock_name(), "villager");
        assert_eq!(BlockInterest::MeetingPoint.bedrock_name(), "villager");
        assert_eq!(
            BlockInterest::Workstation("librarian").bedrock_name(),
            "librarian"
        );
    }
}
