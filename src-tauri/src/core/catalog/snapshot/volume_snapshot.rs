//! Volume snapshots — persisted, point-in-time enumerations of a single
//! volume's file/directory tree.
//!
//! A snapshot caches a volume's file/directory tree in a `.dirz` file:
//! walking a live filesystem to list a directory is cheap
//! per-click but expensive in aggregate (millions of records on a large
//! NTFS volume), and the result vanishes when the process exits. A snapshot
//! freezes the whole subtree to disk inside the case directory so that, in a
//! later session, browsing the same volume is served from the snapshot
//! without re-walking — while file *contents* are still read from the live
//! image (node ids round-trip through [`crate::core::filesystem::tree::fs_tree`]).
//!
//! ## On-disk format
//!
//! Each snapshot is a single zstd-compressed stream
//! (`snapshots/<sanitized_volume_key>.dirz` under the case directory):
//!
//! * line 0 — a [`SnapshotHeader`] as one JSON line (everything known up
//!   front; JSON so the version gate / [`load_snapshot_header`] stay trivial);
//! * body — one [`SnapshotEntry`] per file/dir in walk order, each a
//!   length-prefixed `bincode` frame `[u32 LE len][bytes]` (schema 2).
//!
//! Streaming a frame per entry keeps build-time memory bounded regardless of
//! volume size, and the length prefix lets the loader tell a clean end-of-
//! stream from a truncated tail. The entry *count* and `truncated` flag are
//! NOT in the header (they aren't known when the header line is written
//! first); they are returned by [`build_volume_snapshot`] and persisted in the
//! case manifest instead.
//!
//! The body is positional `bincode`, so any layout change to
//! [`SnapshotEntry`] / [`FsNode`](crate::core::filesystem::tree::fs_tree::FsNode) (add / remove /
//! reorder / retype a field) breaks decoding of existing frames and MUST bump
//! [`SNAPSHOT_SCHEMA`]. That is an accepted trade-off: snapshots are fully
//! rebuildable from the live image, and a stale body that fails to load just
//! falls back to a live walk (then rebuilds on the next staleness check).
//! Schema-1 JSON-lines bodies are no longer parsed.
//!
//! The `by_parent` index is keyed by the *query parent id* used to obtain
//! each node (i.e. the exact `parent_id` the frontend passes to
//! `fs_children`), not by `FsNode.parent_id` — so a snapshot listing is
//! byte-for-byte what a live `children_for_image` call would have returned.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{
    open_ntfs_volume_for_key, FsNode, FsSession, FsWalkEntry, Mactb, NodeKind,
};
use crate::core::filesystem::volumes::ntfs::{NtfsAdsStream, NtfsTreeEdge, NtfsVolume};

/// On-disk schema version for `.dirz` files. Bump on any incompatible
/// layout change; [`load_snapshot`] refuses anything newer than it knows and
/// no longer parses older bodies (the body is positional `bincode` — see the
/// module-level on-disk-format note). v2 = JSON header line + bincode frames;
/// v3 added the `FsNode.has_snapshot` field (ReFS file-snapshot marker).
pub const SNAPSHOT_SCHEMA: u32 = 3;

/// Safety cap so an auto-build on a multi-terabyte volume can't run forever
/// or fill the disk. When hit, the snapshot is marked `truncated` and the
/// uncaptured subtrees fall back to live listings transparently.
pub const DEFAULT_MAX_SNAPSHOT_ENTRIES: usize = 5_000_000;

/// How often the builder reports progress to its callback (every Nth entry).
/// The command layer throttles by wall-clock on top of this.
pub(crate) const PROGRESS_EVERY: u64 = 4_096;

/// Upper bound on nodes [`VolumeSnapshot::subtree_has_file`] visits before it
/// gives up with `None` (unknown). A genuinely empty subtree is tiny and a
/// populated one short-circuits at its first file, so this only fires on a
/// pathological all-empty megatree — there we'd rather report "unknown" and
/// let the caller fall back than stall holding a lock.
const SUBTREE_SCAN_CAP: usize = 200_000;

/// A `parent_id` that [`children_for_image`] accepts — `"part:2"`, `"root"`,
/// etc. This is the unit a snapshot covers and the key it is filed under.
pub type VolumeKey = String;

/// First line of a `.dirz` file. Holds only values known before the walk
/// starts; the entry count / truncation flag live in the case manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotHeader {
    pub schema_version: u32,
    pub volume_key: VolumeKey,
    pub image_id: String,
    /// Size of the source volume at build time, used for staleness checks.
    pub source_volume_size: u64,
    /// RFC3339 build timestamp.
    pub built_at: String,
}

/// One node in a snapshot, paired with the query parent it was listed under
/// and its full path within the volume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotEntry {
    /// The `parent_id` passed to `children_for_image` to obtain `node`.
    pub parent: String,
    pub node: FsNode,
    pub path: String,
}

/// In-memory, indexed snapshot ready to answer directory listings.
/// Mirrors the proven `NtfsDeletedSnapshot` shape (entries + parent index).
#[derive(Debug)]
pub struct VolumeSnapshot {
    pub header: SnapshotHeader,
    pub entries: Vec<SnapshotEntry>,
    by_parent: HashMap<String, Vec<usize>>,
    /// Node ids of every non-deleted directory the build descended into, plus
    /// the volume root key. A directory present here but absent from
    /// `by_parent` was walked and found genuinely empty (vs. never captured),
    /// which lets [`child_count`](Self::child_count) report `Some(0)` for it
    /// instead of forcing a live probe. (`children` deliberately does NOT use
    /// this — empty-dir *listings* still fall back to live so a truncated
    /// snapshot's uncaptured subtrees stay transparent.)
    walked_dirs: HashSet<String>,
}

impl VolumeSnapshot {
    pub fn new(header: SnapshotHeader, mut entries: Vec<SnapshotEntry>) -> Self {
        for entry in &mut entries {
            normalize_legacy_lv_name(&mut entry.node);
        }
        let mut by_parent: HashMap<String, Vec<usize>> = HashMap::new();
        let mut walked_dirs: HashSet<String> = HashSet::new();
        walked_dirs.insert(header.volume_key.clone());
        for (idx, entry) in entries.iter().enumerate() {
            by_parent.entry(entry.parent.clone()).or_default().push(idx);
            // The builder descends into exactly the non-deleted directories,
            // so every such node was walked — record it so a later by_parent
            // miss can be read as "empty" rather than "uncaptured".
            if entry.node.kind == NodeKind::Dir && !entry.node.deleted {
                walked_dirs.insert(entry.node.id.clone());
            }
        }
        Self {
            header,
            entries,
            by_parent,
            walked_dirs,
        }
    }

    /// True when this snapshot recorded a listing for `parent_id`. A genuinely
    /// empty directory that was expanded produces no entries and is therefore
    /// absent here — callers treat "absent" as "fall back to a live listing".
    pub fn covers(&self, parent_id: &str) -> bool {
        self.by_parent.contains_key(parent_id)
            || (parent_id.is_empty() && self.by_parent.contains_key("root"))
    }

    /// The nodes the live dispatcher would have returned for `parent_id`.
    /// Returns `None` when the snapshot has no record for this parent so the
    /// caller can fall back to a live walk instead of surfacing an empty dir.
    pub fn children(&self, parent_id: &str) -> Option<Vec<FsNode>> {
        let key = self.parent_key(parent_id)?;
        Some(
            self.by_parent
                .get(key)
                .into_iter()
                .flatten()
                .map(|idx| self.entries[*idx].node.clone())
                .collect(),
        )
    }

    /// The number of children recorded for `parent_id`, without cloning any
    /// node. Returns `Some(n)` for a populated directory, `Some(0)` for one the
    /// build descended into but found empty (so the frontend skips a live probe
    /// whose only result would be an empty listing), and `None` when this
    /// snapshot has no record of the directory at all — the caller then falls
    /// back to a live count. Same key resolution as [`children`](Self::children).
    ///
    /// Caveat: on a *truncated* snapshot a directory at the truncation boundary
    /// is marked walked but its children may not have been captured, so this can
    /// report `Some(0)` for a non-empty directory. That only suppresses a count
    /// badge (badges hide at 0 regardless), never affects browsing, and only at
    /// the 5M-entry cap — an acceptable cosmetic edge.
    pub fn child_count(&self, parent_id: &str) -> Option<usize> {
        if let Some(key) = self.parent_key(parent_id) {
            return Some(self.by_parent.get(key).map_or(0, |idxs| idxs.len()));
        }
        // Not a populated parent. If the build descended into this directory it
        // was genuinely empty → 0; otherwise we have no record → None.
        let key = if parent_id.is_empty() {
            "root"
        } else {
            parent_id
        };
        if self.walked_dirs.contains(key) {
            Some(0)
        } else {
            None
        }
    }

    /// Whether the subtree rooted at `dir_id` holds any **non-deleted,
    /// non-directory** entry (a real file / symlink / unallocated leaf) — i.e.
    /// whether the directory contains actual *content* anywhere beneath it, as
    /// opposed to being nothing but (recursively) empty directories.
    ///
    /// Returns `Some(true)` the moment such a leaf is found, `Some(false)` once
    /// the whole subtree is walked and only empty directories remain, and
    /// `None` when coverage is incomplete — the start directory or some
    /// descendant directory was never walked (a truncated build, or a parent
    /// the snapshot never recorded) — so the caller falls back to a cheaper
    /// signal rather than wrongly hiding real content. Deleted entries are
    /// ignored to match the browsable tree (which never lists them), and the
    /// scan is capped at [`SUBTREE_SCAN_CAP`] visited nodes (`None` past it).
    ///
    /// Like [`child_count`](Self::child_count), this trusts `walked_dirs`, so on
    /// a *truncated* snapshot a directory at the truncation boundary can be read
    /// as empty when its children simply weren't captured — an acceptable
    /// cosmetic edge that only bites at the 5M-entry cap.
    pub fn subtree_has_file(&self, dir_id: &str) -> Option<bool> {
        // Anchor the start key inside `self` so every stack/visited entry shares
        // one borrow of self: the walked-dirs entry equal to `dir_id`, or the
        // root key when `dir_id` is empty.
        let start: &str = match self.walked_dirs.get(dir_id) {
            Some(key) => key.as_str(),
            None if dir_id.is_empty() => self.walked_dirs.get("root")?.as_str(),
            None => return None,
        };
        let mut stack: Vec<&str> = vec![start];
        let mut visited: HashSet<&str> = HashSet::new();
        visited.insert(start);
        let mut budget = SUBTREE_SCAN_CAP;
        while let Some(parent) = stack.pop() {
            let Some(idxs) = self.by_parent.get(parent) else {
                // Walked but recorded no children → an empty directory.
                continue;
            };
            for &idx in idxs {
                if budget == 0 {
                    return None;
                }
                budget -= 1;
                let node = &self.entries[idx].node;
                if node.deleted {
                    continue;
                }
                if node.kind != NodeKind::Dir {
                    return Some(true);
                }
                // A non-deleted directory we must descend into to judge
                // emptiness; if the build never walked it (truncation) we
                // can't be sure, so the whole answer is unknown.
                let key = self.walked_dirs.get(node.id.as_str())?;
                if visited.insert(key.as_str()) {
                    stack.push(key.as_str());
                }
            }
        }
        Some(false)
    }

    /// Every descendant of `parent_id` in depth-first order, paired with its
    /// path *relative to* `parent_id` (leading `/`), capped at `max_entries`
    /// (`true` in the pair = cap hit). Mirrors the builder's traversal rules:
    /// deleted directories are listed but not descended into (the builder
    /// never captures their children — `fs_flat_list` splices those subtrees
    /// in live), and each directory id is expanded at most once (cycle guard).
    ///
    /// Only meaningful on a NON-truncated snapshot: a truncated build records
    /// directories it never expanded, and `walked_dirs` cannot tell those
    /// apart from genuinely empty ones (see the [`child_count`](Self::child_count)
    /// caveat), so this would silently omit their subtrees. Callers must
    /// check the manifest `truncated` flag and walk live instead. Returns
    /// `None` when `parent_id` isn't a captured parent.
    pub fn flat_descendants(
        &self,
        parent_id: &str,
        max_entries: usize,
    ) -> Option<(Vec<FsWalkEntry>, bool)> {
        let start = self.parent_key(parent_id)?;
        let mut out: Vec<FsWalkEntry> = Vec::new();
        let mut truncated = false;
        let mut visited: HashSet<&str> = HashSet::new();
        visited.insert(start);
        // Preorder DFS over the by_parent index: children pushed in reverse so
        // pop order preserves each listing's original order.
        let mut stack: Vec<(usize, String)> = Vec::new();
        for &idx in self.by_parent.get(start)?.iter().rev() {
            stack.push((idx, String::new()));
        }
        while let Some((idx, prefix)) = stack.pop() {
            if out.len() >= max_entries {
                truncated = true;
                break;
            }
            let entry = &self.entries[idx];
            let path = format!("{prefix}/{}", entry.node.name);
            out.push(FsWalkEntry {
                node: entry.node.clone(),
                path: path.clone(),
            });
            if entry.node.kind != NodeKind::Dir || entry.node.deleted {
                continue;
            }
            if !visited.insert(entry.node.id.as_str()) {
                continue;
            }
            match self.by_parent.get(&entry.node.id) {
                Some(children) => {
                    for &child in children.iter().rev() {
                        stack.push((child, path.clone()));
                    }
                }
                None => {
                    if !self.walked_dirs.contains(&entry.node.id) {
                        return None;
                    }
                }
            }
        }
        Some((out, truncated))
    }

    /// Resolve the `by_parent` key for a query parent id: the id itself when
    /// present, or `"root"` when the id is empty and a root listing exists.
    fn parent_key<'a>(&self, parent_id: &'a str) -> Option<&'a str> {
        if self.by_parent.contains_key(parent_id) {
            Some(parent_id)
        } else if parent_id.is_empty() && self.by_parent.contains_key("root") {
            Some("root")
        } else {
            None
        }
    }
}

/// Returned by [`build_volume_snapshot`]; the case layer folds this into a
/// manifest `SnapshotRef`.
#[derive(Debug, Clone)]
pub struct VolumeSnapshotBuilt {
    pub header: SnapshotHeader,
    pub file_name: String,
    pub entry_count: u64,
    pub truncated: bool,
}

/// Filesystem-safe `.dirz` file name for one volume of one image. Keyed by
/// BOTH `image_id` and `volume_key` so two images in the same case that share a
/// volume key (e.g. both have `"part:1"`) don't clobber each other's snapshot
/// file — `<image_id>__<volume_key>.dirz`, each segment sanitized to
/// `[A-Za-z0-9_-]`.
pub fn snapshot_file_name(image_id: &str, volume_key: &str) -> String {
    fn sanitize(raw: &str, empty_fallback: &str) -> String {
        let mut out = String::with_capacity(raw.len());
        for ch in raw.chars() {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                out.push(ch);
            } else {
                out.push('_');
            }
        }
        if out.is_empty() {
            out.push_str(empty_fallback);
        }
        out
    }
    let image = sanitize(image_id, "image");
    let volume = sanitize(volume_key, "root");
    format!("{image}__{volume}.dirz")
}

/// Size of the source volume for `volume_key`, used to detect a snapshot that
/// no longer matches the underlying image. `"part:N"` resolves to the
/// partition byte length; everything else (docker / folder `"root"`) falls
/// back to the whole-image length.
pub fn volume_size_for_key(image: &EvidenceSession, volume_key: &str) -> u64 {
    if let Some(size) = image.native_volume_size(volume_key) {
        return size;
    }
    if let Some(part_id) = volume_key.strip_prefix("part:") {
        if let Ok(part_id) = part_id.parse::<u8>() {
            if let Some(partition) = image.partitions.iter().find(|p| p.id == part_id) {
                return partition.length_lba * image.sector_size as u64;
            }
        }
    }
    image.len()
}

/// Deletes a partially written snapshot body when dropped, unless [`keep`]
/// was called after the encoder finished successfully. Declare it *before*
/// the `File` / encoder so they are dropped (and the handle closed — required
/// on Windows) before the delete runs.
///
/// [`keep`]: PartialOutput::keep
pub(crate) struct PartialOutput<'p> {
    path: &'p Path,
    keep: bool,
}

impl<'p> PartialOutput<'p> {
    pub(crate) fn new(path: &'p Path) -> Self {
        Self { path, keep: false }
    }

    pub(crate) fn keep(mut self) {
        self.keep = true;
    }
}

impl Drop for PartialOutput<'_> {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        match std::fs::remove_file(self.path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!(
                "snapshot: removing partial output {} failed: {err}",
                self.path.display()
            ),
        }
    }
}

mod build;
mod codec;
pub use build::build_volume_snapshot;
#[cfg(test)]
use build::*;
use codec::*;
pub use codec::{
    load_snapshot, load_snapshot_header, visit_snapshot_entries, visit_snapshot_entries_checked,
};
pub(crate) use codec::{write_snapshot_entry, write_snapshot_header};
#[cfg(test)]
mod tests;
