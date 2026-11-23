//! Coverage gate over every block state of every bundled version.
//!
//! `version_matrix` proves the states a fixture uses are fine. This proves what
//! happens to the tens of thousands it does not use: each state is mapped and
//! mapped back, and the result is sorted into exact, declared loss, silent loss or
//! unmapped.
//!
//! The bar is semantic, not byte equality. The editions keep different fields,
//! defaults and derived state, so a state that cannot survive unchanged is allowed
//! to be lost - but only if the mapper says so. The counts are compared against
//! `mappings/coverage_baseline.json`, so a change has to be deliberate: a
//! regression fails, and an improvement passes without touching the file.

use std::collections::BTreeMap;

use worldport_core::mapping::audit::{Coverage, Outcome, audit};
use worldport_core::mapping::schema::{
    bedrock_catalog, bedrock_catalog_for, bedrock_catalog_versions,
};
use worldport_core::mapping::{BlockMapper, JavaCatalog};

/// Recorded coverage, keyed `"<version> <direction>"`.
fn baseline() -> BTreeMap<String, Recorded> {
    serde_json::from_str(include_str!("../../mappings/coverage_baseline.json"))
        .expect("valid mappings/coverage_baseline.json")
}

#[derive(Debug, serde::Deserialize)]
struct Recorded {
    states: u64,
    exact: u64,
    declared_loss: u64,
    silent_loss: u64,
    unstable: u64,
    unmapped: u64,
    /// States whose produced block or property the target catalog does not
    /// declare. Recorded but not gated: the current population is dominated by
    /// `waterlogged` (a liquid layer in Bedrock, a property in Java) and double
    /// slabs (separate blocks in Bedrock), which are modelling differences that
    /// need triage per family before they can be asserted on.
    #[serde(default)]
    invalid_target: u64,
}

fn record(coverage: &Coverage) -> Recorded {
    Recorded {
        states: coverage.states,
        exact: coverage.exact(),
        declared_loss: coverage.declared_losses(),
        silent_loss: coverage.count(&Outcome::SilentLoss),
        unstable: coverage.count(&Outcome::Unstable),
        unmapped: coverage.count(&Outcome::Unmapped),
        invalid_target: coverage.count(&Outcome::InvalidTarget),
    }
}

fn coverage() -> BTreeMap<String, Recorded> {
    let bedrock = bedrock_catalog();
    let mut rows = BTreeMap::new();
    for (version, _) in JavaCatalog::bundled_versions() {
        let java = JavaCatalog::bundled(version).expect("bundled catalog").to_catalog();
        let mapper = BlockMapper::new(java.clone(), bedrock.clone());
        rows.insert(
            format!("{version} JE->BE"),
            record(&audit(&mapper, &java, &bedrock, true)),
        );
        // The embedded snapshot, measured in the direction a Java target reads it.
        rows.insert(
            format!("{version} BE->JE"),
            record(&audit(&mapper, &bedrock, &java, false)),
        );
    }
    // Bedrock is measured once per bundled release: a world saved by an older
    // build has ids the newest snapshot never listed, and that is exactly the
    // path a version-agnostic mapper gets wrong.
    // These measure *reading* a world of that release, which is the path a user
    // with an old save takes. The reverse direction there would target a catalog
    // we never write to, so instability is not meaningful for these rows; what
    // matters is that every produced Java state exists in the target version.
    for version in bedrock_catalog_versions() {
        let catalog = bedrock_catalog_for(version).expect("bundled bedrock catalog");
        let java = JavaCatalog::bundled("26.2").expect("newest bundled Java catalog").to_catalog();
        let mapper = BlockMapper::new(java.clone(), catalog.clone());
        rows.insert(
            format!("be {version} BE->JE"),
            record(&audit(&mapper, &catalog, &java, false)),
        );
        // Writing into that release: what a Java world would lose. Recorded so the
        // cost of a downgrade cannot grow unnoticed; it is never zero, because
        // older releases genuinely lack blocks.
        rows.insert(
            format!("to be {version} JE->BE"),
            record(&audit(&mapper, &java, &catalog, true)),
        );
    }
    rows
}

#[test]
fn no_java_state_is_unmapped_or_unstable() {
    // Every block a Java world can hold has to reach Bedrock: a gap here is a
    // silently dropped block, which is the failure mode players notice first.
    for (key, row) in coverage() {
        // A downgrade row targets an older release on purpose, so unmapped states
        // are the point of measuring it; only the current target must be complete.
        if !key.ends_with("JE->BE") || key.starts_with("to be ") {
            continue;
        }
        assert_eq!(row.unmapped, 0, "{key}: {row:?}");
        assert_eq!(row.unstable, 0, "{key}: a mapped state must map back: {row:?}");
    }
}

#[test]
fn no_bedrock_state_is_unstable() {
    // Bedrock has blocks Java never gained, so unmapped is expected there; what
    // is not acceptable is a state that maps to something which cannot map back.
    for (key, row) in coverage() {
        if !key.ends_with("BE->JE") {
            // Covers "<je version> BE->JE" and the cross-version "be <v> BE->JE".
            continue;
        }
        // A cross-version row reads a world of that release; its reverse direction
        // would target a catalog we never write to, so only same-catalog rows are
        // held to round-trip stability.
        if key.starts_with("be ") {
            continue;
        }
        assert_eq!(row.unstable, 0, "{key}: {row:?}");
    }
}

#[test]
fn coverage_never_regresses_against_the_baseline() {
    let expected = baseline();
    let actual = coverage();
    let mut problems: Vec<String> = Vec::new();
    for (key, want) in &expected {
        let Some(got) = actual.get(key) else {
            problems.push(format!("{key}: missing from this build"));
            continue;
        };
        if got.states != want.states {
            problems.push(format!(
                "{key}: {} states, baseline {} - the catalog changed size",
                got.states, want.states
            ));
        }
        // More silent losses, more unmapped states or fewer exact states are all
        // regressions. The reverse is an improvement and needs no baseline edit;
        // refreshing the file only has to happen to lock the gain in.
        if got.silent_loss > want.silent_loss {
            problems.push(format!(
                "{key}: {} silent losses, baseline {}",
                got.silent_loss, want.silent_loss
            ));
        }
        if got.unmapped > want.unmapped {
            problems.push(format!(
                "{key}: {} unmapped states, baseline {}",
                got.unmapped, want.unmapped
            ));
        }
        if got.exact < want.exact {
            problems.push(format!(
                "{key}: {} exact states, baseline {} - declared losses grew",
                got.exact, want.exact
            ));
        }
        if got.unstable != want.unstable {
            problems.push(format!(
                "{key}: {} unstable, baseline {} - pinning a regression",
                got.unstable, want.unstable
            ));
        }
    }
    for key in actual.keys() {
        if !expected.contains_key(key) {
            problems.push(format!("{key}: not in the baseline"));
        }
    }
    assert!(problems.is_empty(), "coverage regressions:\n{}", problems.join("\n"));
}

#[test]
fn every_state_lands_in_exactly_one_bucket() {
    // A classification that silently drops a state would make the whole report
    // lie about its own totals.
    for (key, row) in coverage() {
        let summed = row.exact
            + row.declared_loss
            + row.silent_loss
            + row.unstable
            + row.unmapped
            + row.invalid_target;
        assert_eq!(summed, row.states, "{key}: {row:?}");
    }
}
