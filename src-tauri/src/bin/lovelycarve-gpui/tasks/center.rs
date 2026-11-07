//! The task center sheet: every task with its state, progress and actions.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        progress::Progress,
        scroll::ScrollableElement as _,
        tag::Tag,
        v_flex, ActiveTheme as _, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Task, Window,
};

use super::{status_label, TaskRow, TaskStore};
use crate::{format, ui};

/// Which tasks the list shows.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter {
    Active,
    All,
}

pub struct TaskCenter {
    store: Entity<TaskStore>,
    filter: Filter,
    /// Keeps elapsed times of running tasks moving.
    _clock: Task<()>,
    _store: Subscription,
}

impl TaskCenter {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let store = TaskStore::global(cx);
        let subscription = cx.observe(&store, |_, _, cx| cx.notify());
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
            filter: Filter::All,
            _clock: clock,
            _store: subscription,
        }
    }

    fn render_row(&self, row: &TaskRow, now_ms: i64, cx: &mut Context<Self>) -> impl IntoElement {
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
                    }),
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

impl Render for TaskCenter {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = now_ms();
        let store = self.store.read(cx);
        let rows: Vec<TaskRow> = store
            .rows()
            .iter()
            .filter(|row| self.filter == Filter::All || row.is_active())
            .cloned()
            .collect();
        let active = store.active_count();
        let filter = self.filter;
        v_flex()
            .size_full()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("filter-all")
                            .small()
                            .label("全部")
                            .when(filter == Filter::All, |button| button.primary())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.filter = Filter::All;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("filter-active")
                            .small()
                            .label(format!("进行中 {active}"))
                            .when(filter == Filter::Active, |button| button.primary())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.filter = Filter::Active;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .when(rows.is_empty(), |this| {
                        this.child(ui::empty_state(
                            IconName::Inbox,
                            "暂无任务",
                            Some("加载检材、哈希、搜索、时间线与恢复任务都会列在这里。".into()),
                        ))
                    })
                    .children(rows.iter().map(|row| self.render_row(row, now, cx))),
            )
    }
}

/// Open the task center on the right side of `window`.
pub fn open(window: &mut Window, cx: &mut App) {
    use gpui_kit::component::WindowExt as _;
    let center = cx.new(TaskCenter::new);
    window.open_sheet(cx, move |sheet, _, _| {
        sheet
            .title("任务中心")
            .size(gpui_kit::px(460.))
            .child(center.clone())
    });
}
