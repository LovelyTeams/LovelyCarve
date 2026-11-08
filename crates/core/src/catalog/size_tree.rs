//! Space usage of one indexed volume, aggregated for the file list's treemap.
//!
//! The treemap view draws a directory as a nested mosaic sized by everything
//! beneath it. A directory listing only knows its direct children, so a
//! volume's published index is read once into a compact in-memory tree
//! ([`SizeTree`]): a flat arena of rows with compressed child lists, and per
//! directory its subtree totals, once for live entries and once for live and
//! deleted entries together. Building costs one pass over the index table; a
//! view of any directory after that is an in-memory walk, so going in and out
//! of directories is immediate.
//!
//! Sizes are logical sizes, the ones the file list shows. A hard-linked file —
//! one MFT record named in several directories — is charged once, to the name
//! met first in walk order. NTFS's `$BadClus:$Bad` stream is left out: it is
//! sparse and as long as the volume, so it would double every total.
//!
//! The tree keeps no names. A view carries index ordinals, and
//! [`read_view_nodes`] reads the rows it is about to show.

use std::collections::{HashMap, VecDeque};
use std::path::Path;

use rusqlite::{params, params_from_iter};
use serde::Serialize;

use crate::catalog::filedb::{filedb_file, filedb_query};
use crate::filesystem::tree::fs_tree::FsNode;

mod build;
#[cfg(test)]
mod tests;

pub use build::build_size_tree;

/// Most levels one view draws beneath its directory.
pub const MAX_VIEW_DEPTH: u32 = 6;
/// Node id prefix of NTFS files and directories.
pub const NTFS_NODE_PREFIX: &str = "ntfs:";
/// Node id prefix of NTFS named data streams.
pub const NTFS_STREAM_PREFIX: &str = "ntfs-ads:";

/// Children one directory shows; the rest merge into its `others` tail.
const MAX_VIEW_CHILDREN: usize = 96;
/// Entries one view returns at most. Levels fill largest first, and a
/// directory whose children no longer fit stays closed.
const MAX_VIEW_ENTRIES: usize = 6_000;
/// Rows read per `IN (…)` query.
const NODE_READ_BATCH: usize = 500;
/// `SizeNode::ordinal` of the volume root, which has no index row.
const NO_ORDINAL: i64 = -1;
/// `SizeNode::tally` of anything but a directory.
const NO_TALLY: u32 = u32::MAX;
/// A missing timestamp.
const UNKNOWN_TIME: i64 = i64::MIN;

/// What a row is, as far as space is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    Directory,
    File,
    Symlink,
    Unallocated,
}

/// One index row: 32 bytes, since a volume holds millions of them.
#[derive(Clone, Copy, Debug)]
struct SizeNode {
    /// The row's `ordinal` in the index, or [`NO_ORDINAL`] for the root.
    ordinal: i64,
    /// Charged logical size: a file's size for its first name, `0` for the
    /// names after it, for directories and for symbolic links.
    size: u64,
    /// Modification time in Unix seconds, or [`UNKNOWN_TIME`].
    modified: i64,
    /// A directory's slot in [`SizeTree::tallies`], or [`NO_TALLY`].
    tally: u32,
    kind: EntryKind,
    deleted: bool,
}

/// Totals of one subtree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tally {
    bytes: u64,
    files: u64,
    /// Directories beneath, not counting the directory itself.
    dirs: u64,
    /// Newest file modification beneath, or [`UNKNOWN_TIME`].
    newest: i64,
}

impl Default for Tally {
    fn default() -> Self {
        Self {
            bytes: 0,
            files: 0,
            dirs: 0,
            newest: UNKNOWN_TIME,
        }
    }
}

impl Tally {
    fn add(&mut self, other: Tally) {
        // Saturating: a corrupt file table can claim any size.
        self.bytes = self.bytes.saturating_add(other.bytes);
        self.files = self.files.saturating_add(other.files);
        self.dirs = self.dirs.saturating_add(other.dirs);
        self.newest = self.newest.max(other.newest);
    }
}

/// A directory's totals with and without its deleted entries.
#[derive(Clone, Copy, Debug, Default)]
struct DirectoryTally {
    live: Tally,
    all: Tally,
}

/// What a treemap sizes its tiles by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeMetric {
    Bytes,
    Files,
}

/// Whether deleted entries take part in a view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeletedEntries {
    Exclude,
    Include,
}

/// How much of a directory one view returns.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewOptions {
    /// Levels beneath the directory, from 1 (its children only).
    pub depth: u32,
    pub metric: SizeMetric,
    pub deleted: DeletedEntries,
    /// Entries worth less than this share of the directory merge into their
    /// parent's `others` tail: they would be too small to draw.
    pub min_share: f64,
    pub max_children: usize,
    pub max_entries: usize,
}

impl ViewOptions {
    pub fn new(depth: u32, metric: SizeMetric, deleted: DeletedEntries, min_share: f64) -> Self {
        Self {
            depth: depth.clamp(1, MAX_VIEW_DEPTH),
            metric,
            deleted,
            min_share: if min_share.is_finite() {
                min_share.clamp(0.0, 1.0)
            } else {
                0.0
            },
            max_children: MAX_VIEW_CHILDREN,
            max_entries: MAX_VIEW_ENTRIES,
        }
    }
}

/// One directory's treemap data.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeTreeView {
    pub root: SizeTreeEntry,
    /// The index comes from a walk cut short by its entry cap, so totals
    /// can miss entries.
    pub truncated: bool,
    /// Newest plausible file modification on the volume, in Unix seconds:
    /// what ages are measured from.
    pub newest_write: Option<i64>,
    /// When the volume snapshot behind the index was taken (RFC 3339).
    pub built_at: String,
}

/// One entry of a view, with the totals of everything beneath it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeTreeEntry {
    #[serde(skip)]
    ordinal: Option<i64>,
    /// The entry's row; `None` for a volume root, which has none.
    pub node: Option<FsNode>,
    pub bytes: u64,
    pub files: u64,
    /// Directories beneath, not counting this one.
    pub dirs: u64,
    /// Newest file modification at or beneath the entry, in Unix seconds.
    pub newest: Option<i64>,
    /// The shown children, largest first; `None` for a leaf and for a
    /// directory the view does not open.
    pub children: Option<Vec<SizeTreeEntry>>,
    /// The children too small or too many to show, merged.
    pub others: Option<SizeTreeOthers>,
}

/// Children merged into one tail.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeTreeOthers {
    pub count: u64,
    pub bytes: u64,
    pub files: u64,
}

/// A volume index aggregated for treemap views. Built by [`build_size_tree`].
pub struct SizeTree {
    nodes: Vec<SizeNode>,
    /// Children of node `n`: `children[child_start[n]..child_start[n + 1]]`.
    child_start: Vec<u32>,
    children: Vec<u32>,
    tallies: Vec<DirectoryTally>,
    /// Directory node ids, and the volume key for the root, to their node.
    directories: HashMap<Box<str>, u32>,
    newest_write: i64,
    truncated: bool,
    built_at: String,
}

/// A view entry before its children are assembled.
struct Draft {
    node: u32,
    children: Option<Vec<usize>>,
    others: Option<SizeTreeOthers>,
}

impl SizeTree {
    /// Rows the tree holds, the volume root included.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    /// The treemap data of directory `start` (a directory node id, or the
    /// volume key for the root), or `None` when the tree does not hold it.
    pub fn view(&self, start: &str, options: &ViewOptions) -> Option<SizeTreeView> {
        let start = *self.directories.get(start)?;
        let root_value = self.value(start, options);
        let threshold = root_value as f64 * options.min_share;
        let mut drafts = vec![Draft {
            node: start,
            children: None,
            others: None,
        }];
        let mut queue = VecDeque::from([(0usize, 0u32)]);
        let mut entries = 1usize;
        let mut ranked: Vec<(u32, u64)> = Vec::new();
        while let Some((draft, level)) = queue.pop_front() {
            if level >= options.depth {
                continue;
            }
            let node = drafts[draft].node;
            ranked.clear();
            for &child in self.children_of(node) {
                if !self.visible(child, options.deleted) {
                    continue;
                }
                let value = self.value(child, options);
                if value > 0 {
                    ranked.push((child, value));
                }
            }
            // Largest first; ties in walk order, so a view is reproducible.
            let order = |left: &(u32, u64), right: &(u32, u64)| {
                right.1.cmp(&left.1).then_with(|| {
                    self.nodes[left.0 as usize]
                        .ordinal
                        .cmp(&self.nodes[right.0 as usize].ordinal)
                })
            };
            // A directory can hold tens of thousands of children and at most
            // `max_children` show: pick those, and sort only them.
            if options.max_children > 0 && ranked.len() > options.max_children {
                ranked.select_nth_unstable_by(options.max_children - 1, order);
                ranked[..options.max_children].sort_unstable_by(order);
            } else {
                ranked.sort_unstable_by(order);
            }
            let kept = ranked
                .iter()
                .take(options.max_children)
                .take_while(|(_, value)| *value as f64 >= threshold)
                .count();
            // Below the top level a directory opens only when all of its shown
            // children fit; the view's own directory always opens.
            if level > 0 && entries + kept > options.max_entries {
                continue;
            }
            let mut others = SizeTreeOthers::default();
            for &(child, _) in &ranked[kept..] {
                let totals = self.totals(child, options.deleted);
                others.count += 1;
                others.bytes = others.bytes.saturating_add(totals.bytes);
                others.files = others.files.saturating_add(totals.files);
            }
            let mut shown = Vec::with_capacity(kept);
            for &(child, _) in &ranked[..kept] {
                shown.push(drafts.len());
                if self.nodes[child as usize].kind == EntryKind::Directory {
                    queue.push_back((drafts.len(), level + 1));
                }
                drafts.push(Draft {
                    node: child,
                    children: None,
                    others: None,
                });
            }
            entries += kept;
            drafts[draft].children = Some(shown);
            drafts[draft].others = (others.count > 0).then_some(others);
        }

        // Drafts are in breadth-first order, so every child comes after its
        // parent: assembling from the back finds each child already built.
        let mut built: Vec<Option<SizeTreeEntry>> = Vec::with_capacity(drafts.len());
        built.resize_with(drafts.len(), || None);
        for index in (0..drafts.len()).rev() {
            let draft = &drafts[index];
            let children = draft.children.as_ref().map(|shown| {
                shown
                    .iter()
                    .filter_map(|&child| built[child].take())
                    .collect::<Vec<_>>()
            });
            let totals = self.totals(draft.node, options.deleted);
            let ordinal = self.nodes[draft.node as usize].ordinal;
            built[index] = Some(SizeTreeEntry {
                ordinal: (ordinal != NO_ORDINAL).then_some(ordinal),
                node: None,
                bytes: totals.bytes,
                files: totals.files,
                dirs: totals.dirs,
                newest: known_time(totals.newest),
                children,
                others: draft.others,
            });
        }
        let root = built.first_mut()?.take()?;
        Some(SizeTreeView {
            root,
            truncated: self.truncated,
            newest_write: known_time(self.newest_write),
            built_at: self.built_at.clone(),
        })
    }

    fn children_of(&self, node: u32) -> &[u32] {
        child_slice(&self.child_start, &self.children, node)
    }

    fn visible(&self, node: u32, deleted: DeletedEntries) -> bool {
        deleted == DeletedEntries::Include || !self.nodes[node as usize].deleted
    }

    fn totals(&self, node: u32, deleted: DeletedEntries) -> Tally {
        let node = &self.nodes[node as usize];
        match node.kind {
            EntryKind::Directory => match self.tallies.get(node.tally as usize) {
                Some(tally) => match deleted {
                    DeletedEntries::Exclude => tally.live,
                    DeletedEntries::Include => tally.all,
                },
                None => Tally::default(),
            },
            EntryKind::File => Tally {
                bytes: node.size,
                files: 1,
                dirs: 0,
                newest: node.modified,
            },
            EntryKind::Unallocated => Tally {
                bytes: node.size,
                ..Tally::default()
            },
            EntryKind::Symlink => Tally::default(),
        }
    }

    fn value(&self, node: u32, options: &ViewOptions) -> u64 {
        let totals = self.totals(node, options.deleted);
        match options.metric {
            SizeMetric::Bytes => totals.bytes,
            SizeMetric::Files => totals.files,
        }
    }
}

/// The children of `node` in compressed child lists.
fn child_slice<'a>(child_start: &[u32], children: &'a [u32], node: u32) -> &'a [u32] {
    let node = node as usize;
    match (child_start.get(node), child_start.get(node + 1)) {
        (Some(&start), Some(&end)) => children
            .get(start as usize..end as usize)
            .unwrap_or_default(),
        _ => &[],
    }
}

fn known_time(time: i64) -> Option<i64> {
    (time != UNKNOWN_TIME).then_some(time)
}

/// Whether the index walked directory `node_id`, and if so whether it is an
/// NTFS directory: an NTFS directory id, or a volume root holding NTFS
/// entries. `None` when the index does not cover the directory — the
/// snapshot is still building, was cut short, or the directory is deleted.
pub fn ntfs_directory(index: &Path, node_id: &str) -> anyhow::Result<Option<bool>> {
    let conn = filedb_file::open_reader(index)?;
    if !filedb_query::walked_dir_exists(&conn, node_id)? {
        return Ok(None);
    }
    if node_id.starts_with(NTFS_NODE_PREFIX) {
        return Ok(Some(true));
    }
    let holds_ntfs: bool = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM filedb
             WHERE pid = ?1 AND (substr(nid, 1, 5) = ?2 OR substr(nid, 1, 9) = ?3)
         )",
        params![node_id, NTFS_NODE_PREFIX, NTFS_STREAM_PREFIX],
        |row| row.get(0),
    )?;
    Ok(Some(holds_ntfs))
}

/// Fill every entry's `node` from the index the view's tree was built from.
pub fn read_view_nodes(index: &Path, view: &mut SizeTreeView) -> anyhow::Result<()> {
    let mut ordinals = Vec::new();
    collect_ordinals(&view.root, &mut ordinals);
    let conn = filedb_file::open_reader(index)?;
    let mut nodes: HashMap<i64, FsNode> = HashMap::with_capacity(ordinals.len());
    for batch in ordinals.chunks(NODE_READ_BATCH) {
        let placeholders = vec!["?"; batch.len()].join(",");
        let mut statement = conn.prepare(&format!(
            "SELECT ordinal, node_json FROM filedb WHERE ordinal IN ({placeholders})"
        ))?;
        let rows = statement.query_map(params_from_iter(batch.iter()), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                filedb_query::deserialize_node(row, 1)?,
            ))
        })?;
        for row in rows {
            let (ordinal, node) = row?;
            nodes.insert(ordinal, node);
        }
    }
    attach_nodes(&mut view.root, &mut nodes)
}

fn collect_ordinals(entry: &SizeTreeEntry, ordinals: &mut Vec<i64>) {
    ordinals.extend(entry.ordinal);
    for child in entry.children.iter().flatten() {
        collect_ordinals(child, ordinals);
    }
}

fn attach_nodes(entry: &mut SizeTreeEntry, nodes: &mut HashMap<i64, FsNode>) -> anyhow::Result<()> {
    if let Some(ordinal) = entry.ordinal {
        let node = nodes
            .remove(&ordinal)
            .ok_or_else(|| anyhow::anyhow!("index row {ordinal} is missing"))?;
        entry.node = Some(node);
    }
    for child in entry.children.iter_mut().flatten() {
        attach_nodes(child, nodes)?;
    }
    Ok(())
}
