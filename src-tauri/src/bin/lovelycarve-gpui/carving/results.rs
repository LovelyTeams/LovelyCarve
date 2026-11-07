//! The carved results of one scan, one page of `carve_results_page` at a time.
use gpui_kit::{
    component::{
        h_flex,
        progress::Progress,
        table::{Column, TableDelegate, TableState},
        ActiveTheme as _, Icon,
    },
    div, App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
};
use lovelycarve_lib::{
    commands::carving::carve::{self as carve_commands, CarveResultFilter},
    core::carving::carver::CarveArtifact,
    host::State,
};

use crate::{backend::Backend, format, ui};

type ExtraIcon = gpui_kit::assets::IconName;

const PAGE_SIZE: usize = 500;

pub struct ResultTable {
    scan_id: Option<String>,
    category: Option<String>,
    rows: Vec<CarveArtifact>,
    next_cursor: Option<String>,
    /// Where polling a running scan continues: after the last row loaded.
    resume_cursor: Option<String>,
    total: usize,
    truncated: bool,
    finished: bool,
    loading: bool,
    error: Option<SharedString>,
    generation: u64,
    columns: Vec<Column>,
}

impl ResultTable {
    pub fn new() -> Self {
        Self {
            scan_id: None,
            category: None,
            rows: Vec::new(),
            next_cursor: None,
            resume_cursor: None,
            total: 0,
            truncated: false,
            finished: true,
            loading: false,
            error: None,
            generation: 0,
            columns: vec![
                Column::new("name", "文件").width(220.),
                Column::new("offset", "起始偏移").width(170.),
                Column::new("size", "大小").width(100.).text_right(),
                Column::new("confidence", "置信度").width(140.),
                Column::new("region", "扫描区域").width(140.),
            ],
        }
    }

    pub fn row(&self, index: usize) -> Option<&CarveArtifact> {
        self.rows.get(index)
    }

    pub fn loaded(&self) -> usize {
        self.rows.len()
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn show(
        &mut self,
        scan_id: Option<String>,
        category: Option<String>,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.scan_id = scan_id;
        self.category = category;
        self.generation = self.generation.wrapping_add(1);
        self.rows.clear();
        self.next_cursor = None;
        self.resume_cursor = None;
        self.total = 0;
        self.error = None;
        self.loading = false;
        self.fetch(None, cx);
    }

    /// Pick up results a running scan found since the last page.
    pub fn follow(&mut self, cx: &mut Context<TableState<Self>>) {
        if self.loading || self.rows.len() >= 2_000 {
            return;
        }
        if !self.finished {
            self.fetch(self.resume_cursor.clone(), cx);
        }
    }

    fn fetch(&mut self, cursor: Option<String>, cx: &mut Context<TableState<Self>>) {
        let Some(scan_id) = self.scan_id.clone() else {
            cx.notify();
            return;
        };
        self.loading = true;
        let generation = self.generation;
        let filter = CarveResultFilter {
            category: self.category.clone(),
            ..CarveResultFilter::default()
        };
        let page = Backend::global(cx).run(move |_, state| async move {
            carve_commands::carve_results_page(
                State::new(&state),
                scan_id,
                cursor,
                Some(PAGE_SIZE),
                Some(filter),
            )
            .await
        });
        cx.spawn(async move |table, cx| {
            let page = page.await;
            table
                .update(cx, |table, cx| {
                    let delegate = table.delegate_mut();
                    if delegate.generation != generation {
                        return;
                    }
                    delegate.loading = false;
                    match page {
                        Ok(page) => {
                            let known: std::collections::HashSet<String> =
                                delegate.rows.iter().map(|row| row.id.clone()).collect();
                            delegate.rows.extend(
                                page.items
                                    .into_iter()
                                    .filter(|item| !known.contains(&item.id)),
                            );
                            delegate.next_cursor = page.next_cursor;
                            delegate.resume_cursor = page.resume_cursor;
                            delegate.total = page.total;
                            delegate.truncated = page.truncated;
                            delegate.finished = page.scan_finished;
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

impl TableDelegate for ResultTable {
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
        let Some(artifact) = self.rows.get(row_ix) else {
            return div().into_any_element();
        };
        match self.columns[col_ix].key.as_ref() {
            "name" => h_flex()
                .gap_2()
                .min_w_0()
                .child(Icon::new(ExtraIcon::FileSearch).text_color(cx.theme().primary))
                .child(div().truncate().child(artifact.name.clone()))
                .into_any_element(),
            "offset" => ui::mono(format::hex_offset(artifact.offset), cx).into_any_element(),
            "size" => ui::mono(format::bytes(artifact.length), cx).into_any_element(),
            "confidence" => h_flex()
                .gap_2()
                .child(div().w_16().child(
                    Progress::new(("confidence", row_ix)).value(f32::from(artifact.confidence)),
                ))
                .child(ui::mono(format!("{}%", artifact.confidence), cx))
                .into_any_element(),
            "region" => div()
                .truncate()
                .text_color(cx.theme().muted_foreground)
                .child(artifact.region.clone())
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        let artifact = self.rows.get(row_ix)?;
        Some(match self.columns.get(col_ix)?.key.as_ref() {
            "name" => artifact.name.clone().into(),
            "offset" => format::hex_offset(artifact.offset).into(),
            "size" => format::bytes(artifact.length).into(),
            _ => return None,
        })
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (title, description) = match (&self.error, &self.scan_id) {
            (Some(error), _) => ("无法读取结果", Some(error.clone())),
            (None, None) => (
                "没有选中的扫描",
                Some("在左侧选择一条扫描记录，或新建扫描。".into()),
            ),
            (None, Some(_)) if !self.finished => {
                ("正在扫描…", Some("发现的文件会陆续出现在这里。".into()))
            }
            (None, Some(_)) => ("没有找到可恢复的文件", None),
        };
        div()
            .size_full()
            .text_color(cx.theme().muted_foreground)
            .child(ui::empty_state(
                Icon::new(ExtraIcon::FileSearch),
                title,
                description,
            ))
    }

    fn loading(&self, _: &App) -> bool {
        self.loading && self.rows.is_empty()
    }

    fn has_more(&self, _: &App) -> bool {
        !self.loading && self.next_cursor.is_some()
    }

    fn load_more_threshold(&self) -> usize {
        100
    }

    fn load_more(&mut self, _: &mut Window, cx: &mut Context<TableState<Self>>) {
        if let Some(cursor) = self.next_cursor.clone() {
            self.fetch(Some(cursor), cx);
        }
    }
}
