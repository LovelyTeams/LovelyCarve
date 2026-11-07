//! The directory tree beside the file list. Partitions are the roots; a
//! directory lists its subdirectories the first time it is expanded.
use std::collections::HashMap;

use gpui_kit::{
    component::{
        h_flex,
        list::ListItem,
        tree::{tree, TreeEvent, TreeItem, TreeState},
        ActiveTheme as _, Icon, IconName, Sizable as _,
    },
    div, px, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _,
    Render, SharedString, Styled as _, Subscription, Window,
};
use lovelycarve_lib::{
    commands::filesystem::filesystem as fs_commands,
    core::{
        catalog::{directory_query::DirectoryQuery, flat_query::FlatFileFilter},
        filesystem::tree::fs_tree::{FsNode, NodeKind},
    },
    host::State,
};

use super::Crumb;
use crate::backend::Backend;

/// Directories listed per expansion; deeper listings stay in the table.
const TREE_PAGE: usize = 2_000;
const LOADING_SUFFIX: &str = "\u{0}loading";

struct NodeInfo {
    name: String,
    parent: Option<String>,
    loaded: bool,
}

/// The tree asks the browser to show a directory.
#[derive(Clone, Debug)]
pub struct OpenDirectory(pub Vec<Crumb>);

pub struct DirTree {
    image_id: String,
    state: Entity<TreeState>,
    roots: Vec<TreeItem>,
    nodes: HashMap<String, NodeInfo>,
    selected: Option<String>,
    error: Option<SharedString>,
    _tree: Subscription,
}

impl EventEmitter<OpenDirectory> for DirTree {}

fn placeholder(id: &str) -> TreeItem {
    TreeItem::new(format!("{id}{LOADING_SUFFIX}"), "加载中…").disabled(true)
}

fn directory_item(node: &FsNode) -> TreeItem {
    TreeItem::new(node.id.clone(), node.name.clone()).child(placeholder(&node.id))
}

impl DirTree {
    pub fn new(image_id: String, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| TreeState::new(cx));
        let subscription = cx.subscribe(&state, |this, _, event: &TreeEvent, cx| {
            if let TreeEvent::Expanded(id) = event {
                this.expand(id.to_string(), cx);
            }
        });
        let mut tree = Self {
            image_id,
            state,
            roots: Vec::new(),
            nodes: HashMap::new(),
            selected: None,
            error: None,
            _tree: subscription,
        };
        tree.load_children(None, cx);
        tree
    }

    /// The breadcrumb path from the root to `id`, when the tree has loaded it.
    pub fn path_to(&self, id: &str) -> Option<Vec<Crumb>> {
        let mut frames = Vec::new();
        let mut current = Some(id.to_owned());
        let mut guard = 0;
        while let Some(node_id) = current {
            let info = self.nodes.get(&node_id)?;
            frames.push(Crumb {
                id: node_id.clone(),
                name: info.name.clone(),
            });
            current = info.parent.clone();
            guard += 1;
            if guard > 4096 {
                return None;
            }
        }
        frames.push(Crumb {
            id: "root".to_owned(),
            name: "/".to_owned(),
        });
        frames.reverse();
        Some(frames)
    }

    /// Highlight the directory the list shows, when the tree has it.
    pub fn select(&mut self, id: &str, cx: &mut Context<Self>) {
        self.selected = Some(id.to_owned());
        let id = SharedString::from(id.to_owned());
        self.state.update(cx, |state, cx| {
            if let Some(index) = state.index_of(&id) {
                state.set_selected_index(Some(index), cx);
                state.scroll_to_item(index, gpui_kit::ScrollStrategy::Center);
            } else {
                state.set_selected_index(None, cx);
            }
        });
    }

    fn expand(&mut self, id: String, cx: &mut Context<Self>) {
        if self.nodes.get(&id).is_some_and(|info| !info.loaded) {
            self.load_children(Some(id), cx);
        }
    }

    fn load_children(&mut self, parent: Option<String>, cx: &mut Context<Self>) {
        let image_id = self.image_id.clone();
        let parent_id = parent.clone().unwrap_or_else(|| "root".to_owned());
        let query = DirectoryQuery {
            filter: FlatFileFilter {
                kind_query: Some("目录".to_owned()),
                deleted: Some(false),
                ..FlatFileFilter::default()
            },
            ..DirectoryQuery::default()
        };
        let page = Backend::global(cx).run(move |_, state| async move {
            fs_commands::fs_children_page(
                State::new(&state),
                image_id,
                parent_id,
                None,
                Some(TREE_PAGE),
                Some(query),
            )
            .await
        });
        cx.spawn(async move |this, cx| {
            let page = page.await;
            this.update(cx, |this, cx| {
                match page {
                    Ok(page) => {
                        let directories: Vec<FsNode> = page
                            .items
                            .into_iter()
                            .filter(|node| node.kind == NodeKind::Dir && !node.deleted)
                            .collect();
                        this.attach(parent, directories, cx);
                    }
                    Err(error) => {
                        tracing::warn!("directory tree: {error}");
                        if parent.is_none() {
                            this.error = Some(error.message);
                        } else if let Some(parent) = parent {
                            this.attach(Some(parent), Vec::new(), cx);
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Put loaded subdirectories under `parent` (the roots when `None`).
    fn attach(&mut self, parent: Option<String>, directories: Vec<FsNode>, cx: &mut Context<Self>) {
        for node in &directories {
            self.nodes.insert(
                node.id.clone(),
                NodeInfo {
                    name: node.name.clone(),
                    parent: parent.clone(),
                    loaded: false,
                },
            );
        }
        let children: Vec<TreeItem> = directories.iter().map(directory_item).collect();
        match &parent {
            None => self.roots = children,
            Some(parent) => {
                if let Some(info) = self.nodes.get_mut(parent) {
                    info.loaded = true;
                }
                let parent: SharedString = parent.clone().into();
                replace_children(&mut self.roots, &parent, children);
            }
        }
        let roots = self.roots.clone();
        let selected = self.selected.clone();
        self.state.update(cx, |state, cx| {
            state.set_items(roots, cx);
            if let Some(index) = selected.and_then(|id| state.index_of(&id.into())) {
                state.set_selected_index(Some(index), cx);
            }
        });
    }
}

/// Replace the children of the item `id` anywhere below `items`.
fn replace_children(items: &mut [TreeItem], id: &SharedString, children: Vec<TreeItem>) -> bool {
    for item in items.iter_mut() {
        if &item.id == id {
            item.children = children;
            return true;
        }
        if replace_children(&mut item.children, id, children.clone()) {
            return true;
        }
    }
    false
}

impl Render for DirTree {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(error) = &self.error {
            return div()
                .p_3()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(error.clone())
                .into_any_element();
        }
        let view = cx.entity().downgrade();
        tree(&self.state, move |index, entry, selected, _, cx| {
            let item = entry.item();
            let id = item.id.clone();
            let loading = id.ends_with(LOADING_SUFFIX);
            let icon = if loading {
                Icon::new(IconName::Loader)
            } else if entry.is_expanded() {
                Icon::new(IconName::FolderOpen)
            } else {
                Icon::new(IconName::Folder)
            };
            let theme = cx.theme();
            let icon_color = if loading {
                theme.muted_foreground
            } else {
                theme.primary
            };
            let chevron = entry.is_folder().then(|| {
                Icon::new(if entry.is_expanded() {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .small()
                .text_color(theme.muted_foreground)
            });
            let view = view.clone();
            ListItem::new(index)
                .selected(selected)
                .pl(px(6.) + px(14.) * entry.depth() as f32)
                .child(
                    h_flex()
                        .gap_1()
                        .min_w_0()
                        .child(div().w(px(16.)).flex_shrink_0().children(chevron))
                        .child(icon.text_color(icon_color))
                        .child(div().pl_1().truncate().child(item.label.clone())),
                )
                .on_click(move |_, _, cx| {
                    if loading {
                        return;
                    }
                    let id = id.to_string();
                    view.update(cx, |this, cx| {
                        this.selected = Some(id.clone());
                        if let Some(path) = this.path_to(&id) {
                            cx.emit(OpenDirectory(path));
                        }
                    })
                    .ok();
                })
        })
        .size_full()
        .into_any_element()
    }
}
