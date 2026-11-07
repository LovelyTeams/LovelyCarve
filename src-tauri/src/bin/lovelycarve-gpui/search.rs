//! 全局搜索: the searches of this evidence, their hits and the hit detail.
//! Searches run in the backend and keep running when the page closes; the
//! page follows `global-search-progress` while a selected search runs.
mod create;
mod hits;

use std::{rc::Rc, time::Duration};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex, h_resizable,
        input::{Input, InputEvent, InputState},
        resizable_panel,
        scroll::ScrollableElement as _,
        table::{DataTable, TableEvent, TableState},
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
                match listed {
                    Ok(history) => {
                        this.history = history;
                        this.history_error = None;
                        if select_first && this.selected.is_none() {
                            if let Some(first) =
                                this.history.first().map(|meta| meta.search_id.clone())
                            {
                                this.select(first, cx);
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
        let form = cx.new(|cx| SearchForm::new(window, cx));
        let subscription = cx.subscribe_in(
            &form,
            window,
            |this, _, StartSearch(options), window, cx| {
                window.close_dialog(cx);
                this.start_search(options.clone(), window, cx);
            },
        );
        self._subscriptions.push(subscription);
        window.open_dialog(cx, move |dialog, _, _| {
            dialog.title("新建全局搜索").w(px(560.)).child(form.clone())
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
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("搜索记录"),
                    )
                    .child(
                        Tag::secondary()
                            .small()
                            .child(self.history.len().to_string()),
                    ),
            )
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

    fn render_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(hit) = self.hit.clone() else {
            return v_flex()
                .size_full()
                .child(ui::empty_state(
                    Icon::new(IconName::Info),
                    "未选择命中",
                    Some("在命中列表中选择一项查看上下文。".into()),
                ))
                .into_any_element();
        };
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
                    .child(Icon::new(IconName::File).text_color(cx.theme().primary))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child(hit.file_name.clone()),
                    ),
            )
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
            .into_any_element()
    }
}

impl Render for SearchPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let distinct = self.distinct;
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
        let has_selection = self.selected.is_some();
        v_flex()
            .size_full()
            .child(
                ui::page_toolbar(cx)
                    .child(
                        div().w(px(420.)).child(
                            Input::new(&self.filter)
                                .small()
                                .cleanable(true)
                                .prefix(Icon::new(ExtraIcon::Funnel).small()),
                        ),
                    )
                    .children(DISTINCT.iter().map(|(key, label)| {
                        let key = *key;
                        Button::new(SharedString::from(format!("distinct-{key}")))
                            .small()
                            .label(*label)
                            .when(distinct == key, |button| button.primary())
                            .when(distinct != key, |button| button.ghost())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.distinct = key;
                                let keyword = this.filter.read(cx).value().to_string();
                                this.table.update(cx, |table, cx| {
                                    table.delegate_mut().set_filter(Some(keyword), key, cx)
                                });
                                cx.notify();
                            }))
                    }))
                    .child(div().flex_1())
                    .child(
                        Button::new("export-hits")
                            .small()
                            .icon(Icon::new(ExtraIcon::Download))
                            .label("导出 CSV")
                            .disabled(!has_selection)
                            .on_click(cx.listener(|this, _, window, cx| this.export(window, cx))),
                    )
                    .child(
                        Button::new("new-search")
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .label("新建搜索")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.show_form(window, cx)),
                            ),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("search-panels")
                        .child(
                            resizable_panel()
                                .size(px(260.))
                                .size_range(px(180.)..px(480.))
                                .child(
                                    div()
                                        .size_full()
                                        .border_r_1()
                                        .border_color(cx.theme().border)
                                        .child(self.render_history(cx)),
                                ),
                        )
                        .child(
                            resizable_panel()
                                .child(DataTable::new(&self.table).bordered(false).small()),
                        )
                        .child(
                            resizable_panel()
                                .size(px(340.))
                                .size_range(px(280.)..px(800.))
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
