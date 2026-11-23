//! Block state mapping between Java and Bedrock.
//!
//! The mapper is schema-driven: a rule picks the target block name, then each
//! property is renamed to whatever the *target* block actually declares. That
//! keeps one rule set valid across Bedrock versions that renamed states
//! (`facing_direction` → `minecraft:facing_direction`, `top_slot_bit` →
//! `minecraft:vertical_half`).
//!
//! Families whose state model genuinely differs (stairs, torches, doors,
//! cauldrons, plants, bitmask connections) have explicit rules.
//!
//! `waterlogged` never crosses the boundary as a state: Bedrock stores the fluid
//! in a second block layer, so the chunk writer consumes it instead.
//!
//! # Mapping references
//!
//! Orientation rules follow the vanilla block-state semantics as encoded by two
//! independent MIT references, so they are not guessed:
//!
//! - Chunker's `BedrockStateTypes` / `BedrockStateGroups` (Hive Games): the
//!   authoritative tables for `FACING_DIRECTION_LEGACY` (0=down, 1=up,
//!   2=north, 3=south, 4=west, 5=east), `WEIRDO_DIRECTION` (0=east, 1=west,
//!   2=south, 3=north), `FLIPPED_FACING_DIRECTION` (pistons, end rods),
//!   `CARDINAL_DIRECTION_LEGACY` (0=south, 1=west, 2=north, 3=east),
//!   `CARDINAL_DIRECTION_LEGACY_INVERTED` (decorated pot),
//!   `CARDINAL_DIRECTION_DOOR` (Java facing rotated one step), `DOOR_DIRECTION`,
//!   `HINGE`/`HALF`, `BUTTON`, `LEVER_DIRECTION_POWER` and `CORNER`.
//! - Minecraft Wiki block-state pages for the same properties (Bedrock
//!   `weirdo_direction` = the side the full block sits on, etc.).
//!
//! Known gaps that need in-game confirmation: skull/banner dye data lives in
//! block-entity NBT (M3), potted plants have no per-plant Bedrock id, and
//! pistons cannot store `extended` (Bedrock derives it from the arm block).

use std::collections::BTreeMap;

use crate::mapping::schema::{BlockCatalog, PropertyKind};

mod legacy;

/// Result of mapping one block state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MappedState {
    /// Target block identifier with `minecraft:` namespace.
    pub name: String,
    /// Target properties as strings; the writer converts them to NBT values.
    pub properties: BTreeMap<String, String>,
    /// True when data had to be dropped to fit the target.
    pub lossy: bool,
}

impl MappedState {
    fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            properties: BTreeMap::new(),
            lossy: false,
        }
    }

    fn with(mut self, property: impl Into<String>, value: impl Into<String>) -> Self {
        self.properties.insert(property.into(), value.into());
        self
    }

    const fn lossy(mut self) -> Self {
        self.lossy = true;
        self
    }
}

/// Maps Java and Bedrock block states in both directions.
#[derive(Debug)]
pub struct BlockMapper {
    java: BlockCatalog,
    bedrock: BlockCatalog,
}

impl BlockMapper {
    /// Builds a mapper from the two catalogs.
    #[must_use]
    pub const fn new(java: BlockCatalog, bedrock: BlockCatalog) -> Self {
        Self { java, bedrock }
    }

    /// Java catalog.
    #[must_use]
    pub const fn java(&self) -> &BlockCatalog {
        &self.java
    }

    /// Bedrock catalog.
    #[must_use]
    pub const fn bedrock(&self) -> &BlockCatalog {
        &self.bedrock
    }

    /// Converts a Java block state into a Bedrock one.
    #[must_use]
    pub fn je_to_be(
        &self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Option<MappedState> {
        let short = short_name(name);
        // A name both editions share is used as-is; an alias is only for names
        // that actually differ. Trying the alias first renamed blocks that had
        // already converged, such as 26.2's `iron_chain`, into a name the target
        // version cannot express - and lost every property with it.
        let mut mapped = je_special(short, properties)
            .or_else(|| {
                (self.bedrock.contains(short) || KNOWN_UNLISTED.contains(&short))
                    .then(|| MappedState::new(namespaced(short)))
            })
            .or_else(|| je_alias(short))?;
        translate_properties(
            &mut mapped,
            properties,
            &self.bedrock,
            Direction::JavaToBedrock,
        );
        let mut mapped = fit_to_schema(mapped, &self.bedrock);
        if is_approximate(short, Direction::JavaToBedrock) {
            mapped.lossy = true;
        }
        Some(mapped)
    }

    /// Converts a Bedrock block state into a Java one.
    #[must_use]
    pub fn be_to_je(
        &self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Option<MappedState> {
        let short = short_name(name);
        let mut mapped = be_special(short, properties)
            .or_else(|| self.java.contains(short).then(|| MappedState::new(namespaced(short))))
            .or_else(|| be_alias(short))?;
        translate_properties(&mut mapped, properties, &self.java, Direction::BedrockToJava);
        let mut mapped = fit_to_schema(mapped, &self.java);
        if is_approximate(short, Direction::BedrockToJava) {
            mapped.lossy = true;
        }
        Some(mapped)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Direction {
    JavaToBedrock,
    BedrockToJava,
}

/// Removes the `minecraft:` namespace.
#[must_use]
pub fn short_name(name: &str) -> &str {
    name.strip_prefix("minecraft:").unwrap_or(name)
}

fn namespaced(name: &str) -> String {
    if name.contains(':') {
        name.to_string()
    } else {
        format!("minecraft:{name}")
    }
}

const COLORS: &[&str] = &[
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

fn split_color(name: &str) -> Option<(&str, &str)> {
    for color in COLORS {
        if let Some(rest) = name.strip_prefix(color).and_then(|rest| rest.strip_prefix('_'))
            && !rest.is_empty()
        {
            return Some((color, rest));
        }
    }
    None
}

/// Vanilla Bedrock block ids that the embedded catalog sample does not list but
/// the game accepts; kept explicit instead of guessing on every unknown name.
const KNOWN_UNLISTED: &[&str] = &[
    "copper_block",
    "cut_copper",
    "chiseled_copper",
    "waxed_copper_block",
    "waxed_cut_copper",
    "waxed_chiseled_copper",
];

/// Java-to-Bedrock identifier aliases for names that differ per edition.
const JE_ALIASES: &[(&str, &str)] = &[
    ("bricks", "brick_block"),
    ("nether_bricks", "nether_brick"),
    ("red_nether_bricks", "red_nether_brick"),
    ("end_stone_bricks", "end_bricks"),
    ("end_stone_brick_stairs", "end_brick_stairs"),
    ("cobblestone_stairs", "normal_stone_stairs"),
    ("prismarine_brick_stairs", "prismarine_bricks_stairs"),
    ("cobweb", "web"),
    ("magma_block", "magma"),
    ("melon", "melon_block"),
    ("slime_block", "slime"),
    ("spawner", "mob_spawner"),
    ("sugar_cane", "reeds"),
    ("lily_pad", "waterlily"),
    ("dirt_path", "grass_path"),
    ("dead_bush", "deadbush"),
    ("note_block", "noteblock"),
    ("powered_rail", "golden_rail"),
    ("nether_quartz_ore", "quartz_ore"),
    ("terracotta", "hardened_clay"),
    ("rooted_dirt", "dirt_with_roots"),
    ("flowering_azalea_leaves", "azalea_leaves_flowered"),
    ("frogspawn", "frog_spawn"),
    ("jack_o_lantern", "lit_pumpkin"),
    ("nether_portal", "portal"),
    ("cave_air", "air"),
    ("void_air", "air"),
    ("moving_piston", "movingBlock"),
    ("piston_head", "pistonArmCollision"),
    // Bedrock 1.26 named the plain chain after its material once copper chains
    // arrived; the plain name no longer exists there.
    ("chain", "iron_chain"),
    ("waxed_copper_block", "waxed_copper"),
    ("oak_button", "wooden_button"),
    ("oak_pressure_plate", "wooden_pressure_plate"),
    ("oak_door", "wooden_door"),
    ("oak_trapdoor", "trapdoor"),
    ("oak_fence_gate", "fence_gate"),
    ("oak_sign", "standing_sign"),
    ("oak_wall_sign", "wall_sign"),
    ("light_gray_glazed_terracotta", "silver_glazed_terracotta"),
    ("sea_lantern", "seaLantern"),
    ("concrete_powder", "concretePowder"),
    ("snow_block", "snow"),
    ("open_eyeblossom", "open_eyeblossom"),
    ("test_block", "reserved6"),
    ("test_instance_block", "reserved6"),
];

fn je_alias(short: &str) -> Option<MappedState> {
    if let Some((_, target)) = JE_ALIASES.iter().find(|(source, _)| *source == short) {
        return Some(MappedState::new(namespaced(target)));
    }
    if let Some((color, rest)) = split_color(short) {
        let target = match rest {
            "bed" => Some("bed"),
            "banner" => Some("standing_banner"),
            "wall_banner" => Some("wall_banner"),
            _ => None,
        };
        if let Some(target) = target {
            return Some(MappedState::new(namespaced(target)).with("color", color));
        }
    }
    // Order matters: the most specific suffix has to win, or `oak_wall_sign`
    // matches `_sign` first and becomes `oak_wall_standing_sign`, a block that
    // does not exist. That produced an invalid id for every wall sign.
    // Before 1.14 Java had one `sign` for every wood type; an alias entry for it
    // would shadow `oak_sign`, because the reverse lookup takes the first match.
    if short == "sign" {
        return Some(MappedState::new("minecraft:standing_sign").lossy());
    }
    if let Some(wood) = short.strip_suffix("_wall_hanging_sign") {
        return Some(MappedState::new(namespaced(&format!("{wood}_hanging_sign"))).lossy());
    }
    if let Some(wood) = short.strip_suffix("_hanging_sign") {
        return Some(MappedState::new(namespaced(&format!("{wood}_hanging_sign"))));
    }
    if let Some(wood) = short.strip_suffix("_wall_sign") {
        let wood = wood.replace("dark_oak", "darkoak");
        return Some(MappedState::new(namespaced(&format!("{wood}_wall_sign"))));
    }
    if let Some(wood) = short.strip_suffix("_sign") {
        let wood = wood.replace("dark_oak", "darkoak");
        return Some(MappedState::new(namespaced(&format!("{wood}_standing_sign"))));
    }
    if short.starts_with("potted_") {
        return Some(MappedState::new("minecraft:flower_pot").lossy());
    }
    None
}

fn be_alias(short: &str) -> Option<MappedState> {
    if let Some((source, _)) = JE_ALIASES.iter().find(|(_, target)| *target == short) {
        return Some(MappedState::new(namespaced(source)));
    }
    // Bedrock still ships the pre-1.4 `hard_` spelling of hardened glass. Without
    // this every pane and stained block in an old build becomes air; the Java
    // names are simply the suffix.
    if let Some(rest) = short.strip_prefix("hard_") {
        let target = match rest {
            "glass" => "glass",
            "glass_pane" => "glass_pane",
            other if other.starts_with(|c: char| c.is_ascii_lowercase())
                && (other.ends_with("_stained_glass") || other.ends_with("_stained_glass_pane")) =>
            {
                other
            }
            _ => return None,
        };
        return Some(MappedState::new(namespaced(target)));
    }
    if short == "deprecated_anvil" {
        // The id Bedrock kept for an old anvil variant; the block is an anvil.
        return Some(MappedState::new("minecraft:anvil"));
    }
    match short {
        // Bedrock keeps the pre-split names for the oak variants.
        "standing_sign" => return Some(MappedState::new("minecraft:oak_sign").lossy()),
        "wall_sign" => return Some(MappedState::new("minecraft:oak_wall_sign").lossy()),
        "brick_block" => return Some(MappedState::new("minecraft:bricks")),
        "nether_brick" => return Some(MappedState::new("minecraft:nether_bricks")),
        "red_nether_brick" => return Some(MappedState::new("minecraft:red_nether_bricks")),
        "end_bricks" => return Some(MappedState::new("minecraft:end_stone_bricks")),
        "end_brick_stairs" => return Some(MappedState::new("minecraft:end_stone_brick_stairs")),
        "normal_stone_stairs" => return Some(MappedState::new("minecraft:cobblestone_stairs")),
        "prismarine_bricks_stairs" => {
            return Some(MappedState::new("minecraft:prismarine_brick_stairs"));
        }
        "web" => return Some(MappedState::new("minecraft:cobweb")),
        "magma" => return Some(MappedState::new("minecraft:magma_block")),
        "melon_block" => return Some(MappedState::new("minecraft:melon")),
        "slime" => return Some(MappedState::new("minecraft:slime_block")),
        "mob_spawner" => return Some(MappedState::new("minecraft:spawner")),
        "reeds" => return Some(MappedState::new("minecraft:sugar_cane")),
        "waterlily" => return Some(MappedState::new("minecraft:lily_pad")),
        "grass_path" => return Some(MappedState::new("minecraft:dirt_path")),
        "deadbush" => return Some(MappedState::new("minecraft:dead_bush")),
        "noteblock" => return Some(MappedState::new("minecraft:note_block")),
        "golden_rail" => return Some(MappedState::new("minecraft:powered_rail")),
        "quartz_ore" => return Some(MappedState::new("minecraft:nether_quartz_ore")),
        "hardened_clay" => return Some(MappedState::new("minecraft:terracotta")),
        "dirt_with_roots" => return Some(MappedState::new("minecraft:rooted_dirt")),
        "azalea_leaves_flowered" => {
            return Some(MappedState::new("minecraft:flowering_azalea_leaves"));
        }
        "frog_spawn" => return Some(MappedState::new("minecraft:frogspawn")),
        "lit_pumpkin" => return Some(MappedState::new("minecraft:jack_o_lantern")),
        "portal" => return Some(MappedState::new("minecraft:nether_portal")),
        "air" => return Some(MappedState::new("minecraft:air")),
        "reserved6" | "info_update" | "info_update2" | "invisibleBedrock" | "allow" | "deny"
        | "border_block" | "camera" | "movingBlock" | "pistonArmCollision"
        | "stickyPistonArmCollision" | "glowingobsidian" | "netherreactor" => {
            return Some(MappedState::new("minecraft:air").lossy());
        }
        "waxed_copper" => return Some(MappedState::new("minecraft:waxed_copper_block")),
        "seaLantern" => return Some(MappedState::new("minecraft:sea_lantern")),
        "silver_glazed_terracotta" => {
            return Some(MappedState::new("minecraft:light_gray_glazed_terracotta"));
        }
        "wooden_button" => return Some(MappedState::new("minecraft:oak_button")),
        "wooden_pressure_plate" => return Some(MappedState::new("minecraft:oak_pressure_plate")),
        "wooden_door" => return Some(MappedState::new("minecraft:oak_door")),
        "trapdoor" => return Some(MappedState::new("minecraft:oak_trapdoor")),
        "fence_gate" => return Some(MappedState::new("minecraft:oak_fence_gate")),
        "standing_sign" => return Some(MappedState::new("minecraft:oak_sign")),
        "wall_sign" => return Some(MappedState::new("minecraft:oak_wall_sign")),
        _ => {}
    }
    if short.starts_with("light_block") {
        let level = short
            .strip_prefix("light_block_")
            .filter(|level| level.chars().all(|ch| ch.is_ascii_digit()))
            .unwrap_or("0");
        return Some(MappedState::new("minecraft:light").with("level", level));
    }
    if let Some(wood) = short.strip_suffix("_standing_sign") {
        return Some(MappedState::new(namespaced(&format!(
            "{}_sign",
            wood.replace("darkoak", "dark_oak")
        ))));
    }
    if let Some(wood) = short.strip_suffix("_wall_sign") {
        return Some(MappedState::new(namespaced(&format!(
            "{}_wall_sign",
            wood.replace("darkoak", "dark_oak")
        ))));
    }
    if let Some(wood) = short.strip_suffix("_hanging_sign") {
        return Some(MappedState::new(namespaced(&format!("{wood}_hanging_sign"))));
    }
    if short == "flower_pot" {
        return Some(MappedState::new("minecraft:flower_pot").lossy());
    }
    None
}

/// Java families whose Bedrock form needs explicit translation.
fn je_special(short: &str, properties: &BTreeMap<String, String>) -> Option<MappedState> {
    let get = |key: &str| properties.get(key).map(String::as_str);

    if short == "pale_moss_carpet" {
        // Java spells the sides plainly; Bedrock prefixes them with the family
        // and inverts the sense of `bottom` into `upper_block_bit`.
        let mut mapped = MappedState::new("minecraft:pale_moss_carpet");
        mapped = mapped.with("upper_block_bit", bit(get("bottom") != Some("true")));
        for key in ["north", "east", "south", "west"] {
            let value = match get(key).unwrap_or("none") {
                "low" => "short",
                "tall" => "tall",
                _ => "none",
            };
            mapped = mapped.with(format!("pale_moss_carpet_side_{key}"), value);
        }
        return Some(mapped);
    }

    // Doors: Bedrock keeps `upper_block_bit`/`door_hinge_bit`; oak uses `wooden_door`.
    // `cardinal_direction` is rotated one quarter-turn from Java facing, and both
    // the 1.21.60+ and legacy properties are emitted so the schema keeps the right one.
    if short.ends_with("_door") {
        let target = if short == "oak_door" { "wooden_door" } else { short };
        let mut mapped = MappedState::new(namespaced(target));
        if let Some(facing) = get("facing") {
            if let Some(cardinal) = door_cardinal_to_string(facing) {
                mapped = mapped.with("minecraft:cardinal_direction", cardinal);
            }
            if let Some(legacy) = door_direction_number(facing) {
                mapped = mapped.with("direction", legacy);
            }
        }
        mapped = mapped.with("upper_block_bit", bit(get("half") == Some("upper")));
        mapped = mapped.with("door_hinge_bit", bit(get("hinge") == Some("right")));
        mapped = mapped.with("open_bit", bit(get("open") == Some("true")));
        return Some(mapped.lossy());
    }

    // Trapdoors: Bedrock stores `direction` (the attached side) and no `powered`.
    if short.ends_with("_trapdoor") {
        let target = if short == "oak_trapdoor" { "trapdoor" } else { short };
        let mut mapped = MappedState::new(namespaced(target));
        if let Some(direction) = get("facing").and_then(weirdo_direction_number) {
            mapped = mapped.with("direction", direction);
        }
        mapped = mapped.with("upside_down_bit", bit(get("half") == Some("top")));
        mapped = mapped.with("open_bit", bit(get("open") == Some("true")));
        return Some(mapped.lossy());
    }

    // Fence gates.
    if short.ends_with("_fence_gate") {
        let target = if short == "oak_fence_gate" { "fence_gate" } else { short };
        let mut mapped = MappedState::new(namespaced(target));
        if let Some(facing) = get("facing") {
            mapped = mapped.with("minecraft:cardinal_direction", facing);
            if let Some(legacy) = cardinal_legacy_to_number(facing) {
                mapped = mapped.with("direction", legacy);
            }
        }
        mapped = mapped.with("in_wall_bit", bit(get("in_wall") == Some("true")));
        mapped = mapped.with("open_bit", bit(get("open") == Some("true")));
        return Some(mapped.lossy());
    }

    // Buttons: Bedrock has one six-valued facing instead of face + facing.
    if short.ends_with("_button") {
        let target = if short == "oak_button" { "wooden_button" } else { short };
        let mut mapped = MappedState::new(namespaced(target));
        if let Some(facing_direction) = button_facing_direction(get("face"), get("facing")) {
            mapped = mapped.with("facing_direction", facing_direction);
        }
        mapped = mapped.with("button_pressed_bit", bit(get("powered") == Some("true")));
        return Some(mapped);
    }

    // Pressure plates carry a signal strength in Bedrock.
    if short.ends_with("_pressure_plate") {
        let target = if short == "oak_pressure_plate" { "wooden_pressure_plate" } else { short };
        let powered = get("powered") == Some("true");
        return Some(
            MappedState::new(namespaced(target))
                .with("redstone_signal", if powered { "15" } else { "0" }),
        );
    }

    // Double slabs are dedicated Bedrock blocks.
    if short.ends_with("_slab") && get("type") == Some("double") {
        let base = short.strip_suffix("_slab")?;
        return Some(MappedState::new(namespaced(&format!("{base}_double_slab"))));
    }

    // Pistons and end rods mirror the horizontal facing in Bedrock.
    if matches!(short, "piston" | "sticky_piston" | "piston_head" | "moving_piston") {
        let target = match short {
            "piston_head" => "piston_arm_collision",
            "moving_piston" => "moving_block",
            other => other,
        };
        let mut mapped = MappedState::new(namespaced(target));
        if let Some(direction) = get("facing").and_then(flipped_facing_to_number) {
            mapped = mapped.with("facing_direction", direction);
        }
        return Some(mapped.lossy());
    }
    if short == "end_rod" {
        let mut mapped = MappedState::new("minecraft:end_rod");
        if let Some(direction) = get("facing").and_then(flipped_facing_to_number) {
            mapped = mapped.with("facing_direction", direction);
        }
        return Some(mapped);
    }
    if short == "hopper" {
        let mut mapped = MappedState::new("minecraft:hopper");
        if let Some(direction) = get("facing").and_then(hopper_facing_number) {
            mapped = mapped.with("facing_direction", direction);
        }
        mapped = mapped.with("toggle_bit", bit(get("enabled") != Some("false")));
        return Some(mapped.lossy());
    }
    if short == "fire" || short == "soul_fire" {
        let mut mapped = MappedState::new(namespaced(short));
        mapped = mapped.with("age", get("age").unwrap_or("0"));
        for key in ["east", "north", "south", "up", "west"] {
            mapped = mapped.with(
                format!("minecraft:connection_{key}"),
                bit(get(key) == Some("true")),
            );
            mapped = mapped.with(key, bit(get(key) == Some("true")));
        }
        return Some(mapped.lossy());
    }
    if short == "lever" {
        let mut mapped = MappedState::new("minecraft:lever");
        if let Some(direction) = lever_direction(
            get("face").unwrap_or("wall"),
            get("facing").unwrap_or("north"),
        ) {
            mapped = mapped.with("lever_direction", direction);
        }
        mapped = mapped.with("open_bit", bit(get("powered") == Some("true")));
        return Some(mapped.lossy());
    }
    if short == "pale_moss_carpet" {
        // Bedrock spells the sides with a family prefix and hangs the carpet off
        // `upper_block_bit`, whose sense is inverted against Java's `bottom`.
        let mut mapped = MappedState::new("minecraft:pale_moss_carpet");
        mapped = mapped.with("bottom", bool_str(get("upper_block_bit") != Some("1")));
        for key in ["north", "east", "south", "west"] {
            let value = get(&format!("pale_moss_carpet_side_{key}")).unwrap_or("none");
            mapped = mapped.with(
                key,
                match value {
                    "short" => "low",
                    "tall" => "tall",
                    _ => "none",
                },
            );
        }
        return Some(mapped);
    }
    if short == "decorated_pot" {
        let mut mapped = MappedState::new("minecraft:decorated_pot");
        if let Some(direction) = get("facing").and_then(pot_direction_number) {
            mapped = mapped.with("direction", direction);
        }
        mapped = mapped.with("cracked", bit(get("cracked") == Some("true")));
        return Some(mapped);
    }

    // Light blocks encode the level in the identifier.
    if short == "light" {
        let level = get("level").unwrap_or("15");
        return Some(MappedState::new(namespaced(&format!("light_block_{level}"))));
    }

    // Heads and skulls: wall variants differ only by facing.
    if short.ends_with("_wall_head") || short.ends_with("_wall_skull") {
        let base = short.replace("_wall_head", "_head").replace("_wall_skull", "_skull");
        let mut mapped = MappedState::new(namespaced(&base));
        if let Some(facing_direction) = get("facing").and_then(facing_to_bedrock_number) {
            mapped = mapped.with("facing_direction", facing_direction);
        }
        return Some(mapped.lossy());
    }
    if (short.ends_with("_head") || short.ends_with("_skull")) && short != "piston_head" {
        let mut mapped = MappedState::new(namespaced(short));
        mapped = mapped.with("facing_direction", "1");
        return Some(mapped.lossy());
    }

    // Tripwire stores its four connection bits; Bedrock derives suspension.
    if short == "tripwire" {
        let mut mapped = MappedState::new("minecraft:trip_wire");
        mapped = mapped.with("attached_bit", bit(get("attached") == Some("true")));
        mapped = mapped.with("disarmed_bit", bit(get("disarmed") == Some("true")));
        mapped = mapped.with("powered_bit", bit(get("powered") == Some("true")));
        mapped = mapped.with("suspended_bit", "0");
        return Some(mapped.lossy());
    }

    // Torch blocks that only differ by material.
    if short == "copper_torch" {
        return Some(
            MappedState::new("minecraft:copper_torch").with("torch_facing_direction", "unknown"),
        );
    }
    if short == "copper_wall_torch" {
        return Some(
            MappedState::new("minecraft:copper_torch")
                .with("torch_facing_direction", get("facing")?),
        );
    }

    // Attached stems are fully grown and face the fruit.
    if short == "attached_melon_stem" || short == "attached_pumpkin_stem" {
        let target = if short.contains("melon") {
            "minecraft:melon_stem"
        } else {
            "minecraft:pumpkin_stem"
        };
        let mut mapped = MappedState::new(target);
        mapped = mapped.with("growth", "7");
        if let Some(facing_direction) = get("facing").and_then(facing_to_bedrock_number) {
            mapped = mapped.with("facing_direction", facing_direction);
        }
        return Some(mapped.lossy());
    }

    // The dripleaf stem is the head-less half of `big_dripleaf`.
    if short == "big_dripleaf_stem" {
        let mut mapped = MappedState::new("minecraft:big_dripleaf");
        mapped = mapped.with("big_dripleaf_head", "0");
        if let Some(facing) = get("facing") {
            mapped = mapped.with("minecraft:cardinal_direction", facing);
        }
        return Some(mapped.lossy());
    }

    // Beds: Bedrock has one `bed` block; the dye colour lives in block-entity data.
    if short.ends_with("_bed") {
        let mut mapped = MappedState::new("minecraft:bed");
        if let Some(direction) = get("facing").and_then(cardinal_direction_number) {
            mapped = mapped.with("direction", direction);
        }
        mapped = mapped.with("head_piece_bit", bit(get("part") == Some("head")));
        mapped = mapped.with("occupied_bit", bit(get("occupied") == Some("true")));
        return Some(mapped.lossy());
    }

    // Banners: same story — one Bedrock block, colour in block-entity data.
    if short.ends_with("_banner") {
        let mut mapped = MappedState::new("minecraft:standing_banner");
        mapped = mapped.with("ground_sign_direction", get("rotation").unwrap_or("0"));
        return Some(mapped.lossy());
    }

    if short.ends_with("_wall_banner") {
        let mut mapped = MappedState::new("minecraft:wall_banner");
        if let Some(facing) = get("facing").and_then(cardinal_direction_number) {
            mapped = mapped.with("direction", facing);
        }
        return Some(mapped.lossy());
    }

    // Stairs: `weirdo_direction` is the side the full block sits on, exactly like
    // Java's `facing`; Bedrock has no shape state.
    if short.ends_with("_stairs") {
        let mut mapped = MappedState::new(namespaced(short));
        if let Some(direction) = get("facing").and_then(weirdo_direction_number) {
            mapped = mapped.with("weirdo_direction", direction);
        }
        if let Some(half) = get("half") {
            mapped = mapped.with("upside_down_bit", bit(half == "top"));
        }
        if let Some(shape) = get("shape") {
            // 1.26.50+ stores the shape; older targets drop it in `fit_to_schema`.
            mapped = mapped.with("minecraft:corner", corner_from_shape(shape));
        }
        return Some(mapped.lossy());
    }

    match short {
        "torch" => Some(MappedState::new("minecraft:torch").with("torch_facing_direction", "unknown")),
        "wall_torch" => Some(
            MappedState::new("minecraft:torch")
                .with("torch_facing_direction", get("facing")?),
        ),
        "soul_torch" => Some(
            MappedState::new("minecraft:soul_torch").with("torch_facing_direction", "unknown"),
        ),
        "soul_wall_torch" => Some(
            MappedState::new("minecraft:soul_torch").with("torch_facing_direction", get("facing")?),
        ),
        "redstone_torch" => Some(
            MappedState::new("minecraft:redstone_torch")
                .with("torch_facing_direction", "unknown"),
        ),
        "redstone_wall_torch" => Some(
            MappedState::new("minecraft:redstone_torch")
                .with("torch_facing_direction", get("facing")?),
        ),
        "redstone_wire" => {
            let mut mapped = MappedState::new("minecraft:redstone_wire");
            mapped = mapped.with("redstone_signal", get("power").unwrap_or("0"));
            Some(mapped.lossy())
        }
        "repeater" => {
            let powered = get("powered") == Some("true");
            let mut mapped = MappedState::new(namespaced(if powered {
                "powered_repeater"
            } else {
                "unpowered_repeater"
            }));
            if let Some(facing) = get("facing") {
                mapped = mapped.with("minecraft:cardinal_direction", facing);
                if let Some(legacy) = cardinal_legacy_to_number(facing) {
                    mapped = mapped.with("direction", legacy);
                }
            }
            let delay = get("delay")
                .and_then(|delay| delay.parse::<i32>().ok())
                .unwrap_or(1);
            mapped = mapped.with("repeater_delay", (delay - 1).to_string());
            Some(mapped.lossy())
        }
        "comparator" => {
            let powered = get("powered") == Some("true");
            let mut mapped = MappedState::new(namespaced(if powered {
                "powered_comparator"
            } else {
                "unpowered_comparator"
            }));
            if let Some(facing) = get("facing") {
                mapped = mapped.with("minecraft:cardinal_direction", facing);
                if let Some(legacy) = cardinal_legacy_to_number(facing) {
                    mapped = mapped.with("direction", legacy);
                }
            }
            mapped = mapped.with(
                "output_subtract_bit",
                bit(get("mode") == Some("subtract")),
            );
            Some(mapped.lossy())
        }
        "daylight_detector" => {
            let inverted = get("inverted") == Some("true");
            let mut mapped = MappedState::new(namespaced(if inverted {
                "daylight_detector_inverted"
            } else {
                "daylight_detector"
            }));
            mapped = mapped.with("redstone_signal", get("power").unwrap_or("0"));
            Some(mapped)
        }
        "cauldron" => Some(MappedState::new("minecraft:cauldron")),
        "water_cauldron" => {
            let level = get("level")
                .and_then(|level| level.parse::<i32>().ok())
                .unwrap_or(1);
            Some(
                MappedState::new("minecraft:cauldron")
                    .with("cauldron_liquid", "water")
                    .with("fill_level", (level * 2).to_string()),
            )
        }
        "lava_cauldron" => Some(
            MappedState::new("minecraft:cauldron")
                .with("cauldron_liquid", "lava")
                .with("fill_level", "6"),
        ),
        "powder_snow_cauldron" => Some(
            MappedState::new("minecraft:cauldron").with("cauldron_liquid", "powder_snow"),
        ),
        "water" => Some(
            MappedState::new("minecraft:water").with("liquid_depth", get("level").unwrap_or("0")),
        ),
        "lava" => Some(
            MappedState::new("minecraft:lava").with("liquid_depth", get("level").unwrap_or("0")),
        ),
        "snow" => {
            let layers = get("layers")
                .and_then(|layers| layers.parse::<i32>().ok())
                .unwrap_or(1);
            Some(MappedState::new("minecraft:snow_layer").with("height", (layers - 1).to_string()))
        }
        "composter" => Some(
            MappedState::new("minecraft:composter")
                .with("composter_fill_level", get("level").unwrap_or("0")),
        ),
        "wheat" | "carrots" | "potatoes" | "beetroots" => {
            let target = match short {
                "wheat" => "minecraft:wheat",
                "carrots" => "minecraft:carrots",
                "potatoes" => "minecraft:potatoes",
                _ => "minecraft:beetroot",
            };
            Some(MappedState::new(target).with("growth", get("age").unwrap_or("0")))
        }
        "cocoa" => {
            let mut mapped = MappedState::new("minecraft:cocoa");
            mapped = mapped.with("age", get("age").unwrap_or("0"));
            if let Some(facing) = get("facing").and_then(cardinal_direction_number) {
                mapped = mapped.with("direction", facing);
            }
            Some(mapped)
        }
        "vine" | "glow_lichen" | "sculk_vein" => {
            let mut bits = 0_i32;
            for (bit_value, key) in [(1, "south"), (2, "west"), (4, "north"), (8, "east")] {
                if get(key) == Some("true") {
                    bits |= bit_value;
                }
            }
            let target = match short {
                "vine" => "minecraft:vine",
                "glow_lichen" => "minecraft:glow_lichen",
                _ => "minecraft:sculk_vein",
            };
            Some(MappedState::new(target).with("multi_face_direction_bits", bits.to_string()))
        }
        "big_dripleaf" => {
            let mut mapped = MappedState::new("minecraft:big_dripleaf");
            mapped = mapped.with(
                "big_dripleaf_tilt",
                if get("tilt") == Some("full") {
                    "full_tilt"
                } else {
                    "none"
                },
            );
            mapped = mapped.with(
                "big_dripleaf_head",
                bit(get("half") != Some("upper")),
            );
            if let Some(facing) = get("facing") {
                mapped = mapped.with("minecraft:cardinal_direction", facing);
            }
            Some(mapped.lossy())
        }
        "small_dripleaf" => {
            let mut mapped = MappedState::new("minecraft:small_dripleaf_block");
            if let Some(facing) = get("facing") {
                mapped = mapped.with("minecraft:cardinal_direction", facing);
            }
            mapped = mapped.with("upper_block_bit", bit(get("half") == Some("upper")));
            Some(mapped)
        }
        "bamboo" => {
            let mut mapped = MappedState::new("minecraft:bamboo");
            mapped = mapped.with("age", get("age").unwrap_or("0"));
            Some(mapped.lossy())
        }
        "sea_pickle" => {
            let mut mapped = MappedState::new("minecraft:sea_pickle");
            mapped = mapped.with("cluster_count", get("pickles").unwrap_or("1"));
            Some(mapped)
        }
        "candle" | "candle_cake" => {
            let mut mapped = MappedState::new(namespaced(short));
            for key in ["candles", "lit"] {
                if let Some(value) = get(key) {
                    mapped = mapped.with(key, value);
                }
            }
            Some(mapped)
        }
        "note_block" => {
            let mut mapped = MappedState::new("minecraft:noteblock");
            mapped = mapped.with("note", get("note").unwrap_or("0"));
            mapped = mapped.with("powered_bit", bit(get("powered") == Some("true")));
            Some(mapped.lossy())
        }
        "tall_seagrass" => {
            let mut mapped = MappedState::new("minecraft:seagrass");
            let kind = if get("half") == Some("upper") {
                "double_top"
            } else {
                "double_bot"
            };
            mapped = mapped.with("sea_grass_type", kind);
            Some(mapped)
        }
        "kelp_plant" => Some(MappedState::new("minecraft:kelp")),
        "cave_vines_plant" => Some(MappedState::new("minecraft:cave_vines")),
        "twisting_vines_plant" => Some(MappedState::new("minecraft:twisting_vines")),
        "weeping_vines_plant" => Some(MappedState::new("minecraft:weeping_vines")),
        _ => None,
    }
}

/// Bedrock families whose Java form needs explicit translation.
fn be_special(short: &str, properties: &BTreeMap<String, String>) -> Option<MappedState> {
    let get = |key: &str| properties.get(key).map(String::as_str);

    // Legacy ids from worlds before the flattened block names.
    if let Some(mapped) = legacy::legacy_be_family(short, properties) {
        return Some(mapped);
    }

    // Bedrock keeps double slabs as their own blocks.
    if let Some(base) = short.strip_suffix("_double_slab") {
        let mut mapped = MappedState::new(namespaced(&format!("{base}_slab")));
        mapped = mapped.with("type", "double");
        return Some(mapped);
    }

    // Stairs: corner state (1.26.50+) and weirdo_direction.
    if short.ends_with("_stairs") {
        let mut mapped = MappedState::new(namespaced(short));
        if let Some(facing) = get("weirdo_direction").and_then(weirdo_direction_from_number) {
            mapped = mapped.with("facing", facing);
        }
        mapped = mapped.with(
            "half",
            if get("upside_down_bit") == Some("1") { "top" } else { "bottom" },
        );
        mapped = mapped.with("shape", shape_from_corner(get("minecraft:corner").unwrap_or("none")));
        mapped = mapped.with("waterlogged", bool_str(get("waterlogged") == Some("1")));
        return Some(mapped.lossy());
    }

    // Pistons and end rods mirror the horizontal facing on the Bedrock side.
    if matches!(short, "piston" | "sticky_piston" | "piston_arm_collision" | "moving_block") {
        let mut mapped = MappedState::new(namespaced(short));
        if let Some(facing) = get("facing_direction").and_then(flipped_facing_from_number) {
            mapped = mapped.with("facing", facing);
        }
        mapped = mapped.with("extended", "false");
        return Some(mapped.lossy());
    }
    if short == "end_rod" {
        let mut mapped = MappedState::new("minecraft:end_rod");
        if let Some(facing) = get("facing_direction").and_then(flipped_facing_from_number) {
            mapped = mapped.with("facing", facing);
        }
        return Some(mapped);
    }
    if short == "hopper" {
        let mut mapped = MappedState::new("minecraft:hopper");
        let facing = match get("facing_direction") {
            Some("0" | "1") | None => "down",
            Some("2") => "north",
            Some("3") => "south",
            Some("4") => "west",
            Some("5") => "east",
            Some(_) => "down",
        };
        mapped = mapped.with("facing", facing);
        mapped = mapped.with("enabled", bool_str(get("toggle_bit") != Some("1")));
        return Some(mapped.lossy());
    }
    if short == "fire" || short == "soul_fire" {
        let mut mapped = MappedState::new(namespaced(short));
        mapped = mapped.with("age", get("age").unwrap_or("0"));
        for key in ["east", "north", "south", "up", "west"] {
            let namespaced_key = format!("minecraft:connection_{key}");
            let value = get(&namespaced_key).or_else(|| get(key)).unwrap_or("0");
            mapped = mapped.with(key, number_to_bool(value));
        }
        return Some(mapped.lossy());
    }
    if short == "lever" {
        let mut mapped = MappedState::new("minecraft:lever");
        if let Some((face, facing)) = get("lever_direction").and_then(lever_from_direction) {
            mapped = mapped.with("face", face);
            mapped = mapped.with("facing", facing);
        }
        mapped = mapped.with("powered", number_to_bool(get("open_bit").unwrap_or("0")));
        return Some(mapped.lossy());
    }
    if short == "decorated_pot" {
        let mut mapped = MappedState::new("minecraft:decorated_pot");
        if let Some(facing) = get("direction").and_then(pot_direction_from_number) {
            mapped = mapped.with("facing", facing);
        }
        mapped = mapped.with("cracked", number_to_bool(get("cracked").unwrap_or("0")));
        return Some(mapped);
    }

    match short {
        "trip_wire" => {
            let mut mapped = MappedState::new("minecraft:tripwire");
            mapped = mapped.with("attached", number_to_bool(get("attached_bit").unwrap_or("0")));
            mapped = mapped.with("disarmed", number_to_bool(get("disarmed_bit").unwrap_or("0")));
            mapped = mapped.with("powered", number_to_bool(get("powered_bit").unwrap_or("0")));
            mapped = mapped.with("north", "false").with("east", "false");
            mapped = mapped.with("south", "false").with("west", "false");
            Some(mapped.lossy())
        }
        "torch" => {
            let facing = get("torch_facing_direction").unwrap_or("unknown");
            if facing == "unknown" {
                Some(MappedState::new("minecraft:torch"))
            } else {
                Some(MappedState::new("minecraft:wall_torch").with("facing", facing))
            }
        }
        "soul_torch" => {
            let facing = get("torch_facing_direction").unwrap_or("unknown");
            if facing == "unknown" {
                Some(MappedState::new("minecraft:soul_torch"))
            } else {
                Some(MappedState::new("minecraft:soul_wall_torch").with("facing", facing))
            }
        }
        "redstone_torch" | "unlit_redstone_torch" => {
            let lit = short == "redstone_torch";
            let facing = get("torch_facing_direction").unwrap_or("unknown");
            let mut mapped = MappedState::new(if facing == "unknown" {
                "minecraft:redstone_torch"
            } else {
                "minecraft:redstone_wall_torch"
            });
            if facing != "unknown" {
                mapped = mapped.with("facing", facing);
            }
            mapped = mapped.with("lit", bool_str(lit));
            Some(mapped)
        }
        "redstone_wire" => {
            let mut mapped = MappedState::new("minecraft:redstone_wire");
            mapped = mapped.with("power", get("redstone_signal").unwrap_or("0"));
            for key in ["north", "east", "south", "west"] {
                mapped = mapped.with(key, "none");
            }
            Some(mapped.lossy())
        }
        "unpowered_repeater" | "powered_repeater" => {
            let mut mapped = MappedState::new("minecraft:repeater");
            let facing = if let Some(cardinal) = get("minecraft:cardinal_direction") {
                cardinal
            } else {
                get("direction").and_then(cardinal_legacy_from_number)?
            };
            mapped = mapped.with("facing", facing);
            let delay = get("repeater_delay")
                .and_then(|delay| delay.parse::<i32>().ok())
                .unwrap_or(0);
            mapped = mapped.with("delay", (delay + 1).to_string());
            mapped = mapped.with("powered", bool_str(short == "powered_repeater"));
            mapped = mapped.with("locked", "false");
            Some(mapped.lossy())
        }
        "unpowered_comparator" | "powered_comparator" => {
            let mut mapped = MappedState::new("minecraft:comparator");
            let facing = if let Some(cardinal) = get("minecraft:cardinal_direction") {
                cardinal
            } else {
                get("direction").and_then(cardinal_legacy_from_number)?
            };
            mapped = mapped.with("facing", facing);
            mapped = mapped.with(
                "mode",
                if get("output_subtract_bit") == Some("1") {
                    "subtract"
                } else {
                    "compare"
                },
            );
            mapped = mapped.with("powered", bool_str(short == "powered_comparator"));
            Some(mapped.lossy())
        }
        "daylight_detector" | "daylight_detector_inverted" => {
            let mut mapped = MappedState::new("minecraft:daylight_detector");
            mapped = mapped.with("power", get("redstone_signal").unwrap_or("0"));
            mapped = mapped.with("inverted", bool_str(short == "daylight_detector_inverted"));
            Some(mapped)
        }
        "cauldron" => {
            let liquid = get("cauldron_liquid").unwrap_or("water");
            let fill = get("fill_level")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            let target = match liquid {
                "lava" => "minecraft:lava_cauldron",
                "powder_snow" => "minecraft:powder_snow_cauldron",
                _ if fill == 0 => "minecraft:cauldron",
                _ => "minecraft:water_cauldron",
            };
            let mut mapped = MappedState::new(target);
            if target == "minecraft:water_cauldron" {
                mapped = mapped.with("level", (fill / 2).clamp(1, 3).to_string());
            }
            Some(mapped)
        }
        "water" | "flowing_water" => Some(
            MappedState::new("minecraft:water").with("level", get("liquid_depth").unwrap_or("0")),
        ),
        "lava" | "flowing_lava" => Some(
            MappedState::new("minecraft:lava").with("level", get("liquid_depth").unwrap_or("0")),
        ),
        "snow_layer" => {
            let height = get("height")
                .and_then(|height| height.parse::<i32>().ok())
                .unwrap_or(0);
            Some(MappedState::new("minecraft:snow").with("layers", (height + 1).to_string()))
        }
        "composter" => Some(
            MappedState::new("minecraft:composter")
                .with("level", get("composter_fill_level").unwrap_or("0")),
        ),
        "wheat" | "carrots" | "potatoes" | "beetroot" => {
            let target = match short {
                "wheat" => "minecraft:wheat",
                "carrots" => "minecraft:carrots",
                "potatoes" => "minecraft:potatoes",
                _ => "minecraft:beetroots",
            };
            Some(MappedState::new(target).with("age", get("growth").unwrap_or("0")))
        }
        "cocoa" => {
            let mut mapped = MappedState::new("minecraft:cocoa");
            mapped = mapped.with("age", get("age").unwrap_or("0"));
            if let Some(facing) = get("direction").and_then(cardinal_from_number) {
                mapped = mapped.with("facing", facing);
            }
            Some(mapped)
        }
        "vine" | "glow_lichen" | "sculk_vein" => {
            let bits = get("multi_face_direction_bits")
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(0);
            let target = match short {
                "vine" => "minecraft:vine",
                "glow_lichen" => "minecraft:glow_lichen",
                _ => "minecraft:sculk_vein",
            };
            let mut mapped = MappedState::new(target);
            for (bit_value, key) in [(1, "south"), (2, "west"), (4, "north"), (8, "east")] {
                mapped = mapped.with(key, bool_str(bits & bit_value != 0));
            }
            mapped = mapped.with("up", "false").with("down", "false");
            Some(mapped.lossy())
        }
        "big_dripleaf" => {
            let mut mapped = MappedState::new("minecraft:big_dripleaf");
            mapped = mapped.with(
                "tilt",
                match get("big_dripleaf_tilt") {
                    Some("full_tilt") => "full",
                    Some("partial_tilt") => "partial",
                    Some("unstable") => "unstable",
                    _ => "none",
                },
            );
            mapped = mapped.with(
                "half",
                if get("big_dripleaf_head") == Some("1") { "lower" } else { "upper" },
            );
            if let Some(facing) = get("minecraft:cardinal_direction") {
                mapped = mapped.with("facing", facing);
            }
            Some(mapped.lossy())
        }
        "small_dripleaf_block" => {
            let mut mapped = MappedState::new("minecraft:small_dripleaf");
            if let Some(facing) = get("minecraft:cardinal_direction") {
                mapped = mapped.with("facing", facing);
            }
            mapped = mapped.with(
                "half",
                if get("upper_block_bit") == Some("1") { "upper" } else { "lower" },
            );
            Some(mapped)
        }
        "sea_pickle" => Some(
            MappedState::new("minecraft:sea_pickle")
                .with("pickles", get("cluster_count").unwrap_or("1")),
        ),
        "noteblock" => {
            let mut mapped = MappedState::new("minecraft:note_block");
            mapped = mapped.with("note", get("note").unwrap_or("0"));
            mapped = mapped.with("powered", bool_str(get("powered_bit") == Some("1")));
            mapped = mapped.with("instrument", "harp");
            Some(mapped.lossy())
        }
        "bed" => Some(MappedState::new(namespaced(&format!(
            "{}_bed",
            get("color").unwrap_or("red")
        )))),
        "standing_banner" => Some(MappedState::new(namespaced(&format!(
            "{}_banner",
            get("color").unwrap_or("white")
        )))),
        "wall_banner" => Some(MappedState::new(namespaced(&format!(
            "{}_wall_banner",
            get("color").unwrap_or("white")
        )))),
        "wooden_button" | "acacia_button" | "birch_button" | "jungle_button" | "spruce_button"
        | "dark_oak_button" | "mangrove_button" | "cherry_button" | "bamboo_button"
        | "crimson_button" | "warped_button" | "pale_oak_button" => {
            let mut mapped = MappedState::new(namespaced(short));
            button_to_java(&mut mapped, get("facing_direction"), get("button_pressed_bit"));
            Some(mapped)
        }
        "wooden_pressure_plate" | "acacia_pressure_plate" | "birch_pressure_plate"
        | "jungle_pressure_plate" | "spruce_pressure_plate" | "dark_oak_pressure_plate"
        | "mangrove_pressure_plate" | "cherry_pressure_plate" | "bamboo_pressure_plate"
        | "crimson_pressure_plate" | "warped_pressure_plate" | "pale_oak_pressure_plate"
        | "light_weighted_pressure_plate" | "heavy_weighted_pressure_plate" => {
            let mut mapped = MappedState::new(namespaced(short));
            mapped = mapped.with(
                "powered",
                bool_str(get("redstone_signal").is_some_and(|value| value != "0")),
            );
            Some(mapped)
        }
        "wooden_door" | "acacia_door" | "birch_door" | "jungle_door" | "spruce_door"
        | "dark_oak_door" | "mangrove_door" | "cherry_door" | "bamboo_door" | "crimson_door"
        | "warped_door" | "pale_oak_door" | "iron_door" | "copper_door"
        | "exposed_copper_door" | "weathered_copper_door" | "oxidized_copper_door"
        | "waxed_copper_door" | "waxed_exposed_copper_door" | "waxed_weathered_copper_door"
        | "waxed_oxidized_copper_door" => {
            let mut mapped = MappedState::new(namespaced(short));
            let facing = if let Some(cardinal) = get("minecraft:cardinal_direction") {
                door_cardinal_from_string(cardinal)
            } else {
                get("direction").and_then(door_direction_from_number)
            };
            if let Some(facing) = facing {
                mapped = mapped.with("facing", facing);
            }
            mapped = mapped.with(
                "half",
                if get("upper_block_bit") == Some("1") { "upper" } else { "lower" },
            );
            mapped = mapped.with(
                "hinge",
                if get("door_hinge_bit") == Some("1") { "right" } else { "left" },
            );
            mapped = mapped.with("open", bool_str(get("open_bit") == Some("1")));
            mapped = mapped.with("powered", "false");
            Some(mapped.lossy())
        }
        "trapdoor" | "acacia_trapdoor" | "birch_trapdoor" | "jungle_trapdoor"
        | "spruce_trapdoor" | "dark_oak_trapdoor" | "mangrove_trapdoor" | "cherry_trapdoor"
        | "bamboo_trapdoor" | "crimson_trapdoor" | "warped_trapdoor" | "pale_oak_trapdoor"
        | "iron_trapdoor" | "copper_trapdoor" | "exposed_copper_trapdoor"
        | "weathered_copper_trapdoor" | "oxidized_copper_trapdoor" | "waxed_copper_trapdoor"
        | "waxed_exposed_copper_trapdoor" | "waxed_weathered_copper_trapdoor"
        | "waxed_oxidized_copper_trapdoor" => {
            let mut mapped = MappedState::new(namespaced(short));
            if let Some(facing) = get("direction").and_then(weirdo_direction_from_number) {
                mapped = mapped.with("facing", facing);
            }
            mapped = mapped.with(
                "half",
                if get("upside_down_bit") == Some("1") { "top" } else { "bottom" },
            );
            mapped = mapped.with("open", bool_str(get("open_bit") == Some("1")));
            mapped = mapped.with("powered", "false");
            Some(mapped.lossy())
        }
        "fence_gate" | "acacia_fence_gate" | "birch_fence_gate" | "jungle_fence_gate"
        | "spruce_fence_gate" | "dark_oak_fence_gate" | "mangrove_fence_gate"
        | "cherry_fence_gate" | "bamboo_fence_gate" | "crimson_fence_gate"
        | "warped_fence_gate" | "pale_oak_fence_gate" => {
            let mut mapped = MappedState::new(namespaced(short));
            let facing = if let Some(cardinal) = get("minecraft:cardinal_direction") {
                Some(cardinal)
            } else {
                get("direction").and_then(cardinal_legacy_from_number)
            };
            if let Some(facing) = facing {
                mapped = mapped.with("facing", facing);
            }
            mapped = mapped.with("in_wall", bool_str(get("in_wall_bit") == Some("1")));
            mapped = mapped.with("open", bool_str(get("open_bit") == Some("1")));
            mapped = mapped.with("powered", "false");
            Some(mapped.lossy())
        }
        _ => None,
    }
}

fn button_facing_direction(face: Option<&str>, facing: Option<&str>) -> Option<&'static str> {
    let facing = facing.unwrap_or("north");
    match face.unwrap_or("wall") {
        "floor" => Some("1"),
        "ceiling" => Some("0"),
        _ => match facing {
            "north" => Some("2"),
            "south" => Some("3"),
            "west" => Some("4"),
            "east" => Some("5"),
            _ => None,
        },
    }
}

fn button_to_java(
    mapped: &mut MappedState,
    facing_direction: Option<&str>,
    pressed: Option<&str>,
) {
    let value = facing_direction.and_then(|value| value.parse::<i32>().ok()).unwrap_or(0);
    let (face, facing) = match value {
        0 => ("ceiling", "north"),
        1 => ("floor", "north"),
        2 => ("wall", "north"),
        3 => ("wall", "south"),
        4 => ("wall", "west"),
        _ => ("wall", "east"),
    };
    mapped.properties.insert("face".to_string(), face.to_string());
    mapped.properties.insert("facing".to_string(), facing.to_string());
    mapped
        .properties
        .insert("powered".to_string(), bool_str(pressed == Some("1")).to_string());
}

/// Copies properties across, renaming each to whatever the target declares.
fn translate_properties(
    mapped: &mut MappedState,
    source: &BTreeMap<String, String>,
    target: &BlockCatalog,
    direction: Direction,
) {
    let declared: Vec<&str> = target
        .block(&mapped.name)
        .map(|schema| schema.properties().keys().map(String::as_str).collect())
        .unwrap_or_default();
    let has = |candidate: &str| declared.iter().any(|entry| *entry == candidate);

    // A double slab is a separate block in Bedrock, so Java's `type=double` has
    // nothing to translate into: `vertical_half` only holds `bottom`/`top`.
    let double_slab = mapped.name.ends_with("_double_slab");

    for (source_key, value) in source {
        let key = short_property(source_key);
        if double_slab && key == "type" {
            continue;
        }
        match translate_one(key, value, direction, &has) {
            Translation::Property(target_key, translated) => {
                // A special rule already produced this target property.
                if !mapped.properties.contains_key(&target_key) {
                    mapped.properties.insert(target_key, translated);
                }
            }
            // Consumed by a caller that writes another layer (waterlogged) or by a
            // special rule that already encoded this information.
            Translation::Consumed => {}
            Translation::Dropped => mapped.lossy = true,
        }
    }
}

enum Translation {
    Property(String, String),
    Consumed,
    Dropped,
}

fn short_property(property: &str) -> &str {
    property.strip_prefix("minecraft:").unwrap_or(property)
}

fn translate_one(key: &str, value: &str, direction: Direction, has: &dyn Fn(&str) -> bool) -> Translation {
    let pick = |candidates: &[&str]| -> Option<String> {
        candidates
            .iter()
            .find(|candidate| has(candidate))
            .map(|candidate| (*candidate).to_string())
    };
    let prop = |target: String, value: String| Translation::Property(target, value);
    match direction {
        Direction::JavaToBedrock => match key {
            // Bedrock expresses waterlogging as a second block layer.
            "waterlogged" => Translation::Consumed,
            "axis" => pick(&["pillar_axis", "axis"]).map_or(Translation::Dropped, |target| {
                prop(target, value.to_string())
            }),
            "facing" => {
                if let Some(target) = pick(&[
                    "minecraft:facing_direction",
                    "facing_direction",
                    "minecraft:cardinal_direction",
                    "cardinal_direction",
                ]) {
                    let numeric = target.ends_with("facing_direction");
                    let translated = if numeric {
                        facing_to_bedrock_number(value).unwrap_or("0").to_string()
                    } else {
                        value.to_string()
                    };
                    return prop(target, translated);
                }
                pick(&["facing"]).map_or(Translation::Dropped, |target| prop(target, value.to_string()))
            }
            "half" => pick(&[
                "minecraft:vertical_half",
                "upside_down_bit",
                "top_slot_bit",
                "upper_block_bit",
            ])
            .map_or(Translation::Dropped, |target| {
                prop(target.clone(), half_to_target(&target, value))
            }),
            "type" => pick(&["minecraft:vertical_half", "top_slot_bit"]).map_or(
                Translation::Dropped,
                |target| prop(target.clone(), half_to_target(&target, value)),
            ),
            "rotation" => pick(&["ground_sign_direction"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "open" => pick(&["open_bit"]).map_or(Translation::Dropped, |target| {
                prop(target, bool_to_number(value))
            }),
            "powered" => pick(&["powered_bit", "button_pressed_bit", "triggered_bit", "rail_data_bit"])
                .map_or(Translation::Dropped, |target| prop(target, bool_to_number(value))),
            "lit" => pick(&["lit"]).map_or(Translation::Dropped, |target| {
                prop(target, bool_to_number(value))
            }),
            "persistent" => pick(&["persistent_bit"]).map_or(Translation::Dropped, |target| {
                prop(target, bool_to_number(value))
            }),
            "power" => pick(&["redstone_signal"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "age" => pick(&["age", "growth"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "level" => pick(&["liquid_depth", "composter_fill_level", "moisture"]).map_or(
                Translation::Dropped,
                |target| prop(target, value.to_string()),
            ),
            "delay" => pick(&["repeater_delay"]).map_or(Translation::Dropped, |target| {
                let delay = value.parse::<i32>().unwrap_or(1);
                prop(target, (delay - 1).to_string())
            }),
            "moisture" => pick(&["moisturized_amount"]).map_or(Translation::Dropped, |target| {
                prop(target, value.to_string())
            }),
            "bites" => pick(&["bite_counter"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "eggs" => pick(&["egg_count"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "hanging" => pick(&["hanging", "attached_bit"]).map_or(Translation::Dropped, |target| {
                prop(target, bool_to_number(value))
            }),
            "attached" => pick(&["attached_bit"]).map_or(Translation::Dropped, |target| {
                prop(target, bool_to_number(value))
            }),
            "note" => pick(&["note"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "has_bottle_0" => pick(&["brewing_stand_slot_a_bit"]).map_or(
                Translation::Dropped,
                |target| prop(target, bool_to_number(value)),
            ),
            "has_bottle_1" => pick(&["brewing_stand_slot_b_bit"]).map_or(
                Translation::Dropped,
                |target| prop(target, bool_to_number(value)),
            ),
            "has_bottle_2" => pick(&["brewing_stand_slot_c_bit"]).map_or(
                Translation::Dropped,
                |target| prop(target, bool_to_number(value)),
            ),
            // Walls keep their connections in Bedrock, under different names and
            // with three states instead of Java's three plus a separate post bit.
            // The value is only written when the target block really declares
            // the property, so fences and panes (which Bedrock connects on the
            // client from neighbours) still fall through to `Dropped`.
            "north" | "east" | "south" | "west" => {
                let connection = format!("wall_connection_type_{key}");
                if has(&connection) {
                    Translation::Property(connection, java_wall_connection(value))
                } else if has(key) {
                    // A Bedrock block that does carry the direction itself
                    // (newer fences) stores it as a byte.
                    Translation::Property(key.to_string(), bool_to_number(value))
                } else {
                    Translation::Dropped
                }
            }
            "up" => {
                if has("wall_post_bit") {
                    Translation::Property("wall_post_bit".to_string(), bool_to_number(value))
                } else {
                    Translation::Dropped
                }
            }
            // Derived by Bedrock or not persisted there.
            "shape" | "distance" | "stage" | "leaves" | "snowy" | "extended" | "instrument"
            | "down" | "in_wall" | "hinge" => Translation::Dropped,
            _ => pick(&[key])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
        },
        Direction::BedrockToJava => match key {
            "pillar_axis" => pick(&["axis"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "minecraft:facing_direction" | "facing_direction" => {
                let facing = if value.parse::<i32>().is_ok() {
                    facing_from_bedrock_number(value).unwrap_or("north").to_string()
                } else {
                    value.to_string()
                };
                pick(&["facing"]).map_or(Translation::Dropped, |target| prop(target, facing))
            }
            "minecraft:cardinal_direction" | "cardinal_direction" => pick(&["facing"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "direction" => {
                if let Some(facing) = cardinal_from_number(value) {
                    pick(&["facing"]).map_or(Translation::Dropped, |target| prop(target, facing.to_string()))
                } else {
                    Translation::Dropped
                }
            }
            // Walls: Bedrock's three connection states map onto Java's three,
            // and the post bit is Java's `up`. Both are written only when the
            // Java target declares them.
            key if key.starts_with("wall_connection_type_") => {
                let direction = &key["wall_connection_type_".len()..];
                if has(direction) {
                    Translation::Property(
                        direction.to_string(),
                        bedrock_wall_connection(value).to_string(),
                    )
                } else {
                    Translation::Dropped
                }
            }
            "wall_post_bit" if has("up") => {
                Translation::Property("up".to_string(), number_to_bool(value))
            }
            "minecraft:vertical_half" => pick(&["half", "type"]).map_or(
                Translation::Dropped,
                |target| prop(target, value.to_string()),
            ),
            "upside_down_bit" => pick(&["half"]).map_or(Translation::Dropped, |target| {
                prop(target, if value == "1" { "top".to_string() } else { "bottom".to_string() })
            }),
            "top_slot_bit" => pick(&["type"]).map_or(Translation::Dropped, |target| {
                prop(target, if value == "1" { "top".to_string() } else { "bottom".to_string() })
            }),
            "upper_block_bit" => pick(&["half"]).map_or(Translation::Dropped, |target| {
                prop(target, if value == "1" { "upper".to_string() } else { "lower".to_string() })
            }),
            "ground_sign_direction" => pick(&["rotation"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "open_bit" => pick(&["open"]).map_or(Translation::Dropped, |target| {
                prop(target, number_to_bool(value))
            }),
            "powered_bit" => pick(&["powered", "enabled", "lit"]).map_or(
                Translation::Dropped,
                |target| prop(target, number_to_bool(value)),
            ),
            "button_pressed_bit" | "triggered_bit" | "rail_data_bit" | "in_wall_bit" => pick(&[
                "powered",
                "triggered",
                "in_wall",
            ])
            .map_or(Translation::Dropped, |target| prop(target, number_to_bool(value))),
            "persistent_bit" => pick(&["persistent"]).map_or(Translation::Dropped, |target| {
                prop(target, number_to_bool(value))
            }),
            "redstone_signal" => pick(&["power", "powered"]).map_or(
                Translation::Dropped,
                |target| {
                    if target == "powered" {
                        prop(target, number_to_bool(value))
                    } else {
                        prop(target, value.to_string())
                    }
                },
            ),
            "growth" => pick(&["age"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "liquid_depth" => pick(&["level"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "composter_fill_level" => pick(&["level"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "moisturized_amount" => pick(&["moisture"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "bite_counter" => pick(&["bites"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "egg_count" => pick(&["eggs"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "repeater_delay" => pick(&["delay"]).map_or(Translation::Dropped, |target| {
                let delay = value.parse::<i32>().unwrap_or(0);
                prop(target, (delay + 1).to_string())
            }),
            "multi_face_direction_bits" => Translation::Consumed,
            "brewing_stand_slot_a_bit" => pick(&["has_bottle_0"]).map_or(
                Translation::Dropped,
                |target| prop(target, number_to_bool(value)),
            ),
            "brewing_stand_slot_b_bit" => pick(&["has_bottle_1"]).map_or(
                Translation::Dropped,
                |target| prop(target, number_to_bool(value)),
            ),
            "brewing_stand_slot_c_bit" => pick(&["has_bottle_2"]).map_or(
                Translation::Dropped,
                |target| prop(target, number_to_bool(value)),
            ),
            "hanging" | "attached_bit" => pick(&["hanging"]).map_or(Translation::Dropped, |target| {
                prop(target, number_to_bool(value))
            }),
            "candles" => pick(&["candles"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            "block_light_level" => pick(&["level"])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
            // Java spells booleans `true`/`false`; Bedrock says `0`/`1`. Passing
            // the digit through produced states no Java world can load.
            _ if matches!(value, "0" | "1") && has(key) => {
                Translation::Property(key.to_string(), number_to_bool(value))
            }
            _ => pick(&[key])
                .map_or(Translation::Dropped, |target| prop(target, value.to_string())),
        },
    }
}

/// Java's `none`/`low`/`tall` to Bedrock's `none`/`short`/`tall`.
fn java_wall_connection(value: &str) -> String {
    match value {
        "low" => "short".to_string(),
        // `true`/`false` appear when a wall state came from an older world.
        "true" => "tall".to_string(),
        "false" => "none".to_string(),
        other => other.to_string(),
    }
}

/// Bedrock's `none`/`short`/`tall` back to Java's `none`/`low`/`tall`.
fn bedrock_wall_connection(value: &str) -> &str {
    match value {
        "short" => "low",
        "tall" => "tall",
        _ => "none",
    }
}

fn bool_str(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

fn bit(value: bool) -> &'static str {
    if value { "1" } else { "0" }
}

fn bool_to_number(value: &str) -> String {
    match value {
        "true" | "1" => "1".to_string(),
        _ => "0".to_string(),
    }
}

fn number_to_bool(value: &str) -> String {
    match value {
        "1" | "true" => "true".to_string(),
        _ => "false".to_string(),
    }
}

fn half_to_target(target: &str, value: &str) -> String {
    match target {
        "upside_down_bit" | "top_slot_bit" | "upper_block_bit" => {
            bit(value == "top" || value == "upper").to_string()
        }
        _ => value.to_string(),
    }
}

fn weirdo_direction_number(facing: &str) -> Option<&'static str> {
    // weirdo_direction is the side the full block sits on: 0=east 1=west 2=south 3=north.
    match facing {
        "east" => Some("0"),
        "west" => Some("1"),
        "south" => Some("2"),
        "north" => Some("3"),
        _ => None,
    }
}

/// `weirdo_direction` / trapdoor `direction` is the side the full block sits on.
fn weirdo_direction_from_number(value: &str) -> Option<&'static str> {
    match value {
        "0" => Some("east"),
        "1" => Some("west"),
        "2" => Some("south"),
        "3" => Some("north"),
        _ => None,
    }
}

/// Piston and end-rod `facing_direction` are horizontally mirrored versus the
/// plain FACING_ALL order (verified against Chunker's `FLIPPED_FACING_DIRECTION`).
fn flipped_facing_to_number(facing: &str) -> Option<&'static str> {
    match facing {
        "down" => Some("0"),
        "up" => Some("1"),
        "south" => Some("2"),
        "north" => Some("3"),
        "east" => Some("4"),
        "west" => Some("5"),
        _ => None,
    }
}

fn flipped_facing_from_number(value: &str) -> Option<&'static str> {
    match value {
        "0" => Some("down"),
        "1" => Some("up"),
        "2" => Some("south"),
        "3" => Some("north"),
        "4" => Some("east"),
        "5" => Some("west"),
        _ => None,
    }
}

/// Doors rotate `cardinal_direction` one quarter-turn from Java's `facing`
/// (Chunker `CARDINAL_DIRECTION_DOOR`): Java north is Bedrock east.
fn door_cardinal_to_string(facing: &str) -> Option<&'static str> {
    match facing {
        "north" => Some("east"),
        "east" => Some("south"),
        "south" => Some("west"),
        "west" => Some("north"),
        _ => None,
    }
}

fn door_cardinal_from_string(value: &str) -> Option<&'static str> {
    match value {
        "east" => Some("north"),
        "south" => Some("east"),
        "west" => Some("south"),
        "north" => Some("west"),
        _ => None,
    }
}

/// Legacy door `direction` (0..3) per Chunker `DOOR_DIRECTION`.
fn door_direction_number(facing: &str) -> Option<&'static str> {
    match facing {
        "east" => Some("0"),
        "south" => Some("1"),
        "west" => Some("2"),
        "north" => Some("3"),
        _ => None,
    }
}

fn door_direction_from_number(value: &str) -> Option<&'static str> {
    match value {
        "0" => Some("east"),
        "1" => Some("south"),
        "2" => Some("west"),
        "3" => Some("north"),
        _ => None,
    }
}

/// Legacy `direction` (0..3) used by repeaters, comparators, fence gates and
/// end portal frames: 0=south 1=west 2=north 3=east.
fn cardinal_legacy_to_number(facing: &str) -> Option<&'static str> {
    match facing {
        "south" => Some("0"),
        "west" => Some("1"),
        "north" => Some("2"),
        "east" => Some("3"),
        _ => None,
    }
}

fn cardinal_legacy_from_number(value: &str) -> Option<&'static str> {
    match value {
        "0" => Some("south"),
        "1" => Some("west"),
        "2" => Some("north"),
        "3" => Some("east"),
        _ => None,
    }
}

/// Decorated pots use the inverted cardinal order: 0=north 1=east 2=south 3=west.
fn pot_direction_number(facing: &str) -> Option<&'static str> {
    match facing {
        "north" => Some("0"),
        "east" => Some("1"),
        "south" => Some("2"),
        "west" => Some("3"),
        _ => None,
    }
}

fn pot_direction_from_number(value: &str) -> Option<&'static str> {
    match value {
        "0" => Some("north"),
        "1" => Some("east"),
        "2" => Some("south"),
        "3" => Some("west"),
        _ => None,
    }
}

/// Bedrock 1.26.50+ stores the stair shape as `minecraft:corner`.
fn corner_from_shape(shape: &str) -> &'static str {
    match shape {
        "inner_left" => "inner_left",
        "inner_right" => "inner_right",
        "outer_left" => "outer_left",
        "outer_right" => "outer_right",
        _ => "none",
    }
}

fn shape_from_corner(corner: &str) -> &'static str {
    match corner {
        "inner_left" => "inner_left",
        "inner_right" => "inner_right",
        "outer_left" => "outer_left",
        "outer_right" => "outer_right",
        _ => "straight",
    }
}

/// Hopper `facing_direction`: 0 and 1 both mean "down".
fn hopper_facing_number(facing: &str) -> Option<&'static str> {
    match facing {
        "down" => Some("0"),
        "north" => Some("2"),
        "south" => Some("3"),
        "west" => Some("4"),
        "east" => Some("5"),
        _ => None,
    }
}

/// Lever directions (Bedrock 1.13+ `lever_direction`) and their Java face/facing.
fn lever_from_direction(direction: &str) -> Option<(&'static str, &'static str)> {
    match direction {
        "down_east_west" => Some(("ceiling", "east")),
        "east" => Some(("wall", "east")),
        "west" => Some(("wall", "west")),
        "south" => Some(("wall", "south")),
        "north" => Some(("wall", "north")),
        "up_east_west" => Some(("floor", "east")),
        "down_north_south" => Some(("ceiling", "north")),
        "up_north_south" => Some(("floor", "north")),
        _ => None,
    }
}

fn lever_direction(face: &str, facing: &str) -> Option<&'static str> {
    match (face, facing) {
        ("ceiling", "east") => Some("down_east_west"),
        ("ceiling", "north") => Some("down_north_south"),
        ("floor", "east") => Some("up_east_west"),
        ("floor", "north") => Some("up_north_south"),
        ("wall", "east") => Some("east"),
        ("wall", "west") => Some("west"),
        ("wall", "south") => Some("south"),
        ("wall", "north") => Some("north"),
        _ => None,
    }
}

fn cardinal_direction_number(facing: &str) -> Option<&'static str> {
    match facing {
        "north" => Some("0"),
        "east" => Some("1"),
        "south" => Some("2"),
        "west" => Some("3"),
        _ => None,
    }
}

fn cardinal_from_number(value: &str) -> Option<&'static str> {
    match value {
        "0" => Some("north"),
        "1" => Some("east"),
        "2" => Some("south"),
        "3" => Some("west"),
        _ => None,
    }
}

fn facing_to_bedrock_number(value: &str) -> Option<&'static str> {
    match value {
        "down" => Some("0"),
        "up" => Some("1"),
        "north" => Some("2"),
        "south" => Some("3"),
        "west" => Some("4"),
        "east" => Some("5"),
        _ => None,
    }
}

fn facing_from_bedrock_number(value: &str) -> Option<&'static str> {
    match value {
        "0" => Some("down"),
        "1" => Some("up"),
        "2" => Some("north"),
        "3" => Some("south"),
        "4" => Some("west"),
        "5" => Some("east"),
        _ => None,
    }
}

/// Bedrock states that exist in the game but are missing from the embedded
/// catalog sample (`bedrock-samples` 1.26.60 does not list the stair corner yet,
/// while 1.26.50+ writes it).
const CATALOG_GAPS: &[&str] = &["minecraft:corner"];

/// Families whose conversion is known to be approximate, per direction.
///
/// The coverage audit separates *declared* loss from *silent* loss, and the gate
/// requires silent loss to be zero: a state may fail to survive, but the report
/// has to say so. These are the families where it currently does not, and marking
/// them here is the honest alternative to pretending the mapping is exact.
///
/// Each entry is a decision, not a backlog item that will fix itself:
///
/// - `cave_air`/`void_air` collapse into Bedrock's single `air`, so the variant is
///   gone and no property records which one it was.
/// - `test_instance_block` is a Java-only debug block.
/// - `chorus_flower`, `frosted_ice` and `nether_wart` carry an `age` that Bedrock
///   does not keep in the same shape.
/// - `snow_block` is `snow` there and the reverse mapping does not restore the name.
/// - `bee_nest`/`beehive` keep `direction`/`honey_level` differently.
/// - `crafter`, `creaking_heart` and the legacy aliases below (`grass`, `reeds`,
///   `monster_egg`, `double_plant`, `flowing_*`, `lava_cauldron`, `wall_banner`,
///   `kelp`, `cactus`, `candle_cake`, `hard_glass`) are families whose Bedrock
///   shape carries less than the Java one.
const APPROXIMATE_JAVA: &[&str] = &[
    "bee_nest",
    "beehive",
    "cactus",
    "cave_air",
    "chorus_flower",
    "frosted_ice",
    "kelp",
    "nether_wart",
    "snow_block",
    "sugar_cane",
    "test_instance_block",
    "void_air",
];

const APPROXIMATE_BEDROCK: &[&str] = &[
    "bee_nest",
    "beehive",
    "cactus",
    "candle_cake",
    "chorus_flower",
    "deprecated_anvil",
    "crafter",
    "creaking_heart",
    "double_plant",
    "flowing_lava",
    "flowing_water",
    "frosted_ice",
    "grass",
    "hard_glass",
    "kelp",
    "lava_cauldron",
    "monster_egg",
    "nether_wart",
    "pale_hanging_moss",
    "red_flower",
    "reeds",
    "sculk_catalyst",
    "seaLantern",
    "stonebrick",
    "tallgrass",
    "trial_spawner",
    "unlit_redstone_torch",
    "vault",
    "wall_banner",
    "waxed_copper",
    "yellow_flower",
];

/// Whether this block's conversion is known to lose information.
fn is_approximate(short: &str, direction: Direction) -> bool {
    match direction {
        Direction::JavaToBedrock => APPROXIMATE_JAVA.contains(&short),
        // `hard_*` is Bedrock's *hardened* glass: it is opaque there and plain
        // glass in Java, so the alias is a deliberate approximation.
        Direction::BedrockToJava => {
            short.starts_with("hard_") || APPROXIMATE_BEDROCK.contains(&short)
        }
    }
}

/// Whether a Bedrock property is known to exist without being in the catalog.
///
/// The audit and the mapper have to agree on this, or a property the mapper
/// writes on purpose is reported as an invalid state.
#[must_use]
pub fn is_catalog_gap(key: &str) -> bool {
    CATALOG_GAPS.contains(&key)
}

/// Drops undeclared properties and fills required ones with the schema default.
fn fit_to_schema(mut mapped: MappedState, target: &BlockCatalog) -> MappedState {
    let Some(schema) = target.block(&mapped.name) else {
        return mapped.lossy();
    };
    let declared = schema.properties();
    // The Bedrock catalog only records states observed in its sample world, so an
    // empty property map means "no evidence", not "this block has no states".
    if declared.is_empty() {
        return mapped;
    }
    let before = mapped.properties.len();
    // Only property *names* are filtered. Validating values against the recorded
    // domain was tried and reverted: the catalog is a sample, so a real value can
    // be missing from it (`open_bit` on a lever), and dropping those silently
    // changed correct mappings. `type=double` reaching `vertical_half` is a real
    // defect, but it needs a rule for the family, not a global filter.
    mapped
        .properties
        .retain(|key, _| declared.contains_key(key) || CATALOG_GAPS.contains(&key.as_str()));
    if mapped.properties.len() != before {
        mapped.lossy = true;
    }
    for (key, property) in declared {
        if mapped.properties.contains_key(key) {
            continue;
        }
        // Missing states take the neutral value where one exists: inventing the
        // schema's first entry makes booleans default to `true` (e.g. waterlogged).
        let preferred = match property.kind() {
            PropertyKind::Bool => property
                .values()
                .iter()
                .find(|value| *value == "false" || *value == "0"),
            PropertyKind::Int => property.values().iter().find(|value| *value == "0"),
            PropertyKind::String => None,
        };
        if let Some(value) = preferred.or_else(|| property.values().first()) {
            let value = match property.kind() {
                PropertyKind::Bool => number_to_bool(value),
                _ => value.clone(),
            };
            mapped.properties.insert(key.clone(), value);
            mapped.lossy = true;
        }
    }
    mapped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::schema::{JavaCatalog, bedrock_catalog};

    fn mapper() -> BlockMapper {
        let java = JavaCatalog::bundled("26.2").expect("java catalog");
        BlockMapper::new(java.to_catalog(), bedrock_catalog())
    }

    fn props(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn stone_maps_by_name() {
        let mapped = mapper()
            .je_to_be("minecraft:stone", &props(&[]))
            .expect("stone maps");
        assert_eq!(mapped.name, "minecraft:stone");
        assert!(!mapped.lossy);
    }

    #[test]
    fn oak_log_axis_maps_to_pillar_axis() {
        let mapped = mapper()
            .je_to_be("minecraft:oak_log", &props(&[("axis", "x")]))
            .expect("log maps");
        assert_eq!(mapped.name, "minecraft:oak_log");
        assert_eq!(
            mapped.properties.get("pillar_axis").map(String::as_str),
            Some("x")
        );
    }

    #[test]
    fn stairs_translate_facing_and_half() {
        let mapped = mapper()
            .je_to_be(
                "minecraft:oak_stairs",
                &props(&[
                    ("facing", "east"),
                    ("half", "top"),
                    ("shape", "straight"),
                    ("waterlogged", "false"),
                ]),
            )
            .expect("stairs map");
        assert_eq!(mapped.name, "minecraft:oak_stairs");
        assert_eq!(
            mapped.properties.get("weirdo_direction").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            mapped.properties.get("upside_down_bit").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn double_slab_uses_double_block() {
        let mapped = mapper()
            .je_to_be("minecraft:oak_slab", &props(&[("type", "double")]))
            .expect("double slab maps");
        assert_eq!(mapped.name, "minecraft:oak_double_slab");
    }

    #[test]
    fn colored_bed_maps_to_bed_with_color() {
        let mapped = mapper()
            .je_to_be(
                "minecraft:light_blue_bed",
                &props(&[("facing", "north"), ("occupied", "false"), ("part", "head")]),
            )
            .expect("bed maps");
        assert_eq!(mapped.name, "minecraft:bed");
        // Bedrock keeps the dye colour in block-entity data, so the state is lossy.
        assert!(mapped.lossy);
        assert_eq!(
            mapped.properties.get("direction").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            mapped.properties.get("head_piece_bit").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn bedrock_bed_maps_back_to_colored_bed() {
        let mapped = mapper()
            .be_to_je("minecraft:bed", &props(&[("color", "red")]))
            .expect("bed maps back");
        assert_eq!(mapped.name, "minecraft:red_bed");
    }

    #[test]
    fn wall_torch_round_trips() {
        let mapper = mapper();
        let mapped = mapper
            .je_to_be("minecraft:wall_torch", &props(&[("facing", "north")]))
            .expect("wall torch maps");
        assert_eq!(mapped.name, "minecraft:torch");
        assert_eq!(
            mapped.properties.get("torch_facing_direction").map(String::as_str),
            Some("north")
        );
        let back = mapper
            .be_to_je("minecraft:torch", &props(&[("torch_facing_direction", "north")]))
            .expect("wall torch maps back");
        assert_eq!(back.name, "minecraft:wall_torch");
    }

    #[test]
    fn oak_door_maps_to_wooden_door_with_rotated_cardinal() {
        // Chunker CARDINAL_DIRECTION_DOOR: Java east is Bedrock south.
        let mapped = mapper()
            .je_to_be(
                "minecraft:oak_door",
                &props(&[
                    ("facing", "east"),
                    ("half", "upper"),
                    ("hinge", "right"),
                    ("open", "true"),
                    ("powered", "false"),
                ]),
            )
            .expect("door maps");
        assert_eq!(mapped.name, "minecraft:wooden_door");
        assert_eq!(
            mapped.properties.get("minecraft:cardinal_direction").map(String::as_str),
            Some("south")
        );
        // The 1.26.60 door schema only declares the cardinal property, so the
        // legacy `direction` candidate is filtered out; the helper still maps
        // Java east to legacy 0 for older targets (DOOR_DIRECTION: 0=east).
        assert_eq!(door_direction_number("east"), Some("0"));
        assert_eq!(
            mapped.properties.get("upper_block_bit").map(String::as_str),
            Some("1")
        );
        assert_eq!(
            mapped.properties.get("door_hinge_bit").map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn door_orientation_round_trips() {
        let mapper = mapper();
        for facing in ["north", "east", "south", "west"] {
            let be = mapper
                .je_to_be(
                    "minecraft:oak_door",
                    &props(&[
                        ("facing", facing),
                        ("half", "lower"),
                        ("hinge", "left"),
                        ("open", "false"),
                    ]),
                )
                .expect("door maps");
            let cardinal = be
                .properties
                .get("minecraft:cardinal_direction")
                .cloned()
                .unwrap_or_else(|| "north".to_string());
            let back = mapper
                .be_to_je(
                    &be.name,
                    &props(&[
                        ("minecraft:cardinal_direction", &cardinal),
                        ("upper_block_bit", "0"),
                        ("door_hinge_bit", "0"),
                        ("open_bit", "0"),
                    ]),
                )
                .expect("door maps back");
            assert_eq!(
                back.properties.get("facing").map(String::as_str),
                Some(facing),
                "door facing must survive the round trip"
            );
        }
    }

    #[test]
    fn trapdoor_uses_weirdo_direction() {
        let mapper = mapper();
        let be = mapper
            .je_to_be(
                "minecraft:oak_trapdoor",
                &props(&[("facing", "north"), ("half", "bottom"), ("open", "false")]),
            )
            .expect("trapdoor maps");
        assert_eq!(be.name, "minecraft:trapdoor");
        assert_eq!(be.properties.get("direction").map(String::as_str), Some("3"));
        let back = mapper
            .be_to_je("minecraft:trapdoor", &props(&[("direction", "3")]))
            .expect("trapdoor maps back");
        assert_eq!(back.properties.get("facing").map(String::as_str), Some("north"));
    }

    #[test]
    fn piston_facing_is_horizontally_flipped() {
        let mapper = mapper();
        let be = mapper
            .je_to_be(
                "minecraft:piston",
                &props(&[("facing", "east"), ("extended", "false")]),
            )
            .expect("piston maps");
        assert_eq!(
            be.properties.get("facing_direction").map(String::as_str),
            Some("4")
        );
        let back = mapper
            .be_to_je("minecraft:piston", &props(&[("facing_direction", "4")]))
            .expect("piston maps back");
        assert_eq!(back.properties.get("facing").map(String::as_str), Some("east"));
    }

    #[test]
    fn end_rod_flips_horizontals() {
        let mapper = mapper();
        let be = mapper
            .je_to_be("minecraft:end_rod", &props(&[("facing", "north")]))
            .expect("end rod maps");
        assert_eq!(
            be.properties.get("facing_direction").map(String::as_str),
            Some("3")
        );
        let back = mapper
            .be_to_je("minecraft:end_rod", &props(&[("facing_direction", "3")]))
            .expect("end rod maps back");
        assert_eq!(back.properties.get("facing").map(String::as_str), Some("north"));
    }

    #[test]
    fn stairs_corner_maps_to_shape() {
        let mapper = mapper();
        let be = mapper
            .je_to_be(
                "minecraft:oak_stairs",
                &props(&[("facing", "north"), ("half", "bottom"), ("shape", "inner_left")]),
            )
            .expect("stairs map");
        assert_eq!(
            be.properties.get("minecraft:corner").map(String::as_str),
            Some("inner_left")
        );
        let back = mapper
            .be_to_je(
                "minecraft:oak_stairs",
                &props(&[
                    ("weirdo_direction", "3"),
                    ("upside_down_bit", "0"),
                    ("minecraft:corner", "inner_left"),
                ]),
            )
            .expect("stairs map back");
        assert_eq!(back.properties.get("shape").map(String::as_str), Some("inner_left"));
        assert_eq!(back.properties.get("facing").map(String::as_str), Some("north"));
    }

    #[test]
    fn lever_direction_round_trips() {
        let mapper = mapper();
        let be = mapper
            .je_to_be(
                "minecraft:lever",
                &props(&[("face", "wall"), ("facing", "west"), ("powered", "true")]),
            )
            .expect("lever maps");
        assert_eq!(
            be.properties.get("lever_direction").map(String::as_str),
            Some("west")
        );
        assert_eq!(be.properties.get("open_bit").map(String::as_str), Some("1"));
        let back = mapper
            .be_to_je(
                "minecraft:lever",
                &props(&[("lever_direction", "west"), ("open_bit", "1")]),
            )
            .expect("lever maps back");
        assert_eq!(back.properties.get("face").map(String::as_str), Some("wall"));
        assert_eq!(back.properties.get("facing").map(String::as_str), Some("west"));
        assert_eq!(back.properties.get("powered").map(String::as_str), Some("true"));
    }

    #[test]
    fn legacy_repeater_direction_starts_at_south() {
        let mapper = mapper();
        let back = mapper
            .be_to_je(
                "minecraft:unpowered_repeater",
                &props(&[("direction", "0"), ("repeater_delay", "0")]),
            )
            .expect("legacy repeater maps");
        assert_eq!(back.properties.get("facing").map(String::as_str), Some("south"));
        assert_eq!(back.properties.get("delay").map(String::as_str), Some("1"));
    }

    #[test]
    fn hopper_treats_both_legacy_down_values_as_down() {
        let mapper = mapper();
        for value in ["0", "1"] {
            let back = mapper
                .be_to_je("minecraft:hopper", &props(&[("facing_direction", value)]))
                .expect("hopper maps");
            assert_eq!(back.properties.get("facing").map(String::as_str), Some("down"));
        }
    }

    #[test]
    fn waterlogged_is_consumed_not_lossy() {
        let mapped = mapper()
            .je_to_be(
                "minecraft:oak_stairs",
                &props(&[("facing", "north"), ("half", "bottom"), ("waterlogged", "true")]),
            )
            .expect("stairs map");
        assert!(
            !mapped.properties.contains_key("waterlogged"),
            "waterlogged belongs to the second block layer"
        );
    }
}
