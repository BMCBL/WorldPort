//! Item stack translation between Java and Bedrock.
//!
//! Java 1.20.5+ stores stacks as `{ id, count, components }`; older worlds use
//! `{ id, Count, tag }`. Bedrock stores `{ Name, Count, Damage?, tag? }`. Both
//! shapes are read, and the translator normalises to one in-memory stack before
//! writing the target shape.

use std::collections::BTreeMap;

use bedrock_nbt::NbtTag;
use indexmap::IndexMap;

use crate::mapping::enchantments::{enchantment_id, enchantment_name};
use crate::mapping::potions::{potion_id, potion_name};
use crate::nbt::{NbtExt, TagValue, compound, empty_compound, tag_list, tag_string};

/// Edition-independent item stack.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemStack {
    /// Full item identifier (`minecraft:stone`).
    pub id: String,
    /// Stack size.
    pub count: i32,
    /// Enchantments as `(Java enchantment id, level)`.
    pub enchantments: Vec<(String, i16)>,
    /// Damage value (`Damage` / `minecraft:damage`).
    pub damage: Option<i32>,
    /// Custom name as plain text.
    pub custom_name: Option<String>,
    /// Lore lines as plain text.
    pub lore: Vec<String>,
    /// Potion type id (`minecraft:healing`).
    pub potion: Option<String>,
    /// Unbreakable flag.
    pub unbreakable: bool,
    /// Remaining Bedrock `tag` payload fields that have no Java component equivalent.
    pub extra: IndexMap<String, NbtTag>,
}

impl ItemStack {
    /// True when this stack represents air and should be omitted.
    #[must_use]
    pub fn is_air(&self) -> bool {
        self.id.is_empty() || self.id == "minecraft:air"
    }
}

/// Java → Bedrock item id aliases that differ between editions.
const ITEM_ALIASES: &[(&str, &str)] = &[
    ("minecraft:cactus_green", "minecraft:green_dye"),
    ("minecraft:dandelion_yellow", "minecraft:yellow_dye"),
    ("minecraft:rose_red", "minecraft:red_dye"),
    ("minecraft:debug_stick", "minecraft:stick"),
    ("minecraft:furnace_minecart", "minecraft:minecart"),
    ("minecraft:glow_item_frame", "minecraft:glow_frame"),
    ("minecraft:item_frame", "minecraft:frame"),
    ("minecraft:knowledge_book", "minecraft:book"),
    ("minecraft:nether_brick", "minecraft:netherbrick"),
    ("minecraft:spectral_arrow", "minecraft:arrow"),
    ("minecraft:tipped_arrow", "minecraft:arrow"),
    ("minecraft:zombified_piglin_spawn_egg", "minecraft:zombie_pigman_spawn_egg"),
    ("minecraft:pottery_shard_archer", "minecraft:archer_pottery_sherd"),
    ("minecraft:pottery_shard_arms_up", "minecraft:arms_up_pottery_sherd"),
    ("minecraft:pottery_shard_blade", "minecraft:blade_pottery_sherd"),
    ("minecraft:pottery_shard_brewer", "minecraft:brewer_pottery_sherd"),
    ("minecraft:pottery_shard_burn", "minecraft:burn_pottery_sherd"),
    ("minecraft:pottery_shard_danger", "minecraft:danger_pottery_sherd"),
    ("minecraft:pottery_shard_explorer", "minecraft:explorer_pottery_sherd"),
    ("minecraft:pottery_shard_friend", "minecraft:friend_pottery_sherd"),
    ("minecraft:pottery_shard_heart", "minecraft:heart_pottery_sherd"),
    ("minecraft:pottery_shard_heartbreak", "minecraft:heartbreak_pottery_sherd"),
    ("minecraft:pottery_shard_howl", "minecraft:howl_pottery_sherd"),
    ("minecraft:pottery_shard_miner", "minecraft:miner_pottery_sherd"),
    ("minecraft:pottery_shard_mourner", "minecraft:mourner_pottery_sherd"),
    ("minecraft:pottery_shard_plenty", "minecraft:plenty_pottery_sherd"),
    ("minecraft:pottery_shard_prize", "minecraft:prize_pottery_sherd"),
    ("minecraft:pottery_shard_sheaf", "minecraft:sheaf_pottery_sherd"),
    ("minecraft:pottery_shard_shelter", "minecraft:shelter_pottery_sherd"),
    ("minecraft:pottery_shard_skull", "minecraft:skull_pottery_sherd"),
    ("minecraft:pottery_shard_snort", "minecraft:snort_pottery_sherd"),
];

/// Java item identifier → Bedrock identifier.
#[must_use]
pub fn item_to_bedrock(java: &str) -> String {
    let full = namespaced(java);
    ITEM_ALIASES
        .iter()
        .find(|(source, _)| *source == full)
        .map_or(full, |(_, target)| (*target).to_string())
}

/// Bedrock item identifier → Java identifier.
#[must_use]
pub fn item_to_java(bedrock: &str) -> String {
    let full = namespaced(bedrock);
    ITEM_ALIASES
        .iter()
        .find(|(_, target)| *target == full)
        .map_or(full, |(source, _)| (*source).to_string())
}

fn namespaced(id: &str) -> String {
    if id.contains(':') {
        id.to_string()
    } else {
        format!("minecraft:{id}")
    }
}

/// Reads a Java item stack (new or legacy shape).
#[must_use]
pub fn read_java_item(tag: &NbtTag) -> Option<ItemStack> {
    let id = tag.get_str("id")?;
    // Non-vanilla items (mods/addons) are pruned rather than carried across.
    if id == "minecraft:air" || !crate::mapping::is_vanilla(id) {
        return None;
    }
    let count = tag
        .get_int("count")
        .or_else(|| tag.get_int("Count"))
        .unwrap_or(1);
    let mut stack = ItemStack {
        id: id.to_string(),
        count: i32::try_from(count).unwrap_or(1),
        ..ItemStack::default()
    };
    if let Some(components) = tag.get("components").and_then(NbtTag::as_compound) {
        read_components(&mut stack, components);
    }
    if let Some(legacy) = tag.get("tag").and_then(NbtTag::as_compound) {
        read_legacy_tag(&mut stack, legacy);
    }
    Some(stack)
}

/// DataVersion where item components replaced the legacy `tag` shape (1.20.5).
pub const COMPONENTS_DATA_VERSION: i32 = 3837;

/// Builds a Java item stack for a target DataVersion.
///
/// 1.20.5+ uses `{id, count, components}`; older versions use
/// `{id, Count, tag}`.
#[must_use]
pub fn write_java_item(stack: &ItemStack, data_version: i32) -> NbtTag {
    let mut item = IndexMap::new();
    item.insert("id".to_string(), tag_string(item_to_java(&stack.id)));
    if data_version >= COMPONENTS_DATA_VERSION {
        item.insert("count".to_string(), NbtTag::Int(stack.count.max(1)));
        let components = write_components(stack);
        if !matches!(&components, NbtTag::Compound(entries) if entries.is_empty()) {
            item.insert("components".to_string(), components);
        }
    } else {
        item.insert(
            "Count".to_string(),
            NbtTag::Byte(i8::try_from(stack.count.clamp(1, 64)).unwrap_or(1)),
        );
        let payload = write_legacy_tag(stack);
        if !matches!(&payload, NbtTag::Compound(entries) if entries.is_empty()) {
            item.insert("tag".to_string(), payload);
        }
    }
    NbtTag::Compound(item)
}

/// Legacy Java item tag (`Enchantments`, `display`, `Damage`, `Unbreakable`).
fn write_legacy_tag(stack: &ItemStack) -> NbtTag {
    let mut payload = IndexMap::new();
    if !stack.enchantments.is_empty() {
        let list: Vec<NbtTag> = stack
            .enchantments
            .iter()
            .map(|(id, level)| {
                compound([
                    ("id", tag_string(id.clone())),
                    ("lvl", NbtTag::Short(*level)),
                ])
            })
            .collect();
        payload.insert("Enchantments".to_string(), tag_list(list));
    }
    if let Some(damage) = stack.damage
        && damage != 0
    {
        payload.insert("Damage".to_string(), NbtTag::Int(damage));
    }
    if let Some(potion) = &stack.potion {
        payload.insert("Potion".to_string(), tag_string(potion.clone()));
    }
    if stack.custom_name.is_some() || !stack.lore.is_empty() {
        let mut display = IndexMap::new();
        if let Some(name) = &stack.custom_name {
            display.insert("Name".to_string(), tag_string(json_text(name)));
        }
        if !stack.lore.is_empty() {
            display.insert(
                "Lore".to_string(),
                tag_list(stack.lore.iter().map(|line| tag_string(json_text(line))).collect()),
            );
        }
        payload.insert("display".to_string(), NbtTag::Compound(display));
    }
    if stack.unbreakable {
        payload.insert("Unbreakable".to_string(), NbtTag::Byte(1));
    }
    NbtTag::Compound(payload)
}

/// Reads a Bedrock item stack (`{Name, Count, Damage?, tag?}`).
#[must_use]
pub fn read_bedrock_item(tag: &NbtTag) -> Option<ItemStack> {
    let name = tag.get_str("Name")?;
    if name == "minecraft:air" || !crate::mapping::is_vanilla(name) {
        return None;
    }
    let count = tag.get_int("Count").unwrap_or(1);
    let mut stack = ItemStack {
        id: name.to_string(),
        count: i32::try_from(count).unwrap_or(1),
        ..ItemStack::default()
    };
    if let Some(damage) = tag.get_int("Damage")
        && damage != 0
    {
        stack.damage = Some(i32::try_from(damage).unwrap_or(0));
    }
    if let Some(payload) = tag.get("tag").and_then(NbtTag::as_compound) {
        if let Some(ench) = payload.get("ench") {
            stack.enchantments = read_bedrock_enchantments(ench);
        }
        if let Some(potion) = payload.get_int("PotionId").and_then(|id| u32::try_from(id).ok())
            && let Some(name) = potion_name(potion)
        {
            stack.potion = Some(name.to_string());
        }
        if let Some(display) = payload.get("display").and_then(NbtTag::as_compound) {
            if let Some(name) = display.get_str("Name") {
                stack.custom_name = Some(name.to_string());
            }
            if let Some(lore) = display.get_list("Lore") {
                stack.lore = lore
                    .iter()
                    .filter_map(|line| line.as_str_value().map(str::to_owned))
                    .collect();
            }
        }
        if payload.get_bool("Unbreakable").unwrap_or(false) {
            stack.unbreakable = true;
        }
        for (key, value) in payload {
            if !matches!(
                key.as_str(),
                "ench" | "PotionId" | "display" | "Unbreakable" | "Damage"
            ) {
                stack.extra.insert(key.clone(), value.clone());
            }
        }
    }
    Some(stack)
}

/// Builds a Bedrock item stack.
#[must_use]
pub fn write_bedrock_item(stack: &ItemStack) -> NbtTag {
    let mut item = IndexMap::new();
    item.insert("Name".to_string(), tag_string(item_to_bedrock(&stack.id)));
    item.insert(
        "Count".to_string(),
        NbtTag::Byte(i8::try_from(stack.count.clamp(0, 64)).unwrap_or(1)),
    );
    if let Some(damage) = stack.damage
        && damage != 0
    {
        item.insert(
            "Damage".to_string(),
            NbtTag::Short(i16::try_from(damage).unwrap_or(0)),
        );
    }
    let payload = write_bedrock_tag(stack);
    if !matches!(&payload, NbtTag::Compound(entries) if entries.is_empty()) {
        item.insert("tag".to_string(), payload);
    }
    NbtTag::Compound(item)
}

fn read_components(stack: &mut ItemStack, components: &IndexMap<String, NbtTag>) {
    if let Some(enchantments) = components.get("minecraft:enchantments") {
        if let Some(levels) = enchantments
            .get("levels")
            .and_then(NbtTag::as_compound)
        {
            for (id, level) in levels {
                stack
                    .enchantments
                    .push((id.clone(), i16::try_from(level.as_i64_value().unwrap_or(1)).unwrap_or(1)));
            }
        } else if let Some(list) = enchantments.get_list("levels") {
            for entry in list {
                if let (Some(id), Some(level)) = (entry.get_str("id"), entry.get_int("lvl")) {
                    stack.enchantments.push((
                        id.to_string(),
                        i16::try_from(level).unwrap_or(1),
                    ));
                }
            }
        }
    }
    if let Some(damage) = components.get_int("minecraft:damage") {
        stack.damage = Some(i32::try_from(damage).unwrap_or(0));
    }
    if let Some(name) = components.get_str("minecraft:custom_name") {
        stack.custom_name = Some(plain_text(name));
    }
    if let Some(lore) = components.get_list("minecraft:lore") {
        stack.lore = lore
            .iter()
            .filter_map(|line| line.as_str_value().map(plain_text))
            .collect();
    }
    if components.get("minecraft:unbreakable").is_some() {
        stack.unbreakable = true;
    }
    if let Some(potion) = components
        .get("minecraft:potion_contents")
        .and_then(|contents| contents.get_str("potion"))
    {
        stack.potion = Some(potion.to_string());
    }
}

fn read_legacy_tag(stack: &mut ItemStack, payload: &IndexMap<String, NbtTag>) {
    if let Some(list) = payload.get_list("Enchantments") {
        for entry in list {
            if let (Some(id), Some(level)) = (entry.get_str("id"), entry.get_int("lvl")) {
                stack
                    .enchantments
                    .push((id.to_string(), i16::try_from(level).unwrap_or(1)));
            }
        }
    }
    if let Some(damage) = payload.get_int("Damage") {
        stack.damage = Some(i32::try_from(damage).unwrap_or(0));
    }
    if let Some(display) = payload.get("display").and_then(NbtTag::as_compound) {
        if let Some(name) = display.get_str("Name") {
            stack.custom_name.get_or_insert_with(|| plain_text(name));
        }
        if let Some(lore) = display.get_list("Lore") {
            stack.lore = lore
                .iter()
                .filter_map(|line| line.as_str_value().map(plain_text))
                .collect();
        }
    }
    if payload.get_bool("Unbreakable").unwrap_or(false) {
        stack.unbreakable = true;
    }
    if let Some(potion) = payload.get_str("Potion") {
        stack.potion = Some(potion.to_string());
    }
}

fn write_components(stack: &ItemStack) -> NbtTag {
    let mut components = IndexMap::new();
    if !stack.enchantments.is_empty() {
        let mut levels = IndexMap::new();
        for (id, level) in &stack.enchantments {
            levels.insert(id.clone(), NbtTag::Int(i32::from(*level)));
        }
        components.insert(
            "minecraft:enchantments".to_string(),
            compound([("levels", NbtTag::Compound(levels))]),
        );
    }
    if let Some(damage) = stack.damage
        && damage != 0
    {
        components.insert("minecraft:damage".to_string(), NbtTag::Int(damage));
    }
    if let Some(name) = &stack.custom_name {
        components.insert(
            "minecraft:custom_name".to_string(),
            tag_string(json_text(name)),
        );
    }
    if !stack.lore.is_empty() {
        components.insert(
            "minecraft:lore".to_string(),
            tag_list(
                stack
                    .lore
                    .iter()
                    .map(|line| tag_string(json_text(line)))
                    .collect(),
            ),
        );
    }
    if stack.unbreakable {
        components.insert("minecraft:unbreakable".to_string(), empty_compound());
    }
    if let Some(potion) = &stack.potion {
        components.insert(
            "minecraft:potion_contents".to_string(),
            compound([("potion", tag_string(potion.clone()))]),
        );
    }
    NbtTag::Compound(components)
}

fn write_bedrock_tag(stack: &ItemStack) -> NbtTag {
    let mut payload = stack.extra.clone();
    if !stack.enchantments.is_empty() {
        let ench: Vec<NbtTag> = stack
            .enchantments
            .iter()
            .filter_map(|(id, level)| {
                let numeric = enchantment_id(id)?;
                Some(compound([
                    (
                        "id",
                        NbtTag::Short(i16::try_from(numeric).unwrap_or(0)),
                    ),
                    ("lvl", NbtTag::Short(*level)),
                ]))
            })
            .collect();
        payload.insert("ench".to_string(), tag_list(ench));
    }
    if let Some(potion) = &stack.potion
        && let Some(id) = potion_id(potion)
    {
        payload.insert(
            "PotionId".to_string(),
            NbtTag::Short(i16::try_from(id).unwrap_or(0)),
        );
    }
    if stack.custom_name.is_some() || !stack.lore.is_empty() {
        let mut display = IndexMap::new();
        if let Some(name) = &stack.custom_name {
            display.insert("Name".to_string(), tag_string(name.clone()));
        }
        if !stack.lore.is_empty() {
            display.insert(
                "Lore".to_string(),
                tag_list(stack.lore.iter().cloned().map(NbtTag::String).collect()),
            );
        }
        payload.insert("display".to_string(), NbtTag::Compound(display));
    }
    if stack.unbreakable {
        payload.insert("Unbreakable".to_string(), NbtTag::Byte(1));
    }
    NbtTag::Compound(payload)
}

fn read_bedrock_enchantments(tag: &NbtTag) -> Vec<(String, i16)> {
    let NbtTag::List(list) = tag else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|entry| {
            let id = entry
                .get_int("id")
                .and_then(|id| u16::try_from(id).ok())
                .and_then(enchantment_name)?;
            let level = entry.get_int("lvl").unwrap_or(1);
            Some((id.to_string(), i16::try_from(level).unwrap_or(1)))
        })
        .collect()
}

/// Extracts plain text from a Java JSON text component string.
#[must_use]
pub fn plain_text(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(trimmed)
            && let Some(text) = parsed.get("text").and_then(|text| text.as_str())
        {
            return text.to_string();
        }
        return trimmed.to_string();
    }
    if trimmed.starts_with('"')
        && let Ok(parsed) = serde_json::from_str::<String>(trimmed)
    {
        return parsed;
    }
    trimmed.to_string()
}

/// Wraps plain text into a Java JSON text component string.
#[must_use]
pub fn json_text(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

/// Shared slot map for container block entities (identity for same-size containers).
#[must_use]
pub const fn container_slot(slot: i32) -> i32 {
    slot
}

/// Convenience for building a Java `Items` list from stacks.
#[must_use]
pub fn java_items_list(stacks: &[(i32, ItemStack)], data_version: i32) -> NbtTag {
    let entries: Vec<NbtTag> = stacks
        .iter()
        .filter(|(_, stack)| !stack.is_air())
        .map(|(slot, stack)| {
            let mut item = match write_java_item(stack, data_version) {
                NbtTag::Compound(entries) => entries,
                _ => IndexMap::new(),
            };
            item.insert("Slot".to_string(), NbtTag::Byte(*slot as i8));
            NbtTag::Compound(item)
        })
        .collect();
    tag_list(entries)
}

/// Convenience for building a Bedrock `Items` list from stacks.
#[must_use]
pub fn bedrock_items_list(stacks: &[(i32, ItemStack)]) -> NbtTag {
    let entries: Vec<NbtTag> = stacks
        .iter()
        .filter(|(_, stack)| !stack.is_air())
        .map(|(slot, stack)| {
            let mut item = match write_bedrock_item(stack) {
                NbtTag::Compound(entries) => entries,
                _ => IndexMap::new(),
            };
            item.insert("Slot".to_string(), NbtTag::Byte(*slot as i8));
            NbtTag::Compound(item)
        })
        .collect();
    tag_list(entries)
}

/// Reads a list of `(Slot, ItemStack)` from either edition's container payload.
#[must_use]
pub fn read_container_items(list: &[NbtTag], java: bool) -> Vec<(i32, ItemStack)> {
    list.iter()
        .filter_map(|entry| {
            let slot = entry.get_int("Slot").unwrap_or(0);
            let stack = if java {
                read_java_item(entry)
            } else {
                read_bedrock_item(entry)
            }?;
            Some((i32::try_from(slot).unwrap_or(0), stack))
        })
        .collect()
}

/// Groups stacks by slot for callers that need lookup by index.
#[must_use]
pub fn by_slot(stacks: Vec<(i32, ItemStack)>) -> BTreeMap<i32, ItemStack> {
    stacks.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_item_round_trips_through_bedrock() {
        let java = compound([
            ("id", tag_string("minecraft:diamond_sword")),
            ("count", NbtTag::Int(1)),
            (
                "components",
                compound([
                    (
                        "minecraft:enchantments",
                        compound([(
                            "levels",
                            compound([("minecraft:sharpness", NbtTag::Int(5))]),
                        )]),
                    ),
                    ("minecraft:damage", NbtTag::Int(12)),
                    ("minecraft:custom_name", tag_string("\"Excalibur\"")),
                ]),
            ),
        ]);
        let stack = read_java_item(&java).expect("java item");
        assert_eq!(stack.enchantments, vec![("minecraft:sharpness".to_string(), 5)]);
        assert_eq!(stack.damage, Some(12));
        assert_eq!(stack.custom_name.as_deref(), Some("Excalibur"));

        let bedrock = write_bedrock_item(&stack);
        assert_eq!(bedrock.get_str("Name"), Some("minecraft:diamond_sword"));
        let payload = bedrock.get("tag").expect("bedrock tag");
        assert_eq!(payload.get_list("ench").map(|values| values.len()), Some(1));

        let back = read_bedrock_item(&bedrock).expect("bedrock item");
        assert_eq!(back.enchantments, stack.enchantments);
        assert_eq!(back.damage, Some(12));
        assert_eq!(back.custom_name.as_deref(), Some("Excalibur"));
    }

    #[test]
    fn aliases_cover_renamed_items() {
        assert_eq!(item_to_bedrock("minecraft:pottery_shard_skull"), "minecraft:skull_pottery_sherd");
        assert_eq!(item_to_java("minecraft:skull_pottery_sherd"), "minecraft:pottery_shard_skull");
        assert_eq!(item_to_bedrock("minecraft:apple"), "minecraft:apple");
    }

    #[test]
    fn plain_text_extracts_json_components() {
        assert_eq!(plain_text("\"Hello\""), "Hello");
        assert_eq!(plain_text("{\"text\":\"Hi\",\"color\":\"red\"}"), "Hi");
        assert_eq!(plain_text("Plain"), "Plain");
    }

    #[test]
    fn legacy_targets_use_count_and_tag() {
        let stack = ItemStack {
            id: "minecraft:diamond_sword".to_string(),
            count: 1,
            enchantments: vec![("minecraft:sharpness".to_string(), 3)],
            custom_name: Some("Legacy".to_string()),
            ..ItemStack::default()
        };
        let legacy = write_java_item(&stack, 3700);
        assert!(legacy.get("count").is_none());
        assert_eq!(legacy.get_int("Count"), Some(1));
        let payload = legacy.get("tag").expect("legacy tag");
        assert!(payload.get_list("Enchantments").is_some());
        assert_eq!(
            payload
                .get("display")
                .and_then(|display| display.get_str("Name")),
            Some("\"Legacy\"")
        );
        let back = read_java_item(&legacy).expect("legacy read");
        assert_eq!(back.enchantments, stack.enchantments);
        assert_eq!(back.custom_name.as_deref(), Some("Legacy"));
    }

    #[test]
    fn potion_ids_round_trip() {
        let stack = ItemStack {
            id: "minecraft:potion".to_string(),
            count: 1,
            potion: Some("minecraft:healing".to_string()),
            ..ItemStack::default()
        };
        let bedrock = write_bedrock_item(&stack);
        let back = read_bedrock_item(&bedrock).expect("potion");
        assert_eq!(back.potion.as_deref(), Some("minecraft:healing"));
    }
}
