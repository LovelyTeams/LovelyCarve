//! The titlebar task hub and quick analysis for the active evidence.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::{
    actions,
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{Input, InputEvent, InputState},
        progress::Progress,
        scroll::ScrollableElement as _,
        tab::{Tab, TabBar},
        tag::Tag,
        v_flex, ActiveTheme as _, Icon, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, App, AppContext as _, Context, Div, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Task, Window,
};

use super::{status_label, TaskRow, TaskStore};
use crate::{
    analysis::{AnalysisStarted, QuickAnalysis},
    evidence::Page,
    format, ui,
};

type ExtraIcon = gpui_kit::assets::IconName;

actions!(lovelycarve, [DismissTaskHub]);

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", DismissTaskHub, Some("TaskHub"))]);
}

pub enum HubEvent {
    Close,
    OpenCases,
    OpenPage(Page),
}

impl EventEmitter<HubEvent> for TaskCenter {}

pub struct TaskCenter {
    store: Entity<TaskStore>,
    search: Entity<InputState>,
    focus: FocusHandle,
    evidence: Option<(String, SharedString)>,
    has_case: bool,
    show_quick: bool,
    quick: Option<(Entity<QuickAnalysis>, Subscription)>,
    collapsed: [bool; 2],
    /// Keeps elapsed times of running tasks moving.
    _clock: Task<()>,
    _store: Subscription,
    _search: Subscription,
}

impl TaskCenter {
    pub fn new(show_quick: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = TaskStore::global(cx);
        let subscription = cx.observe(&store, |_, _, cx| cx.notify());
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索任务或文件"));
        let search_subscription = cx.subscribe(&search, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let clock = cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let alive = this
                .update(cx, |this, cx| {
                    if this.store.read(cx).active_count() > 0 {
                        cx.notify();
                    }
                })
                .is_ok();
            if !alive {
                break;
            }
        });
        Self {
            store,
            search,
            focus: cx.focus_handle(),
            evidence: None,
            has_case: false,
            show_quick,
            quick: None,
            collapsed: [false, false],
            _clock: clock,
            _store: subscription,
            _search: search_subscription,
        }
    }

    pub fn set_context(
        &mut self,
        evidence: Option<(String, SharedString)>,
        has_case: bool,
        cx: &mut Context<Self>,
    ) {
        if self.evidence == evidence && self.has_case == has_case {
            return;
        }
        if self.evidence.as_ref().map(|(id, _)| id) != evidence.as_ref().map(|(id, _)| id) {
            self.quick = None;
        }
        self.evidence = evidence;
        self.has_case = has_case;
        cx.notify();
    }

    pub fn show_quick(&mut self, cx: &mut Context<Self>) {
        self.show_quick = true;
        cx.notify();
    }

    fn render_row(&self, row: &TaskRow, now_ms: i64, cx: &mut Context<Self>) -> Div {
        let subject = row.subject();
        let message = row.message();
        let percent = row.percent();
        let status_tag = match row.status.as_str() {
            "success" => Tag::success(),
            "failed" => Tag::danger(),
            "running" => Tag::info(),
            "cancel" | "offline" => Tag::secondary(),
            _ => Tag::warning(),
        }
        .small()
        .child(if row.cancelling {
            "正在取消"
        } else {
            status_label(&row.status)
        });
        let id = row.id.clone();
        let cancellable = row.can_cancel();
        let page = if row.source == "plugin" {
            Some((Page::Analysis, "查看分析结果"))
        } else {
            match row.kind.as_deref() {
                Some("globalSearch") => Some((Page::Search, "打开全局搜索")),
                Some("timelineBuild") => Some((Page::Timeline, "打开时间线")),
                Some("carveScan") => Some((Page::Carve, "打开文件恢复")),
                Some("imageHash" | "imageLoad") => Some((Page::Dashboard, "打开镜像信息")),
                _ => None,
            }
        };
        v_flex()
            .w_full()
            .gap_1()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_weight(gpui_kit::FontWeight::MEDIUM)
                            .child(row.title()),
                    )
                    .child(status_tag)
                    .when(cancellable, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("cancel-{id}")))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("取消任务")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    let id = id.clone();
                                    this.store.update(cx, |store, cx| store.cancel(id, cx));
                                })),
                        )
                    })
                    .when_some(
                        page.filter(|_| {
                            !cancellable
                                && row.image_id.is_some()
                                && self
                                    .evidence
                                    .as_ref()
                                    .is_some_and(|(id, _)| row.image_id.as_ref() == Some(id))
                        }),
                        |this, (page, label)| {
                            this.child(
                                Button::new(SharedString::from(format!("open-{}", row.id)))
                                    .ghost()
                                    .small()
                                    .icon(Icon::new(ExtraIcon::ArrowUpRight))
                                    .tooltip(label)
                                    .accessibility_label(label)
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(HubEvent::OpenPage(page))
                                    })),
                            )
                        },
                    ),
            )
            .when(!subject.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .truncate()
                        .child(subject),
                )
            })
            .when(row.is_active(), |this| {
                this.child(
                    Progress::new(SharedString::from(format!("progress-{}", row.id)))
                        .when_some(percent, |progress, percent| progress.value(percent))
                        .when(percent.is_none(), |progress| progress.loading(true)),
                )
            })
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().flex_1().min_w_0().truncate().child(message))
                    .child(format::short_local_time(row.created_at_ms))
                    .child(format::duration_secs(row.elapsed_secs(now_ms))),
            )
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or_default()
}

impl Focusable for TaskCenter {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TaskCenter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.show_quick && self.quick.is_none() {
            if let Some((id, _)) = &self.evidence {
                let id = id.clone();
                let quick = cx.new(|cx| QuickAnalysis::new(id, cx));
                let subscription = cx.subscribe_in(
                    &quick,
                    window,
                    |this, _, AnalysisStarted(count), window, cx| {
                        this.show_quick = false;
                        this.search
                            .update(cx, |search, cx| search.set_value("", window, cx));
                        ui::notify_success(
                            window,
                            cx,
                            "已提交分析",
                            format!("{count} 个分析任务已排队。"),
                        );
                        cx.notify();
                    },
                );
                self.quick = Some((quick, subscription));
            }
        }

        let scoped = |row: &&TaskRow| match &self.evidence {
            Some((id, _)) => row
                .image_id
                .as_ref()
                .is_none_or(|candidate| candidate == id),
            None => self.has_case,
        };
        let store = self.store.read(cx);
        let active = store
            .rows()
            .iter()
            .filter(scoped)
            .filter(|row| row.is_active())
            .count();
        let task_tab = if active == 0 {
            "任务".to_owned()
        } else {
            format!("任务  {active}")
        };
        let body = if self.evidence.is_none() && (!self.has_case || self.show_quick) {
            ui::empty_state(
                IconName::HardDrive,
                if self.has_case {
                    "选择案件检材，开始快速分析"
                } else {
                    "打开案件，开始分析"
                },
                Some("任务与快速分析跟随顶部当前检材标签页。".into()),
            )
            .child(
                Button::new("task-hub-case")
                    .small()
                    .primary()
                    .label("打开案件管理")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(HubEvent::OpenCases))),
            )
            .into_any_element()
        } else if self.show_quick {
            v_flex()
                .size_full()
                .p_3()
                .children(self.quick.as_ref().map(|(quick, _)| quick.clone()))
                .overflow_y_scrollbar()
                .into_any_element()
        } else {
            let query = self.search.read(cx).value().trim().to_lowercase();
            let rows: Vec<_> = store
                .rows()
                .iter()
                .filter(scoped)
                .filter(|row| {
                    query.is_empty()
                        || row.title().to_lowercase().contains(&query)
                        || row.subject().to_lowercase().contains(&query)
                        || row.message().to_lowercase().contains(&query)
                })
                .cloned()
                .collect();
            let now = now_ms();
            let list = if rows.is_empty() {
                ui::empty_state(
                    ExtraIcon::ListTodo,
                    if query.is_empty() {
                        "当前检材还没有任务"
                    } else {
                        "没有匹配的任务"
                    },
                    Some(
                        if query.is_empty() {
                            "在快速分析中选择功能，进度与结果会显示在这里。"
                        } else {
                            "换个关键词再试。"
                        }
                        .into(),
                    ),
                )
                .when(query.is_empty() && self.evidence.is_some(), |state| {
                    state.child(
                        Button::new("task-hub-quick")
                            .small()
                            .primary()
                            .label("快速分析")
                            .on_click(cx.listener(|this, _, _, cx| this.show_quick(cx))),
                    )
                })
                .into_any_element()
            } else {
                v_flex()
                    .p_3()
                    .gap_3()
                    .children(
                        [
                            (true, "进行中", Icon::new(ExtraIcon::Clock)),
                            (false, "已结束", Icon::new(IconName::CircleCheck)),
                        ]
                        .into_iter()
                        .enumerate()
                        .filter_map(|(index, (active, label, icon))| {
                            let rows: Vec<_> = rows
                                .iter()
                                .filter(|row| row.is_active() == active)
                                .collect();
                            if rows.is_empty() {
                                return None;
                            }
                            let collapsed = self.collapsed[index];
                            Some(
                                ui::card(cx)
                                    .flex_shrink_0()
                                    .overflow_hidden()
                                    .child(
                                        ui::card_heading(icon, label, cx)
                                            .px_3()
                                            .h(px(44.))
                                            .child(
                                                Tag::secondary()
                                                    .small()
                                                    .child(format::count(rows.len() as u64)),
                                            )
                                            .child(div().flex_1())
                                            .child(
                                                Button::new(("task-section", index))
                                                    .ghost()
                                                    .small()
                                                    .icon(if collapsed {
                                                        IconName::ChevronRight
                                                    } else {
                                                        IconName::ChevronDown
                                                    })
                                                    .accessibility_label(if collapsed {
                                                        format!("展开{label}")
                                                    } else {
                                                        format!("收起{label}")
                                                    })
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.collapsed[index] =
                                                                !this.collapsed[index];
                                                            cx.notify();
                                                        },
                                                    )),
                                            ),
                                    )
                                    // Rows rise in one after another when
                                    // the hub opens, and each new task as it
                                    // arrives.
                                    .when(!collapsed, |card| {
                                        card.children(rows.into_iter().enumerate().map(
                                            |(index, row)| {
                                                crate::motion::rise(
                                                    self.render_row(row, now, cx),
                                                    SharedString::from(format!(
                                                        "task-row-{}",
                                                        row.id
                                                    )),
                                                    crate::motion::stagger(index),
                                                    6.,
                                                    cx,
                                                )
                                            },
                                        ))
                                    }),
                            )
                        }),
                    )
                    .overflow_y_scrollbar()
                    .into_any_element()
            };
            v_flex()
                .size_full()
                .child(
                    ui::page_toolbar(cx)
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&self.search)
                                    .small()
                                    .cleanable(true)
                                    .prefix(Icon::new(IconName::Search).small()),
                            ),
                        )
                        .child(
                            Button::new("refresh-task-hub")
                                .ghost()
                                .small()
                                .icon(IconName::RefreshCw)
                                .tooltip("刷新任务")
                                .accessibility_label("刷新任务")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.store.update(cx, TaskStore::reload)
                                })),
                        ),
                )
                .child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .bg(cx.theme().table_foot)
                        .child(list),
                )
                .into_any_element()
        };

        v_flex()
            .id("task-hub")
            .key_context("TaskHub")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &DismissTaskHub, _, cx| cx.emit(HubEvent::Close)))
            .size_full()
            .child(
                h_flex()
                    .h(px(46.))
                    .flex_shrink_0()
                    .gap_2()
                    .pl_4()
                    .pr_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        TabBar::new("task-hub-tabs")
                            .underline()
                            .child(Tab::new().label(task_tab))
                            .child(Tab::new().label("快速分析"))
                            .selected_index(usize::from(self.show_quick))
                            .on_click(cx.listener(|this, index: &usize, _, cx| {
                                this.show_quick = *index == 1;
                                cx.notify();
                            })),
                    )
                    .child(div().flex_1().min_w_0())
                    .when_some(
                        self.evidence.as_ref().map(|(_, name)| name.clone()),
                        |bar, name| {
                            bar.child(
                                Tag::secondary()
                                    .small()
                                    .max_w(px(138.))
                                    .child(div().truncate().child(name)),
                            )
                        },
                    )
                    .child(
                        Button::new("close-task-hub")
                            .ghost()
                            .small()
                            .icon(IconName::Close)
                            .tooltip("收起任务中心")
                            .accessibility_label("收起任务中心")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(HubEvent::Close))),
                    ),
            )
            .child(div().flex_1().min_h_0().child(body))
    }
}
