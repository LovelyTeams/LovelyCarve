//! Rows of the directory tree, shared by the list and by the rows pinned
//! above it while it scrolls.
use std::{collections::HashMap, ops::Range, rc::Rc};

use gpui_kit::{
    component::{
        h_flex, list::ListItem, tree::TreeEntry, v_flex, ActiveTheme as _, Icon, IconName,
    },
    div, px, App, HighlightStyle, InteractiveElement as _, InteractiveText, IntoElement,
    ParentElement as _, SharedString, Styled as _, StyledText, WeakEntity,
};

use super::{DirTree, LOADING_SUFFIX};
use crate::files::{
    icons::{chevron_icon, directory_icon},
    Crumb,
};

/// Height of a tree row.
pub(super) const ROW_HEIGHT: f32 = 24.;
/// Indentation per level; a level's guide runs below its parent's chevron.
const INDENT: f32 = 16.;
/// Levels indented before deeper rows stop moving right.
const MAX_INDENT_LEVELS: usize = 6;

/// What every row of one frame shares.
pub(super) struct RowContext {
    /// Directories that share one row, by the row's id, outermost first.
    pub chains: Rc<HashMap<String, Vec<Crumb>>>,
    /// The directory the file list shows.
    pub open: Option<String>,
    /// The guide around the selected row: its level and the rows it spans.
    pub active_guide: Option<(usize, Range<usize>)>,
}

/// One row, copied out of its tree entry.
pub(super) struct Row {
    pub index: usize,
    pub id: SharedString,
    pub label: SharedString,
    pub depth: usize,
    pub folder: bool,
    pub expanded: bool,
    pub selected: bool,
}

impl Row {
    pub(super) fn new(index: usize, entry: &TreeEntry, selected: bool) -> Self {
        let item = entry.item();
        Self {
            index,
            id: item.id.clone(),
            label: item.label.clone(),
            depth: entry.depth(),
            folder: entry.is_folder(),
            expanded: entry.is_expanded(),
            selected,
        }
    }
}

/// Where a row is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Place {
    List,
    Pinned,
}

pub(super) fn render(
    row: Row,
    place: Place,
    context: &RowContext,
    view: WeakEntity<DirTree>,
    cx: &App,
) -> ListItem {
    let theme = cx.theme();
    let loading = row.id.ends_with(LOADING_SUFFIX);
    let guides = (0..row.depth.min(MAX_INDENT_LEVELS)).map(|level| {
        let active = context
            .active_guide
            .as_ref()
            .is_some_and(|(guide, rows)| *guide == level && rows.contains(&row.index));
        div()
            .w(px(INDENT))
            .h_full()
            .flex_shrink_0()
            .pl(px(INDENT / 2. - 1.))
            .child(div().w(px(1.)).h_full().bg(if active {
                theme.muted_foreground
            } else {
                theme.border
            }))
    });
    let icon = if loading {
        Icon::new(IconName::Loader)
            .size(px(16.))
            .text_color(theme.muted_foreground)
    } else {
        directory_icon(&row.id, row.expanded, cx)
    };
    let label = match context.chains.get(row.id.as_ref()) {
        Some(chain) => folded_label(row.index, chain, context, &view, cx).into_any_element(),
        None => row.label.clone().into_any_element(),
    };
    let item = match place {
        Place::List => ListItem::new(row.index),
        Place::Pinned => ListItem::new(("pinned", row.index)),
    };
    let (index, id) = (row.index, row.id.clone());
    // No vertical padding: the content takes the whole row, so the guides of
    // consecutive rows join into one line.
    item.selected(row.selected)
        .py_0()
        .pl(px(6.))
        .pr(px(6.))
        .h(px(ROW_HEIGHT))
        .child(
            h_flex()
                .min_w_0()
                .h(px(ROW_HEIGHT))
                .children(guides)
                .child(
                    div()
                        .w(px(16.))
                        .flex_shrink_0()
                        .children(row.folder.then(|| chevron_icon(row.expanded, cx))),
                )
                .child(div().ml(px(2.)).flex_shrink_0().child(icon))
                .child(div().ml(px(6.)).min_w_0().truncate().child(label)),
        )
        .on_click(move |_, window, cx| {
            if loading {
                return;
            }
            view.update(cx, |tree, cx| match place {
                Place::List => tree.open_row(&id, window, cx),
                Place::Pinned => tree.reveal_pinned(index, &id, window, cx),
            })
            .ok();
        })
}

/// The names of directories sharing one row as one text, so it truncates
/// like any label; each name opens its own directory, and the one the file
/// list shows is highlighted.
fn folded_label(
    row: usize,
    chain: &[Crumb],
    context: &RowContext,
    view: &WeakEntity<DirTree>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let mut text = String::new();
    let mut names = Vec::new();
    let mut highlights = Vec::new();
    for crumb in chain {
        if !text.is_empty() {
            let start = text.len();
            text.push('/');
            highlights.push((
                start..text.len(),
                HighlightStyle {
                    color: Some(theme.muted_foreground),
                    ..HighlightStyle::default()
                },
            ));
        }
        let start = text.len();
        text.push_str(&crumb.name);
        if context.open.as_deref() == Some(crumb.id.as_str()) {
            highlights.push((
                start..text.len(),
                HighlightStyle {
                    color: Some(theme.primary),
                    ..HighlightStyle::default()
                },
            ));
        }
        names.push(start..text.len());
    }
    let ids: Vec<String> = chain.iter().map(|crumb| crumb.id.clone()).collect();
    let view = view.clone();
    InteractiveText::new(
        ("fold", row),
        StyledText::new(text).with_highlights(highlights),
    )
    .on_click(names, move |index, window, cx| {
        // The name opens its directory; the row must not also toggle.
        cx.stop_propagation();
        if let Some(id) = ids.get(index) {
            view.update(cx, |tree, cx| tree.open_folded(id, window, cx))
                .ok();
        }
    })
}

/// The rows pinned above the list, drawn over its first rows.
pub(super) fn pinned(
    rows: Vec<Row>,
    context: &RowContext,
    view: WeakEntity<DirTree>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .occlude()
        .bg(theme.background)
        .border_b_1()
        .border_color(theme.border)
        .children(
            rows.into_iter()
                .map(|row| render(row, Place::Pinned, context, view.clone(), cx)),
        )
}

/// The rows to pin above a list whose first visible row is `first`: that
/// row's ancestors, outermost first, each kept only while its subtree still
/// shows rows below the slot it is pinned in. `depth_of` gives the depth of
/// a row, `None` past the end.
pub(super) fn pinned_indices(
    depth_of: impl Fn(usize) -> Option<usize>,
    first: usize,
) -> Vec<usize> {
    let Some(mut wanted) = depth_of(first) else {
        return Vec::new();
    };
    let mut ancestors = Vec::new();
    for index in (0..first).rev() {
        if wanted == 0 {
            break;
        }
        if let Some(depth) = depth_of(index).filter(|depth| *depth < wanted) {
            ancestors.push((index, depth));
            wanted = depth;
        }
    }
    let mut pinned = Vec::new();
    for (slot, (index, depth)) in ancestors.into_iter().rev().enumerate() {
        let last = (index + 1..)
            .take_while(|&row| depth_of(row).is_some_and(|row_depth| row_depth > depth))
            .last()
            .unwrap_or(index);
        if last <= first + slot {
            break;
        }
        pinned.push(index);
    }
    pinned
}

#[cfg(test)]
mod tests {
    use super::pinned_indices;

    /// A, B (A's child), C and D (B's), E (A's), F: depths by row.
    const DEPTHS: [usize; 6] = [0, 1, 2, 2, 1, 0];

    fn pinned(first: usize) -> Vec<usize> {
        pinned_indices(|row| DEPTHS.get(row).copied(), first)
    }

    #[test]
    fn ancestors_stay_pinned_while_their_subtree_shows_below() {
        assert_eq!(
            [0, 1, 2, 3, 4, 5, 6].map(pinned),
            [vec![], vec![0], vec![0], vec![0], vec![], vec![], vec![],]
        );
    }

    #[test]
    fn a_deep_first_row_pins_every_ancestor_with_rows_left() {
        let depths = [0, 1, 2, 3, 3, 3, 3, 0];
        let pinned = |first| pinned_indices(|row| depths.get(row).copied(), first);
        assert_eq!(
            (pinned(3), pinned(4), pinned(5)),
            (vec![0, 1, 2], vec![0, 1], vec![0])
        );
    }
}
