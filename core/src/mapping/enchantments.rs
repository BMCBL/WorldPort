//! Enchantment id translation between Java names and Bedrock numeric ids.
//!
//! Generated from the MIT-licensed Chunker resolvers.

/// `(Bedrock numeric id, Java id)` sorted by id.
pub const ENCHANTMENT: &[(u16, &str)] = &[
    (0, "minecraft:protection"),
    (1, "minecraft:fire_protection"),
    (2, "minecraft:feather_falling"),
    (3, "minecraft:blast_protection"),
    (4, "minecraft:projectile_protection"),
    (5, "minecraft:thorns"),
    (6, "minecraft:respiration"),
    (7, "minecraft:depth_strider"),
    (8, "minecraft:aqua_affinity"),
    (9, "minecraft:sharpness"),
    (10, "minecraft:smite"),
    (11, "minecraft:bane_of_arthropods"),
    (12, "minecraft:knockback"),
    (13, "minecraft:fire_aspect"),
    (14, "minecraft:looting"),
    (15, "minecraft:efficiency"),
    (16, "minecraft:silk_touch"),
    (17, "minecraft:unbreaking"),
    (18, "minecraft:fortune"),
    (19, "minecraft:power"),
    (20, "minecraft:punch"),
    (21, "minecraft:flame"),
    (22, "minecraft:infinity"),
    (23, "minecraft:luck_of_the_sea"),
    (24, "minecraft:lure"),
    (25, "minecraft:frost_walker"),
    (26, "minecraft:mending"),
    (27, "minecraft:binding_curse"),
    (28, "minecraft:vanishing_curse"),
    (29, "minecraft:impaling"),
    (30, "minecraft:riptide"),
    (31, "minecraft:loyalty"),
    (32, "minecraft:channeling"),
    (33, "minecraft:multishot"),
    (34, "minecraft:piercing"),
    (35, "minecraft:quick_charge"),
    (36, "minecraft:soul_speed"),
    (37, "minecraft:swift_sneak"),
    (38, "minecraft:wind_burst"),
    (39, "minecraft:density"),
    (40, "minecraft:breach"),
    (41, "minecraft:lunge"),
];

/// Bedrock numeric id for a Java identifier.
#[must_use]
pub fn enchantment_id(java: &str) -> Option<u16> {
    let short = java.strip_prefix("minecraft:").unwrap_or(java);
    let full = format!("minecraft:{short}");
    ENCHANTMENT
        .iter()
        .find(|(_, name)| *name == full)
        .map(|(value, _)| *value)
}

/// Java identifier for a Bedrock numeric id.
#[must_use]
pub fn enchantment_name(id: u16) -> Option<&'static str> {
    ENCHANTMENT
        .iter()
        .find(|(value, _)| *value == id)
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_some_ids() {
        for (id, name) in ENCHANTMENT.iter().take(5) {
            assert_eq!(enchantment_id(name), Some(*id));
            assert_eq!(enchantment_name(*id), Some(*name));
        }
    }
}
