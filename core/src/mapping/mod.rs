//! Cross-edition identifier mappings.
//!
//! Block catalogs come from Mojang's datagen reports (Java, one folder per
//! version under `mappings/data/java`) and from the catalog embedded in
//! `bedrock-world` (Bedrock, generated from `bedrock-samples`). [`mapper`] turns
//! states from one edition into the other; everything it cannot express is
//! reported instead of silently guessed.

pub mod biomes;
pub mod effects;
pub mod enchantments;
pub mod entities;
pub mod game_rules;
pub mod map_colors;
pub mod potions;
pub mod mapper;
pub mod schema;

pub use mapper::{BlockMapper, MappedState, short_name};
pub use schema::{BlockCatalog, JavaCatalog, PropertyKind, bedrock_catalog, bedrock_format_version};

/// Strips an optional `minecraft:` namespace.
#[must_use]
pub fn strip_namespace(name: &str) -> &str {
    short_name(name)
}

/// Whether an identifier belongs to the vanilla namespace.
///
/// Mods and addons use their own namespace, so they can be pruned instead of
/// being reported as a mapping gap.
#[must_use]
pub fn is_vanilla(name: &str) -> bool {
    match name.split_once(':') {
        Some((namespace, _)) => namespace == "minecraft",
        None => true,
    }
}

/// Returns the name with exactly one `minecraft:` namespace.
#[must_use]
pub fn with_namespace(name: &str) -> String {
    if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    }
}
