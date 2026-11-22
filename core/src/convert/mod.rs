//! Conversion entry points, options and progress reporting.

pub mod be_to_je;
pub mod je_to_be;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::error::{Result, WorldPortError};
use crate::report::ConversionReport;

/// World edition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edition {
    /// Minecraft Bedrock Edition (`db/` LevelDB).
    Bedrock,
    /// Minecraft Java Edition (Anvil regions).
    Java,
}

impl Edition {
    /// `bedrock` or `java`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Bedrock => "bedrock",
            Self::Java => "java",
        }
    }
}

/// Which data categories to convert. Everything defaults to on.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Features {
    /// Block storage (the terrain itself).
    pub blocks: bool,
    /// Block entities (chests, signs, furnaces, ...).
    pub block_entities: bool,
    /// Entities with their full NBT.
    pub entities: bool,
    /// Biomes.
    pub biomes: bool,
    /// Player data.
    pub players: bool,
    /// Map items.
    pub maps: bool,
    /// `level.dat`: seed, spawn, time, weather, gamerules, generator.
    pub level_dat: bool,
    /// Pending block/fluid ticks and scheduled events.
    pub ticks: bool,
    /// Hardcoded spawn areas and other per-chunk metadata.
    pub chunk_metadata: bool,
}

impl Default for Features {
    fn default() -> Self {
        Self {
            blocks: true,
            block_entities: true,
            entities: true,
            biomes: true,
            players: true,
            maps: true,
            level_dat: true,
            ticks: true,
            chunk_metadata: true,
        }
    }
}

impl Features {
    /// Nothing at all.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            blocks: false,
            block_entities: false,
            entities: false,
            biomes: false,
            players: false,
            maps: false,
            level_dat: false,
            ticks: false,
            chunk_metadata: false,
        }
    }

    /// Everything.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            blocks: true,
            block_entities: true,
            entities: true,
            biomes: true,
            players: true,
            maps: true,
            level_dat: true,
            ticks: true,
            chunk_metadata: true,
        }
    }
}

/// What to do with data the target version cannot represent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnknownDataPolicy {
    /// Replace with a safe fallback and record a warning.
    #[default]
    Fallback,
    /// Drop the record entirely and record a warning.
    Drop,
    /// Abort the conversion.
    Error,
}

/// Pruning behaviour used when upgrading or downgrading versions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PruningPolicy {
    /// Unknown blocks.
    pub blocks: UnknownDataPolicy,
    /// Unknown entities.
    pub entities: UnknownDataPolicy,
    /// Unknown items.
    pub items: UnknownDataPolicy,
    /// Unknown biome ids.
    pub biomes: UnknownDataPolicy,
    /// Keep fields the target version does not know (usually dropped).
    pub keep_unknown_fields: bool,
}

impl Default for PruningPolicy {
    fn default() -> Self {
        Self {
            blocks: UnknownDataPolicy::Fallback,
            entities: UnknownDataPolicy::Drop,
            items: UnknownDataPolicy::Drop,
            biomes: UnknownDataPolicy::Fallback,
            keep_unknown_fields: false,
        }
    }
}

/// Everything needed to run a conversion.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversionPlan {
    /// Source world directory.
    pub source: PathBuf,
    /// Output world directory. Must not exist yet unless `overwrite` is set.
    pub target: PathBuf,
    /// Leave `None` to convert to the other edition.
    pub target_edition: Option<Edition>,
    /// Target version label such as `1.21.4` (Java) or `1.21.60` (Bedrock).
    /// `None` keeps the source version.
    pub target_version: Option<String>,
    /// Data categories to convert.
    pub features: Features,
    /// Pruning policy for version changes.
    pub pruning: PruningPolicy,
    /// Worker threads; `0` means one per available core.
    pub jobs: usize,
    /// Deflate level for written Java region chunks (0..=9).
    pub compression_level: u32,
    /// Replace an existing target directory.
    pub overwrite: bool,
    /// Stop after this many chunks (testing aid; `None` converts everything).
    pub chunk_limit: Option<usize>,
}

impl ConversionPlan {
    /// Builds a plan that converts `source` into `target` with every feature on.
    #[must_use]
    pub fn new(source: impl Into<PathBuf>, target: impl Into<PathBuf>) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            target_edition: None,
            target_version: None,
            features: Features::all(),
            pruning: PruningPolicy::default(),
            jobs: 0,
            compression_level: 6,
            overwrite: false,
            chunk_limit: None,
        }
    }
}

/// Which pipeline stage is running.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Progress {
    /// Stable stage name.
    pub stage: String,
    /// Items completed in this stage.
    pub done: u64,
    /// Items expected in this stage, when known.
    pub total: Option<u64>,
    /// Optional detail for the UI.
    pub message: Option<String>,
}

impl Progress {
    /// Builds a progress tick.
    #[must_use]
    pub fn new(stage: impl Into<String>, done: u64, total: Option<u64>) -> Self {
        Self {
            stage: stage.into(),
            done,
            total,
            message: None,
        }
    }

    /// Adds a detail message.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }
}

/// Callback invoked from the conversion thread(s). Must be cheap and thread-safe.
pub type ProgressSink = Arc<dyn Fn(Progress) + Send + Sync>;

/// Runtime knobs that callers can change while the conversion runs.
#[derive(Clone, Default)]
pub struct ConversionOptions {
    /// Set to stop at the next checkpoint.
    pub cancel: Option<Arc<AtomicBool>>,
    /// Progress callback.
    pub progress: Option<ProgressSink>,
}

impl ConversionOptions {
    /// Creates options with a cancellation flag.
    #[must_use]
    pub fn with_cancel(cancel: Arc<AtomicBool>) -> Self {
        Self {
            cancel: Some(cancel),
            progress: None,
        }
    }

    /// Installs a progress callback.
    #[must_use]
    pub fn with_progress(mut self, progress: ProgressSink) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Emits a progress tick when a sink is installed.
    pub fn report(&self, progress: Progress) {
        if let Some(sink) = &self.progress {
            sink(progress);
        }
    }

    /// Returns an error when cancellation was requested.
    pub fn check_cancelled(&self) -> Result<()> {
        if self
            .cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
        {
            return Err(WorldPortError::Cancelled);
        }
        Ok(())
    }

    /// True when cancellation was requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }
}

/// Detects the edition of a world directory.
pub fn detect_edition(path: &Path) -> Result<Edition> {
    if !path.is_dir() {
        return Err(WorldPortError::invalid(format!(
            "{} is not a directory",
            path.display()
        )));
    }
    let has_level_dat = path.join("level.dat").is_file();
    if path.join("db").is_dir() && has_level_dat {
        return Ok(Edition::Bedrock);
    }
    if has_level_dat && (path.join("region").is_dir() || path.join("dimensions").is_dir()) {
        return Ok(Edition::Java);
    }
    if path.join("chunks.dat").is_file() && has_level_dat {
        return Ok(Edition::Bedrock);
    }
    Err(WorldPortError::invalid(format!(
        "{} has no recognized Minecraft world layout (level.dat + db/ or level.dat + region/)",
        path.display()
    )))
}

/// Runs a conversion, choosing the direction from the source edition.
pub fn run(plan: &ConversionPlan, options: &ConversionOptions) -> Result<ConversionReport> {
    let source = detect_edition(&plan.source)?;
    let target = plan.target_edition.unwrap_or(match source {
        Edition::Bedrock => Edition::Java,
        Edition::Java => Edition::Bedrock,
    });
    if target == source {
        return Err(WorldPortError::invalid(format!(
            "source and target are both {}",
            source.as_str()
        )));
    }
    prepare_target(&plan.target, plan.overwrite)?;
    options.check_cancelled()?;
    match (source, target) {
        (Edition::Java, Edition::Bedrock) => je_to_be::convert(plan, options),
        (Edition::Bedrock, Edition::Java) => be_to_je::convert(plan, options),
        _ => unreachable!("editions differ"),
    }
}

fn prepare_target(target: &Path, overwrite: bool) -> Result<()> {
    if target.exists() {
        let empty = target
            .read_dir()
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false);
        if !empty {
            if !overwrite {
                return Err(WorldPortError::invalid(format!(
                    "{} already exists; enable overwrite or choose another directory",
                    target.display()
                )));
            }
            return Err(WorldPortError::other(
                "overwriting a non-empty world directory is not implemented yet",
            ));
        }
    }
    std::fs::create_dir_all(target).map_err(|error| WorldPortError::io(target, error))?;
    Ok(())
}
