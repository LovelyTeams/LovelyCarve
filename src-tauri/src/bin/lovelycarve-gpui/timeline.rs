//! 时间线: file times and analysis events of this evidence in one stream,
//! an event count chart by year, month or day, and the paged event list.
mod table;

use std::rc::Rc;

use gpui_kit::{
    component::{
        alert::Alert,
        button::{Button, ButtonVariants as _},
        chart::BarChart,
        checkbox::Checkbox,
        h_flex,
        input::{Input, InputEvent, InputState},
        progress::Progress,
        table::{DataTable, TableEvent, TableState},
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, WeakEntity, Window,
};
use lovelycarve_lib::{
    commands::timeline::timeline as timeline_commands,
    core::{
        evidence::image::ImageInfo,
        timeline::{
            timeline_project::{TimelineStatus, TimelineStatusInfo},
            timeline_store::{TimelineBucket, TimelineQuery, TimelineRow, TimelineUnit},
        },
    },
    host::State,
};

use crate::{backend::Backend, evidence::EvidenceTab, format, tasks::TaskStore, ui};
use table::{is_file_row, local_time, zone_hours, zone_label, TimelineTable};

type ExtraIcon = gpui_kit::assets::IconName;

const UNITS: [(TimelineUnit, &str); 3] = [
    (TimelineUnit::Year, "按年"),
    (TimelineUnit::Month, "按月"),
    (TimelineUnit::Day, "按日"),
];
/// Chart bars drawn at most.
const BUCKET_LIMIT: u64 = 400;

pub struct TimelinePanel {
    info: Rc<ImageInfo>,
    tab: WeakEntity<EvidenceTab>,
    status: Option<Result<TimelineStatusInfo, SharedString>>,
    building: bool,
    unit: TimelineUnit,
    show_deleted: bool,
    buckets: Vec<TimelineBucket>,
    table: Entity<TableState<TimelineTable>>,
    filter: Entity<InputState>,
    selected: Option<TimelineRow>,
    tasks: Entity<TaskStore>,
    _subscriptions: Vec<Subscription>,
}

impl TimelinePanel {
    pub fn new(
        info: Rc<ImageInfo>,
        tab: WeakEntity<EvidenceTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let table = cx.new(|cx| {
            TableState::new(TimelineTable::new(info.id.clone()), window, cx).col_movable(false)
        });
        let filter = cx
            .new(|cx| InputState::new(window, cx).placeholder("搜索事件内容或文件路径，回车应用"));
        let tasks = TaskStore::global(cx);
        let subscriptions = vec![
            cx.subscribe_in(
                &table,
                window,
                |this, table, event: &TableEvent, window, cx| match event {
                    TableEvent::SelectRow(index) => {
                        this.selected = table.read(cx).delegate().row(*index).cloned();
                        cx.notify();
                    }
                    TableEvent::DoubleClickedRow(index) => {
                        if let Some(row) = table.read(cx).delegate().row(*index).cloned() {
                            this.locate(&row, window, cx);
                        }
                    }
                    _ => {}
                },
            ),
            cx.observe(&table, |_, _, cx| cx.notify()),
            cx.subscribe_in(&filter, window, |this, _, event: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = event {
                    this.apply(cx);
                }
            }),
            cx.observe(&tasks, |this, _, cx| {
                if this.building {
                    cx.notify();
                }
            }),
        ];
        let mut panel = Self {
            info,
            tab,
            status: None,
            building: false,
            unit: TimelineUnit::Year,
            show_deleted: false,
            buckets: Vec::new(),
            table,
            filter,
            selected: None,
            tasks,
            _subscriptions: subscriptions,
        };
        panel.refresh_status(true, cx);
        panel
    }

    fn query(&self, cx: &Context<Self>) -> TimelineQuery {
        let text = self.filter.read(cx).value().trim().to_owned();
        TimelineQuery {
            time_zone_hours: zone_hours(),
            time_unit: self.unit,
            text: (!text.is_empty()).then_some(text),
            show_delete: self.show_deleted,
            ..TimelineQuery::default()
        }
    }

    fn ready(&self) -> bool {
        matches!(&self.status, Some(Ok(info)) if !matches!(info.status, TimelineStatus::BuildRequired))
    }

    fn refresh_status(&mut self, load: bool, cx: &mut Context<Self>) {
        let image_id = self.info.id.clone();
        let status = Backend::global(cx).run(move |_, state| async move {
            timeline_commands::timeline_status(State::new(&state), image_id).await
        });
        cx.spawn(async move |this, cx| {
            let status = status.await;
            this.update(cx, |this, cx| {
                this.status = Some(status.map_err(|error| error.message));
                if load && this.ready() {
                    this.apply(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Reload the chart and the events for the current filters.
    fn apply(&mut self, cx: &mut Context<Self>) {
        let query = self.query(cx);
        self.selected = None;
        self.table.update(cx, |table, cx| {
            table.clear_selection(cx);
            table.delegate_mut().set_query(query.clone(), cx);
        });
        let image_id = self.info.id.clone();
        let counted = Backend::global(cx).run(move |_, state| async move {
            timeline_commands::timeline_count(
                State::new(&state),
                image_id,
                query,
                Some(BUCKET_LIMIT),
                None,
            )
            .await
        });
        cx.spawn(async move |this, cx| {
            let counted = counted.await;
            this.update(cx, |this, cx| {
                match counted {
                    Ok(buckets) => this.buckets = buckets,
                    Err(error) => tracing::warn!("timeline_count: {error}"),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn build(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.building {
            return;
        }
        self.building = true;
        let image_id = self.info.id.clone();
        let started = Backend::global(cx).run(move |host, state| async move {
            timeline_commands::timeline_start(host, State::new(&state), image_id).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let summary = started.await;
            this.update_in(cx, |this, window, cx| {
                this.building = false;
                match summary {
                    Ok(summary) if summary.status == "cancelled" => {
                        ui::notify_success(window, cx, "时间线构建已取消", "已保留构建前的时间线。")
                    }
                    Ok(summary) => ui::notify_success(
                        window,
                        cx,
                        "时间线已就绪",
                        format!(
                            "文件 {} 条 · 事件 {} 条",
                            format::count(summary.file_rows),
                            format::count(summary.event_rows)
                        ),
                    ),
                    Err(error) => ui::notify_error(window, cx, "时间线构建失败", error.message),
                }
                this.refresh_status(true, cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn locate(&mut self, row: &TimelineRow, window: &mut Window, cx: &mut Context<Self>) {
        if !is_file_row(row) {
            return;
        }
        let node_id = row.oid.clone();
        self.tab
            .update(cx, |tab, cx| tab.locate_node(node_id, window, cx))
            .ok();
    }

    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.query(cx);
        let image_id = self.info.id.clone();
        let directory = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_default();
        let prompt = cx.prompt_for_new_path(&directory, Some("timeline.csv"));
        let backend = Backend::global(cx);
        cx.spawn_in(window, async move |_, cx| {
            let Ok(Ok(Some(path))) = prompt.await else {
                return;
            };
            let destination = path.display().to_string();
            let exported = backend
                .run(move |_, state| async move {
                    timeline_commands::timeline_export(
                        State::new(&state),
                        image_id,
                        query,
                        destination,
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
                        path.display(),
                        format::count(result.record_count)
                    ),
                ),
                Err(error) => ui::notify_error(window, cx, "导出失败", error.message),
            })
            .ok();
        })
        .detach();
    }

    fn render_start(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, description) = match &self.status {
            None => ("正在读取时间线…", None),
            Some(Err(error)) => ("读取时间线失败", Some(error.clone())),
            Some(Ok(_)) => (
                "还没有时间线",
                Some("汇总文件时间与分析事件，查看事件发生顺序。".into()),
            ),
        };
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_4()
            .child(ui::empty_state(
                Icon::new(ExtraIcon::ChartColumn),
                title,
                description,
            ))
            .when(self.status.is_some(), |this| {
                this.child(
                    Button::new("create-timeline")
                        .primary()
                        .icon(IconName::Plus)
                        .label(if self.building {
                            "正在创建…"
                        } else {
                            "创建时间线"
                        })
                        .disabled(self.building)
                        .on_click(cx.listener(|this, _, window, cx| this.build(window, cx))),
                )
            })
            .when(self.building, |this| this.child(self.render_progress(cx)))
    }

    fn render_progress(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let task_id = format!("timeline-build-{}", self.info.id);
        let row = self.tasks.read(cx).row(&task_id).cloned();
        let percent = row.as_ref().and_then(|row| row.percent());
        v_flex()
            .w(px(420.))
            .gap_2()
            .child(
                Progress::new("timeline-progress")
                    .when_some(percent, |progress, percent| progress.value(percent))
                    .when(percent.is_none(), |progress| progress.loading(true)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        row.map(|row| row.message())
                            .unwrap_or_else(|| "正在汇总…".to_owned()),
                    ),
            )
    }

    fn render_chart(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let unit = self.unit;
        let buckets = self.buckets.clone();
        v_flex()
            .w_full()
            .gap_2()
            .px_3()
            .pt_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("事件趋势"),
                    )
                    .child(div().flex_1())
                    .children(UNITS.iter().map(|(value, label)| {
                        let value = *value;
                        Button::new(SharedString::from(format!("unit-{label}")))
                            .xsmall()
                            .label(*label)
                            .when(unit == value, |button| button.primary())
                            .when(unit != value, |button| button.ghost())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.unit = value;
                                this.apply(cx);
                            }))
                    })),
            )
            .child(div().h(px(180.)).w_full().child(if buckets.is_empty() {
                div()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("没有可统计的事件")
                    .into_any_element()
            } else {
                BarChart::new(buckets)
                    .band(|bucket: &TimelineBucket| SharedString::from(bucket.date_str.clone()))
                    .value(|bucket: &TimelineBucket| bucket.count as f64)
                    .value_axis(true)
                    .tooltip_value(|_, value| {
                        SharedString::from(format!("{} 条", format::count(value as u64)))
                    })
                    .into_any_element()
            }))
    }

    fn render_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(row) = self.selected.clone() else {
            return div().into_any_element();
        };
        let locatable = is_file_row(&row);
        let tab = self.tab.clone();
        let node_id = row.oid.clone();
        v_flex()
            .w(px(380.))
            .h_full()
            .flex_shrink_0()
            .border_l_1()
            .border_color(cx.theme().border)
            .px_4()
            .py_3()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                            .child("事件详情"),
                    )
                    .child(
                        Button::new("close-event")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Close)
                            .accessibility_label("关闭详情")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.selected = None;
                                cx.notify();
                            })),
                    ),
            )
            .child(ui::field_row(
                "时间",
                ui::mono(local_time(row.tl_time), cx),
                cx,
            ))
            .child(ui::field_row("事件类型", row.tl_flag.clone(), cx))
            .child(ui::field_row("来源", row.from_app.clone(), cx))
            .when(row.deleted, |this| {
                this.child(ui::field_row(
                    "状态",
                    Tag::danger().xsmall().child("源记录已删除"),
                    cx,
                ))
            })
            .child(ui::section_title("内容", cx))
            .child(div().text_sm().child(row.content.clone()))
            .when(!row.extra.is_empty(), |this| {
                this.child(ui::section_title("附加信息", cx))
                    .child(ui::mono(row.extra.clone(), cx).text_xs())
            })
            .child(
                div().pt_3().child(
                    Button::new("locate-event")
                        .small()
                        .icon(Icon::new(ExtraIcon::Locate))
                        .label("定位文件")
                        .disabled(!locatable)
                        .tooltip(if locatable {
                            "在文件列表中显示"
                        } else {
                            "分析事件没有文件位置"
                        })
                        .on_click(move |_, window, cx| {
                            tab.update(cx, |tab, cx| tab.locate_node(node_id.clone(), window, cx))
                                .ok();
                        }),
                ),
            )
            .into_any_element()
    }
}

impl Render for TimelinePanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.ready() {
            return self.render_start(cx).into_any_element();
        }
        let Some(Ok(info)) = self.status.clone() else {
            return div().into_any_element();
        };
        let (status_tag, stale) = match info.status {
            TimelineStatus::Ready => (Tag::success().small().child("已就绪"), false),
            TimelineStatus::Stale => (Tag::warning().small().child("有新数据"), true),
            TimelineStatus::Running => (Tag::info().small().child("构建中"), false),
            TimelineStatus::BuildRequired => (Tag::secondary().small().child("未创建"), false),
        };
        let building = self.building || matches!(info.status, TimelineStatus::Running);
        let span = match (info.start_time, info.end_time) {
            (Some(start), Some(end)) => format!(
                "{} → {} · {}",
                local_time(start).get(..10).unwrap_or_default(),
                local_time(end).get(..10).unwrap_or_default(),
                zone_label()
            ),
            _ => "暂无时间范围".to_owned(),
        };
        let table = self.table.read(cx).delegate();
        let loaded = format!(
            "已加载 {} / {} 条",
            format::count(table.loaded() as u64),
            format::count(table.total())
        );
        let show_deleted = self.show_deleted;
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
                    .child(
                        Checkbox::new("show-deleted")
                            .label("包含已删除")
                            .checked(show_deleted)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.show_deleted = *checked;
                                this.apply(cx);
                            })),
                    )
                    .child(div().flex_1())
                    .child(status_tag)
                    .child(
                        Button::new("rebuild-timeline")
                            .small()
                            .icon(IconName::RefreshCw)
                            .label(if building {
                                "构建中…"
                            } else if stale {
                                "更新时间线"
                            } else {
                                "重建时间线"
                            })
                            .when(stale, |button| button.primary())
                            .disabled(building)
                            .on_click(cx.listener(|this, _, window, cx| this.build(window, cx))),
                    )
                    .child(
                        Button::new("export-timeline")
                            .small()
                            .icon(Icon::new(ExtraIcon::Download))
                            .label("导出结果")
                            .on_click(cx.listener(|this, _, window, cx| this.export(window, cx))),
                    ),
            )
            .when(building, |this| {
                this.child(div().px_3().pt_2().child(self.render_progress(cx)))
            })
            .when_some(
                info.meta.as_ref().and_then(|meta| meta.error.clone()),
                |this, error| {
                    this.child(
                        div()
                            .px_3()
                            .pt_2()
                            .child(Alert::error("timeline-error", error)),
                    )
                },
            )
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(self.render_chart(cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .pt_2()
                                    .child(DataTable::new(&self.table).bordered(false).small()),
                            ),
                    )
                    .child(self.render_detail(cx)),
            )
            .child(ui::page_footer(
                h_flex()
                    .gap_3()
                    .child(span)
                    .child(format!("事件 {}", format::count(info.record_count)))
                    .when_some(info.meta.clone(), |this, meta| {
                        this.child(format!("文件记录 {}", format::count(meta.file_rows)))
                            .child(format!("分析事件 {}", format::count(meta.event_rows)))
                    }),
                loaded,
                cx,
            ))
            .into_any_element()
    }
}
