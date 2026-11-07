//! 文件列表: the directory tree, the paged listing of the current directory
//! and the detail pane of the selected node. The native-type preview is not
//! part of this app; text and hex views cover file contents.
mod detail;
mod export;
mod hex;
mod table;
mod tree;

use std::{rc::Rc, time::Duration};

use gpui_kit::{
    component::{
        breadcrumb::{Breadcrumb, BreadcrumbItem},
        button::{Button, ButtonVariants as _},
        h_flex, h_resizable,
        input::{Input, InputEvent, InputState},
        resizable_panel,
        spinner::Spinner,
        table::{DataTable, TableEvent, TableState},
        v_flex, ActiveTheme as _, Disableable as _, IconName, Sizable as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    Subscription, Task, WeakEntity, Window,
};
use lovelycarve_lib::{
    commands::{
        app_error::AppError,
        casework::tags as tags_commands,
        filesystem::{
            digests as digest_commands, filesystem as fs_commands,
            filesystem_analysis as analysis_commands,
        },
    },
    core::{
        common::hash::HashAlgo,
        evidence::image::ImageInfo,
        filesystem::tree::fs_tree::{FsNode, NodeKind},
    },
    host::State,
};
use serde_json::{json, Value};

use crate::{backend::Backend, evidence::EvidenceTab, format, ui};
use detail::FileDetail;
use table::{FileTable, FILE_TAGS};
use tree::{DirTree, OpenDirectory};

/// One level of the current directory path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Crumb {
    pub id: String,
    pub name: String,
}

impl Crumb {
    fn root() -> Self {
        Self {
            id: "root".to_owned(),
            name: "/".to_owned(),
        }
    }
}

pub struct FileBrowser {
    info: Rc<ImageInfo>,
    _tab: WeakEntity<EvidenceTab>,
    tree: Entity<DirTree>,
    table: Entity<TableState<FileTable>>,
    detail: Entity<FileDetail>,
    filter: Entity<InputState>,
    stack: Vec<Crumb>,
    show_tree: bool,
    show_detail: bool,
    /// A row to select once the listing that should contain it arrives.
    pending_select: Option<String>,
    locating: bool,
    _filter_debounce: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl FileBrowser {
    pub fn new(
        info: Rc<ImageInfo>,
        tab: WeakEntity<EvidenceTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let browser = cx.entity().downgrade();
        let image_id = info.id.clone();
        let tree = cx.new(|cx| DirTree::new(image_id.clone(), cx));
        let table = cx.new(|cx| {
            TableState::new(
                FileTable::new(browser.clone(), image_id.clone()),
                window,
                cx,
            )
            .col_movable(false)
            .row_selectable(true)
        });
        let detail = cx.new(|cx| FileDetail::new(browser.clone(), image_id.clone(), window, cx));
        let filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("筛选当前目录（支持 * ? 通配）"));
        let subscriptions = vec![
            cx.subscribe_in(&tree, window, |this, _, OpenDirectory(path), window, cx| {
                this.navigate(path.clone(), window, cx);
            }),
            cx.subscribe_in(
                &table,
                window,
                |this, table, event: &TableEvent, window, cx| match event {
                    TableEvent::SelectRow(index) => {
                        let node = table.read(cx).delegate().row(*index).cloned();
                        this.select(node, cx);
                    }
                    TableEvent::DoubleClickedRow(index) => {
                        if let Some(node) = table.read(cx).delegate().row(*index).cloned() {
                            this.open_node(node, window, cx);
                        }
                    }
                    _ => {}
                },
            ),
            cx.observe(&table, |this, _, cx| this.on_rows_changed(cx)),
            cx.subscribe_in(&filter, window, |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                    let needle = input.read(cx).value().to_string();
                    this.schedule_filter(needle, cx);
                }
            }),
        ];
        let mut browser = Self {
            info,
            _tab: tab,
            tree,
            table,
            detail,
            filter,
            stack: vec![Crumb::root()],
            show_tree: true,
            show_detail: true,
            pending_select: None,
            locating: false,
            _filter_debounce: None,
            _subscriptions: subscriptions,
        };
        browser.reload_listing(cx);
        browser.load_tags(cx);
        browser
    }

    /// The examiner's tags of this evidence, kept with the case.
    fn load_tags(&mut self, cx: &mut Context<Self>) {
        let image_id = self.info.id.clone();
        let loading = Backend::global(cx).run(move |_, state| async move {
            tags_commands::fs_tags_load(State::new(&state), image_id).await
        });
        let table = self.table.clone();
        cx.spawn(async move |_, cx| match loading.await {
            Ok(tags) => table.update(cx, |table, cx| {
                table.delegate_mut().set_tags(tags);
                cx.notify();
            }),
            Err(error) => tracing::warn!("load file tags: {}", error.message),
        })
        .detach();
    }

    /// The tag on `node_id`, if any.
    pub fn tag_of(&self, node_id: &str, cx: &gpui_kit::App) -> Option<String> {
        self.table
            .read(cx)
            .delegate()
            .tag(node_id)
            .map(str::to_owned)
    }

    fn current_id(&self) -> String {
        self.stack
            .last()
            .map(|crumb| crumb.id.clone())
            .unwrap_or_else(|| "root".to_owned())
    }

    /// The node's path from the evidence root, as the breadcrumb names it.
    pub fn path_of(&self, node: &FsNode) -> String {
        let mut parts: Vec<&str> = self
            .stack
            .iter()
            .skip(1)
            .map(|crumb| crumb.name.as_str())
            .collect();
        parts.push(node.name.as_str());
        format!("/{}", parts.join("/"))
    }

    fn reload_listing(&mut self, cx: &mut Context<Self>) {
        let parent = self.current_id();
        self.table
            .update(cx, |table, cx| table.delegate_mut().open(parent, cx));
    }

    fn navigate(&mut self, stack: Vec<Crumb>, window: &mut Window, cx: &mut Context<Self>) {
        if stack.is_empty() {
            return;
        }
        self.stack = stack;
        self.filter
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.table.update(cx, |table, cx| {
            table.clear_selection(cx);
            let delegate = table.delegate_mut();
            delegate.set_name_filter(String::new(), cx);
        });
        self.reload_listing(cx);
        let current = self.current_id();
        self.tree.update(cx, |tree, cx| tree.select(&current, cx));
        self.detail
            .update(cx, |detail, cx| detail.show(None, String::new(), cx));
        cx.notify();
    }

    fn go_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.stack.len() > 1 {
            let mut stack = self.stack.clone();
            stack.pop();
            self.navigate(stack, window, cx);
        }
    }

    fn select(&mut self, node: Option<FsNode>, cx: &mut Context<Self>) {
        let path = node
            .as_ref()
            .map(|node| self.path_of(node))
            .unwrap_or_default();
        self.detail
            .update(cx, |detail, cx| detail.show(node, path, cx));
        cx.notify();
    }

    /// Enter a directory, or show a file's details.
    pub fn open_node(&mut self, node: FsNode, window: &mut Window, cx: &mut Context<Self>) {
        if node.kind == NodeKind::Dir {
            let mut stack = self.stack.clone();
            stack.push(Crumb {
                id: node.id.clone(),
                name: node.name.clone(),
            });
            self.navigate(stack, window, cx);
        } else {
            self.show_detail = true;
            self.select(Some(node), cx);
        }
    }

    pub fn export_node(&mut self, node: FsNode, window: &mut Window, cx: &mut Context<Self>) {
        export::export_node(self.info.id.clone(), node, window, cx);
    }

    /// Export one node as usual, or several into one chosen folder.
    pub fn export_nodes(
        &mut self,
        nodes: Vec<FsNode>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match nodes.as_slice() {
            [] => {}
            [single] => export::export_node(self.info.id.clone(), single.clone(), window, cx),
            _ => export::export_many(self.info.id.clone(), nodes, window, cx),
        }
    }

    /// Put `tag` on `node_ids` (or clear their tags) and save the change
    /// with the case.
    pub fn apply_tag(
        &mut self,
        node_ids: Vec<String>,
        tag: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if node_ids.is_empty() {
            return;
        }
        self.table.update(cx, |table, cx| {
            table.delegate_mut().apply_tag(&node_ids, tag);
            cx.notify();
        });
        let image_id = self.info.id.clone();
        let (upserts, deletes) = match tag {
            Some(tag) => (
                node_ids
                    .iter()
                    .map(|id| (id.clone(), tag.to_owned()))
                    .collect(),
                Vec::new(),
            ),
            None => (std::collections::HashMap::new(), node_ids),
        };
        let saving = Backend::global(cx).run(move |_, state| async move {
            tags_commands::fs_tags_apply_changes(State::new(&state), image_id, upserts, deletes)
                .await
        });
        cx.spawn_in(window, async move |_, cx| {
            let saved = saving.await;
            cx.update(|window, cx| match saved {
                Ok(true) => {}
                Ok(false) => ui::notify_error(
                    window,
                    cx,
                    "标签未保存",
                    "当前没有打开的案件，标签只在本次运行中有效。",
                ),
                Err(error) => ui::notify_error(window, cx, "保存标签失败", error.message),
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    /// Hash files (MD5, SHA1, SHA256) one after another and keep the clean
    /// digests with the case; digests over zero-filled bad sectors are
    /// reported but not kept as the files' hashes.
    pub fn hash_nodes(&mut self, nodes: Vec<FsNode>, window: &mut Window, cx: &mut Context<Self>) {
        let files: Vec<FsNode> = nodes
            .into_iter()
            .filter(|node| node.kind != NodeKind::Dir)
            .collect();
        if files.is_empty() {
            return;
        }
        let image_id = self.info.id.clone();
        let detail = self.detail.clone();
        let backend = Backend::global(cx);
        cx.spawn_in(window, async move |_, cx| {
            let total = files.len();
            cx.update(|window, cx| ui::notify_success(window, cx, "开始计算哈希", format!("{total} 个文件")))
                .ok();
            let mut entries = serde_json::Map::new();
            let (mut faulted, mut failed) = (0usize, 0usize);
            let mut last_error = None;
            for node in files {
                let (image, node_id) = (image_id.clone(), node.id.clone());
                let digest = backend
                    .run(move |_, state| async move {
                        analysis_commands::fs_digest(
                            State::new(&state),
                            image,
                            node_id,
                            vec![HashAlgo::Md5, HashAlgo::Sha1, HashAlgo::Sha256],
                        )
                        .await
                    })
                    .await;
                match digest {
                    Ok(digest) if digest.read_faults.read_faults_detected || digest.truncated => faulted += 1,
                    Ok(digest) => {
                        entries.insert(
                            node.id,
                            json!({ "md5": digest.md5, "sha1": digest.sha1, "sha256": digest.sha256 }),
                        );
                    }
                    Err(error) => {
                        failed += 1;
                        last_error = Some(error.message);
                    }
                }
            }
            let hashed = entries.len();
            let stored = Value::Object(entries);
            let saving = stored.clone();
            let persisted = backend
                .run(move |_, state| async move {
                    let entries =
                        serde_json::from_value(saving).map_err(|error| AppError::invalid_input(error.to_string()))?;
                    digest_commands::fs_digest_persist(State::new(&state), image_id, entries).await
                })
                .await;
            cx.update(|window, cx| {
                detail.update(cx, |detail, cx| detail.digests_changed(&stored, cx));
                let mut parts = vec![format!("{hashed} 个文件已计算")];
                if faulted > 0 {
                    parts.push(format!("{faulted} 个含补零未保存"));
                }
                if matches!(persisted, Ok(false)) && hashed > 0 {
                    parts.push("当前没有案件，结果未保存".to_owned());
                }
                match (last_error, persisted) {
                    (Some(error), _) => ui::notify_error(
                        window,
                        cx,
                        "哈希计算完成（部分失败）",
                        format!("{} · 失败 {failed}（{error}）", parts.join(" · ")),
                    ),
                    (None, Err(error)) => ui::notify_error(window, cx, "保存哈希失败", error.message),
                    (None, Ok(_)) => ui::notify_success(window, cx, "哈希计算完成", parts.join(" · ")),
                }
            })
            .ok();
        })
        .detach();
    }

    fn schedule_filter(&mut self, needle: String, cx: &mut Context<Self>) {
        let table = self.table.clone();
        self._filter_debounce = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            table.update(cx, |table, cx| {
                table.delegate_mut().set_name_filter(needle, cx)
            });
        }));
    }

    /// Select the row a locate asked for, once its listing has it.
    fn on_rows_changed(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.pending_select.clone() else {
            return;
        };
        let (index, more) = {
            let delegate = self.table.read(cx).delegate();
            (
                delegate.rows().iter().position(|node| node.id == target),
                delegate.has_more_rows() || delegate.is_loading(),
            )
        };
        match index {
            Some(index) => {
                self.pending_select = None;
                self.table.update(cx, |table, cx| {
                    table.set_selected_row(index, cx);
                    table.scroll_to_row(index, cx);
                });
            }
            None if !more => self.pending_select = None,
            None => {}
        }
    }

    /// Show a node of this evidence: its directory, and the node selected.
    pub fn locate(&mut self, node_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let image_id = self.info.id.clone();
        self.locating = true;
        cx.notify();
        let located = Backend::global(cx).run(move |_, state| async move {
            fs_commands::fs_node_locate(State::new(&state), image_id, node_id).await
        });
        cx.spawn_in(window, async move |this, cx| {
            let located = located.await;
            this.update_in(cx, |this, window, cx| {
                this.locating = false;
                match located {
                    Ok(located) => {
                        // The frames start at the evidence root already.
                        let mut stack: Vec<Crumb> = located
                            .frames
                            .into_iter()
                            .map(|frame| Crumb {
                                id: frame.id,
                                name: frame.name,
                            })
                            .collect();
                        if stack.first().is_none_or(|crumb| crumb.id != "root") {
                            stack.insert(0, Crumb::root());
                        }
                        this.navigate(stack, window, cx);
                        this.pending_select = located.file_id;
                    }
                    Err(error) => ui::notify_error(window, cx, "定位失败", error.message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Batch actions on the checked rows.
    fn render_checked_actions(&self, checked: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let checked_ids = |this: &Self, cx: &gpui_kit::App| -> Vec<String> {
            this.table
                .read(cx)
                .delegate()
                .checked_rows()
                .into_iter()
                .map(|node| node.id)
                .collect()
        };
        h_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("已勾选 {checked} 项")),
            )
            .child(
                Button::new("export-checked")
                    .small()
                    .ghost()
                    .icon(gpui_kit::component::Icon::new(
                        gpui_kit::assets::IconName::Download,
                    ))
                    .label("导出")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let nodes = this.table.read(cx).delegate().checked_rows();
                        this.export_nodes(nodes, window, cx);
                    })),
            )
            .child(
                Button::new("hash-checked")
                    .small()
                    .ghost()
                    .icon(gpui_kit::component::Icon::new(
                        gpui_kit::assets::IconName::FingerprintPattern,
                    ))
                    .label("哈希")
                    .on_click(cx.listener(|this, _, window, cx| {
                        let nodes = this.table.read(cx).delegate().checked_rows();
                        this.hash_nodes(nodes, window, cx);
                    })),
            )
            .children(FILE_TAGS.iter().enumerate().map(|(index, tag)| {
                Button::new(("tag-checked", index))
                    .xsmall()
                    .ghost()
                    .label(*tag)
                    .tooltip(format!("标记为「{tag}」"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let ids = checked_ids(this, cx);
                        this.apply_tag(ids, Some(tag), window, cx);
                    }))
            }))
            .child(
                Button::new("untag-checked")
                    .xsmall()
                    .ghost()
                    .label("清除标签")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let ids = checked_ids(this, cx);
                        this.apply_tag(ids, None, window, cx);
                    })),
            )
            .child(
                Button::new("uncheck-all")
                    .xsmall()
                    .ghost()
                    .icon(IconName::Close)
                    .tooltip("取消勾选")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.table.update(cx, |table, cx| {
                            table.delegate_mut().clear_checked();
                            cx.notify();
                        });
                        cx.notify();
                    })),
            )
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let checked = self.table.read(cx).delegate().checked_count();
        let crumbs = self.stack.clone();
        let breadcrumb =
            Breadcrumb::new().children(crumbs.iter().enumerate().map(|(index, crumb)| {
                let target: Vec<Crumb> = crumbs[..=index].to_vec();
                let view = cx.entity().downgrade();
                let label = if index == 0 {
                    "根".to_owned()
                } else {
                    crumb.name.clone()
                };
                BreadcrumbItem::new(label).on_click(move |_, window, cx| {
                    view.update(cx, |this, cx| this.navigate(target.clone(), window, cx))
                        .ok();
                })
            }));
        ui::page_toolbar(cx)
            .child(
                Button::new("toggle-tree")
                    .ghost()
                    .small()
                    .icon(IconName::PanelLeft)
                    .tooltip(if self.show_tree {
                        "隐藏目录树"
                    } else {
                        "显示目录树"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_tree = !this.show_tree;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("go-up")
                    .ghost()
                    .small()
                    .icon(IconName::ArrowLeft)
                    .tooltip("上一级")
                    .disabled(self.stack.len() <= 1)
                    .on_click(cx.listener(|this, _, window, cx| this.go_up(window, cx))),
            )
            .child(div().flex_1().min_w_0().overflow_hidden().child(breadcrumb))
            .when(self.locating, |this| this.child(Spinner::new().small()))
            .when(checked > 0, |this| {
                this.child(self.render_checked_actions(checked, cx))
            })
            .child(
                div().w(px(260.)).child(
                    Input::new(&self.filter)
                        .small()
                        .cleanable(true)
                        .prefix(gpui_kit::component::Icon::new(IconName::Search).small()),
                ),
            )
            .child(
                Button::new("refresh-listing")
                    .ghost()
                    .small()
                    .icon(IconName::RefreshCw)
                    .tooltip("刷新")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.table
                            .update(cx, |table, cx| table.delegate_mut().reload(cx));
                    })),
            )
            .child(
                Button::new("toggle-detail")
                    .ghost()
                    .small()
                    .icon(IconName::PanelRight)
                    .tooltip(if self.show_detail {
                        "隐藏详情"
                    } else {
                        "显示详情"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_detail = !this.show_detail;
                        cx.notify();
                    })),
            )
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let table = self.table.read(cx).delegate();
        let loaded = table.rows().len();
        let count = match table.total() {
            Some(total) if total > loaded => format!(
                "共 {} 项 · 已加载 {}",
                format::count(total as u64),
                format::count(loaded as u64)
            ),
            Some(total) => format!("共 {} 项", format::count(total as u64)),
            None if table.has_more_rows() => format!("已加载 {} 项", format::count(loaded as u64)),
            None => format!("共 {} 项", format::count(loaded as u64)),
        };
        let selected = self.detail.read(cx).node().map(|node| node.name.clone());
        ui::page_footer(
            h_flex()
                .gap_2()
                .child(count)
                .when(table.is_truncated(), |this| {
                    this.child("（目录条目已截断）")
                })
                .when(table.is_loading(), |this| {
                    this.child(Spinner::new().xsmall())
                }),
            div().when_some(selected, |this, name| this.child(format!("已选 {name}"))),
            cx,
        )
    }
}

impl Render for FileBrowser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let error = self.table.read(cx).delegate().error().cloned();
        let listing = v_flex()
            .size_full()
            .when_some(
                error.filter(|_| !self.table.read(cx).delegate().rows().is_empty()),
                |this, error| this.child(div().px_3().py_1().child(ui::error_text(error, cx))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(DataTable::new(&self.table).bordered(false).small()),
            );
        v_flex()
            .size_full()
            .child(self.render_toolbar(cx))
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("file-browser")
                        .child(
                            resizable_panel()
                                .visible(self.show_tree)
                                .size(px(260.))
                                .size_range(px(160.)..px(520.))
                                .child(
                                    div()
                                        .size_full()
                                        .border_r_1()
                                        .border_color(cx.theme().border)
                                        .child(self.tree.clone()),
                                ),
                        )
                        .child(resizable_panel().child(listing))
                        .child(
                            resizable_panel()
                                .visible(self.show_detail)
                                .size(px(420.))
                                .size_range(px(280.)..px(900.))
                                .child(
                                    div()
                                        .size_full()
                                        .border_l_1()
                                        .border_color(cx.theme().border)
                                        .child(self.detail.clone()),
                                ),
                        ),
                ),
            )
            .child(self.render_footer(cx))
    }
}
