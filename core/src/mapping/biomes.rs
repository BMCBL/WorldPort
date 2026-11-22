//! Bedrock numeric biome ids and their Java biome identifiers.
//!
//! Generated from the MIT-licensed Chunker biome resolvers
//! (`BedrockBiomeIDResolver` / `JavaNamedBiomeResolver`) plus the documented
//! legacy-hill aliases that no longer exist as Java biome ids.

/// `(Java biome id without namespace, Bedrock biome id)`, sorted by Bedrock id.
pub const BE_BIOME_IDS: &[(&str, u32)] = &[
    ("ocean", 0),
    ("plains", 1),
    ("desert", 2),
    ("windswept_hills", 3),
    ("forest", 4),
    ("taiga", 5),
    ("swamp", 6),
    ("river", 7),
    ("nether_wastes", 8),
    ("the_end", 9),
    ("frozen_ocean", 10),
    ("frozen_river", 11),
    ("snowy_plains", 12),
    ("snowy_plains", 13),
    ("mushroom_fields", 14),
    ("mushroom_fields", 15),
    ("beach", 16),
    ("desert", 17),
    ("forest", 18),
    ("taiga", 19),
    ("windswept_hills", 20),
    ("jungle", 21),
    ("jungle", 22),
    ("sparse_jungle", 23),
    ("deep_ocean", 24),
    ("stony_shore", 25),
    ("snowy_beach", 26),
    ("birch_forest", 27),
    ("birch_forest", 28),
    ("dark_forest", 29),
    ("snowy_taiga", 30),
    ("snowy_taiga", 31),
    ("old_growth_pine_taiga", 32),
    ("old_growth_pine_taiga", 33),
    ("windswept_forest", 34),
    ("savanna", 35),
    ("savanna_plateau", 36),
    ("badlands", 37),
    ("wooded_badlands", 38),
    ("badlands", 39),
    ("warm_ocean", 40),
    ("lukewarm_ocean", 42),
    ("deep_lukewarm_ocean", 43),
    ("cold_ocean", 44),
    ("deep_cold_ocean", 45),
    ("frozen_ocean", 46),
    ("deep_frozen_ocean", 47),
    ("bamboo_jungle", 48),
    ("bamboo_jungle", 49),
    ("sunflower_plains", 129),
    ("windswept_gravelly_hills", 131),
    ("flower_forest", 132),
    ("taiga", 133),
    ("ice_spikes", 140),
    ("sparse_jungle", 149),
    ("sparse_jungle", 151),
    ("old_growth_birch_forest", 155),
    ("snowy_taiga", 158),
    ("old_growth_spruce_taiga", 160),
    ("old_growth_spruce_taiga", 161),
    ("windswept_savanna", 163),
    ("windswept_savanna", 164),
    ("eroded_badlands", 165),
    ("wooded_badlands", 166),
    ("badlands", 167),
    ("soul_sand_valley", 178),
    ("crimson_forest", 179),
    ("warped_forest", 180),
    ("basalt_deltas", 181),
    ("jagged_peaks", 182),
    ("frozen_peaks", 183),
    ("snowy_slopes", 184),
    ("grove", 185),
    ("meadow", 186),
    ("lush_caves", 187),
    ("dripstone_caves", 188),
    ("stony_peaks", 189),
    ("deep_dark", 190),
    ("mangrove_swamp", 191),
    ("cherry_grove", 192),
    ("pale_garden", 193),
    ("sulfur_caves", 194),
    ("dappled_forest", 195),
    ("ice_caves", 196),
];

/// Java biome identifier for a Bedrock numeric biome id.
#[must_use]
pub fn java_biome_name(bedrock_id: u32) -> Option<&'static str> {
    BE_BIOME_IDS
        .iter()
        .find(|(_, id)| *id == bedrock_id)
        .map(|(name, _)| *name)
}

/// Bedrock numeric id for a Java biome identifier (with or without namespace).
#[must_use]
pub fn bedrock_biome_id(java_name: &str) -> Option<u32> {
    let short = java_name.strip_prefix("minecraft:").unwrap_or(java_name);
    BE_BIOME_IDS
        .iter()
        .find(|(name, _)| *name == short)
        .map(|(_, id)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_biomes_round_trip() {
        for name in ["ocean", "plains", "desert", "the_end", "nether_wastes"] {
            let id = bedrock_biome_id(name).expect("biome id");
            assert_eq!(java_biome_name(id), Some(name));
        }
        assert_eq!(bedrock_biome_id("minecraft:plains"), Some(1));
    }
}
