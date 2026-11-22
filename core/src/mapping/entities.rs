//! Entity identifier translation between Java and Bedrock.
//!
//! Generated from the MIT-licensed Chunker entity type resolvers, which track
//! the vanilla identifier renames (boats, xp orbs, thrown items, ...).

/// `(Java id, Bedrock id)` for entity types whose identifiers differ.
pub const ENTITY_ALIASES: &[(&str, &str)] = &[
    ("minecraft:breeze_wind_charge", "minecraft:breeze_wind_charge_projectile"),
    ("minecraft:commandblock_minecart", "minecraft:command_block_minecart"),
    ("minecraft:end_crystal", "minecraft:ender_crystal"),
    ("minecraft:evocation_fangs", "minecraft:evocation_fang"),
    ("minecraft:evoker", "minecraft:evocation_illager"),
    ("minecraft:evoker_fangs", "minecraft:evocation_fang"),
    ("minecraft:experience_bottle", "minecraft:xp_bottle"),
    ("minecraft:experience_orb", "minecraft:xp_orb"),
    ("minecraft:eye_of_ender", "minecraft:eye_of_ender_signal"),
    ("minecraft:firework_rocket", "minecraft:fireworks_rocket"),
    ("minecraft:fishing_bobber", "minecraft:fishing_hook"),
    ("minecraft:oak_boat", "minecraft:boat"),
    ("minecraft:oak_chest_boat", "minecraft:chest_boat"),
    ("minecraft:potion", "minecraft:splash_potion"),
    ("minecraft:snowman", "minecraft:snow_golem"),
    ("minecraft:trident", "minecraft:thrown_trident"),
    ("minecraft:tropical_fish", "minecraft:tropicalfish"),
    ("minecraft:villager", "minecraft:villager_v2"),
    ("minecraft:villager_golem", "minecraft:iron_golem"),
    ("minecraft:vindication_illager", "minecraft:vindicator"),
    ("minecraft:wind_charge", "minecraft:wind_charge_projectile"),
    ("minecraft:zombie_villager", "minecraft:zombie_villager_v2"),
    ("minecraft:zombified_piglin", "minecraft:zombie_pigman"),
];

/// Bedrock identifier for a Java entity id (identity when unchanged).
#[must_use]
pub fn entity_to_bedrock(java: &str) -> String {
    let short = java.strip_prefix("minecraft:").unwrap_or(java);
    let full = format!("minecraft:{short}");
    ENTITY_ALIASES
        .iter()
        .find(|(source, _)| *source == full)
        .map(|(_, target)| (*target).to_string())
        .unwrap_or(full)
}

/// Java identifier for a Bedrock entity id (identity when unchanged).
#[must_use]
pub fn entity_to_java(bedrock: &str) -> String {
    let short = bedrock.strip_prefix("minecraft:").unwrap_or(bedrock);
    let full = format!("minecraft:{short}");
    ENTITY_ALIASES
        .iter()
        .find(|(_, target)| *target == full)
        .map(|(source, _)| (*source).to_string())
        .unwrap_or(full)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renames_known_entities() {
        assert_eq!(entity_to_bedrock("minecraft:experience_orb"), "minecraft:xp_orb");
        assert_eq!(entity_to_bedrock("minecraft:oak_boat"), "minecraft:boat");
        assert_eq!(entity_to_java("minecraft:xp_orb"), "minecraft:experience_orb");
        assert_eq!(entity_to_bedrock("minecraft:zombie"), "minecraft:zombie");
    }
}
