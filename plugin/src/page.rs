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
    page = page
        .child(header(state))
        .child(source_panel(state))
        .child(target_panel(state));
    if let Some(info) = &state.info {
        page = page.child(info_card(info));
    }
    page = page.child(content_panel(state));
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
    if !state.native_ready() && state.digest.is_none() {
        page = page.child(native_setup(state));
    }
    if !state.log.is_empty() {
        page = page.child(log_panel(state));
    }
    page.finish()
}

fn header(state: &State) -> View {
    let status = match &state.native {
        Native::Ready(_) => ("原生组件已就绪", ThemeToken::Accent),
        Native::Loading(_) => ("正在加载原生组件", ThemeToken::SecondaryText),
        Native::Creating(..) => ("正在创建会话", ThemeToken::SecondaryText),
        Native::Missing if state.digest.is_some() => ("原生组件待加载", ThemeToken::SecondaryText),
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
    let mut body = panel().child(heading("plug", "源世界"));
    body = body.child(
        row()
            .full_width()
            .child(View::node(ViewNode::Input(InputNode {
                value: state.source.clone(),
                placeholder: "选择 Java 或基岩版世界文件夹".into(),
                action_id: "source-input".into(),
                action_value: None,
                style: ViewStyle {
                    full_width: true,
                    ..default_style()
                },
            })))
            .child(compact_action("浏览…", "browse-source", ThemeToken::Accent))
            .child(compact_action("检测", "detect", ThemeToken::SecondaryText)),
    );
    if state.detecting {
        body = body.child(detail("正在读取世界信息…"));
    }
    body.finish_view()
}

fn target_panel(state: &State) -> View {
    let java_target = state.direction != "bedrock";
    let mut body = panel().child(heading("settings", "输出"));
    body = body.child(
        row()
            .full_width()
            .child(View::node(ViewNode::Input(InputNode {
                value: state.target.clone(),
                placeholder: "输出世界目录（必须是空目录或不存在）".into(),
                action_id: "target-input".into(),
                action_value: None,
                style: ViewStyle {
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

fn info_card(info: &WorldInfo) -> View {
    let edition = if info.edition == "java" {
        "Java"
    } else {
        "基岩版"
    };
    let dimensions = info
        .dimensions
        .iter()
        .filter(|(_, chunks)| *chunks > 0)
        .map(|(dimension, chunks)| format!("{dimension} {chunks}"))
        .collect::<Vec<_>>()
        .join(" · ");
    panel()
        .background(ThemeToken::Surface)
        .child(
            row()
                .full_width()
                .child(badge_styled(
                    format!("{edition} · {}", info.version),
                    default_style(),
                ))
                .child(title(info.level_name.clone())),
        )
        .child(detail(format!(
            "区块 {}{} · 玩家 {} · 地图 {}",
            info.total_chunks(),
            if dimensions.is_empty() {
                String::new()
            } else {
                format!("（{dimensions}）")
            },
            info.players,
            info.maps
        )))
        .finish_view()
}

fn content_panel(state: &State) -> View {
    let features = &state.features;
    let mut body = panel().child(heading("star", "转换内容"));
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
        .child(View::node(checkbox(
            "覆盖已存在的输出目录",
            state.overwrite,
            "overwrite",
        )))
        .finish_view()
}

fn actions(state: &State) -> View {
    let mut body = row().full_width();
    if state.running {
        body = body.child(compact_action("取消转换", "cancel", ThemeToken::Danger));
    } else {
        body = body.child(View::node(button("开始转换", "start")));
    }
    if let Some(report) = &state.report {
        body = body.child(compact_action(
            "打开输出目录",
            "open-output",
            ThemeToken::SecondaryText,
        ));
        let _ = report;
    }
    body.finish_view()
}

fn progress_panel(state: &State) -> View {
    let stage = match state.stage.as_str() {
        "terrain" => "转换区块",
        "maps" => "转换地图",
        "players" => "转换玩家",
        "level.dat" => "写入世界设置",
        "finalize" => "收尾",
        "start" => "启动",
        other if other.is_empty() => "准备中",
        other => other,
    };
    let total = (state.total > 0).then_some(state.total);
    panel()
        .child(View::node(progress(
            format!("{stage} · {} / {}", state.done, state.total),
            state.done,
            total,
        )))
        .child(detail(if state.running {
            "转换期间可以离开本页，任务会继续；回来时进度会自动恢复。"
        } else {
            "等待开始"
        }))
        .finish_view()
}

fn report_panel(state: &State, report: &ReportSummary) -> View {
    let _ = state;
    let mut body = panel().child(heading("plug", "结果"));
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
    panel()
        .background(ThemeToken::Surface)
        .child(heading("alert", "启用原生转换组件"))
        .child(detail(
            "WorldPort 的文件读写在内置原生组件里完成（Wasm 沙箱没有文件系统）。\
             先构建 worldport_native.dll，在「设置 → 插件」批准它，再把它的 SHA-256 填到这里。",
        ))
        .child(
            row()
                .full_width()
                .child(View::node(ViewNode::Input(InputNode {
                    value: state.digest_input.clone(),
                    placeholder: "原生组件的 SHA-256".into(),
                    action_id: "digest-input".into(),
                    action_value: None,
                    style: ViewStyle {
                        text_size: TextSizeToken::Small,
                        full_width: true,
                        ..default_style()
                    },
                })))
                .child(compact_action("保存", "save-digest", ThemeToken::Accent)),
        )
        .finish_view()
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
        text: value.into(),
        style: ViewStyle {
            color: Some(color),
            emphasis,
            full_width,
            ..default_style()
        },
    }))
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

