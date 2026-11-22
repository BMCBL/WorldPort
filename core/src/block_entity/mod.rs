//! Block entity translation between Java and Bedrock.
//!
//! Containers share a slot layout for chests, furnaces, hoppers, dispensers and
//! shulkers, so items are translated with the shared [`crate::item`] module and
//! keep their slot numbers. Fields that both editions store under the same name
//! (`BurnTime`, `CookTime`, `CustomName`, `Lock`, `Delay`, ...) are copied.
//! Families whose payload shapes differ (signs, spawners, jukeboxes, banners,
//! lecterns) get explicit rules below.

use indexmap::IndexMap;

use bedrock_nbt::NbtTag;

use crate::item::{ItemStack, read_container_items, java_items_list, bedrock_items_list};
use crate::nbt::{NbtExt, TagValue, compound, empty_compound, tag_string};
use crate::report::ReportBuilder;

/// Java block entity id → Bedrock block entity id.
#[must_use]
pub fn block_entity_id_to_bedrock(java: &str) -> Option<&'static str> {
    let short = java.strip_prefix("minecraft:").unwrap_or(java);
    BLOCK_ENTITY_IDS
        .iter()
        .find(|(source, _)| *source == short)
        .map(|(_, target)| *target)
}

/// Bedrock block entity id → Java block entity id (with namespace).
#[must_use]
pub fn block_entity_id_to_java(bedrock: &str) -> Option<String> {
    BLOCK_ENTITY_IDS
        .iter()
        .find(|(_, target)| *target == bedrock)
        .map(|(source, _)| format!("minecraft:{source}"))
}

/// `(Java id without namespace, Bedrock id)` for block entities.
pub const BLOCK_ENTITY_IDS: &[(&str, &str)] = &[
    ("barrel", "Barrel"),
    ("beacon", "Beacon"),
    ("bed", "Bed"),
    ("bee_nest", "Beehive"),
    ("beehive", "Beehive"),
    ("bell", "Bell"),
    ("blast_furnace", "BlastFurnace"),
    ("banner", "Banner"),
    ("brewing_stand", "BrewingStand"),
    ("calibrated_sculk_sensor", "CalibratedSculkSensor"),
    ("campfire", "Campfire"),
    ("chest", "Chest"),
    ("chiseled_bookshelf", "ChiseledBookshelf"),
    ("command_block", "CommandBlock"),
    ("conduit", "Conduit"),
    ("copper_golem_statue", "CopperGolemStatue"),
    ("crafter", "Crafter"),
    ("creaking_heart", "CreakingHeart"),
    ("daylight_detector", "DaylightDetector"),
    ("decorated_pot", "DecoratedPot"),
    ("dispenser", "Dispenser"),
    ("dropper", "Dropper"),
    ("enchanting_table", "EnchantTable"),
    ("end_gateway", "EndGateway"),
    ("end_portal", "EndPortal"),
    ("ender_chest", "EnderChest"),
    ("flower_pot", "FlowerPot"),
    ("furnace", "Furnace"),
    ("hanging_sign", "HangingSign"),
    ("hopper", "Hopper"),
    ("jigsaw", "JigsawBlock"),
    ("jukebox", "RecordPlayer"),
    ("lectern", "Lectern"),
    ("lodestone", "Lodestone"),
    ("mob_spawner", "MobSpawner"),
    ("moving_piston", "MovingBlock"),
    ("piston", "PistonArm"),
    ("sculk_catalyst", "SculkCatalyst"),
    ("sculk_sensor", "SculkSensor"),
    ("sculk_shrieker", "SculkShrieker"),
    ("shelf", "Shelf"),
    ("shulker_box", "ShulkerBox"),
    ("sign", "Sign"),
    ("skull", "Skull"),
    ("smoker", "Smoker"),
    ("structure_block", "StructureBlock"),
    ("trapped_chest", "TrappedChest"),
    ("trial_spawner", "TrialSpawner"),
    ("vault", "Vault"),
];

/// Fields copied unchanged because both editions use the same name and type.
const SHARED_FIELDS: &[&str] = &[
    "BurnTime",
    "CookTime",
    "CookTimeTotal",
    "CustomName",
    "Lock",
    "Delay",
    "MinSpawnDelay",
    "MaxSpawnDelay",
    "SpawnCount",
    "MaxNearbyEntities",
    "RequiredPlayerRange",
    "SpawnRange",
    "Level",
    "Primary",
    "Secondary",
    "powered",
    "Power",
    "TransferCooldown",
    "MoveItemSpeed",
    "TicksInHive",
    "MinOccupationTicks",
    "Book",
    "Page",
    "TotalPages",
    "ominous",
    "next_mob_id",
    "entity_type",
    "x",
    "y",
    "z",
];

/// Translates one Java block entity payload into a Bedrock one.
#[must_use]
pub fn java_to_bedrock(je: &NbtTag, report: &mut ReportBuilder) -> Option<IndexMap<String, NbtTag>> {
    let je_id = je.get_str("id")?;
    let be_id = block_entity_id_to_bedrock(je_id);
    let Some(be_id) = be_id else {
        report.warn(
            crate::report::WarningCode::UnmappedEntity,
            je_id.to_string(),
            format!("block entity {je_id} has no Bedrock mapping; it was skipped"),
        );
        return None;
    };
    let mut out = IndexMap::new();
    out.insert("id".to_string(), tag_string(be_id));
    for axis in ["x", "y", "z"] {
        if let Some(value) = je.get(axis) {
            out.insert(axis.to_string(), value.clone());
        }
    }

    if let Some(items) = je.get_list("Items") {
        let stacks = read_container_items(items, true);
        out.insert("Items".to_string(), bedrock_items_list(&stacks));
    }
    if let Some(items) = je.get_list("Inventory") {
        let stacks = read_container_items(items, true);
        out.insert("Items".to_string(), bedrock_items_list(&stacks));
    }

    // Signs: Java keeps four JSON message lines per side.
    if je_id == "minecraft:sign" || je_id == "minecraft:hanging_sign" {
        for (je_key, be_key) in [("front_text", "FrontText"), ("back_text", "BackText")] {
            if let Some(side) = je.get(je_key)
                && let Some(messages) = side.get_list("messages")
            {
                let lines: Vec<String> = messages
                    .iter()
                    .map(|line| crate::item::plain_text(line.as_str_value().unwrap_or("")))
                    .collect();
                let mut text = IndexMap::new();
                text.insert("Text".to_string(), tag_string(lines.join("\n")));
                out.insert(be_key.to_string(), NbtTag::Compound(text));
            }
        }
    }

    // Spawners: Java nests the entity id under SpawnData.
    if je_id == "minecraft:mob_spawner"
        && let Some(entity_id) = je
            .get("SpawnData")
            .and_then(|data| data.get("entity"))
            .and_then(|entity| entity.get_str("id"))
    {
        out.insert(
            "EntityIdentifier".to_string(),
            tag_string(crate::mapping::entities::entity_to_bedrock(entity_id)),
        );
    }

    // Jukeboxes store the disc as an item in both editions.
    if je_id == "minecraft:jukebox"
        && let Some(record) = je.get("RecordItem")
        && let Some(stack) = crate::item::read_java_item(record)
    {
        out.insert(
            "RecordItem".to_string(),
            crate::item::write_bedrock_item(&stack),
        );
    }

    // Beehives keep their bee list, with entity data translated best-effort.
    if let Some(bees) = je.get_list("Bees") {
        let mut converted = Vec::new();
        for bee in bees {
            let mut entry = IndexMap::new();
            if let Some(ticks) = bee.get_int("ticks_in_hive") {
                entry.insert("TicksInHive".to_string(), NbtTag::Int(ticks as i32));
            }
            if let Some(ticks) = bee.get_int("min_ticks_in_hive") {
                entry.insert("MinOccupationTicks".to_string(), NbtTag::Int(ticks as i32));
            }
            if let Some(entity) = bee.get("entity_data") {
                entry.insert("EntityData".to_string(), entity.clone());
            }
            converted.push(NbtTag::Compound(entry));
        }
        out.insert("Bees".to_string(), NbtTag::List(converted));
    }

    for field in SHARED_FIELDS {
        if let Some(value) = je.get(field) {
            // Signs and spawners already consumed the equivalent fields.
            if *field == "Delay" && je_id == "minecraft:mob_spawner" {
                // Same name on both sides, keep it.
            }
            out.insert((*field).to_string(), value.clone());
        }
    }
    // Item stacks nested in JE components (`Book` on lecterns).
    if let Some(book) = out.get("Book").cloned()
        && let Some(stack) = crate::item::read_java_item(&book)
    {
        out.insert("Book".to_string(), crate::item::write_bedrock_item(&stack));
    }
    report.counters().block_entities += 1;
    Some(out)
}

/// Translates one Bedrock block entity payload into a Java one.
#[must_use]
pub fn bedrock_to_java(be: &NbtTag, report: &mut ReportBuilder) -> Option<NbtTag> {
    let be_id = be.get_str("id")?;
    let je_id = block_entity_id_to_java(be_id);
    let Some(je_id) = je_id else {
        report.warn(
            crate::report::WarningCode::UnmappedEntity,
            be_id.to_string(),
            format!("block entity {be_id} has no Java mapping; it was skipped"),
        );
        return None;
    };
    let mut out = IndexMap::new();
    out.insert("id".to_string(), tag_string(je_id.clone()));
    for axis in ["x", "y", "z"] {
        if let Some(value) = be.get(axis) {
            out.insert(axis.to_string(), value.clone());
        }
    }

    if let Some(items) = be.get_list("Items") {
        let stacks = read_container_items(items, false);
        out.insert("Items".to_string(), java_items_list(&stacks));
    }

    if je_id == "minecraft:sign" || je_id == "minecraft:hanging_sign" {
        for (be_key, je_key) in [("FrontText", "front_text"), ("BackText", "back_text")] {
            if let Some(side) = be.get(be_key)
                && let Some(text) = side.get_str("Text")
            {
                let messages: Vec<NbtTag> = text
                    .split('\n')
                    .map(|line| tag_string(crate::item::json_text(line)))
                    .collect();
                let mut side_tag = IndexMap::new();
                side_tag.insert("messages".to_string(), NbtTag::List(messages));
                out.insert(je_key.to_string(), NbtTag::Compound(side_tag));
            }
        }
    }

    if je_id == "minecraft:mob_spawner"
        && let Some(entity_id) = be.get_str("EntityIdentifier")
    {
        out.insert(
            "SpawnData".to_string(),
            compound([(
                "entity",
                compound([(
                    "id",
                    tag_string(crate::mapping::entities::entity_to_java(entity_id)),
                )]),
            )]),
        );
    }

    if je_id == "minecraft:jukebox"
        && let Some(record) = be.get("RecordItem")
        && let Some(stack) = crate::item::read_bedrock_item(record)
    {
        out.insert(
            "RecordItem".to_string(),
            crate::item::write_java_item(&stack),
        );
    }

    if let Some(bees) = be.get_list("Bees") {
        let mut converted = Vec::new();
        for bee in bees {
            let mut entry = IndexMap::new();
            if let Some(ticks) = bee.get_int("TicksInHive") {
                entry.insert("ticks_in_hive".to_string(), NbtTag::Int(ticks as i32));
            }
            if let Some(ticks) = bee.get_int("MinOccupationTicks") {
                entry.insert("min_ticks_in_hive".to_string(), NbtTag::Int(ticks as i32));
            }
            if let Some(entity) = bee.get("EntityData") {
                entry.insert("entity_data".to_string(), entity.clone());
            }
            converted.push(NbtTag::Compound(entry));
        }
        out.insert("Bees".to_string(), NbtTag::List(converted));
    }

    for field in SHARED_FIELDS {
        if let Some(value) = be.get(field) {
            out.insert((*field).to_string(), value.clone());
        }
    }
    if let Some(book) = out.get("Book").cloned()
        && let Some(stack) = crate::item::read_bedrock_item(&book)
    {
        out.insert("Book".to_string(), crate::item::write_java_item(&stack));
    }
    report.counters().block_entities += 1;
    Some(NbtTag::Compound(out))
}

/// Converts a Java item list in a block entity to `(slot, stack)` pairs.
#[must_use]
pub fn container_stacks(list: &[NbtTag], java: bool) -> Vec<(i32, ItemStack)> {
    read_container_items(list, java)
}

/// Marker used by the writers to keep an empty tag shape.
#[must_use]
pub fn empty_payload() -> NbtTag {
    empty_compound()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::item::write_java_item;
    use crate::nbt::tag_list;

    #[test]
    fn chest_items_and_fields_convert_both_ways() {
        let mut report =
            crate::report::ReportBuilder::new("java", "bedrock", "test", "test");
        let je = compound([
            ("id", tag_string("minecraft:chest")),
            ("x", NbtTag::Int(10)),
            ("y", NbtTag::Int(64)),
            ("z", NbtTag::Int(-3)),
            ("CustomName", tag_string("\"Storage\"")),
            (
                "Items",
                tag_list(vec![{
                    let mut item = match write_java_item(&ItemStack {
                        id: "minecraft:diamond".to_string(),
                        count: 5,
                        ..ItemStack::default()
                    }) {
                        NbtTag::Compound(entries) => entries,
                        _ => IndexMap::new(),
                    };
                    item.insert("Slot".to_string(), NbtTag::Byte(3));
                    NbtTag::Compound(item)
                }]),
            ),
        ]);
        let be = java_to_bedrock(&je, &mut report).expect("chest converts");
        assert_eq!(be.get_str("id"), Some("Chest"));
        assert_eq!(be.get_str("CustomName"), Some("\"Storage\""));
        let items = be.get_list("Items").expect("items");
        assert_eq!(items.len(), 1);
        let back = bedrock_to_java(&NbtTag::Compound(be), &mut report).expect("chest returns");
        assert_eq!(back.get_str("id"), Some("minecraft:chest"));
        let stacks = container_stacks(back.get_list("Items").expect("items"), true);
        assert_eq!(stacks[0].0, 3);
        assert_eq!(stacks[0].1.id, "minecraft:diamond");
        assert_eq!(stacks[0].1.count, 5);
    }

    #[test]
    fn sign_text_joins_and_splits_lines() {
        let mut report =
            crate::report::ReportBuilder::new("java", "bedrock", "test", "test");
        let je = compound([
            ("id", tag_string("minecraft:sign")),
            ("x", NbtTag::Int(0)),
            ("y", NbtTag::Int(70)),
            ("z", NbtTag::Int(0)),
            (
                "front_text",
                compound([(
                    "messages",
                    tag_list(vec![
                        tag_string("\"Line 1\""),
                        tag_string("\"Line 2\""),
                        tag_string("\"\""),
                        tag_string("\"\""),
                    ]),
                )]),
            ),
        ]);
        let be = java_to_bedrock(&je, &mut report).expect("sign converts");
        let front = be.get("FrontText").expect("front text");
        assert_eq!(front.get_str("Text"), Some("Line 1\nLine 2\n\n"));
        let back = bedrock_to_java(&NbtTag::Compound(be), &mut report).expect("sign returns");
        let messages = back
            .get("front_text")
            .and_then(|side| side.get_list("messages"))
            .expect("messages");
        assert_eq!(messages.len(), 4);
        assert_eq!(
            messages[0].as_str_value().map(crate::item::plain_text),
            Some("Line 1".to_string())
        );
    }

    #[test]
    fn spawner_entity_id_round_trips() {
        let mut report =
            crate::report::ReportBuilder::new("java", "bedrock", "test", "test");
        let je = compound([
            ("id", tag_string("minecraft:mob_spawner")),
            ("x", NbtTag::Int(0)),
            ("y", NbtTag::Int(64)),
            ("z", NbtTag::Int(0)),
            (
                "SpawnData",
                compound([("entity", compound([("id", tag_string("minecraft:zombie"))]))]),
            ),
        ]);
        let be = java_to_bedrock(&je, &mut report).expect("spawner converts");
        assert_eq!(be.get_str("EntityIdentifier"), Some("minecraft:zombie"));
        let back = bedrock_to_java(&NbtTag::Compound(be), &mut report).expect("spawner returns");
        assert_eq!(
            back.get("SpawnData")
                .and_then(|data| data.get("entity"))
                .and_then(|entity| entity.get_str("id")),
            Some("minecraft:zombie")
        );
    }
}
