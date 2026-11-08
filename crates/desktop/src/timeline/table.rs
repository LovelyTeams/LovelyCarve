//! Timeline events, one page of `timeline_datas` at a time.
use gpui_kit::{
    component::{
        h_flex,
        table::{Column, ColumnSort, TableDelegate, TableState},
        tag::Tag,
        ActiveTheme as _, Icon, Sizable as _,
    },
    div, App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
};
use lovelycarve_backend::{
    commands::timeline::timeline as timeline_commands,
    core::timeline::timeline_store::{TimelineQuery, TimelineRow},
    host::State,
};

use crate::{backend::Backend, ui};

const PAGE_SIZE: u64 = 500;

/// The examiner's zone in whole hours. Rows, the time range and the
/// year/month/day buckets all use this one offset, as the desktop app does.
pub fn zone_hours() -> i32 {
    chrono::Local::now().offset().local_minus_utc() / 3600
}

/// The zone the timeline shows, such as `UTC+8`.
pub fn zone_label() -> String {
    format!("UTC{:+}", zone_hours())
}

/// A timeline time (unix seconds) in the examiner's zone.
pub fn local_time(seconds: i64) -> String {
    let shifted = seconds.saturating_add(i64::from(zone_hours()) * 3600);
    match chrono::DateTime::from_timestamp(shifted, 0) {
        Some(time) => time.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => seconds.to_string(),
    }
}

/// File rows (data type 1) point at a file-system node.
pub fn is_file_row(row: &TimelineRow) -> bool {
    row.data_type == 1
}

pub struct TimelineTable {
    image_id: String,
    query: TimelineQuery,
    ascending: bool,
    rows: Vec<TimelineRow>,
    total: u64,
    loading: bool,
    error: Option<SharedString>,
    generation: u64,
    columns: Vec<Column>,
}

impl TimelineTable {
    pub fn new(image_id: String) -> Self {
        Self {
            image_id,
            query: TimelineQuery::default(),
            ascending: false,
            rows: Vec::new(),
            total: 0,
            loading: false,
            error: None,
            generation: 0,
            columns: vec![
                Column::new("time", "时间")
                    .width(170.)
                    .sortable()
                    .descending(),
                Column::new("flag", "事件类型").width(100.),
                Column::new("content", "内容").width(520.),
                Column::new("source", "来源").width(140.),
            ],
        }
    }

    pub fn row(&self, index: usize) -> Option<&TimelineRow> {
        self.rows.get(index)
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    pub fn loaded(&self) -> usize {
        self.rows.len()
    }

    pub fn set_query(&mut self, query: TimelineQuery, cx: &mut Context<TableState<Self>>) {
        self.query = query;
        self.reload(cx);
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
        if self.loading {
            return;
        }
        self.loading = true;
        let generation = self.generation;
        let image_id = self.image_id.clone();
        let query = self.query.clone();
        let skip = self.rows.len() as u64;
        let order = if self.ascending { "asc" } else { "desc" }.to_owned();
        let page = Backend::global(cx).run(move |_, state| async move {
            timeline_commands::timeline_datas(
                State::new(&state),
                image_id,
                query,
                Some(skip),
                Some(PAGE_SIZE),
                Some(order),
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

impl TableDelegate for TimelineTable {
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
        _: usize,
        sort: ColumnSort,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) {
        self.ascending = matches!(sort, ColumnSort::Ascending);
        self.reload(cx);
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(row) = self.rows.get(row_ix) else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        match self.columns[col_ix].key.as_ref() {
            "time" => ui::mono(local_time(row.tl_time), cx).into_any_element(),
            "flag" => h_flex()
                .child(Tag::secondary().small().child(row.tl_flag.clone()))
                .into_any_element(),
            "content" => div()
                .truncate()
                .text_color(if row.deleted {
                    cx.theme().danger
                } else {
                    cx.theme().foreground
                })
                .child(row.content.clone())
                .into_any_element(),
            "source" => div()
                .truncate()
                .text_color(muted)
                .child(row.from_app.clone())
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        let row = self.rows.get(row_ix)?;
        Some(match self.columns.get(col_ix)?.key.as_ref() {
            "time" => local_time(row.tl_time).into(),
            "flag" => row.tl_flag.clone().into(),
            "content" => row.content.clone().into(),
            "source" => row.from_app.clone().into(),
            _ => return None,
        })
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (title, description) = match &self.error {
            Some(error) => ("无法读取时间线", Some(error.clone())),
            None => ("没有符合条件的事件", None),
        };
        div()
            .size_full()
            .text_color(cx.theme().muted_foreground)
            .child(ui::empty_state(
                Icon::new(gpui_kit::assets::IconName::ChartColumn),
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
