//! ReFS fast path for [`crate::catalog::snapshot::volume_snapshot::build_volume_snapshot`].
//!
//! The Rust volume loads the object table once, then visits each directory's
//! metadata pages once. This module rebuilds the path tree from object IDs and
//! emits the same ordering, IDs, timestamps and reparse targets as live listings.
//! Saved stream versions are emitted as `refs-snap:` pseudo-files.
//!
//! 8.3 short-name aliases: ReFS lists both the long name (`Program Files`) and
//! its short alias (`PROGRA~1`) as separate directory entries that point at the
//! same child object ID. The generic walker descends both (each is a distinct
//! path-addressed node id), so this builder does too — children are grouped by
//! parent object ID, but the DFS de-dups by node id (path), so an aliased
//! directory's subtree is captured under both names exactly as live listings
//! would return it. A per-path ancestor set of object IDs guards against a
//! corrupt directory cycle without suppressing the (legitimate) alias
//! duplication.

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
use crate::filesystem::volumes::refs::{walk_entry_timestamps, RefsVolume};
use crate::filesystem::volumes::refs::{WalkEntry, WalkResult, WalkStream, REFS_ROOT_OBJECT_ID};

/// The partition behind `volume_key`, when that key names a ReFS filesystem
/// (`part:N` only — ReFS never sits inside LVM here). `None` sends the caller
/// down the generic walker exactly as before.
pub(crate) fn refs_partition_for_key(
    image: &EvidenceSession,
    volume_key: &str,
) -> Option<Partition> {
    let part_id = volume_key.strip_prefix("part:")?.parse::<u8>().ok()?;
    image
        .partitions
        .iter()
        .find(|partition| partition.id == part_id && partition.kind == PartitionKind::Refs)
        .cloned()
}

/// Build a `.dirz` snapshot of a ReFS partition with one whole-tree metadata
/// scan. Same contract as the generic builder: cancellation finalises and
/// removes the partial file and bails with "snapshot build cancelled";
/// `max_entries` flips `truncated`. Metadata parsing errors propagate to the caller.
pub(crate) fn build_refs_volume_snapshot(
    image: &Arc<EvidenceSession>,
    volume_key: &str,
    out_path: &Path,
    max_entries: usize,
    built_at: String,
    cancel: &CancellationToken,
    on_progress: &mut dyn FnMut(u64),
) -> anyhow::Result<VolumeSnapshotBuilt> {
    let partition = refs_partition_for_key(image, volume_key)
        .ok_or_else(|| anyhow!("not a ReFS volume key: {volume_key}"))?;
    let volume = RefsVolume::open_with_cancel(Arc::clone(image.disk_image()), &partition, cancel)?;

    let header = SnapshotHeader {
        schema_version: SNAPSHOT_SCHEMA,
        volume_key: volume_key.to_string(),
        image_id: image.id.clone(),
        source_volume_size: volume_size_for_key(image, volume_key),
        built_at,
    };

    // One pass over each directory's metadata pages. This is the single
    // expensive read of the build; everything below is in-memory assembly. The
    // same pass also surfaces every `refsutil streamsnapshot` snapshot stream.
    on_progress(0);
    let WalkResult { entries, streams } = volume.walk_tree(cancel)?;
    if cancel.is_cancelled() {
        anyhow::bail!("snapshot build cancelled");
    }

    // Group entries by their parent directory's object id. Within each group,
    // preserve the walk order; the per-listing sort (dirs-first, then
    // case-insensitive name) is applied when each directory is emitted so it
    // matches the live `RefsVolume::children` ordering exactly.
    let mut by_parent: HashMap<u64, Vec<WalkEntry>> = HashMap::new();
    for entry in entries {
        by_parent
            .entry(entry.parent_object_id)
            .or_default()
            .push(entry);
    }

    // Index snapshot streams by their owning file: (parent dir object id, file
    // name) — exactly the key available when emitting each file node below.
    // One file may carry several snapshot streams (each its own pseudo-row).
    let mut streams_by_owner: HashMap<(u64, String), Vec<WalkStream>> = HashMap::new();
    for stream in streams {
        streams_by_owner
            .entry((stream.owner_parent_object_id, stream.owner_name.clone()))
            .or_default()
            .push(stream);
    }

    let output = crate::catalog::snapshot::volume_snapshot::PartialOutput::new(out_path);
    let file = File::create(out_path)?;
    let mut encoder = zstd::stream::write::Encoder::new(BufWriter::new(file), 3)?;
    write_snapshot_header(&mut encoder, &header)?;
    let mut scratch: Vec<u8> = Vec::new();

    let mut entry_count: u64 = 0;
    let mut truncated = false;

    // Explicit-stack DFS from the volume root object id. Each item is a
    // directory to expand: (object_id, node_id_of_this_dir, path_to_this_dir,
    // ancestor object ids on the current path). The root's children are filed
    // under the volume key (`part:N`); deeper entries under the parent's
    // `refs:…` node id — matching what a live listing returns.
    //
    // `visited` de-dups by node id (path), so a directory reached by two names
    // (8.3 alias) is still expanded under each path, while a genuine path
    // revisit is blocked. `ancestors` additionally breaks object-id cycles from
    // corrupt parent references without suppressing alias duplication.
    let build = (|| -> anyhow::Result<()> {
        let mut visited: HashSet<String> = HashSet::new();
        visited.insert(volume_key.to_string());
        let mut stack: Vec<(u64, String, String, Vec<u64>)> = vec![(
            REFS_ROOT_OBJECT_ID,
            volume_key.to_string(),
            String::new(),
            vec![REFS_ROOT_OBJECT_ID],
        )];

        while let Some((dir_oid, parent_node_id, parent_path, ancestors)) = stack.pop() {
            if cancel.is_cancelled() {
                anyhow::bail!("snapshot build cancelled");
            }
            let Some(group) = by_parent.get(&dir_oid) else {
                continue;
            };

            // Build this directory's children as FsNodes, then sort exactly
            // like `RefsVolume::children` before emitting. The tuple carries
            // the child's own object id (for descent) and its absolute volume
            // path (to mint snapshot-stream pseudo-file ids).
            let mut nodes: Vec<(FsNode, u64, String)> = Vec::with_capacity(group.len());
            for entry in group {
                if entry.name.is_empty() || entry.name == "." || entry.name == ".." {
                    continue;
                }
                let child_path = if parent_path.is_empty() {
                    format!("/{}", entry.name)
                } else {
                    format!("{parent_path}/{}", entry.name)
                };
                let node = FsNode {
                    id: volume.node_id_for_path(&child_path),
                    name: entry.name.clone(),
                    parent_id: Some(parent_node_id.clone()),
                    kind: if entry.is_directory {
                        NodeKind::Dir
                    } else {
                        NodeKind::File
                    },
                    size: entry.size,
                    deleted: false,
                    inode: entry.inode,
                    timestamps: walk_entry_timestamps(entry),
                    link_target: entry.link_target.clone(),
                    has_snapshot: false,
                };
                nodes.push((node, entry.object_id, child_path));
            }
            nodes.sort_by_cached_key(|(node, _, _)| {
                (
                    u8::from(node.kind != NodeKind::Dir),
                    node.name.to_lowercase(),
                )
            });

            // The volume root additionally carries the FS-level "$未分配簇"
            // node the generic dispatcher appends after the sorted children
            // (ReFS has no orphan container). Appended unsorted, exactly as the
            // generic root listing builds it.
            let mut unalloc: Vec<FsNode> = Vec::new();
            if dir_oid == REFS_ROOT_OBJECT_ID {
                unalloc = fs_unalloc_nodes(image, &partition);
            }

            for (mut node, child_oid, owner_path) in nodes {
                if entry_count as usize >= max_entries {
                    truncated = true;
                    return Ok(());
                }
                let descend = node.kind == NodeKind::Dir;
                let child_node_id = node.id.clone();
                let child_path = if parent_path.is_empty() {
                    format!("/{}", node.name)
                } else {
                    format!("{parent_path}/{}", node.name)
                };

                // Snapshot streams owned by this file (keyed by parent dir oid
                // + file name). Flag the owner, then emit one pseudo-file row
                // per snapshot stream immediately after it so the streams sit
                // directly beneath their file in the listing.
                let owner_streams = (node.kind == NodeKind::File)
                    .then(|| streams_by_owner.get(&(dir_oid, node.name.clone())))
                    .flatten();
                if owner_streams.is_some_and(|s| !s.is_empty()) {
                    node.has_snapshot = true;
                }
                let owner_id = node.id.clone();

                let line = SnapshotEntry {
                    parent: parent_node_id.clone(),
                    node,
                    path: child_path.clone(),
                };
                write_snapshot_entry(&mut encoder, &line, &mut scratch)?;
                entry_count += 1;
                if entry_count.is_multiple_of(PROGRESS_EVERY) {
                    on_progress(entry_count);
                }

                // One pseudo-file row per snapshot stream, filed under the same
                // query parent as the owner (so it lists in this directory),
                // pointing back at the owning file via `parent_id`. These are
                // leaf files (never descended). See `parse_refs_snap_id` /
                // `read_named_stream` for the read path.
                if let Some(streams) = owner_streams {
                    for stream in streams {
                        if entry_count as usize >= max_entries {
                            truncated = true;
                            return Ok(());
                        }
                        let snap_id = format!(
                            "refs-snap:part:{}:{}:{}",
                            partition.id,
                            hex::encode(owner_path.as_bytes()),
                            hex::encode(stream.stream_name.as_bytes()),
                        );
                        let snap_node = FsNode {
                            id: snap_id,
                            name: format!("{}:{} (快照)", line.node.name, stream.stream_name),
                            parent_id: Some(owner_id.clone()),
                            kind: NodeKind::File,
                            size: stream.size,
                            deleted: false,
                            inode: line.node.inode,
                            timestamps: line.node.timestamps.clone(),
                            link_target: None,
                            has_snapshot: false,
                        };
                        let snap_line = SnapshotEntry {
                            parent: parent_node_id.clone(),
                            path: format!("{owner_path}:{}", stream.stream_name),
                            node: snap_node,
                        };
                        write_snapshot_entry(&mut encoder, &snap_line, &mut scratch)?;
                        entry_count += 1;
                    }
                }

                // Descend unless this object id is already an ancestor (cycle)
                // or this exact path was already expanded.
                if descend
                    && !ancestors.contains(&child_oid)
                    && visited.insert(child_node_id.clone())
                {
                    let mut child_ancestors = ancestors.clone();
                    child_ancestors.push(child_oid);
                    stack.push((child_oid, child_node_id, child_path, child_ancestors));
                }
            }

            for node in unalloc {
                if entry_count as usize >= max_entries {
                    truncated = true;
                    return Ok(());
                }
                let child_path = format!("/{}", node.name);
                let line = SnapshotEntry {
                    parent: parent_node_id.clone(),
                    node,
                    path: child_path,
                };
                write_snapshot_entry(&mut encoder, &line, &mut scratch)?;
                entry_count += 1;
            }
        }
        Ok(())
    })();
    build?;

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

/// Select the ReFS builder by partition type; mounting happens once in the build.
pub(crate) fn refs_fast_path_available(image: &Arc<EvidenceSession>, volume_key: &str) -> bool {
    refs_partition_for_key(image, volume_key).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::snapshot::volume_snapshot::load_snapshot;
    use crate::filesystem::tree::fs_tree::children_for_image;

    /// Env-gated equivalence guard: `LOVELYCARVE_REFS_SNAPSHOT_TEST` points at
    /// an image with a ReFS partition. Builds a fast-path snapshot and checks
    /// that, for the partition root and a bounded breadth-first sample of
    /// directories, the snapshot listing is JSON-identical to the live
    /// `children_for_image` listing — the exact property `fs_children` relies
    /// on when serving from a snapshot.
    #[test]
    fn fast_path_matches_live_listings_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_REFS_SNAPSHOT_TEST") else {
            return;
        };
        let image = Arc::new(EvidenceSession::open(path).expect("ReFS test image should open"));
        let partition = image
            .partitions
            .iter()
            .find(|partition| partition.kind == PartitionKind::Refs)
            .cloned()
            .expect("expected a ReFS partition");
        let volume_key = format!("part:{}", partition.id);

        assert!(refs_fast_path_available(&image, &volume_key));

        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("refs.dirz");
        let cancel = CancellationToken::new();
        let built = build_refs_volume_snapshot(
            &image,
            &volume_key,
            &out,
            crate::catalog::snapshot::volume_snapshot::DEFAULT_MAX_SNAPSHOT_ENTRIES,
            "2026-06-23T00:00:00Z".to_string(),
            &cancel,
            &mut |_| {},
        )
        .expect("fast path build should succeed");
        assert!(built.entry_count > 0);
        let snapshot = load_snapshot(&out).expect("snapshot should load");

        const MAX_DIRS: usize = 60;
        let mut queue: Vec<String> = vec![volume_key.clone()];
        let mut compared = 0usize;
        // Track that the fast path surfaced at least one snapshot pseudo-row
        // correctly attributed to a flagged owner, so the positive behaviour is
        // asserted on a real image (not just the byte-identity of real rows).
        let mut saw_snapshot_row = false;
        while let Some(parent_id) = queue.pop() {
            if compared >= MAX_DIRS {
                break;
            }
            compared += 1;
            let live = children_for_image(&image, &parent_id).expect("live listing");
            let Some(from_snapshot) = snapshot.children(&parent_id) else {
                assert!(
                    live.is_empty(),
                    "snapshot should cover non-empty directory {parent_id}"
                );
                continue;
            };

            // The snapshot adds `refs-snap:` pseudo-rows (and sets
            // `has_snapshot` on their owners) that the live listing has no
            // notion of. For the byte-identity guarantee `fs_children` relies
            // on, compare only the REAL rows after stripping the pseudo-rows
            // and zeroing `has_snapshot`. Owners whose snapshot row is present
            // are validated separately below.
            let owners_with_snapshot: Vec<String> = from_snapshot
                .iter()
                .filter(|n| n.id.starts_with("refs-snap:"))
                .filter_map(|n| n.parent_id.clone())
                .collect();
            let real_from_snapshot: Vec<FsNode> = from_snapshot
                .iter()
                .filter(|n| !n.id.starts_with("refs-snap:"))
                .cloned()
                .map(|mut n| {
                    n.has_snapshot = false;
                    n
                })
                .collect();
            assert_eq!(
                serde_json::to_value(&real_from_snapshot).unwrap(),
                serde_json::to_value(&live).unwrap(),
                "snapshot listing for {parent_id} diverges from live (real rows)"
            );

            // Every pseudo-row's owner must be a real file in the same listing
            // that the snapshot flagged.
            for owner_id in &owners_with_snapshot {
                let owner = from_snapshot
                    .iter()
                    .find(|n| &n.id == owner_id)
                    .unwrap_or_else(|| {
                        panic!("snapshot row owner {owner_id} missing from listing")
                    });
                assert!(
                    owner.has_snapshot,
                    "owner {owner_id} of a snapshot row is not flagged has_snapshot"
                );
                assert_eq!(owner.kind, NodeKind::File);
                saw_snapshot_row = true;
            }

            for node in live {
                if node.kind == NodeKind::Dir && !node.deleted && !node.id.starts_with("unalloc:") {
                    queue.push(node.id);
                }
            }
        }
        assert!(compared > 1, "sample should descend past the root");
        // The known ground-truth fixture (secret_012.docx:snapshot_sec) lives a
        // few levels deep; the bounded sample may not reach it, so don't hard-
        // fail when no snapshot row was seen — just report it.
        if !saw_snapshot_row {
            eprintln!("note: no snapshot pseudo-row in the sampled directories");
        }
    }

    /// Targeted check on the known ground-truth fixture: the `secrets`
    /// directory holds `secret_012.docx` with a `snapshot_sec` snapshot stream
    /// (size 15001). Asserts the owner is flagged and a `refs-snap:` row with
    /// the right id/size/parent sits in the same listing, and that reading the
    /// pseudo-row's id yields the snapshot bytes.
    #[test]
    fn ground_truth_snapshot_row_when_env_path_is_set() {
        use crate::filesystem::tree::fs_tree::{file_stat, read_file_limited};

        let Some(path) = std::env::var_os("LOVELYCARVE_REFS_SNAPSHOT_TEST") else {
            return;
        };
        let image = Arc::new(EvidenceSession::open(path).expect("ReFS test image should open"));
        let partition = image
            .partitions
            .iter()
            .find(|partition| partition.kind == PartitionKind::Refs)
            .cloned()
            .expect("expected a ReFS partition");
        let volume_key = format!("part:{}", partition.id);
        assert!(refs_fast_path_available(&image, &volume_key));

        let secrets_path = "/Users/Administrator/secrets";
        let secrets_id = format!(
            "refs:part:{}:{}",
            partition.id,
            hex::encode(secrets_path.as_bytes())
        );
        let owner_path = format!("{secrets_path}/secret_012.docx");
        let owner_id = format!(
            "refs:part:{}:{}",
            partition.id,
            hex::encode(owner_path.as_bytes())
        );
        let snap_id = format!(
            "refs-snap:part:{}:{}:{}",
            partition.id,
            hex::encode(owner_path.as_bytes()),
            hex::encode("snapshot_sec".as_bytes())
        );

        let dir = tempfile::tempdir().expect("tempdir");
        let out = dir.path().join("refs.dirz");
        let cancel = CancellationToken::new();
        build_refs_volume_snapshot(
            &image,
            &volume_key,
            &out,
            crate::catalog::snapshot::volume_snapshot::DEFAULT_MAX_SNAPSHOT_ENTRIES,
            "2026-06-23T00:00:00Z".to_string(),
            &cancel,
            &mut |_| {},
        )
        .expect("fast path build should succeed");
        let snapshot = load_snapshot(&out).expect("snapshot should load");

        let listing = snapshot
            .children(&secrets_id)
            .expect("secrets directory should be covered");
        let owner = listing
            .iter()
            .find(|n| n.id == owner_id)
            .expect("secret_012.docx should be listed");
        assert!(owner.has_snapshot, "secret_012.docx should be flagged");
        let snap_row = listing
            .iter()
            .find(|n| n.id == snap_id)
            .expect("snapshot pseudo-row should be listed");
        assert_eq!(snap_row.kind, NodeKind::File);
        assert_eq!(snap_row.parent_id.as_deref(), Some(owner_id.as_str()));
        assert_eq!(snap_row.size, 15001);
        assert!(snap_row.name.starts_with("secret_012.docx:snapshot_sec"));
        assert!(!snap_row.has_snapshot);

        // The pseudo-row id reads the snapshot bytes (ZIP magic; size 15001).
        let bytes = read_file_limited(&image, &snap_id, 64 * 1024).expect("read snapshot stream");
        assert_eq!(bytes.len(), 15001);
        assert_eq!(&bytes[..4], b"PK\x03\x04");
        let stat = file_stat(&image, &snap_id).expect("stat snapshot stream");
        assert_eq!(stat.size, 15001);
    }
}
