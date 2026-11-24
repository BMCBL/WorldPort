//! WorldPort `.bmcblx` plugin: a converter page registered on BMCBL's tools tab.
//!
//! The page only renders and forwards commands. Conversion runs in the
//! separately approved native module for the host platform through BMCBL's
//! native ABI, because the WebAssembly sandbox has no filesystem access.

use std::cell::RefCell;
use std::collections::VecDeque;

use bmcbl_plugin_api::app::{Job, download_verified};
use bmcbl_plugin_api::native::{self, InstanceHandle, ModuleHandle, Platform, RequestId};
use bmcbl_plugin_api::prelude::*;
use self::updates::Component;
use worldport_protocol::{
    Command, ConversionRequest, Edition, Event, EventBatch, Features, PruningPolicy,
    ReportSummary, WarningRow, WorldInfo, decode_batch, encode_command,
};

mod page;
mod updates;

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
    /// No native instance yet; a known digest does not imply installation.
    #[default]
    Missing,
    Resolving(Job<Platform>),
    /// Reading the plugin registry for this platform's component.
    FetchingRegistry(Job<String>),
    /// The registry was unreachable; reading the GitHub release index instead.
    FetchingRelease(Job<String>),
    /// Downloading the component BMCBL will verify and install.
    Downloading {
        component: Component,
        job: Job<String>,
    },
    Installing {
        component: Component,
        job: Job<()>,
    },
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
    platform: Option<Platform>,
    source: String,
    target: String,
    info: Option<WorldInfo>,
    detecting: bool,
    last_probe: Option<String>,
    detect_error: Option<String>,
    browse: Option<Browse>,
    /// Empty for "the other edition".
    direction: String,
    /// Empty for the default target version.
    version: String,
    features: Features,
    show_advanced: bool,
    jobs: u32,
    compression: u32,
    running: bool,
    stage: String,
    done: u64,
    total: u64,
    /// Stages that already reported and then moved on.
    finished_stages: Vec<String>,
    /// Milliseconds since the conversion started, as reported by the kernel.
    elapsed_ms: u64,
    /// Estimated milliseconds left in the current stage; `0` when unknown.
    eta_ms: u64,
    stage_rate: Option<f64>,
    show_log: bool,
    warnings: Vec<WarningRow>,
    unmapped_total: u64,
    report: Option<ReportSummary>,
    message: Option<String>,
    problem: Option<String>,
    digest: Option<String>,
    /// A load failed or installation was declined. Retry the verified install,
    /// not the same unapproved native.load call.
    install_required: bool,
    /// Set only by an explicit install action; startup may load but never download.
    install_requested: bool,
    digest_input: String,
    pending: Option<Command>,
    submit: Option<(Job<RequestId>, bool)>,
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
            platform: None,
            source: String::new(),
            target: String::new(),
            info: None,
            detecting: false,
            last_probe: None,
            detect_error: None,
            browse: None,
            direction: String::new(),
            version: String::new(),
            features: Features::default(),
            show_advanced: false,
            jobs: 0,
            compression: 6,
            running: false,
            stage: String::new(),
            done: 0,
            total: 0,
            finished_stages: Vec::new(),
            elapsed_ms: 0,
            eta_ms: 0,
            stage_rate: None,
            show_log: false,
            warnings: Vec::new(),
            unmapped_total: 0,
            report: None,
            message: None,
            problem: None,
            digest: None,
            install_required: false,
            install_requested: false,
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
        self.digest = stored;
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
            Native::Ready(_)
            | Native::Resolving(_)
            | Native::FetchingRegistry(_)
            | Native::FetchingRelease(_)
            | Native::Downloading { .. }
            | Native::Installing { .. }
            | Native::Loading(_)
            | Native::Creating(..) => {
                return Ok(());
            }
            Native::Missing => {}
        }
        if self.platform.is_none() {
            self.native = Native::Resolving(native::platform()?);
            return Ok(());
        }
        let Some(digest) = self.digest.clone().filter(|_| !self.install_required) else {
            if self.install_requested {
                return self.fetch_component();
            }
            self.install_required = true;
            return Ok(());
        };
        self.note("正在加载原生转换组件");
        self.native = Native::Loading(native::load(&digest)?);
        Ok(())
    }

    fn fetch_component(&mut self) -> PluginResult<()> {
        self.install_required = true;
        self.note("正在查找当前平台的原生转换组件");
        self.native = Native::FetchingRegistry(updates::fetch_registry()?);
        Ok(())
    }

    /// A matching digest proves identity, not installation or authorization.
    /// Always use the verified download/host approval route after a load failure.
    fn begin_component_download(&mut self, component: Component) -> PluginResult<()> {
        self.install_required = true;
        if let Some(expected) = self.platform.as_ref().and_then(packaged_digest)
            && expected != component.sha256()
        {
            return Err(plugin_error("native-digest", "原生组件与当前插件的内置 SHA-256 不一致，请更新插件后重试。"));
        }
        let Some(request) = component.request() else {
            return Err(plugin_error(
                "native-asset",
                "原生组件缺少可校验的下载信息。",
            ));
        };
        self.note(format!("正在下载原生组件 {}（{} 个候选来源）", component.version, request.urls.len()));
        let job = download_verified(request)?;
        self.native = Native::Downloading { component, job };
        Ok(())
    }

    fn native_error(&mut self, message: String) {
        self.running = false;
        self.detecting = false;
        self.stage.clear();
        self.pending = None;
        self.install_requested = false;
        self.problem = Some(message);
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
        self.submit = Some((
            native::submit_command(instance, &bytes)?,
            matches!(command, Command::Start { .. }),
        ));
        Ok(())
    }

    fn set_source(&mut self, value: String) {
        if self.source == value {
            return;
        }
        if let Some(info) = &self.info
            && self.target == suggest_target(&source_directory(&self.source), info)
        {
            self.target.clear();
        }
        self.source = value;
        self.info = None;
        self.detect_error = None;
        self.version.clear();
        if !self.detecting {
            self.last_probe = None;
        }
    }

    // One detection in flight. Rapid edits collapse into the latest path;
    // detection itself reads only metadata, never scans the world database.
    fn try_detect(&mut self) -> PluginResult<bool> {
        if !self.native_ready() || self.running || self.detecting
            || self.browse.is_some() || self.submit.is_some() || self.pending.is_some()
        {
            return Ok(false);
        }
        let path = source_directory(&self.source);
        if path.is_empty() || self.last_probe.as_deref() == Some(path.as_str()) {
            return Ok(false);
        }
        self.last_probe = Some(path.clone());
        self.detect_error = None;
        self.dispatch(Command::Detect { path })?;
        self.detecting = true;
        Ok(true)
    }

    fn start(&mut self) -> PluginResult<()> {
        if self.running {
            return Ok(());
        }
        let source = source_directory(&self.source);
        let target = unquote_path(&self.target);
        if self.info.is_none() || self.detecting || source.is_empty() || target.is_empty() {
            return Err(plugin_error("missing-path", "请先选择有效的源世界和输出目录。"));
        }
        if source == target {
            return Err(plugin_error("same-path", "源存档与输出目录不能相同。"));
        }
        let request = ConversionRequest {
            source,
            target: target.to_string(),
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
            overwrite: false,
        };
        self.problem = None;
        self.message = Some("正在启动转换…".to_string());
        self.warnings.clear();
        self.finished_stages.clear();
        self.elapsed_ms = 0;
        self.eta_ms = 0;
        self.stage_rate = None;
        self.show_log = false;
        self.unmapped_total = 0;
        self.report = None;
        self.persist_paths();
        self.dispatch(Command::Start { request })?;
        self.request = None;
        self.running = true;
        self.stage = "start".to_string();
        self.done = 0;
        self.total = 0;
        Ok(())
    }

    fn cancel(&mut self) -> PluginResult<()> {
        if matches!(self.pending, Some(Command::Start { .. })) {
            self.pending = None;
            self.apply_event(Event::Cancelled);
            return Ok(());
        }
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
                let browse = self.browse.take();
                if !path.is_empty() {
                    match browse {
                        Some(Browse::Source) => self.set_source(path),
                        Some(Browse::Target) => self.target = path,
                        None => {}
                    }
                }
            }
            Event::Detected { info, path } => {
                self.detecting = false;
                if path != source_directory(&self.source) {
                    return;
                }
                self.detect_error = None;
                if self.target.trim().is_empty() {
                    self.target = suggest_target(&path, &info);
                }
                self.note(format!(
                    "已识别 {} 世界「{}」（{}）",
                    if info.edition == "java" { "Java" } else { "基岩" },
                    info.level_name,
                    info.version
                ));
                self.info = Some(info);
            }
            Event::DetectFailed { path, message } => {
                self.detecting = false;
                if path == source_directory(&self.source) {
                    self.info = None;
                    self.detect_error = Some(message);
                }
            }
            Event::Stage {
                stage,
                done,
                total,
                message,
                elapsed_ms,
                eta_ms,
            } => {
                self.running = true;
                if self.message.as_deref() == Some("正在启动转换…") {
                    self.message = None;
                }
                if self.stage == stage {
                    let dt = elapsed_ms.saturating_sub(self.elapsed_ms);
                    let count = done.saturating_sub(self.done);
                    if dt >= 100 && count > 0 {
                        let sample = count as f64 * 1000.0 / dt as f64;
                        self.stage_rate = Some(self.stage_rate.map_or(sample, |old| old * 0.7 + sample * 0.3));
                    }
                } else {
                    self.stage_rate = None;
                }
                if self.stage != stage {
                    // Keep the finished stages so the panel can show what has
                    // already been done instead of only the current step.
                    if !self.stage.is_empty() && !self.finished_stages.contains(&self.stage) {
                        self.finished_stages.push(self.stage.clone());
                    }
                    self.stage = stage;
                }
                self.done = done;
                self.total = total;
                self.elapsed_ms = elapsed_ms;
                self.eta_ms = eta_ms;
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
                self.stage.clear();
                self.message = None;
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
        PAGE.with_borrow_mut(|state| {
            state.load_persisted();
            // A new Wasm generation needs a new native instance, not a new
            // download or approval. Resolve the platform and try native.load.
            // The host validates the persistent approval and actual file hash.
            if let Err(error) = state.ensure_native() {
                state.native_error(error.message);
            }
        });
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
                match handle_action(state, action) {
                    Ok(action_changed) => changed |= action_changed,
                    Err(error) => {
                        state.problem = Some(error.message);
                        changed = true;
                    }
                }
            }
            changed |= advance_jobs(state, &event)?;
            changed |= state.try_detect()?;
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
    if state.running && !matches!(action.action_id.as_str(), "cancel" | "show-log") {
        return Ok(false);
    }
    match action.action_id.as_str() {
        "source-input" => state.set_source(value),
        "source-input-submit" => {
            state.set_source(value);
            if !state.detecting {
                state.last_probe = None;
            }
        }
        "target-input" | "target-input-submit" => state.target = value,
        "show-log" => state.show_log = !state.show_log,
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
            if !state.detecting {
                state.last_probe = None;
            }
        }
        "direction" => {
            state.direction = value;
            state.version.clear();
        }
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
        "start" => state.start()?,
        "cancel" => state.cancel()?,
        "open-output" => {
            if let Some(report) = &state.report {
                state.dispatch(Command::OpenOutput {
                    path: report.target_path.clone(),
                })?;
            }
        }
        // The install card's button: no digest to type, the registry (or the
        // release index) supplies it and the host verifies it.
        "install-native" => {
            if matches!(state.native, Native::Missing) {
                state.problem = None;
                state.message = None;
                state.install_requested = true;
                if let Err(error) = state.ensure_native() {
                    state.native_error(error.message);
                }
            }
        }
        "digest-input" => state.digest_input = value,
        "save-digest" => {
            if !matches!(state.native, Native::Missing) {
                return Ok(false);
            }
            let digest = state.digest_input.trim().to_string();
            if digest.is_empty() {
                let _ = storage_delete(STORAGE_DIGEST);
                state.digest = state.platform.as_ref().and_then(packaged_digest);
            } else {
                if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    return Err(plugin_error("invalid-digest", "SHA-256 必须包含 64 个十六进制字符。"));
                }
                let digest = digest.to_ascii_lowercase();
                storage_set(STORAGE_DIGEST, &digest)?;
                state.digest = Some(digest);
            }
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

fn platform_key(platform: &Platform) -> String {
    format!("{}-{}-{}", platform.os, platform.arch, platform.environment)
}

fn packaged_digest(platform: &Platform) -> Option<String> {
    digest_for_platform(option_env!("WORLDPORT_NATIVE_COMPONENTS")?, &platform_key(platform))
        .map(str::to_owned)
}

fn digest_for_platform<'a>(components: &'a str, key: &str) -> Option<&'a str> {
    components.lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(platform, digest)| (platform == key).then_some(digest))
}

#[cfg(test)]
mod tests {
    use super::{Event, PAGE_ID, ReportSummary, State, ViewNode, digest_for_platform, page};

    #[test]
    fn source_paths_accept_quotes_and_level_dat() {
        assert_eq!(source_directory(r#""C:\Saves\World\level.dat""#), r"C:\Saves\World\");
        assert_eq!(source_directory("/home/test/world/level.dat"), "/home/test/world/");
        assert_eq!(source_directory(r"C:\Saves\World"), r"C:\Saves\World");
        assert_eq!(source_directory("/home/test/world"), "/home/test/world");
    }

    #[test]
    fn outdated_detection_result_cannot_replace_new_source() {
        let mut state = State::default();
        state.source = "/world/old".into();
        state.detecting = true;
        state.set_source("/world/new".into());
        state.apply_event(Event::Detected {
            info: WorldInfo { level_name: "old".into(), ..WorldInfo::default() },
            path: "/world/old".into(),
        });
        assert!(state.info.is_none());
        assert!(!state.detecting);
        assert!(state.target.is_empty());
        state.apply_event(Event::DetectFailed { path: "/world/old".into(), message: "old error".into() });
        assert!(state.detect_error.is_none());
    }

    #[test]
    fn selects_by_os_architecture_and_abi() {
        let components = "linux-x86_64-gnu=linux\nwindows-x86_64-msvc=windows\nwindows-aarch64-msvc=arm";
        assert_eq!(digest_for_platform(components, "windows-x86_64-msvc"), Some("windows"));
        assert_eq!(digest_for_platform(components, "windows-aarch64-msvc"), Some("arm"));
        assert_eq!(digest_for_platform(components, "linux-x86_64-musl"), None);
    }

    #[test]
    fn reopening_tool_shows_latest_progress_and_result() {
        let mut state = State::default();
        state.apply_event(Event::Stage {
            stage: "terrain".to_owned(),
            done: 47,
            total: 100,
            message: String::new(),
            elapsed_ms: 1_500,
            eta_ms: 3_000,
        });
        assert!(!page::render(&state, "another-page")
            .nodes
            .iter()
            .any(|node| matches!(node, ViewNode::Progress(_))));
        // Progress only means something once the conversion can run: without the
        // native component the page shows the install action and nothing else.
        let nodes = page::render(&state, PAGE_ID).nodes;
        assert!(
            !nodes.iter().any(|node| matches!(node, ViewNode::Progress(_))),
            "the page must not show progress while no component is installed"
        );
        assert!(
            nodes.iter().any(|node| matches!(node, ViewNode::Button(button) if button.action_id == "install-native")),
            "the page has to offer the install action instead"
        );

        state.apply_event(Event::Cancelled);
        assert!(!page::render(&state, PAGE_ID)
            .nodes
            .iter()
            .any(|node| matches!(node, ViewNode::Progress(_))));

        state.apply_event(Event::State {
            running: false,
            stage: String::new(),
            done: 100,
            total: 100,
            warnings: Vec::new(),
            last_report: Some(ReportSummary::default()),
        });
        // A finished report is still reachable without the component: the user may
        // simply have reopened the page after a conversion, so the result card is
        // the one thing that survives the gate.
        assert!(
            state.report.is_some() || state.native_ready(),
            "the report should either be shown or unreachable"
        );
    }
}

fn advance_jobs(state: &mut State, event: &PluginEvent) -> PluginResult<bool> {
    let mut changed = false;
    // Native lifecycle.
    changed |= advance_lifecycle(state, event)?;
    if state.submit.as_ref().is_some_and(|(job, _)| job.is_ready(event)) {
        let (job, start) = state.submit.take().expect("matched submit job");
        match job.try_take() {
            Ok(Some(request)) if start => state.request = Some(request),
            Ok(_) => {}
            Err(error) => {
                if start {
                    state.running = false;
                    state.stage.clear();
                }
                state.detecting = false;
                state.browse = None;
                state.problem = Some(format!("提交原生命令失败：{}", error.message));
            }
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
        Native::Resolving(job) if job.is_ready(event) => {
            match job.try_take() {
                Ok(Some(platform)) => {
                    state.digest = packaged_digest(&platform).or(state.digest.take());
                    state.digest_input = state.digest.clone().unwrap_or_default();
                    state.platform = Some(platform);
                    if let Err(error) = state.ensure_native() {
                        state.native_error(error.message);
                    }
                }
                Ok(None) => state.native = Native::Resolving(job),
                Err(error) => state.native_error(format!("检测宿主平台失败：{}", error.message)),
            }
            Ok(true)
        }
        Native::FetchingRegistry(job) if job.is_ready(event) => {
            match job.try_take() {
                Ok(Some(text)) => {
                    let platform = state.platform.clone().ok_or_else(|| {
                        plugin_error("native-platform", "宿主机平台尚未确定")
                    })?;
                    match updates::registry_component(&text, &platform) {
                        Ok(component) => {
                            if let Err(error) = state.begin_component_download(component) {
                                state.note(error.message);
                                state.native = Native::FetchingRelease(updates::fetch_release_index()?);
                            }
                        }
                        Err(error) => {
                            // The registry is a preferred mirror, not a
                            // requirement; a GitHub release works too.
                            state.note(format!("插件目录不可用：{}", error.message));
                            state.native =
                                Native::FetchingRelease(updates::fetch_release_index()?);
                        }
                    }
                }
                Ok(None) => state.native = Native::FetchingRegistry(job),
                Err(error) => {
                    state.note(format!("读取插件目录失败：{}", error.message));
                    state.native = Native::FetchingRelease(updates::fetch_release_index()?);
                }
            }
            Ok(true)
        }
        Native::FetchingRelease(job) if job.is_ready(event) => {
            match job.try_take() {
                Ok(Some(text)) => {
                    let platform = state.platform.clone().ok_or_else(|| {
                        plugin_error("native-platform", "宿主机平台尚未确定")
                    })?;
                    match updates::release_component(&text, &platform) {
                        Ok(component) => {
                            if let Err(error) = state.begin_component_download(component) {
                                state.native_error(error.message);
                            }
                        }
                        Err(error) => {
                            state.native_error(format!(
                                "没有找到当前平台的原生转换组件：{}",
                                error.message
                            ));
                        }
                    }
                }
                Ok(None) => state.native = Native::FetchingRelease(job),
                Err(error) => {
                    state.native_error(format!(
                        "无法获取原生转换组件：{}",
                        error.message
                    ));
                }
            }
            Ok(true)
        }
        Native::Downloading { component, job } if job.is_ready(event) => {
            match job.try_take() {
                Ok(Some(task)) => {
                    state.note(format!(
                        "正在校验原生组件 {}，请在 BMCBL 授权窗口确认安装",
                        component.version
                    ));
                    let job = native::download(&task, component.sha256())?;
                    state.native = Native::Installing { component, job };
                }
                Ok(None) => state.native = Native::Downloading { component, job },
                Err(error) => {
                    state.native_error(format!("下载原生组件失败：{}", error.message));
                }
            }
            Ok(true)
        }
        Native::Installing { component, job } if job.is_ready(event) => {
            match job.try_take() {
                Ok(Some(())) => {
                    let digest = component.sha256().to_owned();
                    state.digest = Some(digest.clone());
                    state.digest_input = digest.clone();
                    let _ = storage_set(STORAGE_DIGEST, &digest);
                    state.note(format!("原生组件 {} 已安装", component.version));
                    // install-download has already requested explicit host approval.
                    // Do not retry installation automatically if this load fails.
                    state.native = Native::Loading(native::load(&digest)?);
                }
                Ok(None) => state.native = Native::Installing { component, job },
                Err(error) => {
                    state.native_error(format!("安装原生组件失败：{}", error.message));
                }
            }
            Ok(true)
        }
        Native::Loading(job) if job.is_ready(event) => {
            match job.try_take() {
                Ok(Some(module)) => {
                    let job = native::create_instance(module, &[])?;
                    state.native = Native::Creating(module, job);
                }
                Ok(None) => state.native = Native::Loading(job),
                Err(error) => {
                    let needs_approval = error.message.contains("native component must be approved by the user");
                    // At startup a missing approval is a normal first-install
                    // state. Never start a download or prompt without a click.
                    if !state.install_required && needs_approval && state.install_requested {
                        state.note("原生组件尚未授权，开始校验安装");
                        if let Err(error) = state.fetch_component() {
                            state.native_error(error.message);
                        }
                    } else {
                        state.install_required = true;
                        state.install_requested = false;
                        if needs_approval {
                            state.note("当前组件尚未获得授权");
                            state.problem = None;
                        } else {
                            state.native_error(format!("加载原生组件失败：{}。请重试校验安装。", error.message));
                        }
                    }
                }
            }
            Ok(true)
        }
        Native::Creating(module, job) if job.is_ready(event) => {
            match job.try_take() {
                Ok(Some(instance)) => {
                    state.note("原生转换组件已就绪");
                    state.install_required = false;
                    state.install_requested = false;
                    if let Some(digest) = &state.digest {
                        let _ = storage_set(STORAGE_DIGEST, digest);
                    }
                    state.problem = None;
                    state.native = Native::Ready(instance);
                }
                Ok(None) => state.native = Native::Creating(module, job),
                Err(error) => {
                    let _ = native::unload(module);
                    state.native_error(format!("创建原生会话失败：{}", error.message));
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

/// Accept paths copied from Explorer, including quoted paths and level.dat.
fn unquote_path(value: &str) -> &str {
    let value = value.trim();
    value.strip_prefix('"').and_then(|value| value.strip_suffix('"')).unwrap_or(value)
}

fn source_directory(value: &str) -> String {
    let path = unquote_path(value);
    let file = path.rsplit(['\\', '/']).next().unwrap_or_default();
    if file.eq_ignore_ascii_case("level.dat") && path.len() > file.len() {
        path[..path.len() - file.len()].to_owned()
    } else {
        path.to_owned()
    }
}

fn suggest_target(source: &str, info: &WorldInfo) -> String {
    let suffix = if info.edition == "java" {
        "bedrock"
    } else {
        "java"
    };
    // The guest receives host paths, including Windows backslashes.
    format!("{}-{suffix}", source.trim_end_matches(['\\', '/']))
}
