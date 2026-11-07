//! The case manager dialog: create, open, switch, lock or delete cases and
//! manage the active case's evidence.
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        description_list::DescriptionList,
        h_flex,
        scroll::ScrollableElement as _,
        spinner::Spinner,
        tag::Tag,
        v_flex, ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Window,
};
use lovelycarve_lib::{commands::casework::case::CaseListItem, core::casework::case::EvidenceRef};

use super::CaseStore;
use crate::{format, ui};

type ExtraIcon = gpui_kit::assets::IconName;

/// What the case manager asks the workspace to do.
#[derive(Clone, Debug)]
pub enum CaseManagerEvent {
    AddEvidence,
    OpenEvidence { path: String },
}

pub struct CaseManager {
    store: Entity<CaseStore>,
    /// The evidence of the open tab, highlighted in the list.
    current_image: Option<String>,
    error: Option<SharedString>,
    _store: Subscription,
}

impl EventEmitter<CaseManagerEvent> for CaseManager {}

impl CaseManager {
    pub fn new(current_image: Option<String>, cx: &mut Context<Self>) -> Self {
        let store = CaseStore::global(cx);
        let subscription = cx.observe(&store, |_, _, cx| cx.notify());
        store.update(cx, |store, cx| {
            store.refresh_registry(cx);
            store.refresh_active(cx);
        });
        Self {
            store,
            current_image,
            error: None,
            _store: subscription,
        }
    }

    fn report<T>(
        &mut self,
        result: Result<T, crate::backend::CommandError>,
        cx: &mut Context<Self>,
    ) {
        self.error = result.err().map(|error| error.message);
        cx.notify();
    }

    fn create_case(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let directory = ui::prompt_directory("选择新建案件的目录", cx);
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let Some(directory) = directory.await else {
                return;
            };
            let dir = directory.display().to_string();
            let name = format::file_name(&dir).to_owned();
            let created = store.update(cx, |store, cx| {
                store.create(dir, name, None, None, None, cx)
            });
            let result = created.await;
            this.update(cx, |this, cx| this.report(result, cx)).ok();
        })
        .detach();
    }

    fn open_case_directory(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let directory = ui::prompt_directory("选择已有案件目录", cx);
        let store = self.store.clone();
        cx.spawn(async move |this, cx| {
            let Some(directory) = directory.await else {
                return;
            };
            let dir = directory.display().to_string();
            let opened = store.update(cx, |store, cx| store.open(dir, cx));
            let result = opened.await;
            this.update(cx, |this, cx| this.report(result, cx)).ok();
        })
        .detach();
    }

    fn open_listed(&mut self, dir: String, cx: &mut Context<Self>) {
        let opened = self.store.update(cx, |store, cx| store.open(dir, cx));
        cx.spawn(async move |this, cx| {
            let result = opened.await;
            this.update(cx, |this, cx| this.report(result, cx)).ok();
        })
        .detach();
    }

    fn remove_case(&mut self, item: CaseListItem, window: &mut Window, cx: &mut Context<Self>) {
        let (title, description) = if item.missing {
            (
                "移除缺失案件记录".to_owned(),
                format!(
                    "案件目录或台账已不可读，仅移除「{}」的登记记录。",
                    item.name
                ),
            )
        } else {
            (
                "删除案件数据".to_owned(),
                format!(
                    "确定删除案件「{}」（{}）吗？将删除目录 {} 中的案件数据，包括案件台账、索引、标签、分析结果与快照；目录里的其他文件和原始检材均不受影响。此操作不可恢复。",
                    item.name, item.case_number, item.dir
                ),
            )
        };
        let view = cx.entity().downgrade();
        ui::confirm(window, cx, title, description, "删除", move |_, cx| {
            let dir = item.dir.clone();
            view.update(cx, |this, cx| {
                let removed = this.store.update(cx, |store, cx| store.remove(dir, cx));
                cx.spawn(async move |this, cx| {
                    let result = removed.await;
                    this.update(cx, |this, cx| this.report(result, cx)).ok();
                })
                .detach();
            })
            .ok();
        });
    }

    fn close_case(&mut self, cx: &mut Context<Self>) {
        let closed = self.store.update(cx, |store, cx| store.close(cx));
        cx.spawn(async move |this, cx| {
            let result = closed.await;
            this.update(cx, |this, cx| this.report(result, cx)).ok();
        })
        .detach();
    }

    fn toggle_lock(&mut self, locked: bool, cx: &mut Context<Self>) {
        let changed = self
            .store
            .update(cx, |store, cx| store.set_locked(!locked, cx));
        cx.spawn(async move |this, cx| {
            let result = changed.await;
            this.update(cx, |this, cx| this.report(result, cx)).ok();
        })
        .detach();
    }

    fn remove_evidence(
        &mut self,
        evidence: EvidenceRef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        let description = format!(
            "从案件中移除检材「{}」？案件里该检材的索引、标签与分析结果会一并删除，原始检材文件不受影响。",
            format::file_name(&evidence.path)
        );
        ui::confirm(
            window,
            cx,
            "移除检材",
            description,
            "移除",
            move |_, cx| {
                let image_id = evidence.image_id.clone();
                view.update(cx, |this, cx| {
                    let removed = this
                        .store
                        .update(cx, |store, cx| store.remove_evidence(image_id, cx));
                    cx.spawn(async move |this, cx| {
                        let result = removed.await;
                        this.update(cx, |this, cx| this.report(result, cx)).ok();
                    })
                    .detach();
                })
                .ok();
            },
        );
    }

    fn render_registry(&self, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let cases = store.registered().to_vec();
        let registry_error = store.registry_error().map(str::to_owned);
        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("create-case")
                            .primary()
                            .icon(IconName::Plus)
                            .label("新建案件")
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.create_case(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("open-case")
                            .icon(IconName::FolderOpen)
                            .label("打开案件目录…")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_case_directory(window, cx)
                            })),
                    ),
            )
            .when_some(registry_error, |this, error| {
                this.child(ui::error_text(error, cx))
            })
            .child(ui::section_title(format!("最近案件 · {}", cases.len()), cx))
            .when(cases.is_empty(), |this| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("暂无最近案件"),
                )
            })
            .children(cases.into_iter().enumerate().map(|(index, item)| {
                let open_dir = item.dir.clone();
                let removed = item.clone();
                h_flex()
                    .w_full()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(Icon::new(ExtraIcon::Briefcase).text_color(cx.theme().muted_foreground))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(div().truncate().child(item.name.clone()))
                                    .when(item.locked, |this| {
                                        this.child(Icon::new(ExtraIcon::Lock).xsmall())
                                    })
                                    .when(item.missing, |this| {
                                        this.child(Tag::danger().small().child("缺失"))
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(ui::mono(item.dir.clone(), cx)),
                            ),
                    )
                    .child(
                        Button::new(("open-listed", index))
                            .small()
                            .label("打开")
                            .disabled(busy || item.missing)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_listed(open_dir.clone(), cx)
                            })),
                    )
                    .child(
                        Button::new(("remove-listed", index))
                            .small()
                            .ghost()
                            .icon(Icon::new(ExtraIcon::Trash))
                            .tooltip(if item.missing {
                                "移除缺失案件记录"
                            } else {
                                "删除案件数据（保留原始检材与其他文件）"
                            })
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_case(removed.clone(), window, cx)
                            })),
                    )
            }))
    }

    fn render_active(&self, busy: bool, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(case) = self.store.read(cx).active().cloned() else {
            return div().into_any_element();
        };
        let locked = case.locked;
        v_flex()
            .w_full()
            .gap_3()
            .child(
                h_flex()
                    .gap_2()
                    .child(ui::section_title(
                        format!("检材 · {}", case.evidence.len()),
                        cx,
                    ))
                    .child(div().flex_1())
                    .child(
                        Button::new("add-evidence")
                            .primary()
                            .small()
                            .icon(IconName::Plus)
                            .label("添加检材")
                            .disabled(busy || locked)
                            .tooltip(if locked {
                                "解锁案件后可添加"
                            } else {
                                "添加镜像、文件夹或磁盘"
                            })
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(CaseManagerEvent::AddEvidence)),
                            ),
                    ),
            )
            .when(case.evidence.is_empty(), |this| {
                this.child(ui::empty_state(
                    Icon::new(IconName::HardDrive),
                    "还没有检材",
                    Some(if locked {
                        "解锁案件后，可添加检材。".into()
                    } else {
                        "添加镜像、文件夹或磁盘，开始查看与分析。".into()
                    }),
                ))
            })
            .children(case.evidence.iter().enumerate().map(|(index, evidence)| {
                let current = self.current_image.as_deref() == Some(evidence.image_id.as_str());
                let path = evidence.path.clone();
                let removed = evidence.clone();
                let format_name = serde_json::to_value(evidence.source_format)
                    .ok()
                    .and_then(|value| value.as_str().map(str::to_uppercase))
                    .unwrap_or_default();
                h_flex()
                    .w_full()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(if current {
                        cx.theme().primary
                    } else {
                        cx.theme().border
                    })
                    .child(Icon::new(IconName::HardDrive).text_color(cx.theme().muted_foreground))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        div()
                                            .truncate()
                                            .child(format::file_name(&evidence.path).to_owned()),
                                    )
                                    .child(Tag::secondary().small().child(format_name))
                                    .when_some(evidence.platform.clone(), |this, platform| {
                                        this.child(Tag::info().small().child(platform))
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .truncate()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{} · {}",
                                        format::binary_bytes(evidence.size_bytes),
                                        evidence.path
                                    )),
                            ),
                    )
                    .child(
                        Button::new(("open-evidence", index))
                            .small()
                            .label(if current { "当前" } else { "打开" })
                            .disabled(busy || current)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(CaseManagerEvent::OpenEvidence { path: path.clone() })
                            })),
                    )
                    .child(
                        Button::new(("remove-evidence", index))
                            .small()
                            .ghost()
                            .icon(Icon::new(ExtraIcon::Trash))
                            .tooltip("从案件移除检材")
                            .disabled(busy || locked)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.remove_evidence(removed.clone(), window, cx)
                            })),
                    )
            }))
            .child(ui::section_title("案件详情", cx))
            .child(
                DescriptionList::horizontal()
                    .columns(1)
                    .label_width(gpui_kit::px(96.))
                    .item("案件编号", case.case_number.clone(), 1)
                    .item(
                        "检验人",
                        if case.examiner.is_empty() {
                            "—".to_owned()
                        } else {
                            case.examiner.clone()
                        },
                        1,
                    )
                    .item("存储位置", case.dir.clone(), 1)
                    .item("最近更新", format::timestamp(Some(&case.updated_at)), 1)
                    .item("卷快照", format!("{} 份", case.snapshots.len()), 1),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("toggle-lock")
                            .small()
                            .icon(Icon::new(if locked {
                                ExtraIcon::LockOpen
                            } else {
                                ExtraIcon::Lock
                            }))
                            .label(if locked {
                                "解锁案件"
                            } else {
                                "锁定案件"
                            })
                            .disabled(busy)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_lock(locked, cx)),
                            ),
                    )
                    .child(
                        Button::new("switch-case")
                            .small()
                            .icon(IconName::FolderOpen)
                            .label("切换案件…")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_case_directory(window, cx)
                            })),
                    )
                    .child(
                        Button::new("close-case")
                            .small()
                            .icon(IconName::Close)
                            .label("关闭案件")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.close_case(cx))),
                    ),
            )
            .into_any_element()
    }
}

impl Render for CaseManager {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.store.read(cx);
        let busy = store.is_busy();
        let has_case = store.active().is_some();
        v_flex()
            .w_full()
            .max_h(gpui_kit::rems(36.))
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(if has_case {
                        self.render_active(busy, cx).into_any_element()
                    } else {
                        self.render_registry(busy, cx).into_any_element()
                    }),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(ui::error_text(error, cx))
            })
            .when(busy, |this| {
                this.child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(Spinner::new().small())
                        .child("正在处理案件…"),
                )
            })
    }
}
