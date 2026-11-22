//! WorldPort `.bmcblx` plugin: a converter page registered on BMCBL's tools tab.
//!
//! The page only renders and forwards commands. Conversion runs in the
//! separately approved native module (`worldport_native.dll`) through BMCBL's
//! native ABI, because the WebAssembly sandbox has no filesystem access.

use std::cell::RefCell;
use std::collections::VecDeque;

use bmcbl_plugin_api::app::Job;
use bmcbl_plugin_api::native::{self, InstanceHandle, ModuleHandle, RequestId};
use bmcbl_plugin_api::prelude::*;
use worldport_protocol::{
    Command, ConversionRequest, Edition, Event, EventBatch, Features, PruningPolicy,
    ReportSummary, WarningRow, WorldInfo, decode_batch, encode_command,
};

mod page;

const PAGE_ID: &str = "convert";
const EVENT_WAIT_MS: u32 = 10_000;
const LOG_LINES: usize = 24;
const STORAGE_SOURCE: &str = "last_source";
const STORAGE_TARGET: &str = "last_target";
const STORAGE_JOBS: &str = "jobs";
const STORAGE_COMPRESSION: &str = "compression";
const STORAGE_DIGEST: &str = "native_sha256";

struct WorldPort;

#[derive(Default)]
enum Native {
    /// No digest configured (or the user must approve the library first).
    #[default]
    Missing,
    Loading(Job<ModuleHandle>),
    Creating(ModuleHandle, Job<InstanceHandle>),
    Ready(InstanceHandle),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Browse {
    Source,
    Target,
}

struct State {
    native: Native,
    source: String,
    target: String,
    info: Option<WorldInfo>,
    detecting: bool,
    browse: Option<Browse>,
    /// Empty for "the other edition".
    direction: String,
    /// Empty for the default target version.
    version: String,
    features: Features,
    show_advanced: bool,
    jobs: u32,
    compression: u32,
    overwrite: bool,
    running: bool,
    stage: String,
    done: u64,
    total: u64,
    warnings: Vec<WarningRow>,
    unmapped_total: u64,
    report: Option<ReportSummary>,
    message: Option<String>,
    problem: Option<String>,
    digest: Option<String>,
    digest_input: String,
    pending: Option<Command>,
    submit: Option<Job<RequestId>>,
    cancel_job: Option<Job<()>>,
    request: Option<RequestId>,
    read: Option<Job<Vec<u8>>>,
    sequence: u64,
    log: VecDeque<String>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            native: Native::default(),
            source: String::new(),
            target: String::new(),
            info: None,
            detecting: false,
            browse: None,
            direction: String::new(),
            version: String::new(),
            features: Features::default(),
            show_advanced: false,
            jobs: 0,
            compression: 6,
            overwrite: false,
            running: false,
            stage: String::new(),
            done: 0,
            total: 0,
            warnings: Vec::new(),
            unmapped_total: 0,
            report: None,
            message: None,
            problem: None,
            digest: None,
            digest_input: String::new(),
            pending: None,
            submit: None,
            cancel_job: None,
            request: None,
            read: None,
            sequence: 0,
            log: VecDeque::new(),
        }
    }
}

thread_local! {
    static PAGE: RefCell<State> = RefCell::new(State::default());
}

impl State {
    fn load_persisted(&mut self) {
        self.source = storage_get(STORAGE_SOURCE).ok().flatten().unwrap_or_default();
        self.target = storage_get(STORAGE_TARGET).ok().flatten().unwrap_or_default();
        self.jobs = storage_get(STORAGE_JOBS)
            .ok()
            .flatten()
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        self.compression = storage_get(STORAGE_COMPRESSION)
            .ok()
            .flatten()
            .and_then(|value| value.parse().ok())
            .unwrap_or(6);
        let stored = storage_get(STORAGE_DIGEST)
            .ok()
            .flatten()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.digest = stored.or_else(|| {
            option_env!("WORLDPORT_NATIVE_SHA256")
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        });
        self.digest_input = self.digest.clone().unwrap_or_default();
    }

    fn persist_paths(&self) {
        let _ = storage_set(STORAGE_SOURCE, &self.source);
        let _ = storage_set(STORAGE_TARGET, &self.target);
        let _ = storage_set(STORAGE_JOBS, self.jobs.to_string());
        let _ = storage_set(STORAGE_COMPRESSION, self.compression.to_string());
    }

    fn native_ready(&self) -> bool {
        matches!(self.native, Native::Ready(_))
    }

    fn note(&mut self, message: impl Into<String>) {
        let message = message.into();
        if self.log.len() >= LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(message);
    }

    /// Loads the native module when an instance is not available yet.
    fn ensure_native(&mut self) -> PluginResult<()> {
        match &self.native {
            Native::Ready(_) | Native::Loading(_) | Native::Creating(..) => return Ok(()),
            Native::Missing => {}
        }
        let Some(digest) = self.digest.clone() else {
            return Err(plugin_error(
                "native-missing",
                "还没有原生组件摘要：在「设置 → 插件」批准 worldport_native.dll 后，把它的 SHA-256 填到本页。",
            ));
        };
        self.note("正在加载原生转换组件");
        self.native = Native::Loading(native::load(&digest)?);
        Ok(())
    }

    fn dispatch(&mut self, command: Command) -> PluginResult<()> {
        if let Native::Ready(_) = self.native {
            self.submit_command(command)
        } else {
            self.ensure_native()?;
            self.pending = Some(command);
            Ok(())
        }
    }

    fn submit_command(&mut self, command: Command) -> PluginResult<()> {
        let Native::Ready(instance) = self.native else {
            self.pending = Some(command);
            return Ok(());
        };
        if self.submit.is_some() {
            self.pending = Some(command);
            return Ok(());
        }
        let bytes = encode_command(&command)
            .map_err(|error| plugin_error("encode-command", error.to_string()))?;
        self.submit = Some(native::submit_command(instance, &bytes)?);
        Ok(())
    }

    fn start(&mut self) -> PluginResult<()> {
        if self.running {
            return Ok(());
        }
        if self.source.trim().is_empty() || self.target.trim().is_empty() {
            return Err(plugin_error("missing-path", "请先选择源世界和输出目录。"));
        }
        let request = ConversionRequest {
            source: self.source.trim().to_string(),
            target: self.target.trim().to_string(),
            target_edition: match self.direction.as_str() {
                "bedrock" => Some(Edition::Bedrock),
                "java" => Some(Edition::Java),
                _ => None,
            },
            target_version: (!self.version.is_empty()).then(|| self.version.clone()),
            features: self.features,
            pruning: PruningPolicy::default(),
            jobs: self.jobs,
            compression_level: self.compression,
            overwrite: self.overwrite,
        };
        self.problem = None;
        self.message = Some("正在启动转换…".to_string());
        self.warnings.clear();
        self.unmapped_total = 0;
        self.report = None;
        self.persist_paths();
        self.dispatch(Command::Start { request })
    }

    fn cancel(&mut self) -> PluginResult<()> {
        if let (Native::Ready(instance), Some(request)) = (&self.native, self.request) {
            self.cancel_job = Some(native::cancel_command(*instance, request)?);
            self.message = Some("正在取消…".to_string());
            return Ok(());
        }
        self.dispatch(Command::Cancel)
    }

    fn apply_batch(&mut self, bytes: &[u8]) -> PluginResult<()> {
        let batch: EventBatch = decode_batch(bytes)
            .map_err(|error| plugin_error("native-events", error.to_string()))?;
        let mut gap = batch.gap;
        for item in batch.events {
            gap |= self.sequence != 0 && item.sequence != self.sequence + 1;
            self.sequence = item.sequence;
            self.apply_event(item.event);
        }
        if gap {
            self.note("事件有丢失，已请求一次完整状态");
            self.pending = Some(Command::Snapshot);
        }
        Ok(())
    }

    fn apply_event(&mut self, event: Event) {
        match event {
            Event::Picked { path } => {
                if !path.is_empty() {
                    match self.browse.take() {
                        Some(Browse::Source) => {
                            self.source = path;
                            self.info = None;
                        }
                        Some(Browse::Target) => self.target = path,
                        None => {}
                    }
                }
            }
            Event::Detected { info, path } => {
                self.detecting = false;
                if self.target.trim().is_empty() {
                    self.target = suggest_target(&path, &info);
                }
                self.note(format!(
                    "检测到 {} 世界「{}」（{}），共 {} 个区块",
                    if info.edition == "java" { "Java" } else { "基岩" },
                    info.level_name,
                    info.version,
                    info.total_chunks()
                ));
                self.info = Some(info);
            }
            Event::DetectFailed { message, .. } => {
                self.detecting = false;
                self.problem = Some(message);
            }
            Event::Stage {
                stage,
                done,
                total,
                message,
            } => {
                self.running = stage != "start";
                self.stage = stage;
                self.done = done;
                self.total = total;
                if !message.is_empty() {
                    self.note(message);
                }
            }
            Event::Warnings { rows, unmapped_total } => {
                self.unmapped_total = unmapped_total;
                self.warnings = rows;
            }
            Event::Finished { report } => {
                self.running = false;
                self.stage.clear();
                self.unmapped_total = report.unmapped_total;
                self.warnings = report.warnings.clone();
                self.message = Some(format!(
                    "转换完成：{} 个区块 / {} 个方块，用时 {:.1} 秒",
                    report.chunks,
                    report.blocks,
                    report.elapsed_ms as f64 / 1000.0
                ));
                self.note(self.message.clone().unwrap_or_default());
                let _ = show_toast(ToastKind::Success, "世界转换完成");
                self.report = Some(report);
            }
            Event::Failed { message } => {
                self.running = false;
                self.problem = Some(message);
            }
            Event::Cancelled => {
                self.running = false;
                self.stage.clear();
                self.message = Some("转换已取消".to_string());
            }
            Event::Log { message, .. } => self.note(message),
            Event::State {
                running,
                stage,
                done,
                total,
                warnings,
                last_report,
            } => {
                self.running = running;
                self.stage = stage;
                self.done = done;
                self.total = total;
                self.unmapped_total = warnings.iter().map(|row| row.count).sum();
                self.warnings = warnings;
                if let Some(report) = last_report {
                    self.report = Some(report);
                }
            }
        }
    }
}

#[bmcbl_plugin]
impl Plugin for WorldPort {
    fn init(_context: PluginContext) -> PluginResult<Vec<Registration>> {
        PAGE.with_borrow_mut(|state| state.load_persisted());
        Ok(registrations! {
            page PAGE_ID, title = "基岩版与 Java 版世界互转", tool = Tool::new("世界转换").icon("network").order(10);
        })
    }

    fn handle_event(event: PluginEvent) -> PluginResult<()> {
        let changed = PAGE.with_borrow_mut(|state| -> PluginResult<bool> {
            let mut changed = false;
            if let PluginEventKind::Action(action) = &event.kind
                && event.page_id.as_deref().is_none_or(|page| page == PAGE_ID)
            {
                changed |= handle_action(state, action)?;
            }
            changed |= advance_jobs(state, &event)?;
            if state.native_ready() && state.read.is_none() {
                if let Native::Ready(instance) = state.native {
                    state.read = Some(native::read_events(instance, EVENT_WAIT_MS)?);
                }
            }
            Ok(changed)
        })?;
        if changed {
            invalidate_page(PAGE_ID)?;
        }
        Ok(())
    }

    fn render_page(request: PageRenderRequest) -> PluginResult<ViewTree> {
        Ok(PAGE.with_borrow(|state| page::render(state, &request.page_id)))
    }
}

fn handle_action(
    state: &mut State,
    action: &bmcbl_plugin_api::ActionEvent,
) -> PluginResult<bool> {
    let value = action.value.clone().unwrap_or_default();
    match action.action_id.as_str() {
        "source-input" => state.source = value,
        "target-input" => state.target = value,
        "browse-source" => {
            state.browse = Some(Browse::Source);
            state.dispatch(Command::BrowseSource)?;
        }
        "browse-target" => {
            state.browse = Some(Browse::Target);
            state.dispatch(Command::BrowseTarget)?;
        }
        "detect" => {
            if state.source.trim().is_empty() {
                return Err(plugin_error("missing-path", "请先填写源世界目录。"));
            }
            state.detecting = true;
            state.problem = None;
            state.dispatch(Command::Detect {
                path: state.source.trim().to_string(),
            })?;
        }
        "direction" => state.direction = value,
        "version" => state.version = value,
        "feature-blocks" => state.features.blocks = value == "true",
        "feature-block-entities" => state.features.block_entities = value == "true",
        "feature-entities" => state.features.entities = value == "true",
        "feature-biomes" => state.features.biomes = value == "true",
        "feature-players" => state.features.players = value == "true",
        "feature-maps" => state.features.maps = value == "true",
        "feature-level-dat" => state.features.level_dat = value == "true",
        "feature-ticks" => state.features.ticks = value == "true",
        "feature-village-interests" => state.features.village_interests = value == "true",
        "advanced" => state.show_advanced = !state.show_advanced,
        "jobs" => state.jobs = value.parse().unwrap_or(0),
        "compression" => state.compression = value.parse().unwrap_or(6),
        "overwrite" => state.overwrite = value == "true",
        "start" => state.start()?,
        "cancel" => state.cancel()?,
        "open-output" => {
            if let Some(report) = &state.report {
                state.dispatch(Command::OpenOutput {
                    path: report.target_path.clone(),
                })?;
            }
        }
        "digest-input" => state.digest_input = value,
        "save-digest" => {
            let digest = state.digest_input.trim().to_string();
            if digest.is_empty() {
                let _ = storage_delete(STORAGE_DIGEST);
                state.digest = None;
            } else {
                storage_set(STORAGE_DIGEST, &digest)?;
                state.digest = Some(digest);
            }
            state.native = Native::Missing;
            state.message = Some("已保存原生组件摘要".to_string());
        }
        other => {
            if other.starts_with("toggle-") {
                // Checkbox actions carry the *next* value in action_value.
                let feature = &other["toggle-".len()..];
                let target = value == "true";
                match feature {
                    "blocks" => state.features.blocks = target,
                    "block-entities" => state.features.block_entities = target,
                    "entities" => state.features.entities = target,
                    "biomes" => state.features.biomes = target,
                    "players" => state.features.players = target,
                    "maps" => state.features.maps = target,
                    "level-dat" => state.features.level_dat = target,
                    "ticks" => state.features.ticks = target,
                    "village-interests" => state.features.village_interests = target,
                    _ => return Ok(false),
                }
            }
        }
    }
    Ok(true)
}

fn advance_jobs(state: &mut State, event: &PluginEvent) -> PluginResult<bool> {
    let mut changed = false;
    // Native lifecycle.
    changed |= advance_lifecycle(state, event)?;
    if state.submit.as_ref().is_some_and(|job| job.is_ready(event)) {
        let job = state.submit.take().expect("matched submit job");
        match job.try_take() {
            Ok(Some(request)) => state.request = Some(request),
            Ok(None) => {}
            Err(error) => state.problem = Some(format!("提交原生命令失败：{}", error.message)),
        }
        changed = true;
    }
    if state
        .cancel_job
        .as_ref()
        .is_some_and(|job| job.is_ready(event))
    {
        let job = state.cancel_job.take().expect("matched cancel job");
        if let Err(error) = job.try_take() {
            state.problem = Some(format!("取消失败：{}", error.message));
        }
        changed = true;
    }
    if state.read.as_ref().is_some_and(|job| job.is_ready(event)) {
        let job = state.read.take().expect("matched read job");
        match job.try_take() {
            Ok(Some(bytes)) if !bytes.is_empty() => {
                state.apply_batch(&bytes)?;
                changed = true;
            }
            Ok(_) => {}
            Err(error) => state.problem = Some(format!("读取原生事件失败：{}", error.message)),
        }
    }
    if state.submit.is_none() && let Some(command) = state.pending.take() {
        state.submit_command(command)?;
        changed = true;
    }
    Ok(changed)
}

fn advance_lifecycle(state: &mut State, event: &PluginEvent) -> PluginResult<bool> {
    let native = std::mem::replace(&mut state.native, Native::Missing);
    match native {
        Native::Loading(job) if job.is_ready(event) => {
            match job.try_take() {
                Ok(Some(module)) => {
                    let job = native::create_instance(module, &[])?;
                    state.native = Native::Creating(module, job);
                }
                Ok(None) => state.native = Native::Loading(job),
                Err(error) => {
                    state.problem = Some(format!(
                        "加载原生组件失败（先在设置里批准 worldport_native.dll）：{}",
                        error.message
                    ));
                }
            }
            Ok(true)
        }
        Native::Creating(module, job) if job.is_ready(event) => {
            match job.try_take() {
                Ok(Some(instance)) => {
                    state.note("原生转换组件已就绪");
                    state.native = Native::Ready(instance);
                }
                Ok(None) => state.native = Native::Creating(module, job),
                Err(error) => {
                    let _ = native::unload(module);
                    state.problem = Some(format!("创建原生会话失败：{}", error.message));
                }
            }
            Ok(true)
        }
        other => {
            state.native = other;
            Ok(false)
        }
    }
}

fn suggest_target(source: &str, info: &WorldInfo) -> String {
    let path = std::path::Path::new(source);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("world");
    let suffix = if info.edition == "java" {
        "bedrock"
    } else {
        "java"
    };
    let parent = path.parent().unwrap_or_else(|| std::path::Path::new(""));
    parent
        .join(format!("{name}-{suffix}"))
        .display()
        .to_string()
}
