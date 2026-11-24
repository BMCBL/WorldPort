//! Tool-page rendering for the converter.

use bmcbl_plugin_api::prelude::*;
use bmcbl_plugin_api::{ButtonNode, Container, InputNode, TextNode, TextSizeToken};

use worldport_protocol::{ReportSummary, WorldInfo};

use super::{Native, State, PAGE_ID};

pub(super) fn render(state: &State, page_id: &str) -> ViewTree {
    if page_id != PAGE_ID {
        return View::column().full_width().finish();
    }
    let mut page = View::column().full_width().gap(12).padding(0);
    page = page.child(header(state));
    if !state.native_ready() {
        // Without the native component nothing on this page can work: the sandbox
        // has no filesystem access, so every control below would only be decoration
        // that fails on click. Show the one action that matters instead.
        page = page.child(native_setup(state));
        if let Some(feedback) = feedback(state) {
            page = page.child(feedback);
        }
        if !state.log.is_empty() {
            page = page.child(log_panel(state));
        }
        return page.finish();
    }
    // Conversion settings cannot change while a native job is active.
    // Keep the active task and cancellation visible instead of dead controls.
    if state.running {
        page = page.child(progress_panel(state));
        if state.problem.is_some() && let Some(feedback) = feedback(state) {
            page = page.child(feedback);
        }
        if state.show_log && !state.log.is_empty() {
            page = page.child(log_panel(state));
        }
        return page.finish();
    }
    page = page
        .child(source_panel(state))
        .child(target_panel(state))
        .child(content_panel(state));
    if state.show_advanced {
        page = page.child(advanced_panel(state));
    }
    page = page.child(actions(state));
    if state.running || !state.stage.is_empty() {
        page = page.child(progress_panel(state));
    }
    if let Some(report) = &state.report {
        page = page.child(report_panel(state, report));
    }
    if let Some(feedback) = feedback(state) {
        page = page.child(feedback);
    }
    if !state.log.is_empty() {
        page = page.child(compact_action(
            if state.show_log { "收起日志" } else { "查看日志" },
            "show-log", ThemeToken::SecondaryText,
        ));
        if state.show_log {
            page = page.child(log_panel(state));
        }
    }
    page.finish()
}

fn header(state: &State) -> View {
    let status = match &state.native {
        Native::Ready(_) => ("原生组件已就绪", ThemeToken::Accent),
        Native::Resolving(_) => ("正在识别宿主平台", ThemeToken::SecondaryText),
        Native::FetchingRegistry(_) | Native::FetchingRelease(_) => {
            ("正在查找原生组件", ThemeToken::SecondaryText)
        }
        Native::Downloading { .. } => ("正在下载原生组件", ThemeToken::SecondaryText),
        Native::Installing { .. } => ("正在安装原生组件", ThemeToken::SecondaryText),
        Native::Loading(_) => ("正在加载原生组件", ThemeToken::SecondaryText),
        Native::Creating(..) => ("正在创建会话", ThemeToken::SecondaryText),
        Native::Missing if state.install_required => ("等待安装或授权", ThemeToken::SecondaryText),
        Native::Missing if state.digest.is_some() || state.platform.is_none() => {
            ("原生组件待加载", ThemeToken::SecondaryText)
        }
        Native::Missing => ("原生组件未配置", ThemeToken::Danger),
    };
    row()
        .full_width()
        .child(title("世界转换"))
        .child(spacer(8))
        .child(text(status.0, status.1, false, false))
        .finish_view()
}

fn source_panel(state: &State) -> View {
    let mut body = panel().child(heading("folder", "1. 选择源存档"));
    body = body.child(
        row()
            .full_width()
            .child(View::node(ViewNode::Input(InputNode {
                value: state.source.clone(),
                placeholder: "选择或输入世界文件夹路径".into(),
                action_id: "source-input".into(),
                action_value: None,
                style: ViewStyle {
                    text_size: TextSizeToken::Small,
                    full_width: true,
                    ..default_style()
                },
            })))
            .child(compact_action("浏览…", "browse-source", ThemeToken::Accent)),
    );
    if state.detecting {
        body = body.child(detail("正在读取世界信息…"));
    } else if let Some(info) = &state.info {
        body = body.child(world_summary(info));
    } else if state.detect_error.is_some() {
        body = body.child(row().full_width()
            .child(detail("未识别到有效存档"))
            .child(compact_action("重试", "detect", ThemeToken::SecondaryText)));
    } else {
        body = body.child(detail("支持存档目录或 level.dat 文件路径，自动识别版本。"));
    }
    body.finish_view()
}

fn target_panel(state: &State) -> View {
    let java_target = match state.direction.as_str() {
        "bedrock" => false,
        "java" => true,
        _ => state.info.as_ref().is_none_or(|info| info.edition == "bedrock"),
    };
    let mut body = panel().child(heading("settings", "2. 设置目标存档"));
    body = body.child(
        row()
            .full_width()
            .child(View::node(ViewNode::Input(InputNode {
                value: state.target.clone(),
                placeholder: "选择或输入输出目录".into(),
                action_id: "target-input".into(),
                action_value: None,
                style: ViewStyle {
                    text_size: TextSizeToken::Small,
                    full_width: true,
                    ..default_style()
                },
            })))
            .child(compact_action("浏览…", "browse-target", ThemeToken::Accent)),
    );
    body = body.child(
        row()
            .full_width()
            .child(View::node(select(
                "方向",
                "direction",
                [
                    option("自动（转成另一版本）", ""),
                    option("转成 Java", "java"),
                    option("转成基岩版", "bedrock"),
                ],
                Some(state.direction.clone()),
            )))
            .child(View::node(select(
                "目标版本",
                "version",
                version_options(java_target),
                Some(state.version.clone()),
            ))),
    );
    body = body.child(detail("输出目录必须为空或尚未创建。"));
    if !java_target {
        body = body.child(detail("基岩版目标目前固定写 1.26.60 方块状态。"));
    }
    body.finish_view()
}

fn version_options(java_target: bool) -> Vec<SelectOption> {
    if java_target {
        vec![
            option("默认（最新 26.2）", ""),
            option("1.20.6", "1.20.6"),
            option("1.21.4", "1.21.4"),
            option("1.21.8", "1.21.8"),
            option("26.1", "26.1"),
            option("26.2", "26.2"),
        ]
    } else {
        vec![option("默认（1.26.60）", "")]
    }
}

/// Compact list item: original world icon, display name and actual edition/version.
fn world_summary(info: &WorldInfo) -> View {
    let edition = if info.edition == "java" { "Java 版" } else { "基岩版" };
    // An empty source uses the same fixed thumbnail slot as a real icon.
    let icon = View::node(image_options(
        // Old native modules may still return a large PNG. Keep the page
        // usable even when the optional preview exceeds legacy host limits.
        if info.map_preview.len() <= 8192 {
            info.map_preview.clone()
        } else {
            String::new()
        },
        "世界图标",
        ImageOptions::new()
            .height(40).min_height(40).max_height(40)
            .aspect_ratio(1, 1).corner_radius(6)
            .fallback("世界").fit(ImageFit::Cover),
    ));
    // This is inline metadata inside the source panel, not another card.
    // Container::new defaults to 24 px padding; explicitly clear it on the
    // text column so it cannot inflate the row by another 48 px.
    row().full_width().gap(10).padding(0)
        .child(icon)
        .child(View::column().full_width().padding(0).gap(2).align(Align::Start)
            .child(text(info.level_name.clone(), ThemeToken::PrimaryText, true, true))
            .child(View::node(ViewNode::Text(TextNode {
                text: bounded_text(format!("{edition} · {}", info.version)),
                style: ViewStyle {
                    color: Some(ThemeToken::SecondaryText),
                    text_size: TextSizeToken::Small,
                    full_width: true,
                    ..default_style()
                },
            }))))
        .finish_view()
}

fn content_panel(state: &State) -> View {
    let features = &state.features;
    let mut body = panel().child(heading("star", "3. 转换内容"));
    body = body.child(
        row()
            .full_width()
            .child(View::node(checkbox("方块", features.blocks, "feature-blocks")))
            .child(View::node(checkbox(
                "方块实体",
                features.block_entities,
                "feature-block-entities",
            )))
            .child(View::node(checkbox("实体 + NBT", features.entities, "feature-entities"))),
    );
    body = body.child(
        row()
            .full_width()
            .child(View::node(checkbox("生物群系", features.biomes, "feature-biomes")))
            .child(View::node(checkbox("玩家", features.players, "feature-players")))
            .child(View::node(checkbox("地图", features.maps, "feature-maps"))),
    );
    body = body.child(
        row()
            .full_width()
            .child(View::node(checkbox(
                "level.dat（种子/规则）",
                features.level_dat,
                "feature-level-dat",
            )))
            .child(View::node(checkbox(
                "计划刻 / 事件",
                features.ticks,
                "feature-ticks",
            ))),
    );
    body = body.child(row().full_width().child(View::node(checkbox(
        "村庄兴趣点（床 / 工作站 / 集合点）",
        features.village_interests,
        "feature-village-interests",
    ))));
    body = body.child(compact_action(
        if state.show_advanced {
            "收起高级选项"
        } else {
            "高级选项"
        },
        "advanced",
        ThemeToken::SecondaryText,
    ));
    body.finish_view()
}

fn advanced_panel(state: &State) -> View {
    panel()
        .child(heading("settings", "高级"))
        .child(
            row()
                .full_width()
                .child(View::node(select(
                    "并行线程",
                    "jobs",
                    [
                        option("自动（全部核心）", "0"),
                        option("1", "1"),
                        option("2", "2"),
                        option("4", "4"),
                        option("8", "8"),
                        option("16", "16"),
                    ],
                    Some(state.jobs.to_string()),
                )))
                .child(View::node(select(
                    "压缩级别",
                    "compression",
                    [
                        option("1（最快）", "1"),
                        option("3", "3"),
                        option("6（默认）", "6"),
                        option("9（最小）", "9"),
                    ],
                    Some(state.compression.to_string()),
                ))),
        )
        .finish_view()
}

fn actions(state: &State) -> View {
    let mut body = row().full_width();
    if state.running {
        body = body.child(compact_action("取消转换", "cancel", ThemeToken::Danger));
    } else {
        if state.info.is_none() || state.detecting || state.target.trim().is_empty() {
            body = body.child(detail("选择源存档和输出目录后即可开始转换。"));
        } else {
            body = body.child(View::node(button("开始转换", "start")));
        }
    }
    if state.report.is_some() {
        body = body.child(compact_action(
            "打开输出目录",
            "open-output",
            ThemeToken::SecondaryText,
        ));
    }
    body.finish_view()
}

fn progress_panel(state: &State) -> View {
    let stage = stage_label(&state.stage);
    let total = (state.total > 0).then_some(state.total);
    let percent = if state.total > 0 {
        // Integer maths avoids a float round trip for a percentage that is only
        // ever shown, never compared.
        Some((state.done.saturating_mul(100) / state.total).min(100))
    } else {
        None
    };
    let mut body = panel().child(
        row().full_width()
            .child(text("转换进度", ThemeToken::PrimaryText, true, true))
            .child(compact_action(
                if state.message.as_deref() == Some("正在取消…") { "正在取消…" } else { "取消转换" },
                "cancel", ThemeToken::Danger,
            )),
    );
    let mut bar = progress(
        if let Some(total) = total {
            format!("{stage} · {} / {total}", state.done)
        } else {
            stage.to_string()
        },
        state.done, total,
    );
    if let ViewNode::Progress(node) = &mut bar {
        node.style.full_width = true;
    }
    body = body.child(View::node(bar));
    if let Some(percent) = percent {
        body = body.child(text(format!("{percent}%"), ThemeToken::Accent, true, false));
    }
    let mut facts = Vec::new();
    if state.stage == "terrain" && let Some(rate) = state.stage_rate {
        facts.push(format!("{rate:.0} 区块/秒"));
    }
    facts.push(format!("已用 {}", duration_text(state.elapsed_ms)));
    if state.eta_ms > 0 {
        facts.push(format!("本阶段预计剩余 {}", duration_text(state.eta_ms)));
    }
    body.child(detail(facts.join(" · ")))
        .child(detail("转换期间可以离开本页，任务会继续。"))
        .child(compact_action(
            if state.show_log { "收起日志" } else { "查看日志" },
            "show-log", ThemeToken::SecondaryText,
        ))
        .finish_view()
}

/// Renders milliseconds the way a person reads them.
fn duration_text(ms: u64) -> String {
    let seconds = ms / 1000;
    if seconds < 60 {
        format!("{seconds} 秒")
    } else if seconds < 3600 {
        format!("{} 分 {} 秒", seconds / 60, seconds % 60)
    } else {
        format!("{} 小时 {} 分", seconds / 3600, (seconds % 3600) / 60)
    }
}

/// User-facing name for a stage id.
fn stage_label(stage: &str) -> &str {
    match stage {
        "terrain" => "转换区块",
        "chunk-index" => "读取区块索引",
        "structures" => "转换结构",
        "poi" => "转换村庄兴趣点",
        "maps" => "转换地图",
        "players" => "转换玩家",
        "level.dat" => "写入世界设置",
        "finalize" => "收尾",
        "start" => "启动",
        "create-world" => "创建目标存档",
        "village-interests" => "转换村庄兴趣点",
        "" => "准备中",
        other => other,
    }
}

fn report_panel(state: &State, report: &ReportSummary) -> View {
    let mut body = panel().child(heading("plug", "转换结果"));
    body = body.child(detail(format!(
        "{} → {} · 用时 {:.1} 秒 · 区块 {} · 方块 {}",
        report.source_version,
        report.target_version,
        report.elapsed_ms as f64 / 1000.0,
        report.chunks,
        report.blocks
    )));
    body = body.child(detail(format!(
        "实体 {} · 方块实体 {} · 玩家 {} · 地图 {}",
        report.entities, report.block_entities, report.players, report.maps
    )));
    if report.clean {
        body = body.child(text("没有需要丢弃的数据", ThemeToken::Accent, false, true));
    } else if !state.warnings.is_empty() {
        let mut rows = body;
        for warning in state.warnings.iter().take(5) {
            rows = rows.child(detail(format!(
                "· {}（{}×）：{}",
                warning.code, warning.count, warning.message
            )));
        }
        body = rows;
        if state.warnings.len() > 5 {
            body = body.child(detail(format!("另有 {} 条警告未在此处展开", state.warnings.len() - 5)));
        }
    }
    body.finish_view()
}

fn feedback(state: &State) -> Option<View> {
    if state.problem.is_none() && state.message.is_none() {
        return None;
    }
    let mut body = panel().background(ThemeToken::Surface);
    if let Some(problem) = &state.problem {
        body = body.child(text(problem.clone(), ThemeToken::Danger, false, true));
    }
    if let Some(message) = &state.message {
        body = body.child(detail(message.clone()));
    }
    Some(body.finish_view())
}

fn native_setup(state: &State) -> View {
    let busy = matches!(
        state.native,
        Native::Resolving(_)
            | Native::FetchingRegistry(_)
            | Native::FetchingRelease(_)
            | Native::Downloading { .. }
            | Native::Installing { .. }
            | Native::Loading(_)
            | Native::Creating(..)
    );
    let mut body = panel()
        .background(ThemeToken::Surface)
        .child(heading("alert", if busy { "准备原生转换组件" } else { "安装原生转换组件" }));
    if !busy {
        body = body.child(detail(
            "首次使用或组件版本变化时，需要校验并授权安装。已授权的相同组件会在下次启动时自动恢复。",
        ));
    }
    // The one action that matters while nothing is installed.
    if busy {
        body = body.child(detail(match &state.native {
            Native::Installing { .. } => "正在校验组件或等待 BMCBL 授权确认…",
            Native::Loading(_) => "正在加载已授权的原生组件…",
            _ => "正在准备原生组件…",
        }));
    } else {
        body = body.child(compact_action(
            if state.install_required { "重新校验并授权安装" } else { "安装并授权" },
            "install-native",
            ThemeToken::Accent,
        ));
    }
    if state.show_advanced {
        // Only reachable from the advanced panel, which is hidden until the
        // component is ready; kept for a build whose library has no registry entry.
        body = body.child(detail("手动填写组件的 SHA-256：")).child(
            row()
                .full_width()
                .child(View::node(ViewNode::Input(InputNode {
                    value: state.digest_input.clone(),
                    placeholder: "64 位十六进制摘要".into(),
                    action_id: "digest-input".into(),
                    action_value: None,
                    style: ViewStyle {
                        text_size: TextSizeToken::Small,
                        full_width: true,
                        ..default_style()
                    },
                })))
                .child(compact_action("保存", "save-digest", ThemeToken::Accent)),
        );
    }
    body.finish_view()
}

fn log_panel(state: &State) -> View {
    let mut body = panel().child(heading("activity", "日志"));
    for line in state.log.iter().rev().take(6) {
        body = body.child(detail(line.clone()));
    }
    body.finish_view()
}

// ── helpers ────────────────────────────────────────────────────────────────

fn panel() -> Container {
    View::column()
        .full_width()
        .gap(10)
        .padding(12)
        .corner_radius(8)
        .background(ThemeToken::Surface)
}

fn row() -> Container {
    View::row().padding(0).gap(8).align(Align::Center)
}

fn heading(icon_name: &str, label: &str) -> View {
    row()
        .child(View::node(bmcbl_plugin_api::icon(icon_name)))
        .child(View::node(title(label)))
        .finish_view()
}

fn detail(value: impl Into<String>) -> View {
    text(value, ThemeToken::SecondaryText, false, true)
}

fn text(
    value: impl Into<String>,
    color: ThemeToken,
    emphasis: bool,
    full_width: bool,
) -> View {
    View::node(ViewNode::Text(TextNode {
        text: bounded_text(value.into()),
        style: ViewStyle {
            color: Some(color),
            emphasis,
            full_width,
            ..default_style()
        },
    }))
}

// Bound diagnostic and world-provided text without splitting UTF-8.
fn bounded_text(mut value: String) -> String {
    const MAX_BYTES: usize = 8192;
    if value.len() > MAX_BYTES {
        let mut end = MAX_BYTES - '…'.len_utf8();
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        value.truncate(end);
        value.push('…');
    }
    value
}

fn compact_action(label: &str, action: &str, color: ThemeToken) -> View {
    View::node(ViewNode::Button(ButtonNode {
        label: label.into(),
        action_id: action.into(),
        action_value: None,
        style: ViewStyle {
            color: Some(color),
            text_size: TextSizeToken::Small,
            ..default_style()
        },
    }))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn world_summary_is_a_flat_compact_row() {
        let info = WorldInfo {
            level_name: "Bedrock level".into(),
            edition: "bedrock".into(),
            version: "1.26.60.5".into(),
            ..WorldInfo::default()
        };
        let tree = world_summary(&info).finish();
        let ViewNode::Container(row) = &tree.nodes[tree.root as usize] else {
            panic!("expected summary row");
        };
        assert_eq!(row.style.padding, 0);
        assert_eq!(row.style.background, None);
        assert_eq!(row.children.len(), 2);
        let ViewNode::Image(icon) = &tree.nodes[row.children[0] as usize] else {
            panic!("expected thumbnail");
        };
        assert_eq!(icon.height, Some(40));
        assert_eq!(icon.min_height, Some(40));
        assert_eq!(icon.max_height, Some(40));
        let ViewNode::Container(metadata) = &tree.nodes[row.children[1] as usize] else {
            panic!("expected metadata column");
        };
        assert_eq!(metadata.style.padding, 0);
        assert_eq!(metadata.style.gap, 2);
        assert_eq!(metadata.children.len(), 2);
    }
}
