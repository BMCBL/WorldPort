//! Conversion report: counters, aggregated warnings and unmapped identifiers.
//!
//! Warnings are aggregated by `(code, key)` so a world with ten thousand unknown
//! blocks reports one row with a count instead of ten thousand rows.

use std::collections::BTreeMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Maximum distinct warning rows kept in a report.
pub const MAX_WARNING_ROWS: usize = 2048;

/// Kind of conversion problem.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WarningCode {
    /// A Bedrock or Java block state had no mapping.
    UnmappedBlock,
    /// An entity identifier had no mapping.
    UnmappedEntity,
    /// An item identifier had no mapping.
    UnmappedItem,
    /// A biome identifier had no mapping.
    UnmappedBiome,
    /// A field could not be expressed in the target edition.
    DroppedField,
    /// A value was rewritten approximately (lossy).
    Lossy,
    /// Data was dropped on purpose because the target version cannot hold it.
    Pruned,
    /// A chunk or record was skipped.
    Skipped,
    /// A feature is not converted at all.
    UnsupportedFeature,
}

impl WarningCode {
    /// Stable machine-readable code used in reports and the plugin UI.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UnmappedBlock => "unmapped-block",
            Self::UnmappedEntity => "unmapped-entity",
            Self::UnmappedItem => "unmapped-item",
            Self::UnmappedBiome => "unmapped-biome",
            Self::DroppedField => "dropped-field",
            Self::Lossy => "lossy",
            Self::Pruned => "pruned",
            Self::Skipped => "skipped",
            Self::UnsupportedFeature => "unsupported-feature",
        }
    }
}

/// One aggregated warning row.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Warning {
    /// Problem kind.
    pub code: WarningCode,
    /// Stable key (identifier, property or field name).
    pub key: String,
    /// How many times this row was hit.
    pub count: u64,
    /// Human-readable explanation.
    pub message: String,
}

/// Unmapped identifiers grouped by kind.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct UnmappedIds {
    /// Block state descriptions (`name[prop=value]`).
    pub blocks: BTreeMap<String, u64>,
    /// Entity identifiers.
    pub entities: BTreeMap<String, u64>,
    /// Item identifiers.
    pub items: BTreeMap<String, u64>,
    /// Biome identifiers.
    pub biomes: BTreeMap<String, u64>,
}

impl UnmappedIds {
    /// Total number of distinct unmapped identifiers.
    #[must_use]
    pub fn total(&self) -> usize {
        self.blocks.len() + self.entities.len() + self.items.len() + self.biomes.len()
    }
}

/// What was converted.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct Counters {
    /// Chunk columns converted.
    pub chunks: u64,
    /// Non-air blocks written.
    pub blocks: u64,
    /// Block entities written.
    pub block_entities: u64,
    /// Entities written.
    pub entities: u64,
    /// Player files written.
    pub players: u64,
    /// Map items written.
    pub maps: u64,
    /// Subchunks written (Bedrock) or sections written (Java).
    pub sections: u64,
    /// Biome cells written.
    pub biome_cells: u64,
    /// Scheduled ticks written.
    pub ticks: u64,
    /// Village interests (beds, workstations, meeting points) written.
    pub village_interests: u64,
    /// Structure areas found (Bedrock hardcoded spawn areas / Java starts).
    pub structures: u64,
}

impl Counters {
    /// Adds another counter set into this one.
    pub fn merge(&mut self, other: &Self) {
        self.chunks += other.chunks;
        self.blocks += other.blocks;
        self.block_entities += other.block_entities;
        self.entities += other.entities;
        self.players += other.players;
        self.maps += other.maps;
        self.sections += other.sections;
        self.biome_cells += other.biome_cells;
        self.ticks += other.ticks;
        self.village_interests += other.village_interests;
        self.structures += other.structures;
    }
}

/// Timing and counters for one pipeline stage.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StageReport {
    /// Stage name (stable, used by the UI).
    pub name: String,
    /// Wall-clock duration in milliseconds.
    pub elapsed_ms: u64,
    /// Warning rows produced by the stage.
    pub warnings: u64,
}

/// Full conversion report.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConversionReport {
    /// `bedrock` or `java`.
    pub source_edition: String,
    /// `bedrock` or `java`.
    pub target_edition: String,
    /// Source version label (`1.21.4`, `1.21.60`, ...).
    pub source_version: String,
    /// Target version label.
    pub target_version: String,
    /// Source world name.
    pub source_name: String,
    /// Output world directory.
    pub target_path: String,
    /// Unix milliseconds when the conversion started.
    pub started_unix_ms: u64,
    /// Unix milliseconds when the conversion finished.
    pub finished_unix_ms: u64,
    /// Aggregate counters.
    pub counters: Counters,
    /// Aggregated warnings, sorted by count descending.
    pub warnings: Vec<Warning>,
    /// Unmapped identifiers.
    pub unmapped: UnmappedIds,
    /// Per-stage timings.
    pub stages: Vec<StageReport>,
}

impl ConversionReport {
    /// True when nothing had to be dropped or approximated.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.warnings.is_empty() && self.unmapped.total() == 0
    }
}

/// Accumulates a report while converting. Not `Sync`: one builder per worker
/// merges into the parent at the end of a batch.
#[derive(Debug)]
pub struct ReportBuilder {
    report: ConversionReport,
    warning_index: BTreeMap<(WarningCode, String), usize>,
    warning_rows_dropped: u64,
    stage_start: Option<(String, Instant)>,
}

impl ReportBuilder {
    /// Starts a report for a conversion.
    #[must_use]
    pub fn new(
        source_edition: impl Into<String>,
        target_edition: impl Into<String>,
        source_name: impl Into<String>,
        target_path: impl Into<String>,
    ) -> Self {
        Self {
            report: ConversionReport {
                source_edition: source_edition.into(),
                target_edition: target_edition.into(),
                source_name: source_name.into(),
                target_path: target_path.into(),
                started_unix_ms: unix_now_ms(),
                ..ConversionReport::default()
            },
            warning_index: BTreeMap::new(),
            warning_rows_dropped: 0,
            stage_start: None,
        }
    }

    /// Sets source/target version labels.
    pub fn set_versions(&mut self, source: impl Into<String>, target: impl Into<String>) {
        self.report.source_version = source.into();
        self.report.target_version = target.into();
    }

    /// Mutable access to counters.
    pub fn counters(&mut self) -> &mut Counters {
        &mut self.report.counters
    }

    /// Mutable access to unmapped identifiers.
    pub fn unmapped(&mut self) -> &mut UnmappedIds {
        &mut self.report.unmapped
    }

    /// Starts a measured stage, finishing any stage already in progress.
    pub fn begin_stage(&mut self, name: impl Into<String>) {
        self.end_stage();
        self.stage_start = Some((name.into(), Instant::now()));
    }

    /// Finishes the current stage.
    pub fn end_stage(&mut self) {
        if let Some((name, started)) = self.stage_start.take() {
            let warnings = self
                .report
                .warnings
                .iter()
                .map(|row| row.count)
                .sum::<u64>();
            self.report.stages.push(StageReport {
                name,
                elapsed_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                warnings,
            });
        }
    }

    /// Records a warning, aggregating by `(code, key)`.
    pub fn warn(
        &mut self,
        code: WarningCode,
        key: impl Into<String>,
        message: impl Into<String>,
    ) {
        let key = key.into();
        if let Some(index) = self.warning_index.get(&(code, key.clone())) {
            if let Some(row) = self.report.warnings.get_mut(*index) {
                row.count += 1;
            }
            return;
        }
        if self.report.warnings.len() >= MAX_WARNING_ROWS {
            self.warning_rows_dropped += 1;
            return;
        }
        self.warning_index
            .insert((code, key.clone()), self.report.warnings.len());
        self.report.warnings.push(Warning {
            code,
            key,
            count: 1,
            message: message.into(),
        });
    }

    /// Records an unmapped block state.
    pub fn unmapped_block(&mut self, state: impl Into<String>, message: impl Into<String>) {
        let state = state.into();
        *self.report.unmapped.blocks.entry(state.clone()).or_insert(0) += 1;
        self.warn(WarningCode::UnmappedBlock, state, message);
    }

    /// Records an unmapped entity id.
    pub fn unmapped_entity(&mut self, id: impl Into<String>) {
        let id = id.into();
        *self.report.unmapped.entities.entry(id.clone()).or_insert(0) += 1;
        self.warn(
            WarningCode::UnmappedEntity,
            id.clone(),
            format!("entity {id} has no target mapping; it was skipped"),
        );
    }

    /// Records an unmapped item id.
    pub fn unmapped_item(&mut self, id: impl Into<String>) {
        let id = id.into();
        *self.report.unmapped.items.entry(id.clone()).or_insert(0) += 1;
        self.warn(
            WarningCode::UnmappedItem,
            id.clone(),
            format!("item {id} has no target mapping; it was replaced with air"),
        );
    }

    /// Records an unmapped biome id.
    pub fn unmapped_biome(&mut self, id: impl Into<String>) {
        let id = id.into();
        *self.report.unmapped.biomes.entry(id.clone()).or_insert(0) += 1;
        self.warn(
            WarningCode::UnmappedBiome,
            id.clone(),
            format!("biome {id} has no target mapping; the plains biome was used"),
        );
    }

    /// Merges another builder's results (worker thread → parent).
    pub fn merge(&mut self, other: Self) {
        let other = other.finish_without_timestamp();
        self.report.counters.merge(&other.counters);
        merge_unmapped(&mut self.report.unmapped.blocks, other.unmapped.blocks);
        merge_unmapped(&mut self.report.unmapped.entities, other.unmapped.entities);
        merge_unmapped(&mut self.report.unmapped.items, other.unmapped.items);
        merge_unmapped(&mut self.report.unmapped.biomes, other.unmapped.biomes);
        for row in other.warnings {
            if let Some(index) = self.warning_index.get(&(row.code, row.key.clone())) {
                self.report.warnings[*index].count += row.count;
                continue;
            }
            if self.report.warnings.len() >= MAX_WARNING_ROWS {
                self.warning_rows_dropped += 1;
                continue;
            }
            self.warning_index
                .insert((row.code, row.key.clone()), self.report.warnings.len());
            self.report.warnings.push(row);
        }
    }

    fn finish_without_timestamp(mut self) -> ConversionReport {
        self.end_stage();
        self.report
    }

    /// Finishes the report, sorting warnings by impact.
    #[must_use]
    pub fn finish(mut self) -> ConversionReport {
        self.end_stage();
        self.report.finished_unix_ms = unix_now_ms();
        self.report
            .warnings
            .sort_by(|left, right| right.count.cmp(&left.count).then(left.key.cmp(&right.key)));
        if self.warning_rows_dropped > 0 {
            self.report.warnings.push(Warning {
                code: WarningCode::Skipped,
                key: "warning-rows".to_string(),
                count: self.warning_rows_dropped,
                message: "additional distinct warnings were omitted from this report".to_string(),
            });
        }
        self.report
    }
}

/// Current unix time in milliseconds.
#[must_use]
pub fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn merge_unmapped(target: &mut BTreeMap<String, u64>, other: BTreeMap<String, u64>) {
    for (key, count) in other {
        *target.entry(key).or_insert(0) += count;
    }
}
