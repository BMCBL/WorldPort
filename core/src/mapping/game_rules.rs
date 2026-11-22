//! Game rule name and type translation between Bedrock and Java.
//!
//! Bedrock stores gamerules as flat lowercase tags in `level.dat`; Java 26.x
//! stores namespaced typed tags in `data/minecraft/game_rules.dat`. The table
//! is generated from the MIT-licensed Chunker level-settings mapping.

/// `(bedrock key, legacy Java key, namespaced Java key, kind, inverted)`.
pub const GAME_RULES: &[(&str, &str, &str, GameRuleKind, bool)] = &[
    ("dodaylightcycle", "doDaylightCycle", "minecraft:advance_time", GameRuleKind::Bool, false),
    ("doweathercycle", "doWeatherCycle", "minecraft:advance_weather", GameRuleKind::Bool, false),
    ("dotiledrops", "doTileDrops", "minecraft:block_drops", GameRuleKind::Bool, false),
    ("commandblockoutput", "commandBlockOutput", "minecraft:command_block_output", GameRuleKind::Bool, false),
    ("disableElytraMovementCheck", "disableElytraMovementCheck", "minecraft:elytra_movement_check", GameRuleKind::Bool, true),
    ("doentitydrops", "doEntityDrops", "minecraft:entity_drops", GameRuleKind::Bool, false),
    ("keepinventory", "keepInventory", "minecraft:keep_inventory", GameRuleKind::Bool, false),
    ("doLimitedCrafting", "doLimitedCrafting", "minecraft:limited_crafting", GameRuleKind::Bool, false),
    ("logAdminCommands", "logAdminCommands", "minecraft:log_admin_commands", GameRuleKind::Bool, false),
    ("maxCommandChainLength", "maxCommandChainLength", "minecraft:max_command_sequence_length", GameRuleKind::Int, false),
    ("maxEntityCramming", "maxEntityCramming", "minecraft:max_entity_cramming", GameRuleKind::Int, false),
    ("domobloot", "doMobLoot", "minecraft:mob_drops", GameRuleKind::Bool, false),
    ("mobgriefing", "mobGriefing", "minecraft:mob_griefing", GameRuleKind::Bool, false),
    ("naturalregeneration", "naturalRegeneration", "minecraft:natural_health_regeneration", GameRuleKind::Bool, false),
    ("randomtickspeed", "randomTickSpeed", "minecraft:random_tick_speed", GameRuleKind::Int, false),
    ("reducedDebugInfo", "reducedDebugInfo", "minecraft:reduced_debug_info", GameRuleKind::Bool, false),
    ("spawnradius", "spawnRadius", "minecraft:respawn_radius", GameRuleKind::Int, false),
    ("sendcommandfeedback", "sendCommandFeedback", "minecraft:send_command_feedback", GameRuleKind::Bool, false),
    ("announceAdvancements", "announceAdvancements", "minecraft:show_advancement_messages", GameRuleKind::Bool, false),
    ("showdeathmessages", "showDeathMessages", "minecraft:show_death_messages", GameRuleKind::Bool, false),
    ("domobspawning", "doMobSpawning", "minecraft:spawn_mobs", GameRuleKind::Bool, false),
    ("spectatorsGenerateChunks", "spectatorsGenerateChunks", "minecraft:spectators_generate_chunks", GameRuleKind::Bool, false),
];

/// Value type of a game rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GameRuleKind {
    /// `true`/`false` (Bedrock byte, Java byte).
    Bool,
    /// Integer value.
    Int,
}

/// Looks up the Bedrock rule for a Java name (legacy or namespaced, with or without
/// the `minecraft:` prefix).
#[must_use]
pub fn find_by_java(java: &str) -> Option<&'static (&'static str, &'static str, &'static str, GameRuleKind, bool)> {
    let short = java.strip_prefix("minecraft:").unwrap_or(java);
    GAME_RULES.iter().find(|(_, legacy, namespaced, _, _)| {
        *namespaced == java || *legacy == short || namespaced.strip_prefix("minecraft:").unwrap_or(namespaced) == short
    })
}

/// Looks up the Java rule for a Bedrock key.
#[must_use]
pub fn find_by_bedrock(bedrock: &str) -> Option<&'static (&'static str, &'static str, &'static str, GameRuleKind, bool)> {
    GAME_RULES.iter().find(|(key, _, _, _, _)| *key == bedrock)
}

/// Bedrock key for a Java rule, falling back to the lowercase short name.
#[must_use]
pub fn bedrock_key(java: &str) -> String {
    find_by_java(java)
        .map(|row| (*row.0).to_string())
        .unwrap_or_else(|| java.strip_prefix("minecraft:").unwrap_or(java).to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_rules() {
        assert_eq!(bedrock_key("minecraft:advance_time"), "dodaylightcycle");
        assert_eq!(bedrock_key("doDaylightCycle"), "dodaylightcycle");
        let row = find_by_bedrock("dodaylightcycle").expect("rule");
        assert_eq!(row.2, "minecraft:advance_time");
    }
}
