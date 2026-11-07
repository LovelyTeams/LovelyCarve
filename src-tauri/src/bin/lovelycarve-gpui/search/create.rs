//! The new-search form: keywords and regular expressions, encodings and the
//! scan options of `global_search_start`.
use std::collections::BTreeSet;

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        checkbox::Checkbox,
        h_flex,
        input::{Textarea, TextareaState},
        v_flex, ActiveTheme as _,
    },
    div,
    prelude::FluentBuilder as _,
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window,
};
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
    code_pages: BTreeSet<&'static str>,
    match_case: bool,
    search_deleted: bool,
    search_unallocated: bool,
    calc_hash: bool,
    stop_after_first: bool,
    error: Option<SharedString>,
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
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            keywords: cx.new(|cx| {
                TextareaState::new(window, cx).placeholder("每行一个关键词，如 password")
            }),
            regexes: cx
                .new(|cx| TextareaState::new(window, cx).placeholder("每行一个正则表达式（可选）")),
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
        cx.emit(StartSearch(json!({
            "keys": keys,
            "regexes": regexes,
            "codepages": self.code_pages.iter().collect::<Vec<_>>(),
            "match_case": self.match_case,
            "search_deleted": self.search_deleted,
            "search_empty_space": self.search_unallocated,
            "calc_hash": self.calc_hash,
            "stop_after_first": self.stop_after_first,
        })));
    }

    fn toggle(&mut self, flag: FlagAccess, value: bool, cx: &mut Context<Self>) {
        *flag(self) = value;
        cx.notify();
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
            .child(ui::section_title("关键词", cx))
            .child(Textarea::new(&self.keywords).h(gpui_kit::px(96.)))
            .child(ui::section_title("正则表达式", cx))
            .child(Textarea::new(&self.regexes).h(gpui_kit::px(64.)))
            .child(ui::section_title("编码", cx))
            .child(
                h_flex()
                    .gap_3()
                    .flex_wrap()
                    .children(CODE_PAGES.into_iter().map(|page| {
                        Checkbox::new(SharedString::from(format!("codepage-{page}")))
                            .label(page)
                            .checked(self.code_pages.contains(page))
                            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                                if *checked {
                                    this.code_pages.insert(page);
                                } else {
                                    this.code_pages.remove(page);
                                }
                                cx.notify();
                            }))
                    })),
            )
            .child(ui::section_title("选项", cx))
            .child(v_flex().gap_2().children(options.into_iter().map(
                |(id, label, flag, checked)| {
                    Checkbox::new(id).label(label).checked(checked).on_click(
                        cx.listener(move |this, value: &bool, _, cx| this.toggle(flag, *value, cx)),
                    )
                },
            )))
            .when_some(self.error.clone(), |this, error| {
                this.child(ui::error_text(error, cx))
            })
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("搜索在后台运行，进度见任务中心；关闭此窗口不会中止搜索。"),
            )
            .child(
                h_flex().justify_end().gap_2().child(
                    Button::new("start-search")
                        .primary()
                        .label("开始搜索")
                        .on_click(cx.listener(|this, _, _, cx| this.submit(cx))),
                ),
            )
    }
}
