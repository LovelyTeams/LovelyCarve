//! Files a scan recovered from filesystem records: deleted entries of known
//! volumes (and their live files on request) and every entry of lost
//! volumes, listed with the paths their records keep.
use std::rc::Rc;

use gpui_kit::{
    component::{
        h_flex,
        table::{Column, TableDelegate, TableState},
        tag::Tag,
        ActiveTheme as _, Icon, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
};
use lovelycarve_backend::core::filesystem::recovery::{
    RecoveryCandidate, RecoveryScan, RecoveryVolume,
};

use crate::{format, ui};

/// Volume names: the label or the filesystem, with the start sector when
/// several volumes share a name (lost NTFS volumes often do).
pub fn volume_names(volumes: &[RecoveryVolume]) -> Vec<String> {
    let names: Vec<String> = volumes
        .iter()
        .map(|volume| {
            volume
                .partition
                .label
                .as_deref()
                .map(str::trim)
                .filter(|label| !label.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| crate::evidence::filesystem_label(&volume.partition.kind))
        })
        .collect();
    names
        .iter()
        .zip(volumes)
        .map(|(name, volume)| {
            if names.iter().filter(|other| *other == name).count() > 1 {
                format!("{name} @ LBA {}", volume.partition.start_lba)
            } else {
                name.clone()
            }
        })
        .collect()
}

/// The readable files' ids, which recovery accepts.
pub fn readable_ids(scan: &RecoveryScan) -> Vec<String> {
    scan.candidates
        .iter()
        .filter(|candidate| candidate.readable)
        .map(|candidate| candidate.id.clone())
        .collect()
}

pub struct FilesystemTable {
    scan: Option<Rc<RecoveryScan>>,
    volumes: Vec<String>,
    /// Indices of the candidates the filter keeps, by path.
    shown: Vec<usize>,
    filter: String,
    loading: bool,
    error: Option<SharedString>,
    columns: Vec<Column>,
}

impl FilesystemTable {
    pub fn new() -> Self {
        Self {
            scan: None,
            volumes: Vec::new(),
            shown: Vec::new(),
            filter: String::new(),
            loading: false,
            error: None,
            columns: vec![
                Column::new("name", "名称").width(220.),
                Column::new("path", "路径").width(320.),
                Column::new("size", "大小").width(96.).text_right(),
                Column::new("modified", "修改时间").width(170.),
                Column::new("volume", "卷").width(120.),
                Column::new("state", "状态").width(90.),
            ],
        }
    }

    pub fn set_loading(&mut self) {
        self.scan = None;
        self.volumes.clear();
        self.shown.clear();
        self.loading = true;
        self.error = None;
    }

    pub fn set_result(&mut self, result: Result<Option<Rc<RecoveryScan>>, SharedString>) {
        self.loading = false;
        match result {
            Ok(scan) => {
                self.volumes = scan
                    .as_ref()
                    .map(|scan| volume_names(&scan.volumes))
                    .unwrap_or_default();
                self.scan = scan;
                self.error = None;
            }
            Err(error) => {
                self.scan = None;
                self.volumes.clear();
                self.error = Some(error);
            }
        }
        self.apply_filter();
    }

    pub fn clear(&mut self) {
        self.scan = None;
        self.volumes.clear();
        self.shown.clear();
        self.loading = false;
        self.error = None;
    }

    pub fn set_filter(&mut self, filter: &str) {
        self.filter = filter.trim().to_lowercase();
        self.apply_filter();
    }

    fn apply_filter(&mut self) {
        let Some(scan) = &self.scan else {
            self.shown.clear();
            return;
        };
        let filter = &self.filter;
        let mut shown: Vec<usize> = scan
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                filter.is_empty()
                    || candidate.node.name.to_lowercase().contains(filter.as_str())
                    || candidate.path.to_lowercase().contains(filter.as_str())
            })
            .map(|(index, _)| index)
            .collect();
        shown.sort_by(|left, right| {
            let left = &scan.candidates[*left];
            let right = &scan.candidates[*right];
            (left.volume_index, &left.path).cmp(&(right.volume_index, &right.path))
        });
        self.shown = shown;
    }

    pub fn scan(&self) -> Option<&Rc<RecoveryScan>> {
        self.scan.as_ref()
    }

    pub fn volume_name(&self, index: usize) -> String {
        self.volumes
            .get(index)
            .cloned()
            .unwrap_or_else(|| format!("卷 {index}"))
    }

    /// The file shown at `row`.
    pub fn candidate(&self, row: usize) -> Option<&RecoveryCandidate> {
        let scan = self.scan.as_ref()?;
        scan.candidates.get(*self.shown.get(row)?)
    }

    pub fn shown_count(&self) -> usize {
        self.shown.len()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    fn cell(&self, row: usize, column: usize) -> Option<String> {
        let candidate = self.candidate(row)?;
        Some(match self.columns.get(column)?.key.as_ref() {
            "name" => candidate.node.name.clone(),
            "path" => candidate.path.clone(),
            "size" => format::bytes(candidate.node.size),
            "modified" => candidate
                .node
                .timestamps
                .modified
                .clone()
                .unwrap_or_default(),
            "volume" => self.volume_name(candidate.volume_index),
            "state" if candidate.readable => "可恢复".to_owned(),
            "state" => "不可读".to_owned(),
            _ => return None,
        })
    }
}

impl TableDelegate for FilesystemTable {
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
        let Some(candidate) = self.candidate(row_ix) else {
            return div().into_any_element();
        };
        let muted = cx.theme().muted_foreground;
        let text = self.cell(row_ix, col_ix).unwrap_or_default();
        match self.columns[col_ix].key.as_ref() {
            // Deleted records keep ordinary rows with red, struck-through names.
            "name" => div()
                .truncate()
                .when(candidate.node.deleted, |this| {
                    this.text_color(cx.theme().danger).line_through()
                })
                .child(text)
                .into_any_element(),
            "path" | "volume" => div()
                .truncate()
                .text_color(muted)
                .child(text)
                .into_any_element(),
            "size" | "modified" => ui::mono(text, cx).truncate().into_any_element(),
            "state" => h_flex()
                .child(if candidate.readable {
                    Tag::success().xsmall().child(text)
                } else {
                    Tag::secondary().xsmall().child(text)
                })
                .into_any_element(),
            _ => div().into_any_element(),
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
        let (title, description) = match (&self.error, &self.scan) {
            (Some(error), _) => ("读取文件系统恢复结果失败", Some(error.clone())),
            (None, None) => ("文件系统阶段尚未完成", None),
            (None, Some(_)) if !self.filter.is_empty() => ("没有匹配的文件", None),
            (None, Some(_)) => ("没有恢复到文件记录", None),
        };
        div()
            .size_full()
            .text_color(cx.theme().muted_foreground)
            .child(ui::empty_state(
                Icon::new(gpui_kit::assets::IconName::FolderSearch),
                title,
                description,
            ))
    }

    fn loading(&self, _: &App) -> bool {
        self.loading
    }
}
