//! Player data translation.
//!
//! Bedrock keeps one `~local_player` record (plus `player_<id>` for servers);
//! Java 26.x keeps `playerdata/<uuid>.dat`. The fields overlap heavily, but
//! slot numbering (Bedrock 106 = offhand), effect ids (numeric vs namespaced)
//! and attribute-based health need translation.

use indexmap::IndexMap;

use bedrock_nbt::NbtTag;

use crate::item::{read_bedrock_item, read_java_item, write_bedrock_item, write_java_item};
use crate::nbt::{NbtExt, TagValue, compound, tag_list, tag_string};

/// Bedrock player inventory slot for the offhand.
pub const BEDROCK_OFFHAND_SLOT: i32 = 106;
/// Java player inventory slot for the offhand.
pub const JAVA_OFFHAND_SLOT: i32 = -106;

/// Java `Dimension` string for a Bedrock dimension id.
#[must_use]
pub const fn dimension_name(id: i64) -> &'static str {
    match id {
        1 => "minecraft:the_nether",
        2 => "minecraft:the_end",
        _ => "minecraft:overworld",
    }
}

/// Bedrock dimension id for a Java `Dimension` string.
#[must_use]
pub fn dimension_id(name: &str) -> i32 {
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "the_nether" => 1,
        "the_end" => 2,
        _ => 0,
    }
}

/// Converts a Bedrock player record into Java `playerdata` NBT.
#[must_use]
pub fn bedrock_to_java(be: &NbtTag, uuid: [i32; 4], data_version: i32) -> NbtTag {
    let mut out = IndexMap::new();
    out.insert("DataVersion".to_string(), NbtTag::Int(data_version));
    out.insert("UUID".to_string(), NbtTag::IntArray(uuid.to_vec()));
    if let Some(pos) = be.get_list("Pos") {
        out.insert("Pos".to_string(), double_list(pos));
    }
    if let Some(rotation) = be.get_list("Rotation") {
        out.insert("Rotation".to_string(), NbtTag::List(rotation.to_vec()));
    }
    if let Some(health) = player_health(be) {
        out.insert("Health".to_string(), NbtTag::Float(health));
    }
    if let Some(level) = be.get_int("XpLevel") {
        out.insert("XpLevel".to_string(), NbtTag::Int(level as i32));
    }
    if let Some(progress) = be.get_f64("XpProgress") {
        out.insert("XpP".to_string(), NbtTag::Float(progress as f32));
    }
    if let Some(total) = be.get_int("XpTotal") {
        out.insert("XpTotal".to_string(), NbtTag::Int(total as i32));
    }
    if let Some(food) = be.get_int("foodLevel") {
        out.insert("foodLevel".to_string(), NbtTag::Int(food as i32));
    }
    if let Some(saturation) = be.get_f64("foodSaturationLevel") {
        out.insert("foodSaturationLevel".to_string(), NbtTag::Float(saturation as f32));
    }
    if let Some(game_type) = be.get_int("PlayerGameType") {
        out.insert("playerGameType".to_string(), NbtTag::Int(game_type as i32));
    }
    if let Some(dimension) = be.get_int("DimensionId") {
        out.insert(
            "Dimension".to_string(),
            tag_string(dimension_name(dimension)),
        );
    }
    if let Some(slot) = be.get_int("SelectedInventorySlot") {
        out.insert("SelectedItemSlot".to_string(), NbtTag::Int(slot as i32));
    }
    if let Some(inventory) = be.get_list("Inventory") {
        out.insert("Inventory".to_string(), java_inventory(inventory, true));
    }
    if let Some(inventory) = be.get_list("EnderChestInventory") {
        out.insert("EnderItems".to_string(), java_inventory(inventory, false));
    }
    if let Some(effects) = be.get_list("ActiveEffects") {
        out.insert("active_effects".to_string(), java_effects(effects));
    }
    for field in ["SpawnX", "SpawnY", "SpawnZ", "SpawnForced", "SpawnAngle"] {
        if let Some(value) = be.get(field) {
            out.insert(field.to_string(), value.clone());
        }
    }
    if let Some(dimension) = be.get_int("SpawnDimension") {
        out.insert(
            "SpawnDimension".to_string(),
            tag_string(dimension_name(dimension)),
        );
    } else if be.get("SpawnX").is_some() {
        out.insert(
            "SpawnDimension".to_string(),
            tag_string("minecraft:overworld"),
        );
    }
    if let Some(abilities) = be.get("abilities") {
        out.insert("abilities".to_string(), abilities.clone());
    }
    NbtTag::Compound(out)
}

/// Converts Java `playerdata` NBT into a Bedrock player record.
#[must_use]
pub fn java_to_bedrock(je: &NbtTag) -> NbtTag {
    let mut out = IndexMap::new();
    if let Some(pos) = je.get_list("Pos") {
        out.insert("Pos".to_string(), float_list(pos));
    }
    if let Some(rotation) = je.get_list("Rotation") {
        out.insert("Rotation".to_string(), NbtTag::List(rotation.to_vec()));
    }
    if let Some(health) = je.get_f64("Health") {
        out.insert("Health".to_string(), NbtTag::Float(health as f32));
    }
    if let Some(level) = je.get_int("XpLevel") {
        out.insert("XpLevel".to_string(), NbtTag::Int(level as i32));
    }
    if let Some(progress) = je.get_f64("XpP") {
        out.insert("XpProgress".to_string(), NbtTag::Float(progress as f32));
    }
    if let Some(total) = je.get_int("XpTotal") {
        out.insert("XpTotal".to_string(), NbtTag::Int(total as i32));
    }
    if let Some(food) = je.get_int("foodLevel") {
        out.insert("foodLevel".to_string(), NbtTag::Int(food as i32));
    }
    if let Some(saturation) = je.get_f64("foodSaturationLevel") {
        out.insert(
            "foodSaturationLevel".to_string(),
            NbtTag::Float(saturation as f32),
        );
    }
    if let Some(game_type) = je.get_int("playerGameType") {
        out.insert("PlayerGameType".to_string(), NbtTag::Int(game_type as i32));
    }
    if let Some(dimension) = je.get_str("Dimension") {
        out.insert(
            "DimensionId".to_string(),
            NbtTag::Int(dimension_id(dimension)),
        );
    }
    if let Some(slot) = je.get_int("SelectedItemSlot") {
        out.insert("SelectedInventorySlot".to_string(), NbtTag::Int(slot as i32));
    }
    if let Some(inventory) = je.get_list("Inventory") {
        out.insert("Inventory".to_string(), bedrock_inventory(inventory, true));
    }
    if let Some(inventory) = je.get_list("EnderItems") {
        out.insert(
            "EnderChestInventory".to_string(),
            bedrock_inventory(inventory, false),
        );
    }
    if let Some(effects) = je.get_list("active_effects") {
        out.insert("ActiveEffects".to_string(), bedrock_effects(effects));
    }
    for field in ["SpawnX", "SpawnY", "SpawnZ"] {
        if let Some(value) = je.get(field) {
            out.insert(field.to_string(), value.clone());
        }
    }
    if let Some(dimension) = je.get_str("SpawnDimension") {
        out.insert(
            "SpawnDimension".to_string(),
            NbtTag::Int(dimension_id(dimension)),
        );
    }
    if let Some(abilities) = je.get("abilities") {
        out.insert("abilities".to_string(), abilities.clone());
    }
    NbtTag::Compound(out)
}

fn player_health(be: &NbtTag) -> Option<f32> {
    if let Some(health) = be.get_f64("Health") {
        return Some(health as f32);
    }
    let attributes = be.get_list("Attributes")?;
    for attribute in attributes {
        if attribute.get_str("Name") == Some("minecraft:health") {
            return attribute
                .get_f64("Current")
                .or_else(|| attribute.get_f64("Base"))
                .map(|value| value as f32);
        }
    }
    None
}

fn java_inventory(list: &[NbtTag], map_offhand: bool) -> NbtTag {
    let entries: Vec<NbtTag> = list
        .iter()
        .filter_map(|entry| {
            let slot = entry.get_int("Slot").unwrap_or(0) as i32;
            let stack = read_bedrock_item(entry)?;
            let slot = if map_offhand && slot == BEDROCK_OFFHAND_SLOT {
                JAVA_OFFHAND_SLOT
            } else {
                slot
            };
            let mut item = match write_java_item(&stack) {
                NbtTag::Compound(entries) => entries,
                _ => IndexMap::new(),
            };
            item.insert("Slot".to_string(), NbtTag::Byte(slot as i8));
            Some(NbtTag::Compound(item))
        })
        .collect();
    tag_list(entries)
}

fn bedrock_inventory(list: &[NbtTag], map_offhand: bool) -> NbtTag {
    let entries: Vec<NbtTag> = list
        .iter()
        .filter_map(|entry| {
            let slot = entry.get_int("Slot").unwrap_or(0) as i32;
            let stack = read_java_item(entry)?;
            let slot = if map_offhand && slot == JAVA_OFFHAND_SLOT {
                BEDROCK_OFFHAND_SLOT
            } else {
                slot
            };
            let mut item = match write_bedrock_item(&stack) {
                NbtTag::Compound(entries) => entries,
                _ => IndexMap::new(),
            };
            item.insert("Slot".to_string(), NbtTag::Byte(slot as i8));
            Some(NbtTag::Compound(item))
        })
        .collect();
    tag_list(entries)
}

fn java_effects(list: &[NbtTag]) -> NbtTag {
    let entries: Vec<NbtTag> = list
        .iter()
        .filter_map(|effect| {
            let id = effect
                .get_int("Id")
                .and_then(|id| u16::try_from(id).ok())
                .and_then(crate::mapping::effects::effect_name)?;
            Some(compound([
                ("id", tag_string(id)),
                (
                    "amplifier",
                    NbtTag::Int(effect.get_int("Amplifier").unwrap_or(0) as i32),
                ),
                (
                    "duration",
                    NbtTag::Int(effect.get_int("Duration").unwrap_or(0) as i32),
                ),
                ("ambient", NbtTag::Byte(0)),
                ("show_particles", NbtTag::Byte(1)),
                ("show_icon", NbtTag::Byte(1)),
            ]))
        })
        .collect();
    tag_list(entries)
}

fn bedrock_effects(list: &[NbtTag]) -> NbtTag {
    let entries: Vec<NbtTag> = list
        .iter()
        .filter_map(|effect| {
            let id = effect
                .get_str("id")
                .and_then(crate::mapping::effects::effect_id)?;
            Some(compound([
                ("Id", NbtTag::Byte(i8::try_from(id).unwrap_or(0))),
                (
                    "Amplifier",
                    NbtTag::Byte(effect.get_int("amplifier").unwrap_or(0) as i8),
                ),
                (
                    "Duration",
                    NbtTag::Int(effect.get_int("duration").unwrap_or(0) as i32),
                ),
                ("ShowParticles", NbtTag::Byte(1)),
            ]))
        })
        .collect();
    tag_list(entries)
}

fn float_list(values: &[NbtTag]) -> NbtTag {
    NbtTag::List(
        values
            .iter()
            .map(|value| NbtTag::Float(value.as_f64_value().unwrap_or(0.0) as f32))
            .collect(),
    )
}

fn double_list(values: &[NbtTag]) -> NbtTag {
    NbtTag::List(
        values
            .iter()
            .map(|value| NbtTag::Double(value.as_f64_value().unwrap_or(0.0)))
            .collect(),
    )
}

/// Derives a stable offline UUID (four ints) from arbitrary bytes.
#[must_use]
pub fn offline_uuid(seed: &[u8]) -> [i32; 4] {
    // Simple FNV-1a expansion; deterministic and collision-resistant enough for
    // local player files.
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut parts = [0_i32; 4];
    for (index, part) in parts.iter_mut().enumerate() {
        for byte in seed {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100_0000_01b3);
            hash ^= (index as u64) << 17;
        }
        *part = (hash >> 32) as i32;
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn player_round_trips_core_fields() {
        let je = compound([
            (
                "Pos",
                tag_list(vec![
                    NbtTag::Double(1.5),
                    NbtTag::Double(70.0),
                    NbtTag::Double(-2.5),
                ]),
            ),
            ("Rotation", tag_list(vec![NbtTag::Float(90.0), NbtTag::Float(0.0)])),
            ("Health", NbtTag::Float(18.5)),
            ("XpLevel", NbtTag::Int(7)),
            ("XpP", NbtTag::Float(0.5)),
            ("XpTotal", NbtTag::Int(130)),
            ("foodLevel", NbtTag::Int(17)),
            ("playerGameType", NbtTag::Int(1)),
            ("Dimension", tag_string("minecraft:the_nether")),
            ("SelectedItemSlot", NbtTag::Int(3)),
            (
                "Inventory",
                tag_list(vec![{
                    let mut item = match write_java_item(&crate::item::ItemStack {
                        id: "minecraft:stone".to_string(),
                        count: 12,
                        ..crate::item::ItemStack::default()
                    }) {
                        NbtTag::Compound(entries) => entries,
                        _ => IndexMap::new(),
                    };
                    item.insert("Slot".to_string(), NbtTag::Byte(5));
                    NbtTag::Compound(item)
                }]),
            ),
        ]);
        let be = java_to_bedrock(&je);
        assert_eq!(be.get_int("PlayerGameType"), Some(1));
        assert_eq!(be.get_int("DimensionId"), Some(1));
        let back = bedrock_to_java(&be, [1, 2, 3, 4], 4903);
        assert_eq!(back.get_str("Dimension"), Some("minecraft:the_nether"));
        assert_eq!(back.get_int("XpLevel"), Some(7));
        let inventory = back.get_list("Inventory").expect("inventory");
        assert_eq!(inventory[0].get_int("Slot"), Some(5));
        assert_eq!(inventory[0].get_str("id"), Some("minecraft:stone"));
    }

    #[test]
    fn offhand_slot_mapping_is_bidirectional() {
        let be_item = {
            let mut item = match crate::item::write_bedrock_item(&crate::item::ItemStack {
                id: "minecraft:shield".to_string(),
                count: 1,
                ..crate::item::ItemStack::default()
            }) {
                NbtTag::Compound(entries) => entries,
                _ => IndexMap::new(),
            };
            item.insert("Slot".to_string(), NbtTag::Byte(BEDROCK_OFFHAND_SLOT as i8));
            NbtTag::Compound(item)
        };
        let je = bedrock_to_java(
            &compound([("Inventory", tag_list(vec![be_item]))]),
            [0, 0, 0, 0],
            4903,
        );
        let inventory = je.get_list("Inventory").expect("inventory");
        assert_eq!(inventory[0].get_int("Slot"), Some(i64::from(JAVA_OFFHAND_SLOT)));
    }
}
