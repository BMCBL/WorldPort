//! Status effect id translation between Java names and Bedrock numeric ids.
//!
//! Generated from the MIT-licensed Chunker resolvers.

/// `(Bedrock numeric id, Java id)` sorted by id.
pub const EFFECT: &[(u16, &str)] = &[
    (0, "minecraft:empty"),
    (1, "minecraft:speed"),
    (2, "minecraft:slowness"),
    (3, "minecraft:haste"),
    (4, "minecraft:mining_fatigue"),
    (5, "minecraft:strength"),
    (6, "minecraft:instant_health"),
    (7, "minecraft:instant_damage"),
    (8, "minecraft:jump_boost"),
    (9, "minecraft:nausea"),
    (10, "minecraft:regeneration"),
    (11, "minecraft:resistance"),
    (12, "minecraft:fire_resistance"),
    (13, "minecraft:water_breathing"),
    (14, "minecraft:invisibility"),
    (15, "minecraft:blindness"),
    (16, "minecraft:night_vision"),
    (17, "minecraft:hunger"),
    (18, "minecraft:weakness"),
    (19, "minecraft:poison"),
    (20, "minecraft:wither"),
    (21, "minecraft:health_boost"),
    (22, "minecraft:absorption"),
    (23, "minecraft:saturation"),
    (24, "minecraft:levitation"),
    (26, "minecraft:conduit_power"),
    (27, "minecraft:slow_falling"),
    (28, "minecraft:bad_omen"),
    (29, "minecraft:hero_of_the_village"),
    (30, "minecraft:darkness"),
    (31, "minecraft:trial_omen"),
    (32, "minecraft:wind_charged"),
    (33, "minecraft:weaving"),
    (34, "minecraft:oozing"),
    (35, "minecraft:infested"),
    (36, "minecraft:raid_omen"),
    (37, "minecraft:breath_of_the_nautilus"),
    (38, "minecraft:freezing"),
];

/// Bedrock numeric id for a Java identifier.
#[must_use]
pub fn effect_id(java: &str) -> Option<u16> {
    let short = java.strip_prefix("minecraft:").unwrap_or(java);
    let full = format!("minecraft:{short}");
    EFFECT
        .iter()
        .find(|(_, name)| *name == full)
        .map(|(value, _)| *value)
}

/// Java identifier for a Bedrock numeric id.
#[must_use]
pub fn effect_name(id: u16) -> Option<&'static str> {
    EFFECT
        .iter()
        .find(|(value, _)| *value == id)
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_some_ids() {
        for (id, name) in EFFECT.iter().take(5) {
            assert_eq!(effect_id(name), Some(*id));
            assert_eq!(effect_name(*id), Some(*name));
        }
    }
}
