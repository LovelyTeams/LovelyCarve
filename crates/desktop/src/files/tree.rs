//! The directory tree beside the file list. Partitions are the roots; a
//! directory lists its subdirectories the first time it is expanded. A
//! directory whose only entry is a subdirectory shares that subdirectory's
//! row (`Windows/System32`), and the ancestors of the first visible row stay
//! pinned above the list while it scrolls.
mod row;

use std::{collections::HashMap, ops::Range, rc::Rc};

use gpui_kit::{
    component::{
        tree::{tree, TreeEntry, TreeEvent, TreeItem, TreeState},
        ActiveTheme as _,
    },
    div,
    prelude::FluentBuilder as _,
    px, App, AppContext as _, Context, Entity, EventEmitter, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, ScrollStrategy, SharedString, Styled as _,
    Subscription, Window,
};
use lovelycarve_backend::{
    commands::{app_error::AppError, filesystem::filesystem as fs_commands},
    core::{
        catalog::{directory_query::DirectoryQuery, flat_query::FlatFileFilter},
        filesystem::tree::fs_tree::{FsNode, NodeKind},
    },
    host::State,
};

use super::Crumb;
use crate::backend::Backend;
use row::{Place, Row, RowContext, ROW_HEIGHT};

/// Directories listed per expansion; deeper listings stay in the table.
const TREE_PAGE: usize = 2_000;
const LOADING_SUFFIX: &str = "\u{0}loading";
/// Bounds the walks along parent and fold links.
const MAX_DEPTH: usize = 4096;

struct NodeInfo {
    name: String,
    parent: Option<String>,
    children: Children,
    /// The subdirectory that is this directory's only entry; its row
    /// stands for both.
    fold: Option<String>,
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
    /// Directories sharing one row, by the row's id, outermost first.
    chains: Rc<HashMap<String, Vec<Crumb>>>,
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
            chains: Rc::default(),
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
            if guard > MAX_DEPTH {
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

    /// The id of the row that shows `id`: a directory folded into its only
    /// subdirectory shares that subdirectory's row.
    fn row_of(&self, id: &str) -> SharedString {
        let mut row = id;
        for _ in 0..MAX_DEPTH {
            match self.nodes.get(row).and_then(|info| info.fold.as_deref()) {
                Some(next) => row = next,
                None => break,
            }
        }
        row.to_owned().into()
    }

    fn open_directory(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(path) = self.path_to(id) {
            self.selected = Some(id.to_owned());
            cx.emit(OpenDirectory(path));
            cx.notify();
        }
    }

    /// A click on a row: the row's own directory, the last of a folded chain.
    fn open_row(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| state.focus(window, cx));
        self.open_directory(id, cx);
    }

    /// A click on one name of a folded row.
    fn open_folded(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let row = self.row_of(id);
        self.state.update(cx, |state, cx| {
            state.focus(window, cx);
            let index = state.index_of(&row);
            state.set_selected_index(index, cx);
        });
        self.open_directory(id, cx);
    }

    /// A click on a pinned row: scroll back to the row itself.
    fn reveal_pinned(
        &mut self,
        index: usize,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.focus(window, cx);
            state.set_selected_index(Some(index), cx);
            state.scroll_to_item(index, ScrollStrategy::Top);
        });
        self.open_directory(id, cx);
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
                    let placeholder =
                        SharedString::from(format!("{}{LOADING_SUFFIX}", self.row_of(parent)));
                    self.state.update(cx, |state, cx| {
                        state.reveal_item(&placeholder, ScrollStrategy::Center, cx)
                    });
                    return;
                }
            }
        }
        self.pending_reveal.clear();
        let row = self.row_of(&target);
        self.state.update(cx, |state, cx| {
            if state.index_of(&row).is_none() && missing.is_none() {
                state.reveal_item(&row, ScrollStrategy::Center, cx);
            }
            state.set_selected_index(state.index_of(&row), cx);
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
        // Partitions keep their own rows; any other directory may share the
        // row of its only entry.
        let foldable = parent
            .as_ref()
            .and_then(|id| self.nodes.get(id))
            .is_some_and(|info| info.parent.is_some());
        let query = DirectoryQuery {
            filter: FlatFileFilter {
                kind_query: Some("目录".to_owned()),
                deleted: Some(false),
                ..FlatFileFilter::default()
            },
            ..DirectoryQuery::default()
        };
        let listing = Backend::global(cx).run(move |_, state| async move {
            let page = fs_commands::fs_children_page(
                State::new(&state),
                image_id.clone(),
                parent_id.clone(),
                None,
                Some(TREE_PAGE),
                Some(query),
            )
            .await?;
            let directories: Vec<FsNode> = page
                .items
                .into_iter()
                .filter(|node| node.kind == NodeKind::Dir && !node.deleted)
                .collect();
            let only_entry = match (foldable, directories.as_slice()) {
                (true, [directory]) => {
                    let entries = fs_commands::fs_children_page(
                        State::new(&state),
                        image_id,
                        parent_id,
                        None,
                        Some(2),
                        None,
                    )
                    .await?;
                    entries.next_cursor.is_none()
                        && matches!(entries.items.as_slice(), [entry] if entry.id == directory.id)
                }
                _ => false,
            };
            Ok::<_, AppError>((directories, only_entry))
        });
        cx.spawn(async move |this, cx| {
            let listing = listing.await;
            this.update(cx, |this, cx| {
                match listing {
                    Ok((directories, only_entry)) => {
                        this.attach(parent, directories, only_entry, cx);
                    }
                    Err(error) => {
                        tracing::warn!("directory tree: {error}");
                        if parent.is_none() {
                            this.error = Some(error.message);
                            this.roots_ready = true;
                            this.pending_reveal.clear();
                        } else if let Some(parent) = parent {
                            this.attach(Some(parent), Vec::new(), false, cx);
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
    /// When `only_entry`, the single subdirectory takes over the parent's row.
    fn attach(
        &mut self,
        parent: Option<String>,
        directories: Vec<FsNode>,
        only_entry: bool,
        cx: &mut Context<Self>,
    ) {
        for node in &directories {
            self.nodes.insert(
                node.id.clone(),
                NodeInfo {
                    name: node.name.clone(),
                    parent: parent.clone(),
                    children: Children::Unloaded,
                    fold: None,
                },
            );
        }
        match &parent {
            None => {
                self.roots = directories.iter().map(directory_item).collect();
                self.roots_ready = true;
            }
            Some(parent) => {
                if let Some(info) = self.nodes.get_mut(parent) {
                    info.children = Children::Loaded;
                }
                match directories.first().filter(|_| only_entry) {
                    Some(child) => self.fold(parent, child, cx),
                    None => {
                        let children = directories.iter().map(directory_item).collect();
                        if let Some(item) = find_mut(&mut self.roots, &parent.clone().into()) {
                            item.children = children;
                        }
                    }
                }
            }
        }
        let roots = self.roots.clone();
        let selected = self.selected.as_deref().map(|id| self.row_of(id));
        self.state.update(cx, |state, cx| {
            state.set_items(roots, cx);
            if let Some(index) = selected.and_then(|row| state.index_of(&row)) {
                state.set_selected_index(Some(index), cx);
            }
        });
        self.reveal_selection(cx);
    }

    /// Let `child`, the only entry of `parent`, take over the parent's row,
    /// still expanded, and list its own subdirectories.
    fn fold(&mut self, parent: &str, child: &FsNode, cx: &mut Context<Self>) {
        let parent_name = match self.nodes.get_mut(parent) {
            Some(info) => {
                info.fold = Some(child.id.clone());
                info.name.clone()
            }
            None => return,
        };
        let chains = Rc::make_mut(&mut self.chains);
        let mut chain = chains.remove(parent).unwrap_or_else(|| {
            vec![Crumb {
                id: parent.to_owned(),
                name: parent_name,
            }]
        });
        chain.push(Crumb {
            id: child.id.clone(),
            name: child.name.clone(),
        });
        let label = chain
            .iter()
            .map(|crumb| crumb.name.as_str())
            .collect::<Vec<_>>()
            .join("/");
        chains.insert(child.id.clone(), chain);
        let row = TreeItem::new(child.id.clone(), label)
            .expanded(true)
            .child(placeholder(&child.id));
        if let Some(item) = find_mut(&mut self.roots, &parent.to_owned().into()) {
            *item = row;
        }
        if let Some(info) = self.nodes.get_mut(&child.id) {
            info.children = Children::Loading;
        }
        self.load_children(Some(child.id.clone()), cx);
    }

    /// The guide around the selected row: its level and the rows it spans.
    fn active_guide(&self, cx: &App) -> Option<(usize, Range<usize>)> {
        let state = self.state.read(cx);
        let selected = state.selected_index()?;
        let level = state.entry(selected)?.depth().checked_sub(1)?;
        let parent = (0..selected).rev().find(|&index| {
            state
                .entry(index)
                .is_some_and(|entry| entry.depth() == level)
        })?;
        let end = (parent + 1..).find(|&index| {
            state
                .entry(index)
                .is_none_or(|entry| entry.depth() <= level)
        })?;
        Some((level, parent + 1..end))
    }

    /// The ancestors of the first visible row to pin above the list.
    fn pinned_rows(&self, cx: &App) -> Vec<Row> {
        let state = self.state.read(cx);
        let scrolled = -state.scroll_handle().0.borrow().base_handle.offset().y;
        let first = (scrolled / px(ROW_HEIGHT)).max(0.) as usize;
        let selected = state.selected_index();
        row::pinned_indices(|index| state.entry(index).map(TreeEntry::depth), first)
            .into_iter()
            .filter_map(|index| {
                state
                    .entry(index)
                    .map(|entry| Row::new(index, entry, selected == Some(index)))
            })
            .collect()
    }
}

/// The item `id` anywhere below `items`.
fn find_mut<'a>(items: &'a mut [TreeItem], id: &SharedString) -> Option<&'a mut TreeItem> {
    for item in items.iter_mut() {
        if &item.id == id {
            return Some(item);
        }
        if let Some(found) = find_mut(&mut item.children, id) {
            return Some(found);
        }
    }
    None
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
        let context = Rc::new(RowContext {
            chains: self.chains.clone(),
            open: self.selected.clone(),
            active_guide: self.active_guide(cx),
        });
        let view = cx.entity().downgrade();
        let rows = tree(&self.state, {
            let context = context.clone();
            let view = view.clone();
            move |index, entry, selected, _, cx| {
                row::render(
                    Row::new(index, entry, selected),
                    Place::List,
                    &context,
                    view.clone(),
                    cx,
                )
            }
        });
        let pinned = self.pinned_rows(cx);
        div()
            .relative()
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
            .when(!pinned.is_empty(), |this| {
                this.child(row::pinned(pinned, &context, view, cx))
            })
            .into_any_element()
    }
}
