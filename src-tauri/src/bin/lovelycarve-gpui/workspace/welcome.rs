//! The start page: create or open a case, add evidence, reopen recent cases.
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex,
        scroll::ScrollableElement as _,
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    },
    div, img,
    prelude::FluentBuilder as _,
    px, Context, Entity, EventEmitter, ExternalPaths, FontWeight, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window,
};

use super::welcome_motion;
use crate::{assets, cases::CaseStore, format, prefs::Preferences, ui};

type ExtraIcon = gpui_kit::assets::IconName;

/// How many registered cases the start page lists.
const RECENT_LIMIT: usize = 8;
const FORMATS: [&str; 13] = [
    ".dd", ".img", ".raw", ".E01", ".AD1", ".vmdk", ".vhdx", ".qcow2", ".vdi", ".iso", ".zip",
    ".7z", ".tar",
];

#[derive(Clone, Copy, Debug)]
pub enum WelcomeEvent {
    AddEvidence,
    ShowCaseManager,
}

pub struct Welcome {
    cases: Entity<CaseStore>,
    error: Option<SharedString>,
    _cases: Subscription,
    _activation: Subscription,
}

impl EventEmitter<WelcomeEvent> for Welcome {}

impl Welcome {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cases = CaseStore::global(cx);
        let subscription = cx.observe(&cases, |_, _, cx| cx.notify());
        Self {
            cases,
            error: None,
            _cases: subscription,
            _activation: cx.observe_window_activation(window, |_, _, cx| cx.notify()),
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.cases.read(cx);
        let busy = store.is_busy();
        let active_name = store.active().map(|case| case.name.clone());
        let recent: Vec<_> = store
            .registered()
            .iter()
            .filter(|item| !item.missing)
            .take(RECENT_LIMIT)
            .cloned()
            .collect();
        let muted = cx.theme().muted_foreground;
        let title = |label| {
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(label)
        };
        let motion = Preferences::global(cx).welcome_motion && !cx.reduce_motion();

        let import_card = ui::card(cx)
            .id("welcome-import")
            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                style.border_color(cx.theme().ring).bg(cx.theme().accent)
            })
            .flex_grow(1.12)
            .flex_basis(px(0.))
            .h_full()
            .items_center()
            .justify_center()
            .rounded(px(crate::theme::DIALOG_RADIUS))
            .p_6()
            .gap_3()
            .child(welcome_motion::artwork(
                motion && window.is_window_active(),
                cx.has_active_drag(),
                cx,
            ))
            .child(title("拖入文件"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(muted)
                    .child("支持镜像与归档格式"),
            )
            .child(
                h_flex()
                    .max_w(px(360.))
                    .justify_center()
                    .flex_wrap()
                    .gap_1p5()
                    .children(FORMATS.into_iter().map(|format| {
                        Tag::secondary()
                            .outline()
                            .small()
                            .text_size(px(11.))
                            .child(ui::mono(format, cx))
                    })),
            )
            .child(
                v_flex()
                    .mt_2()
                    .w(px(280.))
                    .max_w_full()
                    .gap_2()
                    .child(
                        Button::new("welcome-add-evidence")
                            .primary()
                            .w_full()
                            .h(px(38.))
                            .icon(Icon::new(ExtraIcon::FolderOpen))
                            .label(if busy {
                                "正在打开案件…"
                            } else {
                                "选择文件"
                            })
                            .loading(busy)
                            .disabled(busy)
                            .tooltip("添加检材 (Ctrl+O)")
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(WelcomeEvent::AddEvidence)),
                            ),
                    )
                    .child(
                        Button::new("welcome-cases")
                            .secondary()
                            .w_full()
                            .h(px(38.))
                            .icon(Icon::new(ExtraIcon::Briefcase))
                            .label(if active_name.is_some() {
                                "案件管理"
                            } else {
                                "新建或打开案件"
                            })
                            .disabled(busy)
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(WelcomeEvent::ShowCaseManager)),
                            ),
                    ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(muted)
                    .text_center()
                    .child("Ctrl + O  ·  添加镜像、文件夹或磁盘"),
            );

        let mut recent_card = ui::card(cx)
            .flex_grow_1()
            .flex_basis(px(0.))
            .h_full()
            .rounded(px(crate::theme::DIALOG_RADIUS))
            .p_6()
            .gap_3()
            .child(title("最近打开的案件"));
        if recent.is_empty() {
            recent_card =
                recent_card.child(v_flex().flex_1().justify_center().items_center().child(
                    ui::empty_state(
                        Icon::new(ExtraIcon::Clock),
                        "还没有最近打开的案件",
                        Some("创建或打开案件后，会显示在这里".into()),
                    ),
                ));
        } else {
            recent_card = recent_card.child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .children(recent.into_iter().enumerate().map(|(index, item)| {
                        let dir = item.dir;
                        Button::new(("recent-case", index))
                            .ghost()
                            .w_full()
                            .h(px(64.))
                            .flex_shrink_0()
                            .disabled(busy)
                            .accessibility_label(item.name.clone())
                            .tooltip(dir.clone())
                            .child(
                                h_flex()
                                    .w_full()
                                    .min_w_0()
                                    .gap_3()
                                    .text_left()
                                    .child(
                                        h_flex()
                                            .flex_shrink_0()
                                            .size(px(40.))
                                            .justify_center()
                                            .rounded(px(crate::theme::PANEL_RADIUS))
                                            .bg(cx.theme().accent)
                                            .child(
                                                Icon::new(ExtraIcon::Briefcase)
                                                    .text_color(cx.theme().accent_foreground),
                                            ),
                                    )
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_size(px(13.))
                                                    .truncate()
                                                    .font_weight(FontWeight::MEDIUM)
                                                    .child(item.name),
                                            )
                                            .child(
                                                ui::mono(
                                                    format::timestamp(Some(&item.opened_at)),
                                                    cx,
                                                )
                                                .text_size(px(11.))
                                                .text_color(muted)
                                                .truncate(),
                                            ),
                                    )
                                    .child(
                                        Icon::new(IconName::ChevronRight).small().text_color(muted),
                                    ),
                            )
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.open_recent(dir.clone(), cx)
                                }),
                            )
                    }))
                    .overflow_y_scrollbar(),
            );
        }

        v_flex().size_full().overflow_y_scrollbar().child(
            v_flex()
                .w_full()
                .min_h_full()
                .items_center()
                .justify_center()
                .px_8()
                .py_6()
                .child(
                    v_flex()
                        .w_full()
                        .max_w(px(1120.))
                        .flex_shrink_0()
                        .gap_8()
                        .child(
                            h_flex()
                                .gap_5()
                                .child(welcome_motion::rise(
                                    img(assets::BRAND_TILE)
                                        .size(px(96.))
                                        .rounded(px(crate::theme::PANEL_RADIUS))
                                        .flex_shrink_0(),
                                    "welcome-brand-mark",
                                    0,
                                    8.,
                                    motion,
                                ))
                                .child(welcome_motion::rise(
                                    v_flex().gap_2().child(ui::wordmark(44., cx)).child(
                                        div().text_size(px(13.)).text_color(muted).child(
                                            active_name.map_or_else(
                                                || "磁盘取证 / 数据恢复工具".to_owned(),
                                                |name| format!("当前案件 · {name}"),
                                            ),
                                        ),
                                    ),
                                    "welcome-wordmark",
                                    80,
                                    8.,
                                    motion,
                                )),
                        )
                        .child(
                            h_flex()
                                .w_full()
                                .h(px(444.))
                                .gap_6()
                                .child(welcome_motion::rise(
                                    import_card,
                                    "welcome-import-rise",
                                    140,
                                    16.,
                                    motion,
                                ))
                                .child(welcome_motion::rise(
                                    recent_card,
                                    "welcome-recent-rise",
                                    220,
                                    16.,
                                    motion,
                                )),
                        )
                        .when_some(self.error.clone(), |this, error| {
                            this.child(ui::error_text(error, cx))
                        }),
                ),
        )
    }
}
