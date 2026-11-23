//! Proves the bundled Bedrock catalogs describe their own version.
//!
//! These come from Chunker's per-build registries, so the assertion is that a
//! block introduced (or removed) in a given release is present (or absent) in that
//! version's catalog - not that the loader reads a file back.

use worldport_core::mapping::schema::{
    bedrock_catalog_at_or_below, bedrock_catalog_for, bedrock_catalog_versions,
};

#[test]
fn every_configured_version_loads() {
    let versions = bedrock_catalog_versions();
    assert_eq!(versions.len(), 59, "every Chunker registry is bundled: {versions:?}");
    assert_eq!(versions.first().copied(), Some("1.12.0.28"));
    assert_eq!(versions.last().copied(), Some("1.26.70.0"));
    for version in &versions {
        let catalog = bedrock_catalog_for(version).unwrap_or_else(|| panic!("{version}"));
        assert!(!catalog.is_empty(), "{version}: empty catalog");
    }
}

#[test]
fn a_catalog_knows_what_its_own_version_introduced() {
    // 1.12 predates bees, so it cannot contain a beehive; 1.14 predates the
    // nether update; copper arrived in 1.17.
    let oldest = bedrock_catalog_for("1.12.0.28").expect("1.12");
    assert!(!oldest.contains("beehive"), "1.12 predates beehives");
    assert!(!oldest.contains("copper_block"), "1.12 predates copper");
    assert!(oldest.contains("carpet"), "1.12 spells wool carpet `carpet`");

    let fourteen = bedrock_catalog_for("1.14.20.1").expect("1.14");
    assert!(!fourteen.contains("crimson_stem"), "1.14 predates the nether update");

    let seventeen = bedrock_catalog_for("1.17.41.01").expect("1.17");
    assert!(seventeen.contains("copper_block"), "1.17 introduced copper");

    // Bedrock renamed the plain chain once copper chains existed.
    let modern = bedrock_catalog_for("1.26.60.0").expect("1.26.60");
    assert!(modern.contains("iron_chain"), "1.26 names it iron_chain");
    assert!(!modern.contains("chain"), "1.26 dropped the plain chain name");
}

#[test]
fn catalogs_grow_with_the_game() {
    let versions = bedrock_catalog_versions();
    let mut previous = 0_usize;
    for version in &versions {
        let catalog = bedrock_catalog_for(version).expect("catalog");
        assert!(
            catalog.len() >= previous,
            "{version}: {} blocks, fewer than the previous version's {previous}",
            catalog.len()
        );
        previous = catalog.len();
    }
}

#[test]
fn at_or_below_picks_the_closest_catalog() {
    // A build between two bundled versions resolves to the older one, because the
    // newer catalog may contain ids the world cannot.
    let catalog = bedrock_catalog_at_or_below("1.21.0.03").expect("between 1.20 and 1.21.60");
    assert!(!catalog.contains("iron_chain"), "a 1.21.0 world has no iron_chain");
    assert!(catalog.contains("copper_block"));
    assert!(bedrock_catalog_at_or_below("1.11.0.0").is_none(), "nothing predates the oldest");
    assert!(bedrock_catalog_at_or_below("1.26.60.0").is_some());
}
