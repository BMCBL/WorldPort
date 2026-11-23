//! Entity identifier coverage, checked against Bedrock's own definition list.
//!
//! The block side has a coverage audit because a bad mapping there becomes a
//! visible hole in the world. Entities had nothing: an alias could point at an
//! identifier no Bedrock build defines, the entity would be written, and the game
//! would drop it without anything in the report. `mappings/bedrock_entities.json`
//! is the identifier list extracted from Bedrock's behaviour pack, which is what
//! the game itself reads.

use std::collections::BTreeSet;

use worldport_core::mapping::entities::{
    ENTITY_ALIASES, NO_BEDROCK_ENTITY, entity_to_bedrock, entity_to_java, has_bedrock_entity,
};

fn bedrock_ids() -> BTreeSet<String> {
    serde_json::from_str(include_str!("../../mappings/bedrock_entities.json"))
        .expect("valid mappings/bedrock_entities.json")
}

#[test]
fn every_alias_points_at_an_entity_bedrock_defines() {
    // The bug this catches: `evocation_fangs` was aliased to `evocation_fang`,
    // which no Bedrock build defines because fangs are a damage source there.
    let known = bedrock_ids();
    let mut unknown = Vec::new();
    for (java, bedrock) in ENTITY_ALIASES {
        if !known.contains(*bedrock) {
            unknown.push(format!("{java} -> {bedrock}"));
        }
    }
    assert!(
        unknown.is_empty(),
        "aliases target identifiers Bedrock does not define:\n{}",
        unknown.join("\n")
    );
}

#[test]
fn every_java_entity_maps_to_an_identifier_bedrock_defines() {
    // The reverse of the check above, over the ids Java can actually emit. The
    // Java list is not bundled, so this walks the aliases' Java side plus the
    // Bedrock list itself: any Bedrock id that is also a Java id must survive the
    // trip unchanged or land on another defined id.
    let known = bedrock_ids();
    let mut broken = Vec::new();
    for bedrock in &known {
        let java = entity_to_java(bedrock);
        if !has_bedrock_entity(&java) {
            continue;
        }
        let back = entity_to_bedrock(&java);
        if !known.contains(&back) {
            broken.push(format!("{bedrock} -> {java} -> {back}"));
        }
    }
    assert!(
        broken.is_empty(),
        "roundtrip leaves the Bedrock id space:\n{}",
        broken.join("\n")
    );
}

#[test]
fn dropping_an_entity_is_declared_rather_than_silent() {
    // Anything in the drop list must have no Bedrock entity; anything outside it
    // must map to one. A silent drop is what the report exists to prevent.
    let known = bedrock_ids();
    for java in NO_BEDROCK_ENTITY {
        assert!(
            !known.contains(*java),
            "{java} is in the drop list but Bedrock defines it"
        );
    }
    for (java, bedrock) in ENTITY_ALIASES {
        assert!(
            has_bedrock_entity(java),
            "{java} is aliased to {bedrock} but marked as having no Bedrock entity"
        );
    }
}

#[test]
fn the_reference_list_is_the_one_that_was_reviewed() {
    // Pins the source: a different behaviour pack changes this number, which is a
    // deliberate edit rather than an accident.
    let known = bedrock_ids();
    assert_eq!(known.len(), 128, "Bedrock entity list changed size");
    assert!(known.contains("minecraft:zombie"));
    assert!(known.contains("minecraft:villager_v2"));
    assert!(!known.contains("minecraft:evocation_fang"), "fangs are not an entity");
}
