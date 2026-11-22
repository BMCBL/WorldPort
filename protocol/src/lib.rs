//! Wire protocol between the WorldPort `.bmcblx` UI (WebAssembly) and the
//! converter's native module.
//!
//! Both sides are built from this crate, so the payloads are postcard-encoded
//! Rust enums with no cross-language schema to maintain. The protocol is
//! deliberately independent of `worldport-core`: the WebAssembly side must stay
//! small and cannot link the world codecs.

use serde::{Deserialize, Serialize};

/// Protocol revision. Bump when a command or event changes shape.
pub const PROTOCOL_VERSION: u32 = 1;

/// Which side of the conversion the user picked.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Edition {
    /// Bedrock Edition (`db/` LevelDB).
    Bedrock,
    /// Java Edition (Anvil regions).
    Java,
}

/// Data categories exposed as toggles in the UI. Everything defaults to on.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Features {
    /// Terrain blocks.
    pub blocks: bool,
    /// Block entities (chests, signs, furnaces, ...).
    pub block_entities: bool,
    /// Entities with full NBT.
    pub entities: bool,
    /// Biomes.
    pub biomes: bool,
    /// Player data.
    pub players: bool,
    /// Map items.
    pub maps: bool,
    /// `level.dat` (seed, spawn, time, gamerules).
    pub level_dat: bool,
    /// Pending ticks and scheduled events.
    pub ticks: bool,
    /// Per-chunk metadata such as hardcoded spawn areas.
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

/// What to do when the target version cannot represent a value.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum UnknownDataPolicy {
    /// Substitute a safe fallback and warn.
    #[default]
    Fallback,
    /// Drop the record and warn.
    Drop,
    /// Abort with an error.
    Error,
}

/// Pruning behaviour for version changes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PruningPolicy {
    /// Unknown blocks.
    pub blocks: UnknownDataPolicy,
    /// Unknown entities.
    pub entities: UnknownDataPolicy,
    /// Unknown items.
    pub items: UnknownDataPolicy,
    /// Unknown biomes.
    pub biomes: UnknownDataPolicy,
    /// Keep fields the target version does not know.
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

/// A conversion request from the UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ConversionRequest {
    /// Source world directory.
    pub source: String,
    /// Output world directory.
    pub target: String,
    /// Target edition; `None` means "the other edition".
    pub target_edition: Option<Edition>,
    /// Target version label (`1.21.4`, `1.21.60`), or `None` to keep the source version.
    pub target_version: Option<String>,
    /// Category toggles.
    pub features: Features,
    /// Pruning policy.
    pub pruning: PruningPolicy,
    /// Worker threads; `0` = all cores.
    pub jobs: u32,
    /// Deflate level for Java region chunks.
    pub compression_level: u32,
    /// Replace an existing target directory.
    pub overwrite: bool,
}

/// Commands sent from the UI to the native module.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Command {
    /// Ask the host for a directory through a native dialog.
    BrowseSource,
    /// Ask the host for an output directory.
    BrowseTarget,
    /// Inspect a world and answer with [`Event::Detected`].
    Detect {
        /// World directory to inspect.
        path: String,
    },
    /// Start a conversion.
    Start {
        /// Full request.
        request: ConversionRequest,
    },
    /// Stop the running conversion.
    Cancel,
    /// Re-send the current state (used after the page is reopened).
    Snapshot,
    /// Open the output folder in the system file manager.
    OpenOutput {
        /// Folder to open.
        path: String,
    },
}

/// One row of conversion warnings.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WarningRow {
    /// Stable code (`unmapped-block`, `dropped-field`, ...).
    pub code: String,
    /// Identifier or field name.
    pub key: String,
    /// Occurrence count.
    pub count: u64,
    /// Explanation.
    pub message: String,
}

/// What a world looks like, for the UI summary card.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WorldInfo {
    /// `bedrock` or `java`.
    pub edition: String,
    /// World name from `level.dat`.
    pub level_name: String,
    /// Seed.
    pub seed: i64,
    /// Human-readable version (`1.21.4` / `1.21.60.1`).
    pub version: String,
    /// Chunks per dimension as `(dimension, count)`.
    pub dimensions: Vec<(String, u64)>,
    /// Players found in the world.
    pub players: u64,
    /// Map items found in the world.
    pub maps: u64,
}

impl WorldInfo {
    /// Total chunks across dimensions.
    #[must_use]
    pub fn total_chunks(&self) -> u64 {
        self.dimensions.iter().map(|(_, count)| *count).sum()
    }
}

/// Conversion summary shown when a run finishes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ReportSummary {
    /// Where the output was written.
    pub target_path: String,
    /// Source version label.
    pub source_version: String,
    /// Target version label.
    pub target_version: String,
    /// Elapsed milliseconds.
    pub elapsed_ms: u64,
    /// Chunks written.
    pub chunks: u64,
    /// Blocks written.
    pub blocks: u64,
    /// Block entities written.
    pub block_entities: u64,
    /// Entities written.
    pub entities: u64,
    /// Players written.
    pub players: u64,
    /// Maps written.
    pub maps: u64,
    /// Warning rows (already aggregated and sorted).
    pub warnings: Vec<WarningRow>,
    /// Distinct unmapped identifiers.
    pub unmapped_total: u64,
    /// True when no warning and no unmapped identifier was recorded.
    pub clean: bool,
}

/// Events sent from the native module to the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// Answer to `BrowseSource`/`BrowseTarget`; `path` is empty when cancelled.
    Picked {
        /// Selected path.
        path: String,
    },
    /// Answer to `Detect`.
    Detected {
        /// Inspected world.
        info: WorldInfo,
        /// Original path echoed back.
        path: String,
    },
    /// A detection attempt failed.
    DetectFailed {
        /// Original path.
        path: String,
        /// Reason.
        message: String,
    },
    /// Progress tick.
    Stage {
        /// Stage name.
        stage: String,
        /// Items done.
        done: u64,
        /// Items expected.
        total: u64,
        /// Optional detail.
        message: String,
    },
    /// Warnings discovered so far (may repeat as more are found).
    Warnings {
        /// Rows.
        rows: Vec<WarningRow>,
        /// Distinct unmapped identifiers so far.
        unmapped_total: u64,
    },
    /// Conversion finished.
    Finished {
        /// Summary.
        report: ReportSummary,
    },
    /// Conversion failed.
    Failed {
        /// Reason.
        message: String,
    },
    /// Conversion was cancelled.
    Cancelled,
    /// Free-form log line for the UI log panel.
    Log {
        /// `debug`, `info`, `warn`, `error`.
        level: String,
        /// Text.
        message: String,
    },
    /// Answer to `Snapshot`.
    State {
        /// Running?
        running: bool,
        /// Current stage name when running.
        stage: String,
        /// Items done.
        done: u64,
        /// Items expected.
        total: u64,
        /// Accumulated warnings.
        warnings: Vec<WarningRow>,
        /// Last finished report, if any.
        last_report: Option<ReportSummary>,
    },
}

/// One event with a monotonic sequence number.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequencedEvent {
    /// Per-instance sequence; a gap means the UI missed events.
    pub sequence: u64,
    /// Payload.
    pub event: Event,
}

/// A batch of events returned by one `read_events` call.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct EventBatch {
    /// True when events were dropped because the ring buffer overflowed.
    pub gap: bool,
    /// Events in order.
    pub events: Vec<SequencedEvent>,
}

/// Encodes a command for `submit_command`.
pub fn encode_command(command: &Command) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(command)
}

/// Decodes a command inside the native module.
pub fn decode_command(bytes: &[u8]) -> Result<Command, postcard::Error> {
    postcard::from_bytes(bytes)
}

/// Encodes an event batch for `read_events`.
pub fn encode_batch(batch: &EventBatch) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(batch)
}

/// Decodes an event batch inside the WebAssembly module.
pub fn decode_batch(bytes: &[u8]) -> Result<EventBatch, postcard::Error> {
    postcard::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_round_trip() {
        let command = Command::Start {
            request: ConversionRequest {
                source: r"C:\worlds\java".to_string(),
                target: r"C:\worlds\bedrock".to_string(),
                target_edition: Some(Edition::Bedrock),
                target_version: None,
                features: Features::default(),
                pruning: PruningPolicy::default(),
                jobs: 0,
                compression_level: 6,
                overwrite: false,
            },
        };
        let bytes = encode_command(&command).expect("encode");
        let decoded = decode_command(&bytes).expect("decode");
        assert_eq!(format!("{decoded:?}"), format!("{command:?}"));
    }

    #[test]
    fn event_batches_round_trip() {
        let batch = EventBatch {
            gap: false,
            events: vec![SequencedEvent {
                sequence: 7,
                event: Event::Stage {
                    stage: "terrain".to_string(),
                    done: 12,
                    total: 100,
                    message: String::new(),
                },
            }],
        };
        let bytes = encode_batch(&batch).expect("encode");
        assert_eq!(decode_batch(&bytes).expect("decode"), batch);
    }
}
