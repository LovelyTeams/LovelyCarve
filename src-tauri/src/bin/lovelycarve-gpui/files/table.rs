//! Directory and recursive listings page as the table scrolls, sorted and
//! filtered by the backend. Rows can be
//! checked for batch work and carry the examiner's tags.
mod listing;

use std::collections::{BTreeSet, HashMap};

use gpui_kit::{
    component::{
        checkbox::Checkbox,
        h_flex,
        menu::{PopupMenu, PopupMenuItem},
        table::{Column, ColumnSort, TableDelegate, TableState},
        tag::Tag,
        ActiveTheme as _, Icon, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    App, Context, IntoElement, ParentElement as _, SharedString, Styled as _, WeakEntity, Window,
};
use lovelycarve_lib::core::{
    catalog::{
        directory_query::DirectoryQuery,
        flat_query::{FlatFileFilter, FlatSortKey},
    },
    filesystem::tree::fs_tree::{FsNode, NodeKind},
};

use super::{icons::node_icon, FileBrowser};
use crate::{format, ui};

pub(super) use listing::ListingScope;
use listing::PageCursor;

type ExtraIcon = gpui_kit::assets::IconName;

#[derive(Clone, Copy)]
struct ColumnSpec {
    key: &'static str,
    name: &'static str,
    width: f32,
    sort: FlatSortKey,
}

const COLUMNS: [ColumnSpec; 6] = [
    ColumnSpec {
        key: "name",
        name: "名称",
        width: 240.,
        sort: FlatSortKey::Name,
    },
    ColumnSpec {
        key: "kind",
        name: "类型",
        width: 72.,
        sort: FlatSortKey::Kind,
    },
    ColumnSpec {
        key: "size",
        name: "大小",
        width: 92.,
        sort: FlatSortKey::Size,
    },
    ColumnSpec {
        key: "modified",
        name: "修改时间",
        width: 150.,
        sort: FlatSortKey::Modified,
    },
    ColumnSpec {
        key: "created",
        name: "创建时间",
        width: 150.,
        sort: FlatSortKey::Created,
    },
    ColumnSpec {
        key: "accessed",
        name: "访问时间",
        width: 150.,
        sort: FlatSortKey::Accessed,
    },
];

/// The tags an examiner can put on files, persisted with the case.
pub const FILE_TAGS: [&str; 3] = ["重点", "可疑", "已排除"];

/// A tag as a chip: 重点 critical, 可疑 warning, 已排除 cleared.
pub fn tag_chip(tag: &str) -> Tag {
    match tag {
        "重点" => Tag::danger(),
        "可疑" => Tag::warning(),
        "已排除" => Tag::success(),
        _ => Tag::secondary(),
    }
    .xsmall()
    .child(tag.to_owned())
}

pub fn kind_label(node: &FsNode) -> &'static str {
    match node.kind {
        NodeKind::Dir => "目录",
        NodeKind::File => "文件",
        NodeKind::Symlink => "符号链接",
        NodeKind::Unallocated => "未分配",
    }
}

/// The time a node was created, falling back to its birth time.
pub fn created_time(node: &FsNode) -> Option<&str> {
    node.timestamps
        .created
        .as_deref()
        .or(node.timestamps.birth.as_deref())
}

pub struct FileTable {
    browser: WeakEntity<FileBrowser>,
    image_id: String,
    parent_id: String,
    query: DirectoryQuery,
    scope: ListingScope,
    rows: Vec<FsNode>,
    paths: HashMap<String, String>,
    next_cursor: Option<PageCursor>,
    total: Option<usize>,
    truncated: bool,
    loading: bool,
    error: Option<SharedString>,
    /// Bumped on every reload so late pages of an earlier listing are dropped.
    generation: u64,
    columns: Vec<Column>,
    /// Checked rows of the current directory, by node id.
    checked: BTreeSet<String>,
    /// The examiner's tags of this evidence, by node id.
    tags: HashMap<String, String>,
}

impl FileTable {
    pub fn new(browser: WeakEntity<FileBrowser>, image_id: String) -> Self {
        Self {
            browser,
            image_id,
            parent_id: "root".to_owned(),
            query: DirectoryQuery::default(),
            scope: ListingScope::Directory,
            rows: Vec::new(),
            paths: HashMap::new(),
            next_cursor: None,
            total: None,
            truncated: false,
            loading: false,
            error: None,
            generation: 0,
            columns: Self::build_columns(|_| true),
            checked: BTreeSet::new(),
            tags: HashMap::new(),
        }
    }

    fn build_columns(visible: impl Fn(&str) -> bool) -> Vec<Column> {
        let mut columns = vec![Column::new("check", "").width(36.).resizable(false)];
        columns.extend(COLUMNS.iter().filter(|spec| visible(spec.key)).map(|spec| {
            let column = Column::new(spec.key, spec.name)
                .width(spec.width)
                .sortable();
            if spec.key == "size" {
                column.text_right()
            } else {
                column
            }
        }));
        columns
    }

    pub fn column_options(&self) -> Vec<(&'static str, &'static str, bool)> {
        COLUMNS
            .iter()
            .skip(1)
            .map(|spec| {
                (
                    spec.key,
                    spec.name,
                    self.columns
                        .iter()
                        .any(|column| column.key.as_ref() == spec.key),
                )
            })
            .collect()
    }

    pub fn toggle_column(&mut self, key: &str) {
        self.columns = Self::build_columns(|candidate| {
            self.columns
                .iter()
                .any(|column| column.key.as_ref() == candidate)
                != (candidate == key)
        });
    }

    /// Checked rows of the loaded listing, in listing order.
    pub fn checked_rows(&self) -> Vec<FsNode> {
        self.rows
            .iter()
            .filter(|node| self.checked.contains(&node.id))
            .cloned()
            .collect()
    }

    pub fn checked_count(&self) -> usize {
        self.checked.len()
    }

    pub fn clear_checked(&mut self) {
        self.checked.clear();
    }

    /// The rows a menu action on `node` covers: every checked row when
    /// `node` is checked, otherwise `node` alone.
    pub fn targets_for(&self, node: &FsNode) -> Vec<FsNode> {
        if self.checked.contains(&node.id) {
            self.checked_rows()
        } else {
            vec![node.clone()]
        }
    }

    pub fn tag(&self, node_id: &str) -> Option<&str> {
        self.tags.get(node_id).map(String::as_str)
    }

    pub fn set_tags(&mut self, tags: HashMap<String, String>) {
        self.tags = tags;
    }

    /// Put `tag` on `node_ids`, or clear their tags.
    pub fn apply_tag(&mut self, node_ids: &[String], tag: Option<&str>) {
        for node_id in node_ids {
            match tag {
                Some(tag) => {
                    self.tags.insert(node_id.clone(), tag.to_owned());
                }
                None => {
                    self.tags.remove(node_id);
                }
            }
        }
    }

    fn notify_browser(&self, cx: &mut Context<TableState<Self>>) {
        self.browser.update(cx, |_, cx| cx.notify()).ok();
    }

    pub fn rows(&self) -> &[FsNode] {
        &self.rows
    }

    pub fn scope(&self) -> ListingScope {
        self.scope
    }

    pub fn set_scope(&mut self, scope: ListingScope, cx: &mut Context<TableState<Self>>) {
        self.scope = scope;
        self.reload(cx);
    }

    pub fn relative_path(&self, node_id: &str) -> Option<&str> {
        self.paths.get(node_id).map(String::as_str)
    }

    pub fn row(&self, index: usize) -> Option<&FsNode> {
        self.rows.get(index)
    }

    pub fn total(&self) -> Option<usize> {
        self.total
    }

    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn is_loading(&self) -> bool {
        self.loading
    }

    pub fn has_more_rows(&self) -> bool {
        self.next_cursor.is_some()
    }

    /// List `parent_id` from the start.
    pub fn open(&mut self, parent_id: String, cx: &mut Context<TableState<Self>>) {
        self.parent_id = parent_id;
        self.scope = ListingScope::Directory;
        self.query.filter.name_query = None;
        self.reload(cx);
    }

    /// Filter by name (wildcards `*` and `?` allowed); empty shows all.
    pub fn set_name_filter(&mut self, needle: String, cx: &mut Context<TableState<Self>>) {
        let needle = Some(needle.trim().to_owned()).filter(|needle| !needle.is_empty());
        if self.query.filter.name_query == needle {
            return;
        }
        self.query.filter = FlatFileFilter {
            name_query: needle,
            ..FlatFileFilter::default()
        };
        self.reload(cx);
    }

    pub fn reload(&mut self, cx: &mut Context<TableState<Self>>) {
        self.generation = self.generation.wrapping_add(1);
        self.checked.clear();
        self.rows.clear();
        self.paths.clear();
        self.next_cursor = None;
        self.total = None;
        self.truncated = false;
        self.error = None;
        self.fetch(None, cx);
    }

    fn sort_key(&self, col_ix: usize) -> Option<FlatSortKey> {
        let key = self.columns.get(col_ix)?.key.clone();
        COLUMNS
            .iter()
            .find(|spec| spec.key == key.as_ref())
            .map(|spec| spec.sort)
    }
}

impl TableDelegate for FileTable {
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
        match sort {
            ColumnSort::Default => {
                self.query.sort = None;
                self.query.descending = false;
            }
            ColumnSort::Ascending | ColumnSort::Descending => {
                self.query.sort = self.sort_key(col_ix);
                self.query.descending = matches!(sort, ColumnSort::Descending);
            }
        }
        self.reload(cx);
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(node) = self.rows.get(row_ix) else {
            return div().into_any_element();
        };
        let key = self.columns[col_ix].key.clone();
        let muted = cx.theme().muted_foreground;
        let deleted = node.deleted;
        let text_color = if deleted {
            cx.theme().danger
        } else {
            cx.theme().foreground
        };
        match key.as_ref() {
            "check" => {
                let node_id = node.id.clone();
                Checkbox::new(("file-check", row_ix))
                    .checked(self.checked.contains(&node.id))
                    .on_click(cx.listener(move |table, checked: &bool, _, cx| {
                        let delegate = table.delegate_mut();
                        if *checked {
                            delegate.checked.insert(node_id.clone());
                        } else {
                            delegate.checked.remove(&node_id);
                        }
                        delegate.notify_browser(cx);
                        cx.notify();
                    }))
                    .into_any_element()
            }
            "name" => h_flex()
                .gap_2()
                .min_w_0()
                .child(div().flex_shrink_0().child(node_icon(node, cx)))
                .child(
                    div()
                        .truncate()
                        .text_color(text_color)
                        .when(deleted, |this| this.line_through())
                        .child(node.name.clone()),
                )
                .when_some(self.tags.get(&node.id), |this, tag| {
                    this.child(tag_chip(tag))
                })
                .when_some(self.relative_path(&node.id), |this, path| {
                    this.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(muted)
                            .child(
                                path.rsplit_once('/')
                                    .map_or("根", |(parent, _)| parent)
                                    .to_owned(),
                            ),
                    )
                })
                .into_any_element(),
            "kind" => div()
                .text_color(muted)
                .child(if deleted {
                    "已删除"
                } else {
                    kind_label(node)
                })
                .into_any_element(),
            "size" => ui::mono(
                if node.kind == NodeKind::Dir {
                    "—".to_owned()
                } else {
                    format::bytes(node.size)
                },
                cx,
            )
            .into_any_element(),
            "modified" => ui::mono(format::timestamp(node.timestamps.modified.as_deref()), cx)
                .into_any_element(),
            "created" => ui::mono(format::timestamp(created_time(node)), cx).into_any_element(),
            "accessed" => ui::mono(format::timestamp(node.timestamps.accessed.as_deref()), cx)
                .into_any_element(),
            _ => div().into_any_element(),
        }
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(column) = self.columns.get(col_ix) else {
            return div().into_any_element();
        };
        if column.key.as_ref() != "check" {
            return div()
                .size_full()
                .child(column.name.clone())
                .into_any_element();
        }
        let all =
            !self.rows.is_empty() && self.rows.iter().all(|node| self.checked.contains(&node.id));
        Checkbox::new("file-check-all")
            .checked(all)
            .on_click(cx.listener(|table, checked: &bool, _, cx| {
                let delegate = table.delegate_mut();
                if *checked {
                    let ids: Vec<String> =
                        delegate.rows.iter().map(|node| node.id.clone()).collect();
                    delegate.checked.extend(ids);
                } else {
                    delegate.checked.clear();
                }
                delegate.notify_browser(cx);
                cx.notify();
            }))
            .into_any_element()
    }

    fn cell_label(&self, row_ix: usize, col_ix: usize, _: &App) -> Option<SharedString> {
        let node = self.rows.get(row_ix)?;
        Some(match self.columns.get(col_ix)?.key.as_ref() {
            "name" => node.name.clone().into(),
            "kind" => kind_label(node).into(),
            "size" => format::bytes(node.size).into(),
            "modified" => format::timestamp(node.timestamps.modified.as_deref()).into(),
            _ => return None,
        })
    }

    fn cell_text(&self, row_ix: usize, col_ix: usize, cx: &App) -> String {
        self.cell_label(row_ix, col_ix, cx)
            .map(|label| label.to_string())
            .unwrap_or_default()
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let Some(node) = self.rows.get(row_ix).cloned() else {
            return menu;
        };
        let browser = self.browser.clone();
        let targets = self.targets_for(&node);
        let open_node = node.clone();
        let export_targets = targets.clone();
        let copy_name = node.name.clone();
        let path_node = node.clone();
        let open_browser = browser.clone();
        let export_browser = browser.clone();
        let path_browser = browser.clone();
        let hash_browser = browser.clone();
        let hash_targets: Vec<FsNode> = targets
            .iter()
            .filter(|target| target.kind != NodeKind::Dir)
            .cloned()
            .collect();
        let batch = targets.len() > 1;
        let tag_ids: Vec<String> = targets.iter().map(|target| target.id.clone()).collect();
        let menu = FILE_TAGS.iter().fold(menu, |menu, tag| {
            let (browser, ids) = (browser.clone(), tag_ids.clone());
            menu.item(
                PopupMenuItem::new(if batch {
                    format!("标记为「{tag}」（{} 项）", tag_ids.len())
                } else {
                    format!("标记为「{tag}」")
                })
                .icon(gpui_kit::component::Icon::new(ExtraIcon::Hash))
                .on_click(move |_, window, cx| {
                    browser
                        .update(cx, |browser, cx| {
                            browser.apply_tag(ids.clone(), Some(tag), window, cx)
                        })
                        .ok();
                }),
            )
        });
        let clear_ids = tag_ids.clone();
        let clear_browser = browser.clone();
        let menu = menu.item(
            PopupMenuItem::new("清除标签")
                .icon(IconName::Close)
                .on_click(move |_, window, cx| {
                    clear_browser
                        .update(cx, |browser, cx| {
                            browser.apply_tag(clear_ids.clone(), None, window, cx)
                        })
                        .ok();
                }),
        );
        let menu = if hash_targets.is_empty() {
            menu
        } else {
            menu.item(
                PopupMenuItem::new(if hash_targets.len() > 1 {
                    format!("计算哈希（{} 个文件）", hash_targets.len())
                } else {
                    "计算哈希".to_owned()
                })
                .icon(gpui_kit::component::Icon::new(
                    ExtraIcon::FingerprintPattern,
                ))
                .on_click(move |_, window, cx| {
                    hash_browser
                        .update(cx, |browser, cx| {
                            browser.hash_nodes(hash_targets.clone(), window, cx)
                        })
                        .ok();
                }),
            )
        }
        .separator();
        menu.item(
            PopupMenuItem::new(if node.kind == NodeKind::Dir {
                "打开目录"
            } else {
                "查看详情"
            })
            .icon(IconName::FolderOpen)
            .on_click(move |_, window, cx| {
                open_browser
                    .update(cx, |browser, cx| {
                        browser.open_node(open_node.clone(), window, cx)
                    })
                    .ok();
            }),
        )
        .item(
            PopupMenuItem::new(match (export_targets.len(), node.kind == NodeKind::Dir) {
                (1, true) => "导出目录…".to_owned(),
                (1, false) => "导出文件…".to_owned(),
                (count, _) => format!("导出勾选的 {count} 项…"),
            })
            .icon(gpui_kit::component::Icon::new(ExtraIcon::Download))
            .on_click(move |_, window, cx| {
                export_browser
                    .update(cx, |browser, cx| {
                        browser.export_nodes(export_targets.clone(), window, cx)
                    })
                    .ok();
            }),
        )
        .separator()
        .item(
            PopupMenuItem::new("复制名称")
                .icon(IconName::Copy)
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(copy_name.clone()))
                }),
        )
        .item(
            PopupMenuItem::new("复制路径")
                .icon(IconName::Copy)
                .on_click(move |_, _, cx| {
                    let path = path_browser
                        .read_with(cx, |browser, cx| browser.path_of(&path_node, cx))
                        .unwrap_or_default();
                    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(path))
                }),
        )
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let (title, description) = match &self.error {
            Some(error) => ("无法读取目录", Some(error.clone())),
            None if self.query.filter != FlatFileFilter::default() => ("没有匹配的文件", None),
            None => ("此目录为空", None),
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
        self.next_cursor.is_some() && !self.loading && self.error.is_none()
    }

    fn load_more_threshold(&self) -> usize {
        100
    }

    fn load_more(&mut self, _: &mut Window, cx: &mut Context<TableState<Self>>) {
        if self.loading {
            return;
        }
        if let Some(cursor) = self.next_cursor.clone() {
            self.fetch(Some(cursor), cx);
        }
    }
}
