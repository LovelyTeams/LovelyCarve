//! The hits of one search, one page of `global_search_hits_page` at a time.
use gpui_kit::{
    component::{
        h_flex,
        table::{Column, TableDelegate, TableState},
        tag::Tag,
        ActiveTheme as _, Icon, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
};
use lovelycarve_lib::{
    commands::search::global_search as search_commands,
    core::search::global_search_store::GlobalSearchHit, host::State,
};

use crate::{backend::Backend, format, ui};

const PAGE_SIZE: u64 = 500;

pub struct HitTable {
    image_id: String,
    search_id: Option<String>,
    keyword: Option<String>,
    distinct: &'static str,
    rows: Vec<GlobalSearchHit>,
    total: u64,
    loading: bool,
    error: Option<SharedString>,
    generation: u64,
    columns: Vec<Column>,
}

impl HitTable {
    pub fn new(image_id: String) -> Self {
        Self {
            image_id,
            search_id: None,
            keyword: None,
            distinct: "none",
            rows: Vec::new(),
            total: 0,
            loading: false,
            error: None,
            generation: 0,
            columns: vec![
                Column::new("file", "文件").width(160.),
                Column::new("path", "路径 / 来源").width(220.),
                Column::new("context", "命中内容").width(300.),
                Column::new("pattern", "匹配").width(120.),
                Column::new("encoding", "编码").width(80.),
                Column::new("offset", "偏移").width(120.).text_right(),
            ],
        }
    }

    pub fn row(&self, index: usize) -> Option<&GlobalSearchHit> {
        self.rows.get(index)
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn loaded(&self) -> usize {
        self.rows.len()
    }

    pub fn query(&self) -> (Option<String>, Option<String>, &'static str) {
        (self.search_id.clone(), self.keyword.clone(), self.distinct)
    }

    /// Show the hits of `search_id` from the start.
    pub fn show(&mut self, search_id: Option<String>, cx: &mut Context<TableState<Self>>) {
        self.search_id = search_id;
        self.reload(cx);
    }

    pub fn set_filter(
        &mut self,
        keyword: Option<String>,
        distinct: &'static str,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.keyword = keyword.filter(|keyword| !keyword.trim().is_empty());
        self.distinct = distinct;
        self.reload(cx);
    }

    pub fn reload(&mut self, cx: &mut Context<TableState<Self>>) {
        self.generation = self.generation.wrapping_add(1);
        self.rows.clear();
        self.total = 0;
        self.error = None;
        self.fetch(cx);
    }

    /// Fetch the rows after the loaded ones (new hits of a running search).
    pub fn fetch(&mut self, cx: &mut Context<TableState<Self>>) {
        let Some(search_id) = self.search_id.clone() else {
            cx.notify();
            return;
        };
        if self.loading {
            return;
        }
        self.loading = true;
        let generation = self.generation;
        let image_id = self.image_id.clone();
        let skip = self.rows.len() as u64;
        let keyword = self.keyword.clone();
        let distinct = self.distinct.to_owned();
        let page = Backend::global(cx).run(move |_, state| async move {
            search_commands::global_search_hits_page(
                State::new(&state),
                image_id,
                search_id,
                Some(skip),
                Some(PAGE_SIZE),
                keyword,
                Some(distinct),
                None,
            )
            .await
        });
        cx.spawn(async move |table, cx| {
            let page = page.await;
            table
                .update(cx, |table, cx| {
                    let delegate = table.delegate_mut();
                    if delegate.generation != generation {
                        delegate.loading = false;
                        return;
                    }
                    delegate.loading = false;
                    match page {
                        Ok(page) => {
                            delegate.rows.extend(page.items);
                            delegate.total = page.total;
                        }
                        Err(error) => delegate.error = Some(error.message),
                    }
                    table.refresh(cx);
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }
}

impl TableDelegate for HitTable {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(hit) = self.rows.get(row_ix) else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        match self.columns[col_ix].key.as_ref() {
            "file" => h_flex()
                .gap_2()
                .min_w_0()
                .child(Icon::new(IconName::File).text_color(muted))
                .child(div().truncate().child(hit.file_name.clone()))
                .into_any_element(),
            "path" => h_flex()
                .gap_1()
                .min_w_0()
                .when(hit.source_kind != "file", |this| {
                    this.child(
                        Tag::secondary()
                            .xsmall()
                            .child(source_label(&hit.source_kind)),
                    )
                })
                .child(
                    div()
                        .truncate()
                        .text_color(muted)
                        .child(if hit.file_path.is_empty() {
                            "—".to_owned()
                        } else {
                            hit.file_path.clone()
                        }),
                )
                .into_any_element(),
            "context" => ui::mono(hit.context_preview.replace(['\r', '\n', '\t'], " "), cx)
                .truncate()
                .into_any_element(),
            "pattern" => div()
                .truncate()
                .child(hit.pattern.clone())
                .into_any_element(),
            "encoding" => div()
                .text_color(muted)
                .child(hit.encoding.clone())
                .into_any_element(),
            "offset" => ui::mono(format::hex_offset(hit.file_offset), cx).into_any_element(),
            _ => div().into_any_element(),
        }
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        let hit = self.rows.get(row_ix)?;
        Some(match self.columns.get(col_ix)?.key.as_ref() {
            "file" => hit.file_name.clone().into(),
            "path" => hit.file_path.clone().into(),
            "context" => hit.context_preview.clone().into(),
            _ => return None,
        })
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (title, description) = match (&self.error, &self.search_id) {
            (Some(error), _) => ("无法读取命中", Some(error.clone())),
            (None, None) => (
                "没有选中的搜索",
                Some("在左侧选择一条搜索记录，或新建搜索。".into()),
            ),
            (None, Some(_)) if self.keyword.is_some() => ("没有匹配的命中", None),
            (None, Some(_)) => ("暂无命中", None),
        };
        div()
            .size_full()
            .text_color(cx.theme().muted_foreground)
            .child(ui::empty_state(
                Icon::new(IconName::Search),
                title,
                description,
            ))
    }

    fn loading(&self, _: &App) -> bool {
        self.loading && self.rows.is_empty()
    }

    fn has_more(&self, _: &App) -> bool {
        !self.loading && (self.rows.len() as u64) < self.total
    }

    fn load_more_threshold(&self) -> usize {
        100
    }

    fn load_more(&mut self, _: &mut Window, cx: &mut Context<TableState<Self>>) {
        if (self.rows.len() as u64) < self.total {
            self.fetch(cx);
        }
    }
}

/// Where a hit came from: a file, unallocated space or a partition gap.
pub fn source_label(source_kind: &str) -> SharedString {
    match source_kind {
        "file" => "文件".into(),
        "unallocated" => "未分配空间".into(),
        "diskgap" => "分区间隙".into(),
        other => SharedString::from(other.to_owned()),
    }
}
