//! Potion id translation between Java names and Bedrock numeric ids.
//!
//! Generated from the MIT-licensed Chunker resolvers.

/// `(Bedrock numeric id, Java id)` sorted by id.
pub const POTION: &[(u32, &str)] = &[
    (0, "minecraft:water"),
    (1, "minecraft:mundane"),
    (3, "minecraft:thick"),
    (4, "minecraft:awkward"),
    (5, "minecraft:night_vision"),
    (6, "minecraft:long_night_vision"),
    (7, "minecraft:invisibility"),
    (8, "minecraft:long_invisibility"),
    (9, "minecraft:leaping"),
    (10, "minecraft:long_leaping"),
    (11, "minecraft:strong_leaping"),
    (12, "minecraft:fire_resistance"),
    (13, "minecraft:long_fire_resistance"),
    (14, "minecraft:swiftness"),
    (15, "minecraft:long_swiftness"),
    (16, "minecraft:strong_swiftness"),
    (17, "minecraft:slowness"),
    (18, "minecraft:long_slowness"),
    (19, "minecraft:water_breathing"),
    (20, "minecraft:long_water_breathing"),
    (21, "minecraft:healing"),
    (22, "minecraft:strong_healing"),
    (23, "minecraft:harming"),
    (24, "minecraft:strong_harming"),
    (25, "minecraft:poison"),
    (26, "minecraft:long_poison"),
    (27, "minecraft:strong_poison"),
    (28, "minecraft:regeneration"),
    (29, "minecraft:long_regeneration"),
    (30, "minecraft:strong_regeneration"),
    (31, "minecraft:strength"),
    (32, "minecraft:long_strength"),
    (33, "minecraft:strong_strength"),
    (34, "minecraft:weakness"),
    (35, "minecraft:long_weakness"),
    (37, "minecraft:turtle_master"),
    (38, "minecraft:long_turtle_master"),
    (39, "minecraft:strong_turtle_master"),
    (40, "minecraft:slow_falling"),
    (41, "minecraft:long_slow_falling"),
    (42, "minecraft:strong_slowness"),
    (43, "minecraft:wind_charged"),
    (44, "minecraft:weaving"),
    (45, "minecraft:oozing"),
    (46, "minecraft:infested"),
    (47, "minecraft:freezing"),
    (48, "minecraft:long_freezing"),
];

/// Bedrock numeric id for a Java identifier.
#[must_use]
pub fn potion_id(java: &str) -> Option<u32> {
    let short = java.strip_prefix("minecraft:").unwrap_or(java);
    let full = format!("minecraft:{short}");
    POTION
        .iter()
        .find(|(_, name)| *name == full)
        .map(|(value, _)| *value)
}

/// Java identifier for a Bedrock numeric id.
#[must_use]
pub fn potion_name(id: u32) -> Option<&'static str> {
    POTION
        .iter()
        .find(|(value, _)| *value == id)
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_some_ids() {
        for (id, name) in POTION.iter().take(5) {
            assert_eq!(potion_id(name), Some(*id));
            assert_eq!(potion_name(*id), Some(*name));
        }
    }
}
