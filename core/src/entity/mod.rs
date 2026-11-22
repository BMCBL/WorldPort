//! Entity translation between Java and Bedrock.
//!
//! Identifiers come from [`crate::mapping::entities`]. Position and motion are
//! doubles in Java and floats in Bedrock; equipment lists become Bedrock's
//! `Mainhand`/`Offhand`/`Armor` slots; status effects move between Java's string
//! ids and Bedrock's numeric ids. Everything else is copied when both editions
//! use the same field name, and reported when it cannot be expressed.

use indexmap::IndexMap;

use bedrock_nbt::NbtTag;

use crate::item::{read_bedrock_item, read_java_item, write_bedrock_item, write_java_item};
use crate::mapping::effects::{effect_id, effect_name};
use crate::mapping::entities::{entity_to_bedrock, entity_to_java};
use crate::nbt::{NbtExt, TagValue, compound, empty_compound, tag_list, tag_string};
use crate::report::{ReportBuilder, WarningCode};

/// Fields copied between editions without translation.
const SHARED_FIELDS: &[&str] = &[
    "Silent",
    "NoGravity",
    "Invulnerable",
    "PersistenceRequired",
    "Glowing",
    "Fire",
    "Air",
    "FallDistance",
    "OnGround",
    "Saddle",
    "Tame",
    "Owner",
    "Leash",
    "Sheared",
    "Color",
    "Variant",
    "CollarColor",
    "Trusting",
    "Sitting",
    "EatingHaystack",
    "ChestedHorse",
    "Angry",
    "Strength",
    "CarriedBlockState",
    "ItemRotation",
    "ItemDropChance",
    "Facing",
    "Direction",
    "Motive",
    "ExplosionRadius",
    "Fuse",
    "BlockState",
    "poke",
    "Offers",
    "CanPickUpLoot",
    "LeftHanded",
];

/// Java-only entity fields that must not be copied to Bedrock.
const JAVA_ONLY: &[&str] = &[
    "id",
    "UUID",
    "WorldUUIDMost",
    "WorldUUIDLeast",
    "Pos",
    "Motion",
    "ArmorItems",
    "HandItems",
    "active_effects",
    "ActiveEffects",
    "Age",
    "CustomName",
    "CustomNameVisible",
    "Health",
    "Item",
    "Equipment",
    "Brain",
    "Memories",
    "needsFirstTick",
    "DataVersion",
    "PortalCooldown",
    "VillagerData",
];

/// Bedrock-only entity fields that must not be copied to Java.
const BEDROCK_ONLY: &[&str] = &[
    "identifier",
    "internalComponents",
    "definitions",
    "InternalComponents",
    "UniqueID",
    "Pos",
    "Motion",
    "Armor",
    "Mainhand",
    "Offhand",
    "ActiveEffects",
    "IsBaby",
    "CustomName",
    "CustomNameVisible",
    "Health",
    "Item",
    "BoundingBox",
    "CanClimb",
    "CanShowName",
    "IsAutonomous",
    "IsPersistent",
    "IsShaking",
    "LimitedLife",
    "Surface",
    "buoyancy",
    "TargetID",
    "VillagerV2",
    "Offers",
];

/// Converts a Java entity into a Bedrock entity payload.
///
/// `uid` becomes `UniqueID` and must be unique per world.
#[must_use]
pub fn java_to_bedrock(je: &NbtTag, uid: i64, report: &mut ReportBuilder) -> Option<NbtTag> {
    let je_id = je.get_str("id")?;
    let be_id = entity_to_bedrock(je_id);
    let mut out = IndexMap::new();
    out.insert("identifier".to_string(), tag_string(be_id.clone()));
    out.insert("UniqueID".to_string(), NbtTag::Long(uid));
    out.insert(
        "definitions".to_string(),
        tag_list(Vec::new()),
    );
    if let Some(pos) = je.get_list("Pos") {
        out.insert("Pos".to_string(), float_list(pos));
    }
    if let Some(motion) = je.get_list("Motion") {
        out.insert("Motion".to_string(), float_list(motion));
    }
    if let Some(rotation) = je.get_list("Rotation") {
        out.insert("Rotation".to_string(), NbtTag::List(rotation.to_vec()));
    }
    if let Some(health) = je.get("Health") {
        out.insert("Health".to_string(), health.clone());
    }
    if let Some(name) = je.get_str("CustomName") {
        out.insert(
            "CustomName".to_string(),
            tag_string(crate::item::plain_text(name)),
        );
    }
    if let Some(visible) = je.get("CustomNameVisible") {
        out.insert("CustomNameVisible".to_string(), visible.clone());
    }
    if let Some(age) = je.get_int("Age") {
        out.insert("Age".to_string(), NbtTag::Short(i16::try_from(age).unwrap_or(0)));
        if age < 0 {
            out.insert("IsBaby".to_string(), NbtTag::Byte(1));
        }
    }
    if let Some(items) = je.get_list("HandItems") {
        // Java order: main hand, off hand. Real Bedrock saves store each hand as a
        // one-element list, so follow that shape.
        if let Some(main) = items.first()
            && let Some(stack) = read_java_item(main)
        {
            out.insert(
                "Mainhand".to_string(),
                tag_list(vec![write_bedrock_item(&stack)]),
            );
        }
        if let Some(off) = items.get(1)
            && let Some(stack) = read_java_item(off)
        {
            out.insert(
                "Offhand".to_string(),
                tag_list(vec![write_bedrock_item(&stack)]),
            );
        }
    }
    if let Some(items) = je.get_list("ArmorItems")
        && let Ok(armor) = armor_list(items)
    {
        out.insert("Armor".to_string(), armor);
    }
    if let Some(item) = je.get("Item")
        && let Some(stack) = read_java_item(item)
    {
        out.insert("Item".to_string(), write_bedrock_item(&stack));
    }
    if let Some(effects) = read_java_effects(je) {
        out.insert("ActiveEffects".to_string(), effects);
    }

    for field in SHARED_FIELDS {
        if let Some(value) = je.get(field)
            && !out.contains_key(*field)
        {
            out.insert((*field).to_string(), value.clone());
        }
    }
    for (key, value) in je.as_compound().into_iter().flatten() {
        if JAVA_ONLY.contains(&key.as_str())
            || SHARED_FIELDS.contains(&key.as_str())
            || out.contains_key(key)
        {
            continue;
        }
        // Copy remaining scalar/list fields; nested Bedrock-incompatible shapes
        // are reported so the conversion summary is honest.
        match value {
            NbtTag::Compound(_) => {
                report.warn(
                    WarningCode::DroppedField,
                    key.clone(),
                    format!("entity field {key} is a compound with different shapes per edition"),
                );
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }

    // Bedrock links the actor record to its storage key; UniqueID is a separate
    // runtime id, as observed in vanilla saves.
    let storage_key = uid.to_be_bytes().to_vec();
    out.insert(
        "internalComponents".to_string(),
        compound([(
            "EntityStorageKeyComponent",
            compound([("StorageKey", NbtTag::ByteString(storage_key))]),
        )]),
    );
    if let Some(attributes) = write_bedrock_attributes(je) {
        out.insert("Attributes".to_string(), attributes);
    }

    if let Some(villager) = je.get("VillagerData") {
        let profession = villager.get_str("profession").unwrap_or("minecraft:none");
        report.warn(
            WarningCode::Lossy,
            "villager-trades",
            format!("villager profession {profession} is mapped best-effort on Bedrock"),
        );
    }
    report.counters().entities += 1;
    Some(NbtTag::Compound(out))
}

/// Converts a Bedrock entity payload into a Java entity.
#[must_use]
pub fn bedrock_to_java(be: &NbtTag, report: &mut ReportBuilder) -> Option<NbtTag> {
    let identifier = be.get_str("identifier")?;
    let je_id = entity_to_java(identifier);
    let mut out = IndexMap::new();
    out.insert("id".to_string(), tag_string(je_id));
    if let Some(pos) = be.get_list("Pos") {
        out.insert("Pos".to_string(), double_list(pos));
    }
    if let Some(motion) = be.get_list("Motion") {
        out.insert("Motion".to_string(), double_list(motion));
    }
    if let Some(rotation) = be.get_list("Rotation") {
        out.insert("Rotation".to_string(), NbtTag::List(rotation.to_vec()));
    }
    if let Some(health) = be.get("Health") {
        out.insert("Health".to_string(), health.clone());
    }
    if let Some(name) = be.get_str("CustomName") {
        out.insert(
            "CustomName".to_string(),
            tag_string(crate::item::json_text(name)),
        );
    }
    if let Some(visible) = be.get("CustomNameVisible") {
        out.insert("CustomNameVisible".to_string(), visible.clone());
    }
    if be.get_bool("IsBaby").unwrap_or(false) {
        out.insert("Age".to_string(), NbtTag::Int(-24000));
    }
    if be.get("Mainhand").is_some() || be.get("Offhand").is_some() {
        let hands = vec![
            read_bedrock_hand(be.get("Mainhand"))
                .map_or_else(empty_compound, |stack| write_java_item(&stack)),
            read_bedrock_hand(be.get("Offhand"))
                .map_or_else(empty_compound, |stack| write_java_item(&stack)),
        ];
        out.insert("HandItems".to_string(), tag_list(hands));
    }
    if let Some(armor) = be.get_list("Armor") {
        let items: Vec<NbtTag> = armor
            .iter()
            .map(|entry| {
                read_bedrock_item(entry).map_or_else(empty_compound, |stack| write_java_item(&stack))
            })
            .collect();
        let mut items = items;
        while items.len() < 4 {
            items.push(empty_compound());
        }
        out.insert("ArmorItems".to_string(), tag_list(items));
    }
    if let Some(item) = be.get("Item")
        && let Some(stack) = read_bedrock_item(item)
    {
        out.insert("Item".to_string(), write_java_item(&stack));
    }
    if !out.contains_key("Health")
        && let Some(health) = bedrock_health(be)
    {
        out.insert("Health".to_string(), NbtTag::Float(health));
    }
    if let Some(attributes) = write_java_attributes(be) {
        out.insert("attributes".to_string(), attributes);
    }
    if let Some(effects) = write_java_effects(be) {
        out.insert("active_effects".to_string(), effects);
    }
    // Java needs a UUID; derive a stable one from the Bedrock UniqueID.
    if let Some(uid) = be.get_int("UniqueID") {
        let bytes = uid.to_be_bytes();
        out.insert(
            "UUID".to_string(),
            NbtTag::IntArray(vec![
                i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
                i32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
                0,
                0,
            ]),
        );
    }

    for field in SHARED_FIELDS {
        if let Some(value) = be.get(field)
            && !out.contains_key(*field)
        {
            out.insert((*field).to_string(), value.clone());
        }
    }
    for (key, value) in be.as_compound().into_iter().flatten() {
        if BEDROCK_ONLY.contains(&key.as_str())
            || SHARED_FIELDS.contains(&key.as_str())
            || out.contains_key(key)
        {
            continue;
        }
        match value {
            NbtTag::Compound(_) => {
                report.warn(
                    WarningCode::DroppedField,
                    key.clone(),
                    format!("entity field {key} is a compound with different shapes per edition"),
                );
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    report.counters().entities += 1;
    Some(NbtTag::Compound(out))
}

/// Reads a Bedrock hand slot, which vanilla saves store as a one-element list
/// while other writers use a bare compound.
fn read_bedrock_hand(value: Option<&NbtTag>) -> Option<crate::item::ItemStack> {
    match value? {
        NbtTag::List(items) => items.first().and_then(read_bedrock_item),
        other => read_bedrock_item(other),
    }
}

/// Health from Bedrock's attribute list (mobs do not store `Health` directly).
fn bedrock_health(be: &NbtTag) -> Option<f32> {
    let attributes = be.get_list("Attributes")?;
    for attribute in attributes {
        if attribute.get_str("Name") == Some("minecraft:health") {
            let current = attribute
                .get_f64("Current")
                .or_else(|| attribute.get_f64("Base"))?;
            return Some(current as f32);
        }
    }
    None
}

/// Java attributes (new `attributes` list or legacy `Attributes`) → Bedrock list.
fn write_bedrock_attributes(je: &NbtTag) -> Option<NbtTag> {
    let list = je
        .get_list("attributes")
        .or_else(|| je.get_list("Attributes"))?;
    let mut out = Vec::new();
    for attribute in list {
        let Some(name) = attribute.get_str("id").or_else(|| attribute.get_str("Name")) else {
            continue;
        };
        let base = attribute
            .get_f64("base")
            .or_else(|| attribute.get_f64("Base"))
            .unwrap_or(0.0);
        let mut entry = IndexMap::new();
        entry.insert("Name".to_string(), tag_string(name));
        entry.insert("Base".to_string(), NbtTag::Float(base as f32));
        entry.insert("Current".to_string(), NbtTag::Float(base as f32));
        entry.insert("DefaultMax".to_string(), NbtTag::Float(base.max(0.0) as f32 + 1024.0));
        entry.insert("DefaultMin".to_string(), NbtTag::Float(0.0));
        entry.insert("Max".to_string(), NbtTag::Float(base.max(0.0) as f32 + 1024.0));
        entry.insert("Min".to_string(), NbtTag::Float(0.0));
        out.push(NbtTag::Compound(entry));
    }
    if out.is_empty() {
        return None;
    }
    // Java stores current health separately; carry it into the health attribute.
    if let Some(health) = je.get_f64("Health")
        && let Some(entry) = out.iter_mut().find(|entry| {
            entry.get_str("Name") == Some("minecraft:health")
        })
    {
        entry.set("Current", NbtTag::Float(health as f32));
    }
    Some(tag_list(out))
}

/// Bedrock attributes → Java `attributes` list.
fn write_java_attributes(be: &NbtTag) -> Option<NbtTag> {
    let list = be.get_list("Attributes")?;
    let entries: Vec<NbtTag> = list
        .iter()
        .filter_map(|attribute| {
            let name = attribute.get_str("Name")?;
            let base = attribute
                .get_f64("Base")
                .or_else(|| attribute.get_f64("Current"))
                .unwrap_or(0.0);
            Some(compound([
                (
                    "id",
                    tag_string(name.replace("minecraft:health", "minecraft:max_health")),
                ),
                ("base", NbtTag::Double(base)),
            ]))
        })
        .collect();
    (!entries.is_empty()).then(|| tag_list(entries))
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

fn armor_list(items: &[NbtTag]) -> Result<NbtTag, ()> {
    let converted: Vec<NbtTag> = items
        .iter()
        .map(|entry| {
            read_java_item(entry).map_or_else(empty_compound, |stack| write_bedrock_item(&stack))
        })
        .collect();
    Ok(tag_list(converted))
}

fn read_java_effects(je: &NbtTag) -> Option<NbtTag> {
    let list = je
        .get_list("active_effects")
        .or_else(|| je.get_list("ActiveEffects"))?;
    let mut effects = Vec::new();
    for effect in list {
        let id = effect
            .get_str("id")
            .and_then(effect_id)
            .or_else(|| effect.get_int("Id").and_then(|id| u16::try_from(id).ok()));
        let Some(id) = id else {
            continue;
        };
        let mut entry = IndexMap::new();
        entry.insert("Id".to_string(), NbtTag::Byte(i8::try_from(id).unwrap_or(0)));
        entry.insert(
            "Amplifier".to_string(),
            NbtTag::Byte(effect.get_int("amplifier").or_else(|| effect.get_int("Amplifier")).unwrap_or(0) as i8),
        );
        entry.insert(
            "Duration".to_string(),
            NbtTag::Int(effect.get_int("duration").or_else(|| effect.get_int("Duration")).unwrap_or(0) as i32),
        );
        effects.push(NbtTag::Compound(entry));
    }
    (!effects.is_empty()).then(|| tag_list(effects))
}

fn write_java_effects(be: &NbtTag) -> Option<NbtTag> {
    let list = be.get_list("ActiveEffects")?;
    let mut effects = Vec::new();
    for effect in list {
        let Some(id) = effect
            .get_int("Id")
            .and_then(|id| u16::try_from(id).ok())
            .and_then(effect_name)
        else {
            continue;
        };
        effects.push(compound([
            ("id", tag_string(id)),
            (
                "amplifier",
                NbtTag::Int(effect.get_int("Amplifier").unwrap_or(0) as i32),
            ),
            (
                "duration",
                NbtTag::Int(effect.get_int("Duration").unwrap_or(0) as i32),
            ),
            ("show_particles", NbtTag::Byte(1)),
            ("show_icon", NbtTag::Byte(1)),
            ("ambient", NbtTag::Byte(0)),
        ]));
    }
    (!effects.is_empty()).then(|| tag_list(effects))
}

/// Extracts the chunk position an entity belongs to from its `Pos`.
#[must_use]
pub fn chunk_of(nbt: &NbtTag) -> Option<(i32, i32)> {
    let pos = nbt.get_list("Pos")?;
    let x = pos.first()?.as_f64_value()?;
    let z = pos.get(2)?.as_f64_value()?;
    Some((
        (x.floor() as i32) >> 4,
        (z.floor() as i32) >> 4,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> ReportBuilder {
        ReportBuilder::new("java", "bedrock", "test", "test")
    }

    #[test]
    fn zombie_with_equipment_round_trips() {
        let je = compound([
            ("id", tag_string("minecraft:zombie")),
            (
                "Pos",
                tag_list(vec![
                    NbtTag::Double(10.5),
                    NbtTag::Double(64.0),
                    NbtTag::Double(-3.25),
                ]),
            ),
            (
                "Motion",
                tag_list(vec![NbtTag::Double(0.0), NbtTag::Double(-0.08), NbtTag::Double(0.0)]),
            ),
            ("Rotation", tag_list(vec![NbtTag::Float(90.0), NbtTag::Float(0.0)])),
            ("Health", NbtTag::Float(20.0)),
            ("Age", NbtTag::Int(-24000)),
            (
                "HandItems",
                tag_list(vec![
                    crate::item::write_java_item(&crate::item::ItemStack {
                        id: "minecraft:iron_sword".to_string(),
                        count: 1,
                        ..crate::item::ItemStack::default()
                    }),
                    empty_compound(),
                ]),
            ),
            (
                "ArmorItems",
                tag_list(vec![
                    crate::item::write_java_item(&crate::item::ItemStack {
                        id: "minecraft:iron_helmet".to_string(),
                        count: 1,
                        ..crate::item::ItemStack::default()
                    }),
                    empty_compound(),
                    empty_compound(),
                    empty_compound(),
                ]),
            ),
        ]);
        let mut report = report();
        let be = java_to_bedrock(&je, 42, &mut report).expect("entity converts");
        assert_eq!(be.get_str("identifier"), Some("minecraft:zombie"));
        assert_eq!(be.get_int("UniqueID"), Some(42));
        assert_eq!(be.get_bool("IsBaby"), Some(true));
        // Bedrock saves store each hand as a one-element list.
        let bedrock_nbt::NbtTag::List(mainhand) = be.get("Mainhand").expect("mainhand") else {
            panic!("mainhand must be a list");
        };
        assert_eq!(mainhand[0].get_str("Name"), Some("minecraft:iron_sword"));
        let NbtTag::List(armor) = be.get("Armor").expect("armor list") else {
            panic!("armor must be a list");
        };
        assert_eq!(armor.len(), 4);
        assert_eq!(
            armor[0].get_str("Name"),
            Some("minecraft:iron_helmet")
        );
        let back = bedrock_to_java(&be, &mut report).expect("entity returns");
        assert_eq!(back.get_str("id"), Some("minecraft:zombie"));
        let pos = back.get_list("Pos").expect("pos");
        assert_eq!(pos[0].as_f64_value(), Some(10.5));
        assert_eq!(back.get_int("Age"), Some(-24000));
        assert_eq!(
            back.get_list("HandItems")
                .and_then(|items| items.first())
                .and_then(|item| item.get_str("id")),
            Some("minecraft:iron_sword")
        );
    }

    #[test]
    fn experience_orb_uses_bedrock_identifier() {
        let je = compound([
            ("id", tag_string("minecraft:experience_orb")),
            ("Pos", tag_list(vec![NbtTag::Double(0.0), NbtTag::Double(64.0), NbtTag::Double(0.0)])),
        ]);
        let be = java_to_bedrock(&je, 7, &mut report()).expect("orb converts");
        assert_eq!(be.get_str("identifier"), Some("minecraft:xp_orb"));
    }

    #[test]
    fn chunk_of_uses_block_position() {
        let nbt = compound([(
            "Pos",
            tag_list(vec![NbtTag::Double(-1.0), NbtTag::Double(64.0), NbtTag::Double(33.0)]),
        )]);
        assert_eq!(chunk_of(&nbt), Some((-1, 2)));
    }
}
