//! 分析结果: the plugin results of this evidence by category, analyzer run
//! and record type; the records of one type; and one record's fields and
//! sources. Record layouts come from the plugins' record schemas.
mod records;
mod run;

use std::{collections::HashMap, rc::Rc, time::Duration};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        list::ListItem,
        scroll::ScrollableElement as _,
        table::{TableEvent, TableState},
        tag::Tag,
        tree::{tree, TreeItem, TreeState},
        v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, AppContext as _, ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
};
use lovelycarve_backend::{
    commands::analysis::analysis_results as analysis_commands,
    core::{
        analysis::results::models::{AnalysisRecord, AnalysisTreeNode},
        evidence::image::ImageInfo,
    },
    host::State,
};

use crate::{
    backend::{Backend, BackendEvent},
    events::EventHub,
    evidence::EvidenceTab,
    format, ui,
};
use records::{severity_tag, value_text, RecordSet, RecordTable};
pub use run::{AnalysisStarted, QuickAnalysis};

type ExtraIcon = gpui_kit::assets::IconName;

fn filtered_tree_item(node: &AnalysisTreeNode, keyword: &str) -> Option<TreeItem> {
    let matches = keyword.is_empty()
        || node.label.to_lowercase().contains(keyword)
        || node.target_label.to_lowercase().contains(keyword);
    let children: Vec<_> = node
        .children
        .iter()
        .filter_map(|child| filtered_tree_item(child, if matches { "" } else { keyword }))
        .collect();
    if !matches && children.is_empty() {
        return None;
    }
    let label = match node.kind {
        "result" if node.child_count > 0 => {
            format!("{} · {}", node.label, format::count(node.child_count))
        }
        _ => node.label.clone(),
    };
    Some(
        TreeItem::new(node.node_id.clone(), label)
            .expanded(!keyword.is_empty() || node.kind != "function")
            .children(children),
    )
}

fn index_nodes(nodes: &[AnalysisTreeNode], index: &mut HashMap<String, AnalysisTreeNode>) {
    for node in nodes {
        let mut flat = node.clone();
        flat.children.clear();
        index.insert(node.node_id.clone(), flat);
        index_nodes(&node.children, index);
    }
}

pub struct AnalysisPanel {
    info: Rc<ImageInfo>,
    tab: WeakEntity<EvidenceTab>,
    tree: Entity<TreeState>,
    nodes: HashMap<String, AnalysisTreeNode>,
    roots: Vec<AnalysisTreeNode>,
    directory_filter: Entity<InputState>,
    tree_loading: bool,
    tree_error: Option<SharedString>,
    selected_node: Option<String>,
    table: Entity<TableState<RecordTable>>,
    filter: Entity<InputState>,
    record: Option<AnalysisRecord>,
    reload: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl AnalysisPanel {
    pub fn new(
        info: Rc<ImageInfo>,
        tab: WeakEntity<EvidenceTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let tree = cx.new(|cx| TreeState::new(cx));
        let table = cx.new(|cx| TableState::new(RecordTable::new(), window, cx).col_movable(false));
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("搜索当前结果，回车确认"));
        let directory_filter = cx.new(|cx| InputState::new(window, cx).placeholder("搜索结果项…"));
        let hub = EventHub::global(cx);
        let subscriptions = vec![
            cx.subscribe_in(&table, window, |this, table, event: &TableEvent, _, cx| {
                if let TableEvent::SelectRow(index) = event {
                    this.record = table.read(cx).delegate().row(*index).cloned();
                    cx.notify();
                }
            }),
            cx.observe(&table, |_, _, cx| cx.notify()),
            cx.subscribe_in(
                &directory_filter,
                window,
                |this, _, event: &InputEvent, _, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.update_directory(cx);
                        cx.notify();
                    }
                },
            ),
            cx.subscribe_in(&filter, window, |this, input, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let keyword = input.read(cx).value().to_string();
                    this.table.update(cx, |table, cx| {
                        table.delegate_mut().set_keyword(keyword, cx)
                    });
                }
            }),
            // A finished plugin job publishes new results.
            cx.subscribe(&hub, |this, _, event: &BackendEvent, cx| {
                let finished = event.name == "task-entry"
                    && event
                        .payload
                        .pointer("/entry/source")
                        .and_then(serde_json::Value::as_str)
                        == Some("plugin")
                    && event
                        .payload
                        .pointer("/entry/status")
                        .and_then(serde_json::Value::as_str)
                        == Some("success");
                if finished {
                    this.schedule_reload(cx);
                }
            }),
        ];
        let mut panel = Self {
            info,
            tab,
            tree,
            nodes: HashMap::new(),
            roots: Vec::new(),
            directory_filter,
            tree_loading: true,
            tree_error: None,
            selected_node: None,
            table,
            filter,
            record: None,
            reload: None,
            _subscriptions: subscriptions,
        };
        panel.load_tree(cx);
        panel
    }

    fn load_tree(&mut self, cx: &mut Context<Self>) {
        let loaded = Backend::global(cx).run(|_, state| async move {
            analysis_commands::analysis_result_tree(State::new(&state)).await
        });
        let image_id = self.info.id.clone();
        cx.spawn(async move |this, cx| {
            let loaded = loaded.await;
            this.update(cx, |this, cx| {
                this.tree_loading = false;
                match loaded {
                    Ok(evidence) => {
                        // The tree covers the whole case; this page shows its own evidence.
                        let roots: Vec<AnalysisTreeNode> = evidence
                            .into_iter()
                            .filter(|node| node.image_id == image_id)
                            .flat_map(|node| node.children)
                            .collect();
                        let mut nodes = HashMap::new();
                        index_nodes(&roots, &mut nodes);
                        this.nodes = nodes;
                        this.roots = roots;
                        this.tree_error = None;
                        this.update_directory(cx);
                    }
                    Err(error) => this.tree_error = Some(error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn update_directory(&self, cx: &mut Context<Self>) {
        let keyword = self.directory_filter.read(cx).value().trim().to_lowercase();
        let items: Vec<_> = self
            .roots
            .iter()
            .filter_map(|node| filtered_tree_item(node, &keyword))
            .collect();
        let selected = self.selected_node.clone();
        self.tree.update(cx, |state, cx| {
            state.set_items(items, cx);
            state.set_selected_index(selected.and_then(|id| state.index_of(&id.into())), cx);
        });
    }

    fn schedule_reload(&mut self, cx: &mut Context<Self>) {
        if self.reload.is_some() {
            return;
        }
        self.reload = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(800))
                .await;
            this.update(cx, |this, cx| {
                this.reload = None;
                this.load_tree(cx);
            })
            .ok();
        }));
    }

    fn select_node(&mut self, node_id: String, cx: &mut Context<Self>) {
        let Some(node) = self.nodes.get(&node_id).cloned() else {
            return;
        };
        if node.kind != "result" {
            return;
        }
        self.selected_node = Some(node_id);
        self.record = None;
        let set = RecordSet {
            image_id: node.image_id.clone(),
            run_id: node.run_id.clone(),
            record_type: node.artifact_type.clone(),
            legacy: node.legacy,
        };
        self.table.update(cx, |table, cx| {
            table.clear_selection(cx);
            table.delegate_mut().show(Some(set), cx);
        });
        cx.notify();
    }

    fn render_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(error) = &self.tree_error {
            return div()
                .p_3()
                .child(ui::error_text(error.clone(), cx))
                .into_any_element();
        }
        if self.tree.read(cx).entry(0).is_none() {
            return div()
                .p_3()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .child(if self.tree_loading {
                    "正在读取分析结果…"
                } else if self.directory_filter.read(cx).value().trim().is_empty() {
                    "暂无分析结果"
                } else {
                    "未找到结果项"
                })
                .into_any_element();
        }
        let view = cx.entity().downgrade();
        let nodes = self.nodes.clone();
        tree(&self.tree, move |index, entry, selected, _, cx| {
            let id = entry.item().id.to_string();
            let kind = nodes.get(&id).map(|node| node.kind).unwrap_or("result");
            let icon = match kind {
                "class" => Icon::new(if entry.is_expanded() {
                    IconName::FolderOpen
                } else {
                    IconName::Folder
                }),
                "function" => Icon::new(ExtraIcon::Puzzle),
                _ => Icon::new(ExtraIcon::Table2),
            };
            let warnings = nodes.get(&id).map(|node| node.warning_count).unwrap_or(0);
            let theme = cx.theme();
            let view = view.clone();
            ListItem::new(index)
                .selected(selected)
                .pl(px(8.) + px(14.) * entry.depth() as f32)
                .child(
                    h_flex()
                        .gap_2()
                        .min_w_0()
                        .child(icon.small().text_color(if kind == "result" {
                            theme.muted_foreground
                        } else {
                            theme.primary
                        }))
                        .child(div().truncate().text_sm().child(entry.item().label.clone()))
                        .when(warnings > 0, |this| {
                            this.child(Tag::warning().xsmall().child(format!("{warnings} 告警")))
                        }),
                )
                .on_click(move |_, _, cx| {
                    let id = id.clone();
                    view.update(cx, |this, cx| this.select_node(id, cx)).ok();
                })
        })
        .size_full()
        .into_any_element()
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(record) = self.record.clone() else {
            return v_flex()
                .size_full()
                .child(ui::empty_state(
                    Icon::new(IconName::Info),
                    "未选择记录",
                    Some("在记录列表中选择一项查看详情。".into()),
                ))
                .into_any_element();
        };
        let schema = self.table.read(cx).delegate().schema().to_vec();
        let mut fields: Vec<(String, String)> = schema
            .iter()
            .map(|column| {
                (
                    column.label.clone(),
                    value_text(record.values.get(&column.id), column.kind),
                )
            })
            .filter(|(_, value)| !value.is_empty())
            .collect();
        for (key, value) in &record.values {
            if !schema.iter().any(|column| &column.id == key) {
                let text = value_text(Some(value), "text");
                if !text.is_empty() {
                    fields.push((key.clone(), text));
                }
            }
        }
        v_flex()
            .size_full()
            .px_4()
            .py_3()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(ExtraIcon::Table2).text_color(cx.theme().primary))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(record.title.clone()),
                    )
                    .when_some(record.severity.clone(), |this, severity| {
                        this.child(severity_tag(&severity))
                    })
                    .when(record.legacy, |this| {
                        this.child(Tag::secondary().xsmall().child("旧版结果"))
                    }),
            )
            .when(!record.sources.is_empty(), |this| {
                this.child(ui::section_title("源文件", cx)).children(
                    record.sources.iter().enumerate().map(|(index, source)| {
                        let label = match (&source.root, &source.path) {
                            (Some(root), Some(path)) => format!("{root}:{path}"),
                            (None, Some(path)) => path.clone(),
                            (Some(root), None) => root.clone(),
                            (None, None) => source
                                .note
                                .clone()
                                .unwrap_or_else(|| "（无路径）".to_owned()),
                        };
                        let node_id = source.node_id.clone();
                        let tab = self.tab.clone();
                        h_flex()
                            .gap_2()
                            .child(div().flex_1().min_w_0().truncate().text_sm().child(label))
                            .when_some(node_id, |this, node_id| {
                                this.child(
                                    Button::new(("locate-source", index))
                                        .xsmall()
                                        .ghost()
                                        .icon(Icon::new(ExtraIcon::Locate))
                                        .label("定位")
                                        .on_click(move |_, window, cx| {
                                            tab.update(cx, |tab, cx| {
                                                tab.locate_node(node_id.clone(), window, cx)
                                            })
                                            .ok();
                                        }),
                                )
                            })
                    }),
                )
            })
            .child(ui::section_title("记录内容", cx))
            .children(fields.into_iter().map(|(label, value)| {
                let copied = value.clone();
                ui::field_row(
                    label,
                    h_flex()
                        .gap_1()
                        .items_start()
                        .child(div().flex_1().min_w_0().child(value))
                        .child(
                            Button::new(SharedString::from(format!("copy-field-{copied}")))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Copy)
                                .accessibility_label("复制")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))
                                }),
                        ),
                    cx,
                )
            }))
            .overflow_y_scrollbar()
            .into_any_element()
    }
}

/// The result tree lives in the workspace navigation while its table stays in
/// the evidence page. Both use the same analysis entity and selection.
pub struct AnalysisDirectory {
    panel: Entity<AnalysisPanel>,
    _panel: Subscription,
}

impl AnalysisDirectory {
    pub fn new(panel: Entity<AnalysisPanel>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&panel, |_, _, cx| cx.notify());
        Self {
            panel,
            _panel: subscription,
        }
    }
}

impl Render for AnalysisDirectory {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.panel.update(cx, |panel, cx| {
            v_flex()
                .size_full()
                .pt_3()
                .px_2()
                .gap_1()
                .child(
                    Input::new(&panel.directory_filter)
                        .small()
                        .cleanable(true)
                        .prefix(Icon::new(IconName::Search).small()),
                )
                .child(div().flex_1().min_h_0().child(panel.render_tree(cx)))
        })
    }
}

impl Render for AnalysisPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let quick = Button::new("quick-analysis")
            .primary()
            .small()
            .h(px(30.))
            .icon(Icon::new(ExtraIcon::Play))
            .label("快速分析")
            .on_click(cx.listener(|this, _, _, cx| {
                this.tab
                    .update(cx, |_, cx| {
                        cx.emit(crate::evidence::EvidenceTabEvent::QuickAnalysis)
                    })
                    .ok();
            }));
        if self.selected_node.is_none() {
            let state = if let Some(error) = &self.tree_error {
                ui::empty_state(
                    IconName::TriangleAlert,
                    "分析结果读取失败",
                    Some(error.clone()),
                )
                .child(
                    Button::new("retry-analysis-directory")
                        .primary()
                        .label("重试")
                        .on_click(cx.listener(|this, _, _, cx| this.load_tree(cx))),
                )
            } else {
                ui::empty_state(
                    ExtraIcon::ScanSearch,
                    if self.tree_loading {
                        "正在读取分析结果…"
                    } else if self.roots.is_empty() {
                        "当前检材还没有分析结果"
                    } else {
                        "选择一个结果项"
                    },
                    Some(
                        if self.roots.is_empty() {
                            "选择分析功能，完成后结果会显示在这里。"
                        } else {
                            "在左侧结果目录中选择结果项，查看解析出的记录。"
                        }
                        .into(),
                    ),
                )
                .when(!self.tree_loading && self.roots.is_empty(), |state| {
                    state.child(quick)
                })
            };
            return ui::workspace_start(div(), state, cx);
        }
        let table = self.table.read(cx).delegate();
        let footer_right = format!(
            "已加载 {} / {} 条记录",
            format::count(table.loaded() as u64),
            format::count(table.total())
        );
        let footer_left = self
            .selected_node
            .as_ref()
            .and_then(|id| self.nodes.get(id))
            .map(|node| {
                let target = if node.target_label.is_empty() {
                    String::new()
                } else {
                    format!(" · {}", node.target_label)
                };
                format!("{}{target}", node.label)
            })
            .unwrap_or_else(|| "未选择结果".to_owned());
        let detail = self.record.as_ref().map(|record| {
            v_flex()
                .size_full()
                .child(ui::detail_header(
                    record.title.clone(),
                    Button::new("close-analysis-record")
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .accessibility_label("关闭记录详情")
                        .tooltip("关闭记录详情")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.record = None;
                            this.table.update(cx, |table, cx| table.clear_selection(cx));
                            cx.notify();
                        })),
                    cx,
                ))
                .child(div().flex_1().min_h_0().child(self.render_detail(cx)))
                .into_any_element()
        });
        v_flex()
            .size_full()
            .child(
                ui::page_toolbar(cx)
                    .child(
                        div().flex_1().min_w(px(140.)).max_w(px(360.)).child(
                            Input::new(&self.filter)
                                .small()
                                .cleanable(true)
                                .prefix(Icon::new(IconName::Search).small()),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("reload-analysis")
                            .small()
                            .ghost()
                            .icon(IconName::RefreshCw)
                            .tooltip("刷新结果")
                            .accessibility_label("刷新结果")
                            .on_click(cx.listener(|this, _, _, cx| this.load_tree(cx))),
                    )
                    .child(quick),
            )
            .child(ui::detail_frame(
                "analysis-details",
                ui::data_table(&self.table),
                detail,
                window.viewport_size().width < px(1096.),
                px(380.),
                cx,
            ))
            .child(ui::page_footer(
                div().truncate().child(footer_left),
                footer_right,
                cx,
            ))
    }
}
