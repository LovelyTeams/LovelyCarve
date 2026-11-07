//! 分析结果: the plugin results of this evidence by category, analyzer run
//! and record type; the records of one type; and one record's fields and
//! sources. Record layouts come from the plugins' record schemas.
mod records;
mod run;

use std::{collections::HashMap, rc::Rc, time::Duration};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex, h_resizable,
        input::{Input, InputEvent, InputState},
        list::ListItem,
        resizable_panel,
        scroll::ScrollableElement as _,
        table::{DataTable, TableEvent, TableState},
        tag::Tag,
        tree::{tree, TreeItem, TreeState},
        v_flex, ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, AppContext as _, ClipboardItem, Context, Entity, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Subscription, Task, WeakEntity, Window,
};
use lovelycarve_lib::{
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
use run::{AnalysisStarted, QuickAnalysis};

type ExtraIcon = gpui_kit::assets::IconName;

fn tree_item(node: &AnalysisTreeNode) -> TreeItem {
    let label = match node.kind {
        "result" if node.child_count > 0 => {
            format!("{} · {}", node.label, format::count(node.child_count))
        }
        _ => node.label.clone(),
    };
    TreeItem::new(node.node_id.clone(), label)
        .expanded(node.kind != "function")
        .children(node.children.iter().map(tree_item))
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
    roots: usize,
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
        let hub = EventHub::global(cx);
        let subscriptions = vec![
            cx.subscribe_in(&table, window, |this, table, event: &TableEvent, _, cx| {
                if let TableEvent::SelectRow(index) = event {
                    this.record = table.read(cx).delegate().row(*index).cloned();
                    cx.notify();
                }
            }),
            cx.observe(&table, |_, _, cx| cx.notify()),
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
            roots: 0,
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
                        this.roots = roots.len();
                        this.tree_error = None;
                        let items: Vec<TreeItem> = roots.iter().map(tree_item).collect();
                        let selected = this.selected_node.clone();
                        this.tree.update(cx, |state, cx| {
                            state.set_items(items, cx);
                            if let Some(index) = selected.and_then(|id| state.index_of(&id.into()))
                            {
                                state.set_selected_index(Some(index), cx);
                            }
                        });
                    }
                    Err(error) => this.tree_error = Some(error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
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

    fn show_quick_analysis(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let image_id = self.info.id.clone();
        let dialog = cx.new(|cx| QuickAnalysis::new(image_id, cx));
        let subscription = cx.subscribe_in(
            &dialog,
            window,
            |_, _, AnalysisStarted(count), window, cx| {
                window.close_dialog(cx);
                ui::notify_success(
                    window,
                    cx,
                    "已提交分析",
                    format!("{count} 个分析任务已排队，进度见任务中心。"),
                );
            },
        );
        self._subscriptions.push(subscription);
        window.open_dialog(cx, move |dialog_shell, _, _| {
            dialog_shell
                .title("快速分析")
                .w(px(560.))
                .child(dialog.clone())
        });
    }

    fn render_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(error) = &self.tree_error {
            return div()
                .p_3()
                .child(ui::error_text(error.clone(), cx))
                .into_any_element();
        }
        if self.roots == 0 {
            return v_flex()
                .size_full()
                .p_3()
                .gap_3()
                .child(ui::empty_state(
                    Icon::new(ExtraIcon::ScanSearch),
                    "暂无分析结果",
                    Some("用「快速分析」运行插件分析器，结果会按分类显示在这里。".into()),
                ))
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

impl Render for AnalysisPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        v_flex()
            .size_full()
            .child(
                ui::page_toolbar(cx)
                    .child(
                        div().w(px(360.)).child(
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
                    .child(
                        Button::new("quick-analysis")
                            .small()
                            .primary()
                            .icon(Icon::new(ExtraIcon::Play))
                            .label("快速分析")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.show_quick_analysis(window, cx)
                            })),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("analysis-panels")
                        .child(
                            resizable_panel()
                                .size(px(280.))
                                .size_range(px(200.)..px(520.))
                                .child(
                                    div()
                                        .size_full()
                                        .border_r_1()
                                        .border_color(cx.theme().border)
                                        .child(self.render_tree(cx)),
                                ),
                        )
                        .child(resizable_panel().child(if self.selected_node.is_some() {
                            DataTable::new(&self.table)
                                .bordered(false)
                                .small()
                                .into_any_element()
                        } else {
                            v_flex()
                                .size_full()
                                .child(ui::empty_state(
                                    Icon::new(IconName::Inbox),
                                    "选择一类结果",
                                    Some("在左侧结果树中选择记录类型查看明细。".into()),
                                ))
                                .into_any_element()
                        }))
                        .child(
                            resizable_panel()
                                .size(px(380.))
                                .size_range(px(260.)..px(760.))
                                .child(
                                    div()
                                        .size_full()
                                        .border_l_1()
                                        .border_color(cx.theme().border)
                                        .child(self.render_detail(cx)),
                                ),
                        ),
                ),
            )
            .child(ui::page_footer(
                div().truncate().child(footer_left),
                footer_right,
                cx,
            ))
    }
}
