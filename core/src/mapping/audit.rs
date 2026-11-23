//! Measures how much of a catalog actually converts.
//!
//! A roundtrip test proves the states it happens to use are fine; it says nothing
//! about the other tens of thousands. This walks **every** state a version can
//! express and sorts each one into a bucket, so a regression shows up as a changed
//! count instead of as a bug report from a player.
//!
//! The standard is semantic equivalence, not byte equality: the two editions keep
//! different fields, different defaults and different derived state, so
//! "identical" is the wrong bar. What matters is that a state either survives the
//! roundtrip or is *declared* lossy on the way out. An undeclared loss is a
//! silent one, and that is the number that has to stay at zero.

use std::collections::BTreeMap;

use super::mapper::{BlockMapper, MappedState};
use super::schema::BlockCatalog;

/// Where one source state ended up.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Outcome {
    /// The target state came back unchanged.
    Exact,
    /// The target state came back unchanged and the mapper declared the loss.
    DeclaredExact,
    /// The state did not come back, and the mapper declared it lossy.
    DeclaredChanged,
    /// The target could represent the state exactly, but the reverse mapping lost
    /// it anyway. This is a mapping-table bug, not an inherent difference.
    Unstable,
    /// The state did not come back and nothing was declared: information vanished
    /// without a trace.
    SilentLoss,
    /// No target block at all.
    Unmapped,
    /// A target state was produced, but the target catalog does not declare it.
    ///
    /// Reading an old world can otherwise emit a block or property the target
    /// version never had, which a game would reject rather than approximate.
    InvalidTarget,
}

impl Outcome {
    /// Whether this outcome means data was lost without being declared.
    #[must_use]
    pub const fn is_defect(&self) -> bool {
        matches!(self, Self::Unstable | Self::SilentLoss | Self::InvalidTarget)
    }

    /// The report column this outcome belongs to.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::DeclaredExact => "declared-exact",
            Self::DeclaredChanged => "declared-changed",
            Self::Unstable => "unstable",
            Self::SilentLoss => "silent-loss",
            Self::Unmapped => "unmapped",
            Self::InvalidTarget => "invalid-target",
        }
    }
}

/// One direction's coverage for one version.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Coverage {
    /// States that exist in the source catalog.
    pub states: u64,
    /// Per-bucket counts.
    pub outcomes: BTreeMap<String, u64>,
    /// Blocks with at least one unmapped state, with the number of such states.
    pub unmapped_blocks: BTreeMap<String, u64>,
    /// Examples of undeclared losses, for the report. Bounded: a report that
    /// lists ten thousand rows is one nobody reads.
    pub defects: Vec<String>,
}

impl Coverage {
    /// Count for one outcome.
    #[must_use]
    pub fn count(&self, outcome: &Outcome) -> u64 {
        self.outcomes.get(outcome.label()).copied().unwrap_or(0)
    }

    /// Declared losses only: the mapping did its job and said so.
    #[must_use]
    pub fn declared_losses(&self) -> u64 {
        self.count(&Outcome::DeclaredChanged)
    }

    /// States that survived the roundtrip, exactly.
    #[must_use]
    pub fn exact(&self) -> u64 {
        self.count(&Outcome::Exact) + self.count(&Outcome::DeclaredExact)
    }

    /// Share of source states that convert at all.
    #[must_use]
    pub fn mapped_ratio(&self) -> f64 {
        if self.states == 0 {
            return 1.0;
        }
        let unmapped = self.count(&Outcome::Unmapped);
        (self.states - unmapped) as f64 / self.states as f64
    }
}

/// Most undeclared losses kept per direction in a report.
const MAX_DEFECT_ROWS: usize = 20;

/// Audits every state in a source catalog against a target catalog.
///
/// `properties_of` turns a resolved state into a property map; the enumerator and
/// the mapper have to agree on what "a state" is, which is why the walker is the
/// caller's job and this only classifies.
#[must_use]
pub fn audit(
    mapper: &BlockMapper,
    source: &BlockCatalog,
    target: &BlockCatalog,
    forward: bool,
) -> Coverage {
    let mut coverage = Coverage::default();
    for (name, schema) in source.blocks() {
        let mut values: Vec<(&str, Vec<&str>)> = schema
            .properties()
            .iter()
            .map(|(key, property)| (key.as_str(), property.values().iter().map(String::as_str).collect()))
            .collect();
        // Deterministic order so a report diff shows real changes only.
        values.sort_by(|left, right| left.0.cmp(right.0));

        let mut counters: Vec<usize> = vec![0; values.len()];
        loop {
            let properties: BTreeMap<String, String> = values
                .iter()
                .zip(&counters)
                .map(|((key, options), index)| ((*key).to_string(), options[*index].to_string()))
                .collect();
            let outcome = classify(mapper, name, &properties, target, forward);
            *coverage
                .outcomes
                .entry(outcome.label().to_string())
                .or_default() += 1;
            coverage.states += 1;
            if outcome.is_defect() && coverage.defects.len() < MAX_DEFECT_ROWS {
                coverage
                    .defects
                    .push(format!("{} {:?}: {}", name, properties, outcome.label()));
            }
            if outcome == Outcome::Unmapped {
                *coverage.unmapped_blocks.entry(name.clone()).or_default() += 1;
            }
            // Odometer increment over the cartesian product of property values.
            let mut advanced = false;
            for (index, (_, options)) in values.iter().enumerate() {
                counters[index] += 1;
                if counters[index] < options.len() {
                    advanced = true;
                    break;
                }
                counters[index] = 0;
            }
            if !advanced {
                break;
            }
        }
    }
    coverage
}

/// Classifies one state by converting it and its result.
fn classify(
    mapper: &BlockMapper,
    name: &str,
    properties: &BTreeMap<String, String>,
    target: &BlockCatalog,
    forward: bool,
) -> Outcome {
    let (mapped, back): (Option<MappedState>, Option<MappedState>) = if forward {
        let mapped = mapper.je_to_be(name, properties);
        let back = mapped
            .as_ref()
            .and_then(|state| mapper.be_to_je(&state.name, &state.properties));
        (mapped, back)
    } else {
        let mapped = mapper.be_to_je(name, properties);
        let back = mapped
            .as_ref()
            .and_then(|state| mapper.je_to_be(&state.name, &state.properties));
        (mapped, back)
    };
    let Some(mapped) = mapped else {
        return Outcome::Unmapped;
    };
    if !declares(target, &mapped.name, &mapped.properties) {
        return Outcome::InvalidTarget;
    }
    let Some(back) = back else {
        // A mapped state whose result cannot be mapped at all loses everything
        // the target did express.
        return Outcome::Unstable;
    };
    // The roundtrip must reproduce the source state's name and the properties it
    // declared. A property the target cannot hold is a declared loss, not a
    // failure, which is why `lossy` decides the bucket.
    let same_name = back.name == name;
    let same_properties = properties.iter().all(|(key, value)| {
        // Only compare properties that survived into the target; the rest were
        // dropped on the way and are covered by `lossy`.
        let kept = mapped.properties.contains_key(key)
            || mapped.properties.contains_key(&renamed_key(key));
        !kept || back.properties.get(key).is_some_and(|actual| actual == value)
    });
    let roundtrips = same_name && same_properties;
    match (roundtrips, mapped.lossy) {
        (true, false) => Outcome::Exact,
        (true, true) => Outcome::DeclaredExact,
        (false, true) => Outcome::DeclaredChanged,
        (false, false) => Outcome::SilentLoss,
    }
}

/// Whether `catalog` declares this block and every property value used.
fn declares(catalog: &BlockCatalog, name: &str, properties: &BTreeMap<String, String>) -> bool {
    let Some(schema) = catalog.block(name) else {
        return false;
    };
    properties.iter().all(|(key, value)| {
        if super::mapper::is_catalog_gap(key) {
            return true;
        }
        let Some(property) = schema.property(key) else {
            return false;
        };
        if property.accepts(value) {
            return true;
        }
        // A boolean is written as `true`/`false` here and turned into a Bedrock
        // byte by the writer, so the catalog's `0`/`1` domain still accepts it.
        match value.as_str() {
            "true" | "false" => property.accepts(if value == "true" { "1" } else { "0" }),
            _ => false,
        }
    })
}

/// Property names differ between the editions for the same concept; a quiet
/// mismatch here would be reported as data loss.
fn renamed_key(key: &str) -> String {
    match key {
        "axis" => "pillar_axis".to_string(),
        "facing" => "facing_direction".to_string(),
        other => other.to_string(),
    }
}
