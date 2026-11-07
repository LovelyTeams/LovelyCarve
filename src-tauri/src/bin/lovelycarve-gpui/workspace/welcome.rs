//! The start page: create or open a case, add evidence, reopen recent cases.
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        scroll::ScrollableElement as _,
        spinner::Spinner,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    Context, Entity, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window,
};

use crate::{cases::CaseStore, format, ui};

type ExtraIcon = gpui_kit::assets::IconName;

/// How many registered cases the start page lists.
const RECENT_LIMIT: usize = 8;

#[derive(Clone, Copy, Debug)]
pub enum WelcomeEvent {
    AddEvidence,
    ShowCaseManager,
}

pub struct Welcome {
    cases: Entity<CaseStore>,
    error: Option<SharedString>,
    _cases: Subscription,
}

impl EventEmitter<WelcomeEvent> for Welcome {}

impl Welcome {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let cases = CaseStore::global(cx);
        let subscription = cx.observe(&cases, |_, _, cx| cx.notify());
        Self {
            cases,
            error: None,
            _cases: subscription,
        }
    }

    fn open_recent(&mut self, dir: String, cx: &mut Context<Self>) {
        let opened = self.cases.update(cx, |cases, cx| cases.open(dir, cx));
        cx.spawn(async move |this, cx| {
            let result = opened.await;
            this.update(cx, |this, cx| {
                this.error = result.err().map(|error| error.message);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

impl Render for Welcome {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.cases.read(cx);
        let busy = store.is_busy();
        let active = store.active().cloned();
        let recent: Vec<_> = store
            .registered()
            .iter()
            .filter(|item| !item.missing)
            .take(RECENT_LIMIT)
            .cloned()
            .collect();
        let muted = cx.theme().muted_foreground;
        v_flex()
            .size_full()
            .items_center()
            .overflow_y_scrollbar()
            .child(
                v_flex()
                    .w_full()
                    .max_w(gpui_kit::rems(40.))
                    .px_6()
                    .py_12()
                    .gap_6()
                    .child(
                        v_flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_2xl()
                                    .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                                    .child("LovelyCarve"),
                            )
                            .child(div().text_sm().text_color(muted).child(match &active {
                                Some(case) => format!(
                                    "当前案件「{}」还没有打开的检材。添加镜像、文件夹或磁盘，开始查看与分析。",
                                    case.name
                                ),
                                None => "取证检材分析：打开案件后添加镜像、文件夹或磁盘，浏览文件系统、搜索、时间线与文件恢复。".to_owned(),
                            })),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("welcome-add-evidence")
                                    .primary()
                                    .icon(IconName::Plus)
                                    .label("添加检材")
                                    .disabled(busy)
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(WelcomeEvent::AddEvidence))),
                            )
                            .child(
                                Button::new("welcome-cases")
                                    .icon(Icon::new(ExtraIcon::Briefcase))
                                    .label(if active.is_some() { "案件管理" } else { "新建或打开案件" })
                                    .disabled(busy)
                                    .on_click(cx.listener(|_, _, _, cx| cx.emit(WelcomeEvent::ShowCaseManager))),
                            )
                            .when(busy, |this| {
                                this.child(
                                    h_flex()
                                        .gap_2()
                                        .text_sm()
                                        .text_color(muted)
                                        .child(Spinner::new().small())
                                        .child("正在打开案件…"),
                                )
                            }),
                    )
                    .when_some(self.error.clone(), |this, error| this.child(ui::error_text(error, cx)))
                    .when(active.is_none() && !recent.is_empty(), |this| {
                        this.child(
                            v_flex()
                                .gap_2()
                                .child(ui::section_title("最近案件", cx))
                                .children(recent.into_iter().enumerate().map(|(index, item)| {
                                    let dir = item.dir.clone();
                                    h_flex()
                                        .id(("recent-case", index))
                                        .w_full()
                                        .gap_3()
                                        .px_3()
                                        .py_2()
                                        .rounded(cx.theme().radius)
                                        .cursor_pointer()
                                        .hover(|style| style.bg(cx.theme().list_hover))
                                        .child(Icon::new(ExtraIcon::Clock).text_color(muted))
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .child(div().truncate().child(item.name.clone()))
                                                .child(
                                                    div()
                                                        .text_xs()
                                                        .truncate()
                                                        .text_color(muted)
                                                        .child(format!(
                                                            "{} · {}",
                                                            item.case_number,
                                                            item.dir
                                                        )),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .flex_shrink_0()
                                                .text_xs()
                                                .text_color(muted)
                                                .child(format::timestamp(Some(&item.opened_at))),
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.open_recent(dir.clone(), cx)
                                        }))
                                })),
                        )
                    }),
            )
    }
}
