//! Paged reads for the current directory and recursive file scope.
use std::collections::HashMap;

use gpui_kit::{component::table::TableState, Context};
use lovelycarve_backend::{
    commands::filesystem::{filesystem as fs_commands, filesystem_flat as flat_commands},
    core::{catalog::flat_query::FlatFileFilter, filesystem::tree::fs_tree::FsNode},
    host::State,
};

use super::{FileTable, COLUMNS};
use crate::backend::Backend;

const PAGE_SIZE: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ListingScope {
    Directory,
    Descendants,
}

#[derive(Clone)]
pub(super) enum PageCursor {
    Directory(String),
    Descendants(usize),
}

struct ListingPage {
    rows: Vec<FsNode>,
    paths: HashMap<String, String>,
    next: Option<PageCursor>,
    total: Option<usize>,
    truncated: bool,
}

impl FileTable {
    pub(super) fn fetch(&mut self, cursor: Option<PageCursor>, cx: &mut Context<TableState<Self>>) {
        self.loading = true;
        let generation = self.generation;
        let image_id = self.image_id.clone();
        let parent_id = self.parent_id.clone();
        let query = self.query.clone();
        let scope = self.scope;
        let filtered =
            query.sort.is_some() || query.descending || query.filter != FlatFileFilter::default();
        let page = Backend::global(cx).run(move |_, state| async move {
            match scope {
                ListingScope::Directory => {
                    let cursor = match cursor {
                        Some(PageCursor::Directory(cursor)) => Some(cursor),
                        _ => None,
                    };
                    let page = fs_commands::fs_children_page(
                        State::new(&state),
                        image_id,
                        parent_id,
                        cursor,
                        Some(PAGE_SIZE),
                        filtered.then_some(query),
                    )
                    .await?;
                    Ok(ListingPage {
                        rows: page.items,
                        paths: HashMap::new(),
                        next: page.next_cursor.map(PageCursor::Directory),
                        total: page.total,
                        truncated: page.truncated,
                    })
                }
                ListingScope::Descendants => {
                    let skip = match cursor {
                        Some(PageCursor::Descendants(skip)) => skip,
                        _ => 0,
                    };
                    let sort = COLUMNS.iter().find(|spec| Some(spec.sort) == query.sort);
                    let page = flat_commands::fs_flat_page(
                        State::new(&state),
                        image_id,
                        parent_id,
                        true,
                        None,
                        sort.map(|spec| spec.key.to_owned()),
                        Some(query.descending),
                        Some(skip),
                        Some(PAGE_SIZE),
                        Some(query.filter),
                        None,
                    )
                    .await?;
                    let total = usize::try_from(page.total).unwrap_or(usize::MAX);
                    let loaded = skip.saturating_add(page.items.len());
                    let next = (!page.items.is_empty() && loaded < total)
                        .then_some(PageCursor::Descendants(loaded));
                    let mut paths = HashMap::with_capacity(page.items.len());
                    let rows = page
                        .items
                        .into_iter()
                        .map(|entry| {
                            paths.insert(entry.node.id.clone(), entry.path);
                            entry.node
                        })
                        .collect();
                    Ok(ListingPage {
                        rows,
                        paths,
                        next,
                        total: Some(total),
                        truncated: page.truncated,
                    })
                }
            }
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
                            delegate.rows.extend(page.rows);
                            delegate.paths.extend(page.paths);
                            delegate.next_cursor = page.next;
                            delegate.total = page.total;
                            delegate.truncated = page.truncated;
                        }
                        Err(error) => delegate.error = Some(error.message),
                    }
                    let browser = delegate.browser.clone();
                    table.refresh(cx);
                    cx.notify();
                    browser.update(cx, |_, cx| cx.notify()).ok();
                })
                .ok();
        })
        .detach();
        cx.notify();
    }
}
