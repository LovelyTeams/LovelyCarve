//! 全局搜索: the searches of this evidence, their hits and the hit detail.
//! Searches run in the backend and keep running when the page closes; the
//! page follows `global-search-progress` while a selected search runs.
mod create;
mod hits;

use std::{rc::Rc, time::Duration};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        menu::{DropdownMenu as _, PopupMenuItem},
        scroll::ScrollableElement as _,
        table::{TableEvent, TableState},
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, WeakEntity, Window,
};
use lovelycarve_lib::{
    commands::search::global_search as search_commands,
    core::{
        evidence::image::ImageInfo,
        search::global_search_store::{GlobalSearchHit, GlobalSearchMeta},
    },
    host::State,
};

use crate::{
    backend::{Backend, BackendEvent},
    events::EventHub,
    evidence::EvidenceTab,
    format, ui,
};
use create::{SearchForm, StartSearch};
use hits::HitTable;

type ExtraIcon = gpui_kit::assets::IconName;

const DISTINCT: [(&str, &str); 3] = [
    ("none", "全部命中"),
    ("file", "按文件去重"),
    ("sha256", "按哈希去重"),
];
/// Rows a running search streams in by itself; scrolling loads the rest.
const LIVE_ROWS: usize = 2_000;

pub struct SearchPanel {
    info: Rc<ImageInfo>,
    tab: WeakEntity<EvidenceTab>,
    history: Vec<GlobalSearchMeta>,
    history_visible: bool,
    loading_history: bool,
    history_error: Option<SharedString>,
    selected: Option<String>,
    table: Entity<TableState<HitTable>>,
    filter: Entity<InputState>,
    distinct: &'static str,
    hit: Option<GlobalSearchHit>,
    refresh: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl SearchPanel {
    pub fn new(
        info: Rc<ImageInfo>,
        tab: WeakEntity<EvidenceTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let table = cx.new(|cx| {
            TableState::new(HitTable::new(info.id.clone()), window, cx).col_movable(false)
        });
        let filter = cx.new(|cx| {
            InputState::new(window, cx).placeholder("在结果中筛选内容、文件名或路径，回车应用")
        });
        let hub = EventHub::global(cx);
        let subscriptions = vec![
            cx.subscribe_in(&table, window, |this, table, event: &TableEvent, _, cx| {
                if let TableEvent::SelectRow(index) = event {
                    this.hit = table.read(cx).delegate().row(*index).cloned();
                    cx.notify();
                }
            }),
            cx.observe(&table, |_, _, cx| cx.notify()),
            cx.subscribe_in(&filter, window, |this, input, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    let keyword = input.read(cx).value().to_string();
                    let distinct = this.distinct;
                    this.table.update(cx, |table, cx| {
                        table.delegate_mut().set_filter(Some(keyword), distinct, cx)
                    });
                }
            }),
            cx.subscribe(&hub, |this, _, event: &BackendEvent, cx| {
                if event.name == "global-search-progress" {
                    this.schedule_refresh(cx);
                }
            }),
        ];
        let mut panel = Self {
            info,
            tab,
            history: Vec::new(),
            history_visible: false,
            loading_history: true,
            history_error: None,
            selected: None,
            table,
            filter,
            distinct: "none",
            hit: None,
            refresh: None,
            _subscriptions: subscriptions,
        };
        panel.load_history(true, cx);
        panel
    }

    fn selected_meta(&self) -> Option<&GlobalSearchMeta> {
        let selected = self.selected.as_ref()?;
        self.history.iter().find(|meta| &meta.search_id == selected)
    }

    fn load_history(&mut self, select_first: bool, cx: &mut Context<Self>) {
        let image_id = self.info.id.clone();
        let listed = Backend::global(cx).run(move |_, state| async move {
            search_commands::global_search_history_list(State::new(&state), image_id, Some(100))
                .await
        });
        cx.spawn(async move |this, cx| {
            let listed = listed.await;
            this.update(cx, |this, cx| {
                this.loading_history = false;
                match listed {
                    Ok(history) => {
                        this.history = history;
                        this.history_error = None;
                        if select_first && this.selected.is_none() {
                            if let Some(first) =
                                this.history.first().map(|meta| meta.search_id.clone())
                            {
                                this.select(first, cx);
                                this.history_visible = true;
                            }
                        }
                    }
                    Err(error) => this.history_error = Some(error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select(&mut self, search_id: String, cx: &mut Context<Self>) {
        self.selected = Some(search_id.clone());
        self.hit = None;
        self.table.update(cx, |table, cx| {
            table.clear_selection(cx);
            table.delegate_mut().show(Some(search_id), cx);
        });
        cx.notify();
    }

    /// Follow a running search: at most one refresh every half second.
    fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        if self.refresh.is_some() {
            return;
        }
        self.refresh = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            this.update(cx, |this, cx| {
                this.refresh = None;
                this.load_history(false, cx);
                let running = this
                    .selected_meta()
                    .is_some_and(|meta| meta.status == "running");
                this.table.update(cx, |table, cx| {
                    let delegate = table.delegate_mut();
                    if running && delegate.loaded() < LIVE_ROWS {
                        delegate.fetch(cx);
                    }
                });
            })
            .ok();
        }));
    }

    fn start_search(
        &mut self,
        options: serde_json::Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let options = match serde_json::from_value(options) {
            Ok(options) => options,
            Err(error) => {
                ui::notify_error(window, cx, "搜索参数无效", error.to_string());
                return;
            }
        };
        let search_id = format!("search-{}", uuid::Uuid::new_v4());
        let image_id = self.info.id.clone();
        let started = search_id.clone();
        self.history_visible = true;
        self.select(search_id.clone(), cx);
        let search = Backend::global(cx).run(move |host, state| async move {
            search_commands::global_search_start(
                host,
                State::new(&state),
                image_id,
                started,
                options,
            )
            .await
        });
        cx.spawn_in(window, async move |this, cx| {
            let summary = search.await;
            this.update_in(cx, |this, window, cx| {
                match summary {
                    Ok(summary) if summary.status == "cancelled" => ui::notify_success(
                        window,
                        cx,
                        "搜索已取消",
                        format!(
                            "已保留 {} 条部分结果。",
                            format::count(summary.record_count)
                        ),
                    ),
                    Ok(summary) => ui::notify_success(
                        window,
                        cx,
                        "搜索完成",
                        format!(
                            "{} 个命中文件 · {} 处命中",
                            format::count(summary.matched_file_count),
                            format::count(summary.total_matches)
                        ),
                    ),
                    Err(error) => ui::notify_error(window, cx, "全局搜索失败", error.message),
                }
                this.load_history(false, cx);
                if this.selected.as_deref() == Some(search_id.as_str()) {
                    this.table
                        .update(cx, |table, cx| table.delegate_mut().reload(cx));
                }
            })
            .ok();
        })
        .detach();
        // List the new search once it has registered.
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(400))
                .await;
            this.update(cx, |this, cx| this.load_history(false, cx))
                .ok();
        })
        .detach();
    }

    fn show_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let form = cx.new(|cx| SearchForm::new(&self.info, window, cx));
        let subscription = cx.subscribe_in(
            &form,
            window,
            |this, _, StartSearch(options), window, cx| {
                window.close_dialog(cx);
                this.start_search(options.clone(), window, cx);
            },
        );
        self._subscriptions.push(subscription);
        window.open_dialog(cx, move |dialog, _, cx| {
            dialog
                .centered()
                .title(ui::dialog_heading(
                    IconName::Search,
                    "新建搜索",
                    "在检材的文件内容中查找关键词或正则表达式",
                    cx,
                ))
                .w(px(560.))
                .child(form.clone())
        });
    }

    fn delete_search(&mut self, search_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.entity().downgrade();
        let image_id = self.info.id.clone();
        ui::confirm(
            window,
            cx,
            "删除搜索记录",
            "删除这条搜索及其全部命中？运行中的搜索会先取消。",
            "删除",
            move |_, cx| {
                let image_id = image_id.clone();
                let search_id = search_id.clone();
                view.update(cx, |_, cx| {
                    let removing = search_id.clone();
                    let deleted = Backend::global(cx).run(move |_, state| async move {
                        search_commands::global_search_history_delete(
                            State::new(&state),
                            image_id,
                            removing,
                        )
                        .await
                    });
                    cx.spawn(async move |this, cx| {
                        let deleted = deleted.await;
                        this.update(cx, |this, cx| {
                            if deleted.is_ok()
                                && this.selected.as_deref() == Some(search_id.as_str())
                            {
                                this.selected = None;
                                this.hit = None;
                                this.table
                                    .update(cx, |table, cx| table.delegate_mut().show(None, cx));
                            }
                            this.load_history(false, cx);
                        })
                        .ok();
                    })
                    .detach();
                })
                .ok();
            },
        );
    }

    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(search_id), keyword, distinct) = self.table.read(cx).delegate().query() else {
            return;
        };
        let image_id = self.info.id.clone();
        let directory = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_default();
        let prompt = cx.prompt_for_new_path(&directory, Some(&format!("{search_id}.csv")));
        let backend = Backend::global(cx);
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = prompt.await else {
                return;
            };
            let destination = path.display().to_string();
            let exported = backend
                .run(move |_, state| async move {
                    search_commands::global_search_export(
                        State::new(&state),
                        image_id,
                        search_id,
                        destination,
                        keyword,
                        Some(distinct.to_owned()),
                    )
                    .await
                })
                .await;
            cx.update(|window, cx| match exported {
                Ok(result) => ui::notify_success(
                    window,
                    cx,
                    "导出完成",
                    format!(
                        "{} · {} 条",
                        result.output_path,
                        format::count(result.record_count)
                    ),
                ),
                Err(error) => ui::notify_error(window, cx, "导出失败", error.message),
            })
            .ok();
        })
        .detach();
    }

    fn render_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected.clone();
        v_flex()
            .size_full()
            .child(ui::history_header(
                "搜索记录",
                self.history.len(),
                Button::new("close-search-history")
                    .ghost()
                    .small()
                    .icon(IconName::PanelLeftClose)
                    .tooltip("收起搜索记录")
                    .accessibility_label("收起搜索记录")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.history_visible = false;
                        cx.notify();
                    })),
                cx,
            ))
            .when_some(self.history_error.clone(), |this, error| {
                this.child(div().px_3().child(ui::error_text(error, cx)))
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .gap_1()
                    .px_2()
                    .when(self.history.is_empty(), |this| {
                        this.child(
                            div()
                                .p_3()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("还没有搜索；点击「新建搜索」开始。"),
                        )
                    })
                    .children(self.history.iter().enumerate().map(|(index, meta)| {
                        let active = selected.as_deref() == Some(meta.search_id.as_str());
                        let search_id = meta.search_id.clone();
                        let delete_id = meta.search_id.clone();
                        let mut patterns = meta.keywords.clone();
                        patterns.extend(meta.regexes.iter().map(|regex| format!("/{regex}/")));
                        let status = match meta.status.as_str() {
                            "running" => Tag::info().xsmall().child("搜索中"),
                            "success" => Tag::success().xsmall().child("已完成"),
                            "cancelled" => Tag::secondary().xsmall().child("已取消"),
                            _ => Tag::danger().xsmall().child("失败"),
                        };
                        v_flex()
                            .id(("search-history", index))
                            .px_3()
                            .py_2()
                            .gap_1()
                            .rounded(cx.theme().radius)
                            .cursor_pointer()
                            .when(active, |this| this.bg(cx.theme().list_active))
                            .hover(|style| style.bg(cx.theme().list_hover))
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.select(search_id.clone(), cx)
                                }),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_sm()
                                            .child(patterns.join("、")),
                                    )
                                    .child(
                                        Button::new(("delete-search", index))
                                            .ghost()
                                            .xsmall()
                                            .icon(Icon::new(ExtraIcon::Trash))
                                            .accessibility_label("删除搜索")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                cx.stop_propagation();
                                                this.delete_search(delete_id.clone(), window, cx)
                                            })),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(status)
                                    .child(format!("{} 处命中", format::count(meta.total_matches)))
                                    .child(format::short_local_time(meta.started_ms)),
                            )
                    }))
                    .overflow_y_scrollbar(),
            )
    }

    fn render_detail(&self, hit: &GlobalSearchHit, cx: &mut Context<Self>) -> impl IntoElement {
        let locatable = hit.source_kind == "file";
        let tab = self.tab.clone();
        let node_id = hit.node_id.clone();
        let path = if hit.file_path.is_empty() {
            "—".to_owned()
        } else {
            hit.file_path.clone()
        };
        let copy_path = hit.file_path.clone();
        v_flex()
            .size_full()
            .px_4()
            .py_3()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .pt_1()
                    .child(
                        Button::new("locate-hit")
                            .small()
                            .icon(Icon::new(ExtraIcon::Locate))
                            .label("定位文件")
                            .disabled(!locatable)
                            .tooltip(if locatable {
                                "在文件列表中显示"
                            } else {
                                "未分配空间与分区间隙没有文件位置"
                            })
                            .on_click(move |_, window, cx| {
                                tab.update(cx, |tab, cx| {
                                    tab.locate_node(node_id.clone(), window, cx)
                                })
                                .ok();
                            }),
                    )
                    .child(
                        Button::new("copy-hit-path")
                            .small()
                            .ghost()
                            .icon(IconName::Copy)
                            .label("复制路径")
                            .disabled(copy_path.is_empty())
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(copy_path.clone()))
                            }),
                    ),
            )
            .child(ui::section_title("基本信息", cx))
            .child(ui::field_row("路径", path, cx))
            .child(ui::field_row("大小", format::bytes(hit.file_size), cx))
            .child(ui::field_row(
                "来源",
                hits::source_label(&hit.source_kind),
                cx,
            ))
            .when(!hit.md5.is_empty(), |this| {
                this.child(ui::field_row("MD5", ui::mono(hit.md5.clone(), cx), cx))
            })
            .when(!hit.sha256.is_empty(), |this| {
                this.child(ui::field_row(
                    "SHA256",
                    ui::mono(hit.sha256.clone(), cx).text_xs(),
                    cx,
                ))
            })
            .child(ui::section_title("命中信息", cx))
            .child(ui::field_row(
                "匹配方式",
                if hit.pattern_kind == "regex" {
                    "正则"
                } else {
                    "关键词"
                },
                cx,
            ))
            .child(ui::field_row("匹配内容", hit.pattern.clone(), cx))
            .child(ui::field_row(
                "偏移",
                ui::mono(
                    format!(
                        "{}（{}）",
                        format::hex_offset(hit.file_offset),
                        format::count(hit.file_offset)
                    ),
                    cx,
                ),
                cx,
            ))
            .child(ui::field_row("编码", hit.encoding.clone(), cx))
            .child(ui::field_row(
                "文件内命中",
                format!("{} 处", format::count(hit.match_count)),
                cx,
            ))
            .child(ui::section_title("命中上下文", cx))
            .child(
                div()
                    .p_3()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().muted)
                    .child(ui::mono(hit.context_full.clone(), cx).text_xs()),
            )
            .overflow_y_scrollbar()
    }
}

impl Render for SearchPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let history = self
            .history_visible
            .then(|| self.render_history(cx).into_any_element());
        let history_toggle = ui::history_toggle(
            "search-history-toggle",
            "搜索记录",
            self.history_visible,
            self.selected.is_none(),
        )
        .on_click(cx.listener(|this, _, _, cx| {
            this.history_visible = !this.history_visible;
            cx.notify();
        }));
        let new_search = Button::new("new-search")
            .primary()
            .small()
            .h(px(30.))
            .icon(IconName::Plus)
            .label("新建搜索")
            .on_click(cx.listener(|this, _, window, cx| this.show_form(window, cx)));
        if self.selected.is_none() {
            let state = if let Some(error) = &self.history_error {
                ui::empty_state(
                    IconName::TriangleAlert,
                    "读取搜索记录失败",
                    Some(error.clone()),
                )
                .child(
                    Button::new("retry-search-history")
                        .primary()
                        .icon(IconName::RefreshCw)
                        .label("重试")
                        .on_click(cx.listener(|this, _, _, cx| this.load_history(true, cx))),
                )
            } else {
                ui::empty_state(
                    IconName::Search,
                    if self.loading_history {
                        "正在读取搜索记录…"
                    } else {
                        "还没有搜索记录"
                    },
                    Some("输入关键词或正则，查找检材中的文件内容".into()),
                )
                .child(new_search.disabled(self.loading_history))
            };
            return ui::result_workspace(
                history,
                ui::workspace_start(history_toggle, state, cx),
                window,
                cx,
            );
        }
        let distinct = self.distinct;
        let current_distinct = DISTINCT
            .iter()
            .find(|(key, _)| *key == distinct)
            .map_or(DISTINCT[0].1, |(_, label)| *label);
        let view = cx.entity().downgrade();
        let table = self.table.read(cx).delegate();
        let footer_left = match self.selected_meta() {
            Some(meta) => {
                let mut patterns = meta.keywords.clone();
                patterns.extend(meta.regexes.iter().cloned());
                format!(
                    "{} · 已扫描 {} / {} 个文件{}",
                    patterns.join("、"),
                    format::count(meta.files_scanned),
                    format::count(meta.files_total),
                    if meta.read_errors > 0 {
                        format!(" · {} 个对象读取失败已跳过", meta.read_errors)
                    } else {
                        String::new()
                    }
                )
            }
            None => "未选择搜索".to_owned(),
        };
        let footer_right = format!(
            "已加载 {} / {} 条结果",
            format::count(table.loaded() as u64),
            format::count(table.total())
        );
        let detail = self.hit.clone().map(|hit| {
            v_flex()
                .size_full()
                .child(ui::detail_header(
                    hit.file_name.clone(),
                    Button::new("close-search-hit")
                        .ghost()
                        .small()
                        .icon(IconName::Close)
                        .accessibility_label("关闭命中详情")
                        .tooltip("关闭命中详情")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.hit = None;
                            this.table.update(cx, |table, cx| table.clear_selection(cx));
                            cx.notify();
                        })),
                    cx,
                ))
                .child(div().flex_1().min_h_0().child(self.render_detail(&hit, cx)))
                .into_any_element()
        });
        let main = v_flex()
            .size_full()
            .child(
                ui::page_toolbar(cx)
                    .child(history_toggle)
                    .child(
                        div().flex_1().min_w(px(120.)).child(
                            Input::new(&self.filter)
                                .small()
                                .cleanable(true)
                                .prefix(Icon::new(ExtraIcon::Funnel).small()),
                        ),
                    )
                    .child(
                        Button::new("search-distinct")
                            .small()
                            .h(px(30.))
                            .label(current_distinct)
                            .dropdown_caret(true)
                            .dropdown_menu(move |menu, _, _| {
                                DISTINCT.into_iter().fold(menu, |menu, (key, label)| {
                                    let view = view.clone();
                                    menu.item(
                                        PopupMenuItem::new(label)
                                            .checked(distinct == key)
                                            .on_click(move |_, _, cx| {
                                                view.update(cx, |this, cx| {
                                                    this.distinct = key;
                                                    let keyword =
                                                        this.filter.read(cx).value().to_string();
                                                    this.table.update(cx, |table, cx| {
                                                        table.delegate_mut().set_filter(
                                                            Some(keyword),
                                                            key,
                                                            cx,
                                                        )
                                                    });
                                                    cx.notify();
                                                })
                                                .ok();
                                            }),
                                    )
                                })
                            }),
                    )
                    .child(
                        Button::new("export-hits")
                            .small()
                            .icon(Icon::new(ExtraIcon::Download))
                            .label("导出 CSV")
                            .on_click(cx.listener(|this, _, window, cx| this.export(window, cx))),
                    )
                    .child(new_search),
            )
            .child(ui::detail_frame(
                "search-details",
                ui::data_table(&self.table),
                detail,
                window.viewport_size().width < px(if self.history_visible { 1352. } else { 1096. }),
                px(380.),
                cx,
            ))
            .child(ui::page_footer(
                div().truncate().child(footer_left),
                footer_right,
                cx,
            ));
        ui::result_workspace(history, main, window, cx)
    }
}
