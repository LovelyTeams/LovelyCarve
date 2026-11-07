//! One page of a BCP dataset's records, filtered by the toolbar text.
use gpui_kit::{
    component::{
        table::{Column, TableDelegate, TableState},
        ActiveTheme as _, Icon,
    },
    div, App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
};
use lovelycarve_lib::core::analysis::bcp::{BcpField, BcpRow};

use crate::{format, ui};

/// A record of the current page.
#[derive(Clone, Debug)]
pub struct Record {
    pub number: u64,
    pub file_name: String,
    pub values: Vec<String>,
}

impl From<BcpRow> for Record {
    fn from(row: BcpRow) -> Self {
        Self {
            number: row.number,
            file_name: row.file_name,
            values: row.values,
        }
    }
}

/// The fields a page shows: the first five besides the collection target,
/// and a content field when one exists further on.
pub fn visible_fields(fields: &[BcpField]) -> Vec<usize> {
    let candidates: Vec<usize> = fields
        .iter()
        .enumerate()
        .filter(|(_, field)| field.name != "COLLECT_TARGET_ID")
        .map(|(index, _)| index)
        .collect();
    let mut chosen: Vec<usize> = candidates.iter().copied().take(5).collect();
    let content = candidates.iter().copied().find(|index| {
        fields.get(*index).is_some_and(|field| {
            ["内容", "消息", "文本"]
                .iter()
                .any(|word| field.label.contains(word))
        })
    });
    if let Some(content) = content.filter(|content| !chosen.contains(content)) {
        chosen.push(content);
    }
    chosen
}

/// A field's column title.
pub fn field_title(field: &BcpField) -> String {
    if field.label.is_empty() {
        field.name.clone()
    } else {
        field.label.clone()
    }
}

pub struct BcpTable {
    fields: Vec<BcpField>,
    visible: Vec<usize>,
    records: Vec<Record>,
    /// Indices of the records the filter keeps.
    shown: Vec<usize>,
    filter: String,
    loading: bool,
    error: Option<SharedString>,
    columns: Vec<Column>,
}

impl BcpTable {
    pub fn new() -> Self {
        Self {
            fields: Vec::new(),
            visible: Vec::new(),
            records: Vec::new(),
            shown: Vec::new(),
            filter: String::new(),
            loading: false,
            error: None,
            columns: Vec::new(),
        }
    }

    /// Show a dataset: its columns, no records until the first page arrives.
    pub fn set_fields(&mut self, fields: Vec<BcpField>) {
        self.visible = visible_fields(&fields);
        self.columns = std::iter::once(Column::new("number", "#").width(75.))
            .chain(self.visible.iter().filter_map(|index| {
                let field = fields.get(*index)?;
                Some(
                    Column::new(
                        SharedString::from(format!("field-{index}")),
                        field_title(field),
                    )
                    .width(160.),
                )
            }))
            .chain(std::iter::once(
                Column::new("source", "来源文件").width(190.),
            ))
            .collect();
        self.fields = fields;
        self.records.clear();
        self.shown.clear();
        self.error = None;
    }

    pub fn set_loading(&mut self) {
        self.loading = true;
        self.error = None;
        self.records.clear();
        self.shown.clear();
    }

    pub fn set_page(&mut self, page: Result<Vec<Record>, SharedString>) {
        self.loading = false;
        match page {
            Ok(records) => {
                self.records = records;
                self.error = None;
            }
            Err(error) => {
                self.records.clear();
                self.error = Some(error);
            }
        }
        self.apply_filter();
    }

    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.trim().to_lowercase();
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        let filter = &self.filter;
        self.shown = self
            .records
            .iter()
            .enumerate()
            .filter(|(_, record)| {
                filter.is_empty()
                    || record
                        .values
                        .iter()
                        .any(|value| value.to_lowercase().contains(filter.as_str()))
            })
            .map(|(index, _)| index)
            .collect();
    }

    /// The record shown at `row`.
    pub fn record(&self, row: usize) -> Option<&Record> {
        self.records.get(*self.shown.get(row)?)
    }

    pub fn shown_count(&self) -> usize {
        self.shown.len()
    }

    pub fn page_count(&self) -> usize {
        self.records.len()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    fn cell(&self, row: usize, column: usize) -> Option<String> {
        let record = self.record(row)?;
        let key = self.columns.get(column)?.key.as_ref();
        Some(match key {
            "number" => format::count(record.number),
            "source" => record.file_name.clone(),
            key => {
                let index: usize = key.strip_prefix("field-")?.parse().ok()?;
                record.values.get(index).cloned().unwrap_or_default()
            }
        })
    }
}

impl TableDelegate for BcpTable {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.shown.len()
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
        let text = self.cell(row_ix, col_ix).unwrap_or_default();
        match self.columns.get(col_ix).map(|column| column.key.as_ref()) {
            Some("number" | "source") => ui::mono(text, cx)
                .truncate()
                .text_color(cx.theme().muted_foreground)
                .into_any_element(),
            _ if text.is_empty() => div()
                .text_color(cx.theme().muted_foreground)
                .child("—")
                .into_any_element(),
            _ => div().truncate().child(text).into_any_element(),
        }
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        self.cell(row_ix, col_ix).map(SharedString::from)
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (title, description) = match &self.error {
            Some(error) => ("无法读取记录", Some(error.clone())),
            None if !self.filter.is_empty() => ("当前页没有匹配的记录", None),
            None => ("该数据集没有记录", None),
        };
        div()
            .size_full()
            .text_color(cx.theme().muted_foreground)
            .child(ui::empty_state(
                Icon::new(gpui_kit::assets::IconName::Database),
                title,
                description,
            ))
    }

    fn loading(&self, _: &App) -> bool {
        self.loading
    }
}

#[cfg(test)]
mod tests {
    use lovelycarve_lib::core::analysis::bcp::BcpField;

    use super::visible_fields;

    fn field(name: &str, label: &str) -> BcpField {
        BcpField {
            key: name.to_owned(),
            name: name.to_owned(),
            label: label.to_owned(),
        }
    }

    #[test]
    fn pages_show_five_fields_and_the_content_field() {
        let fields = [
            field("COLLECT_TARGET_ID", "采集目标"),
            field("A", "账号"),
            field("B", "昵称"),
            field("C", "时间"),
            field("D", "类型"),
            field("E", "方向"),
            field("F", "状态"),
            field("G", "消息内容"),
        ];
        assert_eq!(visible_fields(&fields), vec![1, 2, 3, 4, 5, 7]);
        assert_eq!(visible_fields(&fields[..3]), vec![1, 2]);
    }
}
