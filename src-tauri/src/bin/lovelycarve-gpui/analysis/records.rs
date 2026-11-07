//! The records of one result node, one page of `analysis_records_page` at a
//! time. Columns come from the plugin's record schema.
use gpui_kit::{
    component::{
        table::{Column, ColumnSort, TableDelegate, TableState},
        tag::Tag,
        ActiveTheme as _, Icon, IconName, Sizable as _,
    },
    div, App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
};
use lovelycarve_lib::{
    commands::analysis::analysis_results as analysis_commands,
    core::analysis::results::models::{AnalysisRecord, RecordColumn},
    host::State,
};

use crate::{backend::Backend, format, ui};

const PAGE_SIZE: i64 = 200;

/// The node whose records the table lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordSet {
    pub image_id: String,
    pub run_id: String,
    pub record_type: String,
    pub legacy: bool,
}

/// A record value as text, formatted by its column kind.
pub fn value_text(value: Option<&serde_json::Value>, kind: &str) -> String {
    match value {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(text)) if kind == "timestamp" => {
            format::timestamp(Some(text))
        }
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(serde_json::Value::Number(number)) if kind == "size" => number
            .as_u64()
            .map(format::bytes)
            .unwrap_or_else(|| number.to_string()),
        Some(serde_json::Value::Bool(flag)) => if *flag { "是" } else { "否" }.to_owned(),
        Some(other) => other.to_string(),
    }
}

pub struct RecordTable {
    set: Option<RecordSet>,
    keyword: Option<String>,
    sort: Option<(String, bool)>,
    schema: Vec<RecordColumn>,
    rows: Vec<AnalysisRecord>,
    total: u64,
    loading: bool,
    error: Option<SharedString>,
    generation: u64,
    columns: Vec<Column>,
}

impl RecordTable {
    pub fn new() -> Self {
        Self {
            set: None,
            keyword: None,
            sort: None,
            schema: Vec::new(),
            rows: Vec::new(),
            total: 0,
            loading: false,
            error: None,
            generation: 0,
            columns: Vec::new(),
        }
    }

    pub fn row(&self, index: usize) -> Option<&AnalysisRecord> {
        self.rows.get(index)
    }

    pub fn schema(&self) -> &[RecordColumn] {
        &self.schema
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn loaded(&self) -> usize {
        self.rows.len()
    }

    pub fn show(&mut self, set: Option<RecordSet>, cx: &mut Context<TableState<Self>>) {
        if self.set != set {
            self.sort = None;
        }
        self.set = set;
        self.reload(cx);
    }

    pub fn set_keyword(&mut self, keyword: String, cx: &mut Context<TableState<Self>>) {
        let keyword = Some(keyword.trim().to_owned()).filter(|keyword| !keyword.is_empty());
        if keyword != self.keyword {
            self.keyword = keyword;
            self.reload(cx);
        }
    }

    pub fn reload(&mut self, cx: &mut Context<TableState<Self>>) {
        self.generation = self.generation.wrapping_add(1);
        self.rows.clear();
        self.total = 0;
        self.error = None;
        self.loading = false;
        self.fetch(cx);
    }

    fn fetch(&mut self, cx: &mut Context<TableState<Self>>) {
        let Some(set) = self.set.clone() else {
            self.schema.clear();
            self.columns.clear();
            cx.notify();
            return;
        };
        self.loading = true;
        let generation = self.generation;
        let keyword = self.keyword.clone();
        let (sort, descending) = match &self.sort {
            Some((column, descending)) => (Some(column.clone()), Some(*descending)),
            None => (None, None),
        };
        let skip = self.rows.len() as i64;
        let page = Backend::global(cx).run(move |_, state| async move {
            analysis_commands::analysis_records_page(
                State::new(&state),
                set.image_id,
                set.run_id,
                set.record_type,
                Some(set.legacy),
                keyword,
                sort,
                descending,
                skip,
                PAGE_SIZE,
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
                    let mut columns_changed = false;
                    match page {
                        Ok(page) => {
                            if delegate.schema != page.columns {
                                delegate.schema = page.columns;
                                delegate.columns = schema_columns(&delegate.schema);
                                columns_changed = true;
                            }
                            delegate.rows.extend(page.items);
                            delegate.total = page.total;
                        }
                        Err(error) => delegate.error = Some(error.message),
                    }
                    if columns_changed {
                        table.refresh_header_layout(cx);
                    }
                    table.refresh(cx);
                    cx.notify();
                })
                .ok();
        })
        .detach();
    }
}

/// Visible schema fields, or the record title when the schema has none.
fn schema_columns(schema: &[RecordColumn]) -> Vec<Column> {
    let mut columns: Vec<Column> = schema
        .iter()
        .filter(|column| !column.hidden)
        .map(|column| {
            let width = match column.kind {
                "timestamp" => 170.,
                "size" | "integer" | "number" => 110.,
                _ => 180.,
            };
            Column::new(column.id.clone(), column.label.clone())
                .width(width)
                .sortable()
        })
        .collect();
    if columns.is_empty() {
        columns.push(Column::new("__title", "名称").width(320.));
    }
    columns
}

impl TableDelegate for RecordTable {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.rows.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns[col_ix].clone()
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        let key = self
            .columns
            .get(col_ix)
            .map(|column| column.key.to_string());
        self.sort = match (sort, key) {
            (ColumnSort::Default, _) | (_, None) => None,
            (ColumnSort::Ascending, Some(key)) => Some((key, false)),
            (ColumnSort::Descending, Some(key)) => Some((key, true)),
        };
        self.reload(cx);
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (Some(record), Some(column)) = (self.rows.get(row_ix), self.columns.get(col_ix)) else {
            return div().into_any_element();
        };
        if column.key.as_ref() == "__title" {
            return div()
                .truncate()
                .child(record.title.clone())
                .into_any_element();
        }
        let kind = self
            .schema
            .iter()
            .find(|schema| schema.id == column.key.as_ref())
            .map(|schema| schema.kind)
            .unwrap_or("text");
        let text = value_text(record.values.get(column.key.as_ref()), kind);
        if kind == "timestamp" || kind == "size" {
            ui::mono(text, cx).into_any_element()
        } else {
            div().truncate().child(text).into_any_element()
        }
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        let record = self.rows.get(row_ix)?;
        let column = self.columns.get(col_ix)?;
        if column.key.as_ref() == "__title" {
            return Some(record.title.clone().into());
        }
        Some(value_text(record.values.get(column.key.as_ref()), "text").into())
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (title, description) = match (&self.error, &self.set) {
            (Some(error), _) => ("无法读取分析结果", Some(error.clone())),
            (None, None) => (
                "选择一类结果",
                Some("在左侧结果树中选择记录类型查看明细。".into()),
            ),
            (None, Some(_)) if self.keyword.is_some() => ("没有匹配的记录", None),
            (None, Some(_)) => ("没有记录", None),
        };
        div()
            .size_full()
            .text_color(cx.theme().muted_foreground)
            .child(ui::empty_state(
                Icon::new(IconName::Inbox),
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
        50
    }

    fn load_more(&mut self, _: &mut Window, cx: &mut Context<TableState<Self>>) {
        if (self.rows.len() as u64) < self.total {
            self.fetch(cx);
        }
    }
}

/// A severity chip, when the analyzer rated the record.
pub fn severity_tag(severity: &str) -> Tag {
    match severity {
        "critical" | "high" => Tag::danger().xsmall().child("高"),
        "medium" | "warning" | "warn" => Tag::warning().xsmall().child("中"),
        "low" => Tag::info().xsmall().child("低"),
        _ => Tag::secondary().xsmall().child("信息"),
    }
}
