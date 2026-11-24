//! Worker thread, command loop, event ring and cancellation.
//!
//! The host calls `submit_command` from its UI thread and polls `read_events`;
//! the conversion itself runs on one dedicated worker thread. Cancellation is
//! delivered out-of-band through the ABI's `cancel_command`, because a queued
//! cancel command would only be read after the conversion finished.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bmcbl_plugin_api::native::abi::{self, EventWake};
use worldport_core::convert::{
    self, ConversionOptions, ConversionPlan, Edition as CoreEdition, Features as CoreFeatures,
    PruningPolicy as CorePruning,
};
use worldport_core::error::{Result as CoreResult, WorldPortError};
use worldport_core::report::ConversionReport as CoreReport;
use worldport_protocol::{
    Command, ConversionRequest, Edition, Event, EventBatch, Features, PruningPolicy, ReportSummary,
    SequencedEvent, UnknownDataPolicy, WarningRow, decode_command, encode_batch,
};

use crate::info::world_info;

/// Queued commands per instance.
const COMMANDS: usize = 64;
/// Events kept when the UI stops reading.
const EVENTS: usize = 256;
/// Events per `read_events` batch.
const EVENTS_PER_BATCH: usize = 32;
/// Minimum interval between two progress events.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(120);

#[derive(Default)]
struct PendingEvents {
    sequence: u64,
    gap: bool,
    events: VecDeque<SequencedEvent>,
}

pub struct Events {
    pending: Mutex<PendingEvents>,
    context: usize,
    wake: Option<unsafe extern "C" fn(*mut c_void)>,
}

impl Events {
    fn new(wake: EventWake) -> Self {
        Self {
            pending: Mutex::default(),
            context: wake.context as usize,
            wake: wake.notify,
        }
    }

    fn publish(&self, event: Event) {
        let Ok(mut pending) = self.pending.lock() else {
            return;
        };
        if pending.events.len() >= EVENTS {
            pending.events.pop_front();
            pending.gap = true;
        }
        pending.sequence += 1;
        let sequence = pending.sequence;
        pending.events.push_back(SequencedEvent { sequence, event });
        drop(pending);
        if let Some(wake) = self.wake {
            // SAFETY: the host leases the callback until destroy joined the worker.
            unsafe { wake(self.context as *mut c_void) };
        }
    }

    /// Returns one encoded batch, or the capacity the host must provide.
    pub fn read_batch(&self, capacity: usize) -> Result<Vec<u8>, usize> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pending.events.is_empty() && !pending.gap {
            return Ok(Vec::new());
        }
        let mut count = pending.events.len().min(EVENTS_PER_BATCH);
        loop {
            let batch = EventBatch {
                gap: pending.gap,
                events: pending.events.iter().take(count).cloned().collect(),
            };
            let Ok(output) = encode_batch(&batch) else {
                return Ok(Vec::new());
            };
            if output.len() > abi::MAX_EVENT_BATCH && count > 1 {
                count /= 2;
                continue;
            }
            if output.len() > capacity {
                return Err(output.len());
            }
            pending.events.drain(..count);
            pending.gap = false;
            return Ok(output);
        }
    }
}

/// Shared view of the running conversion for `Snapshot`.
#[derive(Default)]
struct SessionState {
    running: bool,
    stage: String,
    done: u64,
    total: u64,
    /// Milliseconds since the conversion started.
    elapsed_ms: u64,
    /// Estimated milliseconds left, `0` when unknown.
    eta_ms: u64,
    warnings: Vec<WarningRow>,
    last_report: Option<ReportSummary>,
}

pub struct Session {
    inbox: SyncSender<Command>,
    pub(crate) events: Arc<Events>,
    cancel: Arc<AtomicBool>,
    active: Arc<Mutex<Option<u64>>>,
    stop: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Session {
    /// Starts the worker thread for one instance.
    pub fn start(wake: EventWake) -> std::io::Result<Self> {
        let (inbox, receiver) = sync_channel::<Command>(COMMANDS);
        let events = Arc::new(Events::new(wake));
        let state = Arc::new(Mutex::new(SessionState::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let active = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let worker = std::thread::Builder::new()
            .name("worldport-native".into())
            .spawn({
                let events = Arc::clone(&events);
                let state = Arc::clone(&state);
                let cancel = Arc::clone(&cancel);
                let active = Arc::clone(&active);
                let stop = Arc::clone(&stop);
                move || {
                    while !stop.load(Ordering::Relaxed) {
                        let Ok(request) = receiver.recv() else {
                            break;
                        };
                        run(&events, &state, &cancel, &active, request);
                    }
                }
            })?;
        Ok(Self {
            inbox,
            events,
            cancel,
            active,
            stop,
            worker: Some(worker),
        })
    }

    /// Queues a command for the worker.
    pub fn submit(&self, request: u64, command: &[u8]) -> i32 {
        let Ok(command) = decode_command(command) else {
            return abi::INVALID_ARGUMENT;
        };
        if matches!(command, Command::Cancel) {
            self.cancel.store(true, Ordering::Relaxed);
            return abi::OK;
        }
        let is_start = matches!(command, Command::Start { .. });
        let mut active = self.active.lock().unwrap_or_else(|error| error.into_inner());
        if is_start {
            if active.is_some() {
                return abi::BUSY;
            }
            self.cancel.store(false, Ordering::Relaxed);
            *active = Some(request);
        }
        match self.inbox.try_send(command) {
            Ok(()) => abi::OK,
            Err(error) => {
                if is_start {
                    *active = None;
                }
                match error {
                    TrySendError::Full(_) => abi::BUSY,
                    TrySendError::Disconnected(_) => abi::CLOSED,
                }
            }
        }
    }

    /// Requests cancellation of the running conversion.
    pub fn cancel(&self, request: u64) -> i32 {
        let active = self.active.lock().unwrap_or_else(|error| error.into_inner());
        if *active == Some(request) {
            self.cancel.store(true, Ordering::Relaxed);
        }
        abi::OK
    }

    /// Stops the worker; the ABI requires this to join before returning.
    pub fn stop(mut self) -> i32 {
        self.stop.store(true, Ordering::Relaxed);
        self.cancel.store(true, Ordering::Relaxed);
        drop(self.inbox);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        abi::OK
    }
}

fn run(
    events: &Arc<Events>,
    state: &Arc<Mutex<SessionState>>,
    cancel: &Arc<AtomicBool>,
    active: &Mutex<Option<u64>>,
    command: Command,
) {
    match command {
        Command::BrowseSource => match pick_folder("选择要转换的世界目录") {
            Some(path) => events.publish(Event::Picked {
                path: path.display().to_string(),
            }),
            None => events.publish(Event::Picked {
                path: String::new(),
            }),
        },
        Command::BrowseTarget => match pick_folder("选择输出目录") {
            Some(path) => events.publish(Event::Picked {
                path: path.display().to_string(),
            }),
            None => events.publish(Event::Picked {
                path: String::new(),
            }),
        },
        Command::BrowseArchive { mcworld } => {
            let extension = if mcworld { "mcworld" } else { "zip" };
            let path = rfd::FileDialog::new()
                .set_title("选择输出文件")
                .add_filter(if mcworld { "Minecraft 基岩版地图包" } else { "ZIP 压缩包" }, &[extension])
                .set_file_name(format!("world.{extension}"))
                .save_file()
                .map(|mut path| {
                    if !path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case(extension)) {
                        path.set_extension(extension);
                    }
                    path.display().to_string()
                }).unwrap_or_default();
            events.publish(Event::Picked { path });
        }
        Command::Detect { path } => {
            let path = PathBuf::from(&path);
            match world_info(&path) {
                Ok(info) => events.publish(Event::Detected {
                    info,
                    path: path.display().to_string(),
                }),
                Err(error) => events.publish(Event::DetectFailed {
                    path: path.display().to_string(),
                    message: error.to_string(),
                }),
            }
        }
        Command::Start { request } => start_conversion(events, state, cancel, active, request),
        Command::Cancel => {}
        Command::Snapshot => {
            let snapshot = state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            events.publish(Event::State {
                running: snapshot.running,
                stage: snapshot.stage.clone(),
                done: snapshot.done,
                total: snapshot.total,
                warnings: snapshot.warnings.clone(),
                last_report: snapshot.last_report.clone(),
            });
        }
        Command::OpenOutput { path } => {
            let path = Path::new(&path);
            if path.is_file() {
                open_folder(path.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or_else(|| Path::new(".")));
            } else {
                open_folder(path);
            }
        }
    }
}

fn start_conversion(
    events: &Arc<Events>,
    state: &Arc<Mutex<SessionState>>,
    cancel: &Arc<AtomicBool>,
    active: &Mutex<Option<u64>>,
    request: ConversionRequest,
) {
    {
        let mut current = state.lock().unwrap_or_else(|error| error.into_inner());
        current.running = true;
        current.stage = "start".to_string();
        current.done = 0;
        current.total = 0;
        current.warnings.clear();
    }

    let target_path = request.target.clone();
    let plan = build_plan(&request);
    events.publish(Event::Log {
        level: "info".to_string(),
        message: format!(
            "开始转换 {} → {}",
            request.source, request.target
        ),
    });
    let started = Instant::now();
    let progress = progress_sink(events, state);
    let options = ConversionOptions::with_cancel(Arc::clone(cancel)).with_progress(progress);
    let outcome = plan.and_then(|plan| convert::run(&plan, &options));

    let mut current = state.lock().unwrap_or_else(|error| error.into_inner());
    current.running = false;
    current.stage.clear();
    *active.lock().unwrap_or_else(|error| error.into_inner()) = None;
    match outcome {
        Ok(report) => {
            let summary = summarize(&report, &target_path, started);
            current.last_report = Some(summary.clone());
            current.warnings = summary.warnings.clone();
            if !summary.warnings.is_empty() {
                events.publish(Event::Warnings {
                    rows: summary.warnings.clone(),
                    unmapped_total: summary.unmapped_total,
                });
            }
            events.publish(Event::Log {
                level: "info".to_string(),
                message: format!(
                    "转换完成：{} 个区块，{} 个方块，耗时 {:.1} s",
                    summary.chunks,
                    summary.blocks,
                    summary.elapsed_ms as f64 / 1000.0
                ),
            });
            events.publish(Event::Finished { report: summary });
        }
        Err(WorldPortError::Cancelled) => {
            events.publish(Event::Cancelled);
        }
        Err(error) => {
            events.publish(Event::Failed {
                message: error.to_string(),
            });
        }
    }
}

/// Progress throttling state, kept per conversion.
struct Throttle {
    emitted_at: Instant,
    done: u64,
    stage: String,
    /// When the current stage first reported, so its rate is its own.
    stage_started: Instant,
}

fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Estimated time left in a stage, or `0` when it cannot be estimated.
///
/// Only reports an estimate once enough of the stage has run for the rate to
/// mean something: a number that swings wildly is worse than no number.
fn eta_ms(done: u64, total: u64, stage_ms: u64) -> u64 {
    if total == 0 || done == 0 || done >= total || stage_ms < 500 {
        return 0;
    }
    let per_item = stage_ms as f64 / done as f64;
    let remaining = (total - done) as f64 * per_item;
    if remaining.is_finite() {
        remaining.round().min(u64::MAX as f64) as u64
    } else {
        0
    }
}

fn progress_sink(
    events: &Arc<Events>,
    state: &Arc<Mutex<SessionState>>,
) -> Arc<dyn Fn(convert::Progress) + Send + Sync> {
    let events = Arc::clone(events);
    let state = Arc::clone(state);
    let started = Instant::now();
    let last = Mutex::new(Throttle {
        emitted_at: started,
        done: u64::MAX,
        stage: String::new(),
        stage_started: started,
    });
    Arc::new(move |progress: convert::Progress| {
        let Ok(mut throttle) = last.lock() else {
            return;
        };
        let now = Instant::now();
        let same_stage = throttle.stage == progress.stage;
        let complete = progress.total.is_some_and(|total| total > 0 && progress.done >= total);
        if same_stage
            && !complete
            && now.duration_since(throttle.emitted_at) < PROGRESS_INTERVAL
        {
            return;
        }
        if !same_stage {
            // A new stage resets the rate window: the previous stage's pace says
            // nothing about this one.
            throttle.stage_started = now;
        }
        throttle.emitted_at = now;
        throttle.done = progress.done;
        throttle.stage.clone_from(&progress.stage);
        let elapsed_ms = millis(now.duration_since(started));
        let stage_ms = millis(now.duration_since(throttle.stage_started));
        drop(throttle);
        let total = progress.total.unwrap_or(0);
        if let Ok(mut current) = state.lock() {
            current.stage = progress.stage.clone();
            current.done = progress.done;
            current.total = total;
            current.elapsed_ms = elapsed_ms;
            current.eta_ms = eta_ms(progress.done, total, stage_ms);
        }
        events.publish(Event::Stage {
            stage: progress.stage,
            done: progress.done,
            total,
            message: progress.message.unwrap_or_default(),
            elapsed_ms,
            eta_ms: eta_ms(progress.done, total, stage_ms),
        });
    })
}

fn build_plan(request: &ConversionRequest) -> CoreResult<ConversionPlan> {
    let mut plan = ConversionPlan::new(PathBuf::from(&request.source), PathBuf::from(&request.target));
    plan.target_edition = request.target_edition.map(|edition| match edition {
        Edition::Bedrock => CoreEdition::Bedrock,
        Edition::Java => CoreEdition::Java,
    });
    plan.target_version = request.target_version.clone();
    plan.features = features(&request.features);
    plan.pruning = pruning(&request.pruning);
    plan.jobs = usize::try_from(request.jobs).unwrap_or(0);
    plan.compression_level = request.compression_level.min(9);
    plan.overwrite = request.overwrite;
    Ok(plan)
}

fn features(features: &Features) -> CoreFeatures {
    CoreFeatures {
        blocks: features.blocks,
        block_entities: features.block_entities,
        entities: features.entities,
        biomes: features.biomes,
        players: features.players,
        maps: features.maps,
        level_dat: features.level_dat,
        ticks: features.ticks,
        chunk_metadata: features.chunk_metadata,
        village_interests: features.village_interests,
    }
}

fn pruning(pruning: &PruningPolicy) -> CorePruning {
    CorePruning {
        blocks: policy(pruning.blocks),
        entities: policy(pruning.entities),
        items: policy(pruning.items),
        biomes: policy(pruning.biomes),
        keep_unknown_fields: pruning.keep_unknown_fields,
    }
}

const fn policy(policy: UnknownDataPolicy) -> convert::UnknownDataPolicy {
    match policy {
        UnknownDataPolicy::Fallback => convert::UnknownDataPolicy::Fallback,
        UnknownDataPolicy::Drop => convert::UnknownDataPolicy::Drop,
        UnknownDataPolicy::Error => convert::UnknownDataPolicy::Error,
    }
}

fn summarize(report: &CoreReport, target_path: &str, started: Instant) -> ReportSummary {
    let warnings: Vec<WarningRow> = report
        .warnings
        .iter()
        .map(|warning| WarningRow {
            code: warning.code.as_str().to_string(),
            key: warning.key.clone(),
            count: warning.count,
            message: warning.message.clone(),
        })
        .collect();
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    ReportSummary {
        target_path: target_path.to_string(),
        source_version: report.source_version.clone(),
        target_version: report.target_version.clone(),
        elapsed_ms,
        chunks: report.counters.chunks,
        blocks: report.counters.blocks,
        block_entities: report.counters.block_entities,
        entities: report.counters.entities,
        players: report.counters.players,
        maps: report.counters.maps,
        clean: report.is_clean(),
        unmapped_total: u64::try_from(report.unmapped.total()).unwrap_or(u64::MAX),
        warnings,
    }
}

fn pick_folder(title: &str) -> Option<PathBuf> {
    rfd::FileDialog::new().set_title(title).pick_folder()
}

fn open_folder(path: &Path) {
    #[cfg(target_os = "windows")]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let program = "xdg-open";
    let _ = std::process::Command::new(program).arg(path).spawn();
}
