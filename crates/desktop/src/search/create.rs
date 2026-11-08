//! The new-search form: keywords and regular expressions, encodings and the
//! scan options of `global_search_start`.
use std::collections::BTreeSet;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{Textarea, TextareaState},
        scroll::ScrollableElement as _,
        tab::{Tab, TabBar},
        v_flex, ActiveTheme as _, Sizable as _, WindowExt as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window,
};
use lovelycarve_backend::core::evidence::image::ImageInfo;
use serde_json::json;

use crate::ui;

const CODE_PAGES: [&str; 5] = ["ASCII", "UTF-8", "UTF-16LE", "UTF-16BE", "GBK"];
const DEFAULT_CODE_PAGES: [&str; 4] = ["UTF-8", "GBK", "UTF-16LE", "UTF-16BE"];

/// The form asks to start a search with these options (the JSON shape of
/// `GlobalSearchOptions`).
#[derive(Clone, Debug)]
pub struct StartSearch(pub serde_json::Value);

pub struct SearchForm {
    keywords: Entity<TextareaState>,
    regexes: Entity<TextareaState>,
    paths: Entity<TextareaState>,
    partition_options: Vec<(u8, String)>,
    partitions: BTreeSet<u8>,
    tags: BTreeSet<&'static str>,
    scope: Scope,
    scope_open: bool,
    advanced_open: bool,
    code_pages: BTreeSet<&'static str>,
    match_case: bool,
    search_deleted: bool,
    search_unallocated: bool,
    calc_hash: bool,
    stop_after_first: bool,
    error: Option<SharedString>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Partitions,
    Tags,
    Paths,
}

impl Scope {
    const ALL: [Self; 3] = [Self::Partitions, Self::Tags, Self::Paths];
}

impl EventEmitter<StartSearch> for SearchForm {}

/// Reaches one of the form's on/off options.
type FlagAccess = fn(&mut SearchForm) -> &mut bool;

fn patterns(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

impl SearchForm {
    pub fn new(info: &ImageInfo, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            keywords: cx.new(|cx| {
                TextareaState::new(window, cx).placeholder("每行一个关键词，如 password")
            }),
            regexes: cx
                .new(|cx| TextareaState::new(window, cx).placeholder("每行一个正则表达式（可选）")),
            paths: cx
                .new(|cx| TextareaState::new(window, cx).placeholder("每行一个目录或文件路径")),
            partition_options: info
                .partitions
                .iter()
                .map(|partition| {
                    (
                        partition.id,
                        format!(
                            "分区 {} · {}",
                            partition.id,
                            crate::evidence::filesystem_label(&partition.kind)
                        ),
                    )
                })
                .collect(),
            partitions: BTreeSet::new(),
            tags: BTreeSet::new(),
            scope: Scope::Partitions,
            scope_open: false,
            advanced_open: false,
            code_pages: DEFAULT_CODE_PAGES.into_iter().collect(),
            match_case: false,
            search_deleted: true,
            search_unallocated: false,
            calc_hash: false,
            stop_after_first: false,
            error: None,
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let keys = patterns(&self.keywords.read(cx).value());
        let regexes = patterns(&self.regexes.read(cx).value());
        if keys.is_empty() && regexes.is_empty() {
            self.error = Some("至少输入一个关键词或正则".into());
            cx.notify();
            return;
        }
        if self.code_pages.is_empty() {
            self.error = Some("至少选择一种编码".into());
            cx.notify();
            return;
        }
        self.error = None;
        let scope = match self.scope {
            Scope::Partitions => {
                json!({"partitions": self.partitions.iter().map(|id| format!("part:{id}")).collect::<Vec<_>>() })
            }
            Scope::Tags => json!({"tags": self.tags}),
            Scope::Paths => json!({"tree_nodes": patterns(&self.paths.read(cx).value())}),
        };
        cx.emit(StartSearch(json!({
            "keys": keys,
            "regexes": regexes,
            "codepages": self.code_pages.iter().collect::<Vec<_>>(),
            "match_case": self.match_case,
            "search_deleted": self.search_deleted,
            "search_empty_space": self.search_unallocated,
            "calc_hash": self.calc_hash,
            "stop_after_first": self.stop_after_first,
            "scope": scope,
        })));
    }

    fn toggle(&mut self, flag: FlagAccess, value: bool, cx: &mut Context<Self>) {
        *flag(self) = value;
        cx.notify();
    }

    fn render_scope(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = match self.scope {
            Scope::Partitions if !self.partitions.is_empty() => {
                format!("{} 个分区", self.partitions.len())
            }
            Scope::Tags if !self.tags.is_empty() => format!("{} 个标签", self.tags.len()),
            Scope::Paths if !self.paths.read(cx).value().trim().is_empty() => {
                "按目录或文件".to_owned()
            }
            _ => "整个检材".to_owned(),
        };
        ui::card(cx)
            .child(
                ui::disclosure_button("search-scope", "搜索范围", summary, self.scope_open, cx)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.scope_open = !this.scope_open;
                        cx.notify();
                    })),
            )
            .when(self.scope_open, |card| {
                let tabs = TabBar::new("search-scope-tabs")
                    .segmented()
                    .small()
                    .children(["按分区", "按标签", "按目录"].map(|label| Tab::new().label(label)))
                    .selected_index(self.scope as usize)
                    .on_click(cx.listener(|this, index: &usize, _, cx| {
                        if let Some(&scope) = Scope::ALL.get(*index) {
                            this.scope = scope;
                            cx.notify();
                        }
                    }));
                let body = match self.scope {
                    Scope::Partitions => h_flex()
                        .gap_3()
                        .flex_wrap()
                        .when(self.partition_options.is_empty(), |row| {
                            row.child("未检测到分区，默认搜索整个检材。")
                        })
                        .children(self.partition_options.iter().map(|(id, label)| {
                            let id = *id;
                            Checkbox::new(("search-partition", usize::from(id)))
                                .label(label.clone())
                                .checked(self.partitions.contains(&id))
                                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                    if *checked {
                                        this.partitions.insert(id);
                                    } else {
                                        this.partitions.remove(&id);
                                    }
                                    cx.notify();
                                }))
                        }))
                        .into_any_element(),
                    Scope::Tags => h_flex()
                        .gap_3()
                        .flex_wrap()
                        .children(crate::files::FILE_TAGS.into_iter().map(|tag| {
                            Checkbox::new(tag)
                                .label(tag)
                                .checked(self.tags.contains(tag))
                                .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                    if *checked {
                                        this.tags.insert(tag);
                                    } else {
                                        this.tags.remove(tag);
                                    }
                                    cx.notify();
                                }))
                        }))
                        .into_any_element(),
                    Scope::Paths => Textarea::new(&self.paths).h(px(76.)).into_any_element(),
                };
                card.child(v_flex().p_3().gap_3().child(tabs).child(body))
            })
    }
}

impl Render for SearchForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let options: [(&str, &str, FlagAccess, bool); 5] = [
            (
                "match-case",
                "区分大小写",
                |this| &mut this.match_case,
                self.match_case,
            ),
            (
                "search-deleted",
                "包含已删除文件",
                |this| &mut this.search_deleted,
                self.search_deleted,
            ),
            (
                "search-unallocated",
                "搜索未分配空间",
                |this| &mut this.search_unallocated,
                self.search_unallocated,
            ),
            (
                "calc-hash",
                "为命中文件计算 MD5 / SHA256",
                |this| &mut this.calc_hash,
                self.calc_hash,
            ),
            (
                "stop-first",
                "每个文件首个命中即停",
                |this| &mut this.stop_after_first,
                self.stop_after_first,
            ),
        ];
        v_flex()
            .w_full()
            .gap_3()
            .child(
                v_flex()
                    .gap_3()
                    .max_h(px(520.))
                    .overflow_y_scrollbar()
                    .child(ui::section_title("关键词（每行一个）", cx))
                    .child(Textarea::new(&self.keywords).h(px(96.)))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("至少输入一个关键词，或在高级选项中填写正则表达式"),
                    )
                    .child(self.render_scope(cx))
                    .child(
                        ui::card(cx)
                            .child(
                                ui::disclosure_button(
                                    "search-advanced",
                                    "高级选项",
                                    format!("{} 种编码", self.code_pages.len()),
                                    self.advanced_open,
                                    cx,
                                )
                                .on_click(cx.listener(
                                    |this, _, _, cx| {
                                        this.advanced_open = !this.advanced_open;
                                        cx.notify();
                                    },
                                )),
                            )
                            .when(self.advanced_open, |card| {
                                card.child(
                                    v_flex()
                                        .p_3()
                                        .gap_3()
                                        .child(ui::section_title("正则表达式（每行一个）", cx))
                                        .child(Textarea::new(&self.regexes).h(px(64.)))
                                        .child(ui::section_title("编码", cx))
                                        .child(h_flex().gap_3().flex_wrap().children(
                                            CODE_PAGES.into_iter().map(|page| {
                                                Checkbox::new(SharedString::from(format!(
                                                    "codepage-{page}"
                                                )))
                                                .label(page)
                                                .checked(self.code_pages.contains(page))
                                                .on_click(cx.listener(
                                                    move |this, checked: &bool, _, cx| {
                                                        if *checked {
                                                            this.code_pages.insert(page);
                                                        } else {
                                                            this.code_pages.remove(page);
                                                        }
                                                        cx.notify();
                                                    },
                                                ))
                                            }),
                                        ))
                                        .child(ui::section_title("选项", cx))
                                        .child(v_flex().gap_2().children(options.into_iter().map(
                                            |(id, label, flag, checked)| {
                                                Checkbox::new(id)
                                                    .label(label)
                                                    .checked(checked)
                                                    .on_click(cx.listener(
                                                        move |this, value: &bool, _, cx| {
                                                            this.toggle(flag, *value, cx)
                                                        },
                                                    ))
                                            },
                                        ))),
                                )
                            }),
                    )
                    .when_some(self.error.clone(), |this, error| {
                        this.child(ui::error_text(error, cx))
                    }),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        Button::new("cancel-search")
                            .label("取消")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("start-search")
                            .primary()
                            .label("开始搜索")
                            .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                    ),
            )
    }
}
