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
    div, px, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, ScrollStrategy, SharedString, Styled as _,
    Subscription, Window,
};
use lovelycarve_backend::{
    commands::filesystem::filesystem as fs_commands,
    core::{
        catalog::{directory_query::DirectoryQuery, flat_query::FlatFileFilter},
        filesystem::tree::fs_tree::{FsNode, NodeKind},
    },
    host::State,
};

use super::{icons::directory_icon, Crumb};
use crate::backend::Backend;

/// Directories listed per expansion; deeper listings stay in the table.
const TREE_PAGE: usize = 2_000;
const LOADING_SUFFIX: &str = "\u{0}loading";

struct NodeInfo {
    name: String,
    parent: Option<String>,
    children: Children,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Children {
    Unloaded,
    Loading,
    Loaded,
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
    pending_reveal: Vec<SharedString>,
    roots_ready: bool,
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
            pending_reveal: Vec::new(),
            roots_ready: false,
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

    fn open_directory(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(path) = self.path_to(id) {
            self.selected = Some(id.to_owned());
            cx.emit(OpenDirectory(path));
        }
    }

    /// Reveal the directory the list shows, loading only its missing ancestors.
    pub fn select(&mut self, stack: &[Crumb], cx: &mut Context<Self>) {
        self.pending_reveal = stack
            .iter()
            .skip(1)
            .map(|crumb| crumb.id.clone().into())
            .collect();
        self.selected = self.pending_reveal.last().map(ToString::to_string);
        if self.selected.is_none() {
            self.state
                .update(cx, |state, cx| state.set_selected_index(None, cx));
            return;
        }
        self.reveal_selection(cx);
    }

    fn reveal_selection(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.pending_reveal.last().cloned() else {
            return;
        };
        if !self.roots_ready {
            return;
        }
        let missing = self
            .pending_reveal
            .iter()
            .position(|id| !self.nodes.contains_key(id.as_str()));
        if let Some(index) = missing {
            // A lazy row already contains a placeholder. Revealing that row
            // opens its parent and uses the normal expansion event to load it.
            if let Some(parent) = index
                .checked_sub(1)
                .and_then(|index| self.pending_reveal.get(index))
            {
                if self
                    .nodes
                    .get(parent.as_str())
                    .is_some_and(|node| node.children != Children::Loaded)
                {
                    let placeholder = SharedString::from(format!("{parent}{LOADING_SUFFIX}"));
                    self.state.update(cx, |state, cx| {
                        state.reveal_item(&placeholder, ScrollStrategy::Center, cx)
                    });
                    return;
                }
            }
        }
        self.pending_reveal.clear();
        self.state.update(cx, |state, cx| {
            if state.index_of(&target).is_none() && missing.is_none() {
                state.reveal_item(&target, ScrollStrategy::Center, cx);
            }
            state.set_selected_index(state.index_of(&target), cx);
            if let Some(index) = state.selected_index() {
                state.scroll_to_item(index, ScrollStrategy::Center);
            }
        });
    }

    fn expand(&mut self, id: String, cx: &mut Context<Self>) {
        if let Some(info) = self
            .nodes
            .get_mut(&id)
            .filter(|info| info.children == Children::Unloaded)
        {
            info.children = Children::Loading;
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
                            this.roots_ready = true;
                            this.pending_reveal.clear();
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
                    children: Children::Unloaded,
                },
            );
        }
        let children: Vec<TreeItem> = directories.iter().map(directory_item).collect();
        match &parent {
            None => {
                self.roots = children;
                self.roots_ready = true;
            }
            Some(parent) => {
                if let Some(info) = self.nodes.get_mut(parent) {
                    info.children = Children::Loaded;
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
        self.reveal_selection(cx);
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
        let rows = tree(&self.state, move |index, entry, selected, _, cx| {
            let item = entry.item();
            let id = item.id.clone();
            let loading = id.ends_with(LOADING_SUFFIX);
            let theme = cx.theme();
            let icon = if loading {
                Icon::new(IconName::Loader)
                    .size(px(16.))
                    .text_color(theme.muted_foreground)
            } else {
                directory_icon(&id, entry.is_expanded(), cx)
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
                .pl(px(6.))
                .pr(px(6.))
                .h(px(28.))
                .child(
                    h_flex()
                        .min_w_0()
                        .h_full()
                        .children((0..entry.depth().min(6)).map(|_| {
                            div()
                                .w(px(16.))
                                .h_full()
                                .flex_shrink_0()
                                .pl(px(7.))
                                .child(div().w(px(1.)).h_full().bg(theme.border))
                        }))
                        .child(div().w(px(16.)).flex_shrink_0().children(chevron))
                        .child(div().ml_1().flex_shrink_0().child(icon))
                        .child(div().ml_2().min_w_0().truncate().child(item.label.clone())),
                )
                .on_click(move |_, window, cx| {
                    if loading {
                        return;
                    }
                    view.update(cx, |this, cx| {
                        this.state.update(cx, |state, cx| state.focus(window, cx));
                        this.open_directory(&id, cx);
                    })
                    .ok();
                })
        });
        div()
            .size_full()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if event.is_held
                    || event.keystroke.key != "space"
                    || event.keystroke.modifiers.modified()
                {
                    return;
                }
                let id = this
                    .state
                    .read(cx)
                    .selected_item()
                    .map(|item| item.id.clone());
                if let Some(id) = id {
                    cx.stop_propagation();
                    this.open_directory(&id, cx);
                }
            }))
            .child(rows.size_full())
            .into_any_element()
    }
}
