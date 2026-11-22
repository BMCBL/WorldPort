//! Villager profession, type and trade tier across editions.
//!
//! Bedrock and Java identify a villager's profession differently:
//!
//! - Bedrock: numeric `SkinID` (profession) and `MarkVariant` (biome type), plus
//!   `TradeTier`/`TradeExperience` and an `Offers` list.
//! - Java: `VillagerData` with `profession`, `type` and `level` string ids.
//!
//! The numbers are Bedrock's internal enum order, verified against a real
//! 1.20.12 save: its `VILLAGE_*_POI` records name the professions those
//! villagers use, and the entity payloads carry the matching `SkinID`.
//!
//! Vanilla re-derives profession, type and level from the POI and trade data
//! when a villager loads, so these fields are a hint rather than authority: they
//! are written so a converted villager starts in the right role instead of
//! starting as an unemployed villager and re-acquiring one.

use crate::report::{ReportBuilder, WarningCode};

/// Bedrock `SkinID` ↔ Java profession id.
///
/// `SkinID` 0 is the unemployed villager; the rest follow Bedrock's profession
/// enum order.
const PROFESSIONS: &[(i32, &str)] = &[
    (0, "minecraft:none"),
    (1, "minecraft:farmer"),
    (2, "minecraft:fisherman"),
    (3, "minecraft:shepherd"),
    (4, "minecraft:fletcher"),
    (5, "minecraft:librarian"),
    (6, "minecraft:cartographer"),
    (7, "minecraft:cleric"),
    (8, "minecraft:armorer"),
    (9, "minecraft:weaponsmith"),
    (10, "minecraft:toolsmith"),
    (11, "minecraft:butcher"),
    (12, "minecraft:leatherworker"),
    (13, "minecraft:mason"),
    (14, "minecraft:nitwit"),
];

/// Bedrock `MarkVariant` ↔ Java villager type id.
const TYPES: &[(i32, &str)] = &[
    (0, "minecraft:plains"),
    (1, "minecraft:desert"),
    (2, "minecraft:jungle"),
    (3, "minecraft:savanna"),
    (4, "minecraft:snow"),
    (5, "minecraft:swamp"),
    (6, "minecraft:taiga"),
];

/// Java profession id for a Bedrock `SkinID`, or `None` when unmapped.
#[must_use]
pub fn profession_from_skin_id(skin_id: i32) -> Option<&'static str> {
    PROFESSIONS
        .iter()
        .find(|(id, _)| *id == skin_id)
        .map(|(_, name)| *name)
}

/// Bedrock `SkinID` for a Java profession id, or `None` when unmapped.
#[must_use]
pub fn skin_id_from_profession(profession: &str) -> Option<i32> {
    let profession = namespaced(profession);
    PROFESSIONS
        .iter()
        .find(|(_, name)| *name == profession)
        .map(|(id, _)| *id)
}

/// Java villager type id for a Bedrock `MarkVariant`.
#[must_use]
pub fn villager_type_from_mark_variant(mark_variant: i32) -> Option<&'static str> {
    TYPES
        .iter()
        .find(|(id, _)| *id == mark_variant)
        .map(|(_, name)| *name)
}

/// Bedrock `MarkVariant` for a Java villager type id.
#[must_use]
pub fn mark_variant_from_villager_type(villager_type: &str) -> Option<i32> {
    let villager_type = namespaced(villager_type);
    TYPES
        .iter()
        .find(|(_, name)| *name == villager_type)
        .map(|(id, _)| *id)
}

/// Java `VillagerData.level` from Bedrock `TradeTier`.
///
/// Bedrock's tier is 0-based and Java's level is 1-based; both cap at 5 for
/// master level, so the conversion is a shift with a clamp.
#[must_use]
pub fn villager_level(trade_tier: i64) -> i32 {
    i32::try_from(trade_tier)
        .unwrap_or(0)
        .saturating_add(1)
        .clamp(1, 5)
}

/// Bedrock `TradeTier` from Java `VillagerData.level`.
#[must_use]
pub fn trade_tier(villager_level: i64) -> i32 {
    i32::try_from(villager_level)
        .unwrap_or(1)
        .saturating_sub(1)
        .clamp(0, 4)
}

fn namespaced(name: &str) -> String {
    if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    }
}

/// Reports a villager whose profession could not be carried over.
pub fn report_unknown_profession(skin_id: i32, report: &mut ReportBuilder) {
    report.warn(
        WarningCode::UnmappedEntity,
        format!("villager-skin-{skin_id}"),
        format!("villager profession SkinID {skin_id} has no Java equivalent"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_vanilla_profession_round_trips() {
        for (skin_id, profession) in PROFESSIONS {
            assert_eq!(
                profession_from_skin_id(*skin_id),
                Some(*profession),
                "skin {skin_id}"
            );
            assert_eq!(
                skin_id_from_profession(profession),
                Some(*skin_id),
                "{profession}"
            );
        }
    }

    #[test]
    fn every_villager_type_round_trips() {
        for (mark_variant, villager_type) in TYPES {
            assert_eq!(
                villager_type_from_mark_variant(*mark_variant),
                Some(*villager_type)
            );
            assert_eq!(
                mark_variant_from_villager_type(villager_type),
                Some(*mark_variant)
            );
        }
    }

    #[test]
    fn professions_are_the_thirteen_vanilla_roles_plus_unemployed_and_nitwit() {
        // The real save's POI records name thirteen working professions; with
        // `none` and `nitwit` that is the full vanilla set.
        assert_eq!(PROFESSIONS.len(), 15);
        let working: Vec<_> = PROFESSIONS
            .iter()
            .filter(|(_, name)| !matches!(*name, "minecraft:none" | "minecraft:nitwit"))
            .collect();
        assert_eq!(working.len(), 13);
    }

    #[test]
    fn unknown_ids_are_reported_not_guessed() {
        assert_eq!(profession_from_skin_id(99), None);
        assert_eq!(skin_id_from_profession("minecraft:modded"), None);
        assert_eq!(villager_type_from_mark_variant(99), None);
    }

    #[test]
    fn namespacing_is_optional_on_input() {
        assert_eq!(skin_id_from_profession("farmer"), Some(1));
        assert_eq!(mark_variant_from_villager_type("desert"), Some(1));
    }

    #[test]
    fn villager_level_and_trade_tier_shift_and_clamp() {
        // Bedrock tiers are 0-based, Java levels 1-based.
        assert_eq!(villager_level(0), 1);
        assert_eq!(villager_level(4), 5);
        assert_eq!(villager_level(9), 5);
        assert_eq!(trade_tier(1), 0);
        assert_eq!(trade_tier(5), 4);
        assert_eq!(trade_tier(0), 0);
    }
}
