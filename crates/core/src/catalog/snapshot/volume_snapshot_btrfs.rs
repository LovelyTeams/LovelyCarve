//! Btrfs fast path for [`crate::catalog::snapshot::volume_snapshot::build_volume_snapshot`].
//!
//! The generic per-directory walker is pathological on Btrfs: every directory
//! listing re-runs `BtrfsVolume::open` (primary superblock read, a full chunk
//! tree iteration, a full root tree iteration to enumerate subvolumes), then
//! pays one B-tree descent for the directory's `DIR_INDEX` range plus one
//! more root-to-leaf descent *per child* to join its `INODE_ITEM` — each
//! block read going through the image backend (EWF decompression for E01;
//! for an mdadm-assembled array additionally through the RAID striping
//! layer). A whole-volume walk multiplies all of that by the number of
//! directories.
//!
//! This module mirrors the NTFS `$MFT` / APFS catalog fast paths instead: the
//! volume is opened once, and each subvolume the walk descends into gets ONE
//! streaming pass over its whole FS tree (every tree node read exactly once,
//! inode items and directory entries collected in the same pass). The
//! directory tree is then rebuilt in memory and streamed to the `.dirz`
//! file. Output is identical to what the generic walker records — same node
//! ids, parent keys, paths, ordering rules and timestamps — so snapshot
//! consumers can't tell the difference.
//!
//! Error stance: a subvolume scan reads strictly more blocks than the
//! targeted descents of the live path, so on damaged evidence it can fail
//! where live listings still succeed. Any scan failure therefore aborts the
//! whole fast path with `Err` — recording a partial listing under a covered
//! parent key would silently hide evidence with no live fallback — and
//! [`build_volume_snapshot`](crate::catalog::snapshot::volume_snapshot::build_volume_snapshot)
//! falls back to the generic walker, which degrades per directory instead.
//! Only a dangling subvolume mount (a `ROOT_ITEM` dir entry whose target the
//! root tree no longer knows) is skipped entry-by-entry, exactly like the
//! live listing.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;
use std::sync::Arc;

use anyhow::anyhow;
use tokio_util::sync::CancellationToken;

use crate::catalog::snapshot::volume_snapshot::{
    snapshot_file_name, volume_size_for_key, write_snapshot_entry, write_snapshot_header,
    SnapshotEntry, SnapshotHeader, VolumeSnapshotBuilt, PROGRESS_EVERY, SNAPSHOT_SCHEMA,
};
use crate::evidence::image::{Partition, PartitionKind};
use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::{fs_unalloc_nodes, FsNode, NodeKind};
use crate::filesystem::volumes::btrfs::{
    open_btrfs_source, parse_btrfs_id, sort_dirs_first_then_name, BtrfsInode, BtrfsSource,
    BtrfsTreeRecord, BtrfsVolume, DirEntry, DirEntryTarget, FIRST_FREE_OBJECTID,
};

/// The partition behind `volume_key`, when that key names a Btrfs filesystem
/// (`part:N` only — a Btrfs inside LVM lives under its host LVM partition's
/// `part:N` snapshot and keeps the generic walk, which now reuses one cached
/// volume handle per subtree via the `FsSession`). `None` sends the
/// caller down the generic walker exactly as before.
pub(crate) fn btrfs_partition_for_key(
    image: &EvidenceSession,
    volume_key: &str,
) -> Option<Partition> {
    let part_id = volume_key.strip_prefix("part:")?.parse::<u8>().ok()?;
    image
        .partitions
        .iter()
        .find(|partition| partition.id == part_id && partition.kind == PartitionKind::Btrfs)
        .cloned()
}

/// Everything one FS-tree pass collects for a subvolume: inode metadata by
/// objectid (first record wins; `None` pins a record that failed to parse,
/// hiding the inode exactly like the live `read_inode` failure path), and
/// directory entries grouped by parent inode in `DIR_INDEX` (key offset)
/// order — the exact order the live `dir_entries` iteration yields.
#[derive(Default)]
struct SubvolData {
    inodes: HashMap<u64, Option<BtrfsInode>>,
    dirs: HashMap<u64, Vec<DirEntry>>,
}

/// Per-build memo of scanned subvolumes. A subvolume is scanned at most once
/// no matter how many mount points reference it, and only when the walk
/// actually descends into it (mount-node metadata uses a targeted
/// `read_inode` instead, like the live listing). `budget` bounds the total
/// directory entries held in memory to the writer's `max_entries` cap — the
/// scan-side mirror of the NTFS fast path's capped `$MFT` scan.
struct SubvolScans<'v, 'a> {
    volume: &'v BtrfsVolume<'a>,
    cache: HashMap<u64, SubvolData>,
    budget: usize,
    /// Set when a scan stopped early on the budget; the builder ORs it into
    /// the snapshot's `truncated` flag so uncaptured subtrees fall back to
    /// live listings.
    truncated: bool,
}

impl SubvolScans<'_, '_> {
    /// The scanned data for `subvol`, scanning on first use. `Err` on
    /// cancellation or any scan failure — the fast path must abort rather
    /// than record listings it knows are incomplete.
    fn get(
        &mut self,
        subvol: u64,
        cancel: &CancellationToken,
        tick: &mut dyn FnMut(),
    ) -> anyhow::Result<&SubvolData> {
        if !self.cache.contains_key(&subvol) {
            let (data, collected, budget_hit) =
                scan_subvol(self.volume, subvol, self.budget, cancel, tick)?;
            self.budget = self.budget.saturating_sub(collected);
            self.truncated |= budget_hit;
            self.cache.insert(subvol, data);
        }
        Ok(&self.cache[&subvol])
    }
}

/// One streaming pass over `subvol`'s FS tree, collecting at most `budget`
/// directory entries. Returns `(data, dir_entries_collected, budget_hit)`.
/// Cancellation is polled every few thousand tree items (of any key type)
/// and propagates as `Err`, as does any IO/parse failure of the tree itself.
fn scan_subvol(
    volume: &BtrfsVolume<'_>,
    subvol: u64,
    budget: usize,
    cancel: &CancellationToken,
    tick: &mut dyn FnMut(),
) -> anyhow::Result<(SubvolData, usize, bool)> {
    let mut data = SubvolData::default();
    let mut collected = 0usize;
    let mut budget_hit = false;
    let completed = volume.scan_subvol_tree(
        subvol,
        &mut || {
            tick();
            !cancel.is_cancelled()
        },
        &mut |record| {
            match record {
                BtrfsTreeRecord::Inode { objectid, item } => {
                    data.inodes.entry(objectid).or_insert(item);
                }
                BtrfsTreeRecord::DirIndex { parent, entry } => {
                    if collected >= budget {
                        budget_hit = true;
                        return false;
                    }
                    collected += 1;
                    data.dirs.entry(parent).or_default().push(entry);
                }
            }
            true
        },
    )?;
    if !completed && !budget_hit {
        anyhow::bail!("snapshot build cancelled");
    }
    Ok((data, collected, budget_hit))
}

/// The nodes the live `BtrfsVolume::children(subvol, dir_inode)` call would
/// return: entries resolved against the scanned inode map (same
/// skip-on-missing rule as the live `read_inode` failure path), child
/// subvolume mounts resolved with the very same targeted
/// `read_inode(child, 256)` the live listing does, sorted dirs-first then
/// case-insensitive name.
fn listing(
    volume: &BtrfsVolume<'_>,
    scans: &mut SubvolScans<'_, '_>,
    subvol: u64,
    dir_inode: u64,
    cancel: &CancellationToken,
    tick: &mut dyn FnMut(),
) -> anyhow::Result<Vec<FsNode>> {
    let data = scans.get(subvol, cancel, tick)?;
    let Some(entries) = data.dirs.get(&dir_inode) else {
        return Ok(Vec::new());
    };
    let mut nodes = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry.target() {
            DirEntryTarget::Inode(inode) => {
                if let Some(Some(item)) = data.inodes.get(&inode) {
                    nodes.push(volume.node_from_inode(
                        subvol,
                        dir_inode,
                        inode,
                        &entry.name,
                        entry.ftype,
                        item,
                    ));
                }
            }
            DirEntryTarget::Subvol(child) => {
                // Same targeted read as the live path: a dangling mount is
                // skipped; a readable root inode lists the mount even when
                // the child tree is damaged elsewhere (the later descent
                // then fails the whole fast path and the generic walker
                // takes over).
                match volume.read_inode(child, FIRST_FREE_OBJECTID) {
                    Ok(item) => nodes.push(volume.node_for_subvol(
                        subvol,
                        dir_inode,
                        child,
                        &entry.name,
                        &item,
                    )),
                    Err(err) => tracing::debug!(
                        parent_subvol = subvol,
                        parent = dir_inode,
                        child_subvol = child,
                        name = %entry.name,
                        error = %err,
                        "snapshot: btrfs subvolume target unreadable; skipping"
                    ),
                }
            }
            DirEntryTarget::Unsupported(_) => {}
        }
    }
    sort_dirs_first_then_name(&mut nodes);
    Ok(nodes)
}

/// Record one directory's listing into the `.dirz` stream and queue its
/// subdirectories. Returns `false` when the `max_entries` cap was hit (the
/// caller flips `truncated` and stops).
#[allow(clippy::too_many_arguments)]
fn write_listing(
    encoder: &mut zstd::stream::write::Encoder<'static, BufWriter<File>>,
    nodes: Vec<FsNode>,
    parent_key: &str,
    parent_path: &str,
    max_entries: usize,
    entry_count: &mut u64,
    visited: &mut HashSet<String>,
    stack: &mut Vec<(String, String)>,
    scratch: &mut Vec<u8>,
    on_progress: &mut dyn FnMut(u64),
) -> anyhow::Result<bool> {
    for node in nodes {
        if *entry_count as usize >= max_entries {
            return Ok(false);
        }
        let path = if parent_path.is_empty() {
            format!("/{}", node.name)
        } else {
            format!("{parent_path}/{}", node.name)
        };
        let descend =
            node.kind == NodeKind::Dir && !node.deleted && visited.insert(node.id.clone());
        let next = (node.id.clone(), path.clone());
        let line = SnapshotEntry {
            parent: parent_key.to_string(),
            node,
            path,
        };
        write_snapshot_entry(encoder, &line, scratch)?;
        *entry_count += 1;
        if (*entry_count).is_multiple_of(PROGRESS_EVERY) {
            on_progress(*entry_count);
        }
        if descend {
            stack.push(next);
        }
    }
    Ok(true)
}

/// Build a `.dirz` snapshot of a Btrfs partition with one FS-tree scan per
/// descended subvolume. Same cancellation contract as the generic builder
/// (partial file finalised + removed, "snapshot build cancelled" error);
/// `max_entries` flips `truncated` (on either the write side or the scan
/// budget). Any scan failure aborts with `Err` so the caller falls back to
/// the generic walker — this path never records a listing it knows is
/// incomplete (see the module doc).
pub(crate) fn build_btrfs_volume_snapshot(
    image: &Arc<EvidenceSession>,
    volume_key: &str,
    out_path: &Path,
    max_entries: usize,
    built_at: String,
    cancel: &CancellationToken,
    on_progress: &mut dyn FnMut(u64),
) -> anyhow::Result<VolumeSnapshotBuilt> {
    let partition = btrfs_partition_for_key(image, volume_key)
        .ok_or_else(|| anyhow!("not a btrfs volume key: {volume_key}"))?;
    let volume = open_btrfs_source(image, BtrfsSource::Partition(partition.id))?;

    let header = SnapshotHeader {
        schema_version: SNAPSHOT_SCHEMA,
        volume_key: volume_key.to_string(),
        image_id: image.id.clone(),
        source_volume_size: volume_size_for_key(image, volume_key),
        built_at,
    };

    let output = crate::catalog::snapshot::volume_snapshot::PartialOutput::new(out_path);
    let file = File::create(out_path)?;
    let mut encoder = zstd::stream::write::Encoder::new(BufWriter::new(file), 3)?;
    write_snapshot_header(&mut encoder, &header)?;

    let mut scans = SubvolScans {
        volume: &volume,
        cache: HashMap::new(),
        budget: max_entries,
        truncated: false,
    };
    let mut entry_count: u64 = 0;
    let mut truncated = false;
    // DFS worklist of directories to expand, paired with their full paths.
    // `visited` guards node-id cycles (a subvolume mounted twice descends
    // once), mirroring the generic builder.
    let mut stack: Vec<(String, String)> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(volume_key.to_string());
    // Reused per-entry bincode encode buffer, threaded through both
    // `write_listing` calls (root + each descended directory).
    let mut scratch: Vec<u8> = Vec::new();

    // The partition root listing the generic walker records: the default
    // subvolume's root children plus independent subvolume roots (exactly
    // `partition_children`), then the whole-volume "$未分配簇" leaf the
    // generic dispatcher appends (Btrfs has no orphan container).
    let build = (|| -> anyhow::Result<()> {
        let root_nodes = {
            let mut tick = || on_progress(0);
            let mut nodes = listing(
                &volume,
                &mut scans,
                volume.default_subvol(),
                volume.root_inode(),
                cancel,
                &mut tick,
            )?;
            nodes.extend(volume.independent_subvol_root_nodes());
            sort_dirs_first_then_name(&mut nodes);
            nodes.extend(fs_unalloc_nodes(image, &partition));
            nodes
        };
        if !write_listing(
            &mut encoder,
            root_nodes,
            volume_key,
            "",
            max_entries,
            &mut entry_count,
            &mut visited,
            &mut stack,
            &mut scratch,
            on_progress,
        )? {
            truncated = true;
            stack.clear();
        }

        while let Some((dir_id, dir_path)) = stack.pop() {
            if cancel.is_cancelled() {
                anyhow::bail!("snapshot build cancelled");
            }
            // Every queued id was minted by `node_from_inode`/`node_for_subvol`
            // and round-trips; anything else (defensive) is skipped.
            let Some((_, subvol, dir_inode)) = parse_btrfs_id(&dir_id) else {
                continue;
            };
            let children = {
                let count = entry_count;
                let mut tick = || on_progress(count);
                listing(&volume, &mut scans, subvol, dir_inode, cancel, &mut tick)?
            };
            if !write_listing(
                &mut encoder,
                children,
                &dir_id,
                &dir_path,
                max_entries,
                &mut entry_count,
                &mut visited,
                &mut stack,
                &mut scratch,
                on_progress,
            )? {
                truncated = true;
                stack.clear();
            }
        }
        Ok(())
    })();
    build?;
    truncated |= scans.truncated;

    encoder.finish()?;
    output.keep();
    on_progress(entry_count);

    let file_name = out_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| snapshot_file_name(&header.image_id, volume_key));

    Ok(VolumeSnapshotBuilt {
        header,
        file_name,
        entry_count,
        truncated,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::snapshot::volume_snapshot::load_snapshot;
    use crate::filesystem::tree::fs_tree::children_for_image;

    /// Env-gated equivalence guard: `LOVELYCARVE_BTRFS_SNAPSHOT_TEST` points
    /// at an image with a Btrfs partition. Builds a fast-path snapshot and
    /// checks that, for the partition root and a bounded breadth-first sample
    /// of directories, the snapshot listing is JSON-identical to the live
    /// `children_for_image` listing — the exact property `fs_children` relies
    /// on when serving from a snapshot.
    #[test]
    fn fast_path_matches_live_listings_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_BTRFS_SNAPSHOT_TEST") else {
            return;
        };
        let image = Arc::new(EvidenceSession::open(path).expect("btrfs test image should open"));
        let partition = image
            .partitions
            .iter()
            .find(|partition| partition.kind == PartitionKind::Btrfs)
            .cloned()
            .expect("expected a btrfs partition");
        let volume_key = format!("part:{}", partition.id);

        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("btrfs.dirz");
        let cancel = CancellationToken::new();
        let built = build_btrfs_volume_snapshot(
            &image,
            &volume_key,
            &out,
            crate::catalog::snapshot::volume_snapshot::DEFAULT_MAX_SNAPSHOT_ENTRIES,
            "2026-06-12T00:00:00Z".to_string(),
            &cancel,
            &mut |_| {},
        )
        .expect("fast path build should succeed");
        assert!(built.entry_count > 0);
        let snapshot = load_snapshot(&out).expect("snapshot should load");

        // Breadth-first sample: the root plus the first directories
        // discovered, bounded so the live side stays fast on a big volume.
        const MAX_DIRS: usize = 40;
        let mut queue: Vec<String> = vec![volume_key.clone()];
        let mut compared = 0usize;
        while let Some(parent_id) = queue.pop() {
            if compared >= MAX_DIRS {
                break;
            }
            compared += 1;
            let live = children_for_image(&image, &parent_id).expect("live listing");
            // A genuinely empty directory records no entries, so the snapshot
            // deliberately reports "uncovered" and the caller goes live.
            let Some(from_snapshot) = snapshot.children(&parent_id) else {
                assert!(
                    live.is_empty(),
                    "snapshot should cover non-empty directory {parent_id}"
                );
                continue;
            };
            assert_eq!(
                serde_json::to_value(&from_snapshot).unwrap(),
                serde_json::to_value(&live).unwrap(),
                "snapshot listing for {parent_id} diverges from live"
            );
            for node in live {
                if node.kind == NodeKind::Dir && !node.deleted {
                    queue.push(node.id);
                }
            }
        }
        assert!(compared > 1, "sample should descend past the root");
    }
}
