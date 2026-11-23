//! Why some Bedrock blocks have no Java counterpart.
//!
//! A conversion that reports "548 blocks skipped" tells a user nothing. These are
//! not mapping gaps to fix - they are blocks Java does not have - so the reason is
//! named and counted per category, and the report can say "191 education-edition
//! blocks were skipped" instead of listing a hundred identifiers.
//!
//! This is deliberately a closed list. Anything not named here is expected to map,
//! so the classifier cannot quietly excuse a real gap.

/// Why a Bedrock block has no Java block.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum NoJavaBlock {
    /// Education Edition: elements, lab equipment, coloured torches, chalkboard.
    Education,
    /// Java models it as an entity (`frame`, `glow_frame`).
    Entity,
    /// A name Bedrock kept for compatibility and never shipped in Java.
    Legacy,
    /// A Bedrock utility block Java spells differently or lacks.
    Utility,
}

impl NoJavaBlock {
    /// Every category, in report order.
    pub const ALL: [Self; 4] = [
        Self::Education,
        Self::Entity,
        Self::Legacy,
        Self::Utility,
    ];

    /// The report label for this reason.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Education => "education edition",
            Self::Entity => "modelled as an entity in Java",
            Self::Legacy => "retired Bedrock block",
            Self::Utility => "Bedrock-only utility block",
        }
    }
}

/// Why a Bedrock block has no Java block, if that is the case.
///
/// Returns `None` for anything expected to map.
#[must_use]
pub fn no_java_block(name: &str) -> Option<NoJavaBlock> {
    let short = name.strip_prefix("minecraft:").unwrap_or(name);
    if short.starts_with("element_")
        || short.starts_with("colored_torch_")
        || short.starts_with("hard_")
        || matches!(
            short,
            "compound_creator"
                | "lab_table"
                | "material_reducer"
                | "chemical_heat"
                | "underwater_torch"
                | "chalkboard"
                | "camera"
                | "sparkler"
                | "glow_stick"
                | "balloon"
                | "ice_bomb"
                | "bleach"
                | "rapid_fertilizer"
                | "super_fertilizer"
                | "medicine"
                | "elixir"
        )
    {
        return Some(NoJavaBlock::Education);
    }
    if matches!(short, "frame" | "glow_frame") {
        return Some(NoJavaBlock::Entity);
    }
    if short.starts_with("deprecated_")
        || matches!(
            short,
            "client_request_placeholder_block"
                | "invisible_bedrock"
                | "info_update"
                | "info_update2"
                | "reserved6"
                | "unknown"
        )
    {
        return Some(NoJavaBlock::Legacy);
    }
    if matches!(
        short,
        "disc_press" | "underwater_tnt" | "monster_egg" | "structure_void" | "moving_block"
    ) {
        return Some(NoJavaBlock::Utility);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn education_blocks_are_named_as_such() {
        // The elements alone are 118 identifiers; a report has to collapse them.
        for name in [
            "minecraft:element_0",
            "minecraft:element_118",
            "minecraft:compound_creator",
            "minecraft:chalkboard",
            "minecraft:colored_torch_blue",
            "minecraft:hard_glass",
        ] {
            assert_eq!(no_java_block(name), Some(NoJavaBlock::Education), "{name}");
        }
    }

    #[test]
    fn an_entity_block_is_not_called_a_mapping_gap() {
        assert_eq!(no_java_block("minecraft:frame"), Some(NoJavaBlock::Entity));
        assert_eq!(no_java_block("minecraft:glow_frame"), Some(NoJavaBlock::Entity));
    }

    #[test]
    fn a_block_that_should_map_is_never_classified() {
        // The classifier must not become a place to hide real gaps.
        for name in [
            "minecraft:stone",
            "minecraft:oak_planks",
            "minecraft:stonecutter_block",
            "minecraft:normal_stone_slab",
            "minecraft:barrier",
            "minecraft:cobblestone_wall",
        ] {
            assert_eq!(no_java_block(name), None, "{name} must be expected to map");
        }
    }
}
