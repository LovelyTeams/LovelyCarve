//! build implementation.
use super::*;
use crate::filesystem::evidence_session::EvidenceSession;

/// Walk the whole subtree rooted at `volume_key` and stream it to
/// `out_path` as a zstd JSON-lines `.dirz` file.
///
/// The walk drives [`children_for_image`] exactly as the live FS tree does,
/// so every recorded node id round-trips. Each entry is written to the
/// encoder as it is produced (bounded memory). Deleted entries are recorded
/// as leaves but not descended into — recovered ids don't round-trip back
/// through the dispatcher (same rule as `fs_tree::export_dir`).
///
/// Cancellation: polled between siblings; on cancel (or any error) the
/// partial file is removed by [`PartialOutput`] and the error returned.
/// `on_progress` is called with the running entry count every
/// [`PROGRESS_EVERY`] entries; the caller is responsible for throttling any
/// downstream event emission.
pub fn build_volume_snapshot(
    image: &Arc<EvidenceSession>,
    volume_key: &str,
    out_path: &Path,
    max_entries: usize,
    built_at: String,
    cancel: &CancellationToken,
    on_progress: &mut dyn FnMut(u64),
) -> anyhow::Result<VolumeSnapshotBuilt> {
    let image = image.filesystem_session(volume_key);
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // NTFS volumes take a dedicated fast path: one sequential `$MFT` scan plus
    // an in-memory tree rebuild from `$FILE_NAME` parent references, instead of
    // the generic per-directory walk below (which re-opens the volume and
    // re-reads the 128 KiB `$UpCase` table on every directory — minutes on a
    // large E01). Non-NTFS keys, and NTFS volumes that fail to open, fall
    // through to the generic walker.
    if let Some(opened) = open_ntfs_volume_for_key(image, volume_key) {
        match opened {
            Ok(volume) => {
                return build_ntfs_volume_snapshot(
                    image,
                    &volume,
                    volume_key,
                    out_path,
                    max_entries,
                    built_at,
                    cancel,
                    on_progress,
                );
            }
            Err(err) => {
                tracing::warn!(
                    "snapshot: NTFS fast path unavailable for {volume_key} ({err}); \
                     falling back to generic walk"
                );
            }
        }
    }

    // APFS containers get the same treatment: per volume, one streaming pass
    // over the catalog B-tree (OMAP preloaded) plus an in-memory tree rebuild,
    // instead of the generic walk that pays a root-to-leaf path resolution per
    // directory and a B-tree descent per child for the inode join — hours on
    // a large E01. Cancellation propagates; any other failure falls back to
    // the generic walker below.
    if crate::catalog::snapshot::volume_snapshot_apfs::apfs_container_for_key(image, volume_key)
        .is_some()
    {
        match crate::catalog::snapshot::volume_snapshot_apfs::build_apfs_volume_snapshot(
            image,
            volume_key,
            out_path,
            max_entries,
            built_at.clone(),
            cancel,
            on_progress,
        ) {
            Ok(built) => return Ok(built),
            Err(err) if cancel.is_cancelled() => return Err(err),
            Err(err) => {
                tracing::warn!(
                    "snapshot: APFS fast path failed for {volume_key} ({err}); \
                     falling back to generic walk"
                );
            }
        }
    }

    // Btrfs partitions likewise: the volume is opened once and each reachable
    // subvolume gets a single streaming FS-tree pass (inode items + dir
    // entries in one scan), instead of the generic walk that re-opens the
    // volume (superblock + chunk tree + whole root tree) per directory and
    // pays a B-tree descent per child for the inode join. Cancellation
    // propagates; any other failure falls back to the generic walker below.
    if crate::catalog::snapshot::volume_snapshot_btrfs::btrfs_partition_for_key(image, volume_key)
        .is_some()
    {
        match crate::catalog::snapshot::volume_snapshot_btrfs::build_btrfs_volume_snapshot(
            image,
            volume_key,
            out_path,
            max_entries,
            built_at.clone(),
            cancel,
            on_progress,
        ) {
            Ok(built) => return Ok(built),
            Err(err) if cancel.is_cancelled() => return Err(err),
            Err(err) => {
                tracing::warn!(
                    "snapshot: btrfs fast path failed for {volume_key} ({err}); \
                     falling back to generic walk"
                );
            }
        }
    }

    // ReFS uses its Rust object table and one visit per directory, then rebuilds
    // the path tree and saved-stream rows. Cancellation propagates.
    if crate::catalog::snapshot::volume_snapshot_refs::refs_fast_path_available(image, volume_key) {
        match crate::catalog::snapshot::volume_snapshot_refs::build_refs_volume_snapshot(
            image,
            volume_key,
            out_path,
            max_entries,
            built_at.clone(),
            cancel,
            on_progress,
        ) {
            Ok(built) => return Ok(built),
            Err(err) if cancel.is_cancelled() => return Err(err),
            Err(err) => {
                tracing::warn!(
                    "snapshot: ReFS fast path failed for {volume_key} ({err}); \
                     falling back to generic walk"
                );
            }
        }
    }

    let header = SnapshotHeader {
        schema_version: SNAPSHOT_SCHEMA,
        volume_key: volume_key.to_string(),
        image_id: image.id.clone(),
        source_volume_size: volume_size_for_key(image, volume_key),
        built_at,
    };

    let output = PartialOutput::new(out_path);
    let file = File::create(out_path)?;
    let mut encoder = zstd::stream::write::Encoder::new(BufWriter::new(file), 3)?;
    write_snapshot_header(&mut encoder, &header)?;
    let mut scratch: Vec<u8> = Vec::new();

    // Explicit-stack DFS: each item is a directory to expand, paired with the
    // path that leads to it. Seeded with the volume root. The walker caches
    // opened ext4 volumes so an ext4 (or LVM-backed ext4) subtree is parsed
    // once for the whole build instead of re-opened per directory.
    let mut walker = FsSession::new(image);
    let mut stack: Vec<(String, String)> = vec![(volume_key.to_string(), String::new())];
    let mut entry_count: u64 = 0;
    let mut truncated = false;
    // Guard against directory cycles (corrupt parent refs, hard-linked dirs):
    // never expand the same node id twice. Seeded with the root.
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(volume_key.to_string());

    'walk: while let Some((parent_id, parent_path)) = stack.pop() {
        if cancel.is_cancelled() {
            anyhow::bail!("snapshot build cancelled");
        }
        let children = match walker.children(&parent_id) {
            Ok(children) => children,
            Err(err) => {
                // A single unreadable subtree shouldn't abort the whole
                // snapshot — record nothing for it and move on.
                tracing::warn!("snapshot: list {parent_id} failed: {err}");
                continue;
            }
        };
        for child in children {
            if entry_count as usize >= max_entries {
                truncated = true;
                break 'walk;
            }
            let child_path = if parent_path.is_empty() {
                format!("/{}", child.name)
            } else {
                format!("{parent_path}/{}", child.name)
            };
            let descend = child.kind == NodeKind::Dir && !child.deleted;
            let next = (child.id.clone(), child_path.clone());
            let line = SnapshotEntry {
                parent: parent_id.clone(),
                node: child,
                path: child_path,
            };
            write_snapshot_entry(&mut encoder, &line, &mut scratch)?;
            entry_count += 1;
            if entry_count.is_multiple_of(PROGRESS_EVERY) {
                on_progress(entry_count);
            }
            if descend && visited.insert(next.0.clone()) {
                stack.push(next);
            }
        }
    }

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

/// NTFS fast path for [`build_volume_snapshot`]: scan the whole `$MFT` once,
/// then rebuild the directory tree in memory from `$FILE_NAME` parent
/// references via [`write_tree_snapshot`]. See [`NtfsVolume::scan_tree_edges`].
#[allow(clippy::too_many_arguments)]
pub(super) fn build_ntfs_volume_snapshot(
    image: &Arc<EvidenceSession>,
    volume: &NtfsVolume<'_>,
    volume_key: &str,
    out_path: &Path,
    max_entries: usize,
    built_at: String,
    cancel: &CancellationToken,
    on_progress: &mut dyn FnMut(u64),
) -> anyhow::Result<VolumeSnapshotBuilt> {
    let header = SnapshotHeader {
        schema_version: SNAPSHOT_SCHEMA,
        volume_key: volume_key.to_string(),
        image_id: image.id.clone(),
        source_volume_size: volume_size_for_key(image, volume_key),
        built_at,
    };

    let (edges, ads_streams, scan_truncated) =
        volume.scan_tree_edges(max_entries, cancel, on_progress)?;
    write_tree_snapshot(
        edges,
        ads_streams,
        header,
        out_path,
        max_entries,
        NTFS_ROOT_RECORD,
        scan_truncated,
        cancel,
        on_progress,
    )
}

/// The NTFS root directory's MFT record number. Children of this record are
/// filed under the volume root key (`"part:N"` / `"lvm:N:M"`) rather than an
/// `ntfs:…` node id, matching what a live root listing returns.
pub(super) const NTFS_ROOT_RECORD: u64 = 5;

/// Assemble a directory tree from a flat list of [`NtfsTreeEdge`]s and stream
/// it to `out_path` as a `.dirz` file. Pure given its inputs (no NTFS access),
/// so it is unit-tested with synthetic edges.
///
/// Edges are grouped by parent record, each group sorted dirs-first then by
/// case-insensitive name (matching the live listing's order), and the tree is
/// walked depth-first from `root_record`. Deleted directories are recorded as
/// leaves but not descended into; a `visited` set guards against cycles from
/// corrupt parent references. Emission stops at `max_entries`, OR-ing in the
/// `scan_truncated` flag carried from the scan.
///
/// `ads_streams` carries every surfaced named ADS keyed by owning MFT record.
/// Each owning file is flagged `has_snapshot = true` and one `ntfs-ads:`
/// pseudo-file row is emitted directly beneath it per stream — the NTFS analog
/// of the ReFS file-snapshot rows (see [`crate::catalog::snapshot::volume_snapshot_refs`]).
#[allow(clippy::too_many_arguments)]
pub(super) fn write_tree_snapshot(
    edges: Vec<NtfsTreeEdge>,
    ads_streams: Vec<NtfsAdsStream>,
    header: SnapshotHeader,
    out_path: &Path,
    max_entries: usize,
    root_record: u64,
    scan_truncated: bool,
    cancel: &CancellationToken,
    on_progress: &mut dyn FnMut(u64),
) -> anyhow::Result<VolumeSnapshotBuilt> {
    let volume_key = header.volume_key.clone();

    let mut by_parent: HashMap<u64, Vec<usize>> = HashMap::new();
    for (idx, edge) in edges.iter().enumerate() {
        by_parent
            .entry(edge.parent_record_number)
            .or_default()
            .push(idx);
    }
    for idxs in by_parent.values_mut() {
        idxs.sort_by_cached_key(|&idx| {
            let node = &edges[idx].node;
            (
                u8::from(node.kind != NodeKind::Dir),
                node.name.to_lowercase(),
            )
        });
    }

    // Index surfaced ADS by owning MFT record. A hard-linked file lists under
    // several directories; emitting the streams beneath each occurrence matches
    // how the owner itself surfaces under each path.
    let mut streams_by_owner: HashMap<u64, Vec<NtfsAdsStream>> = HashMap::new();
    for stream in ads_streams {
        streams_by_owner
            .entry(stream.record_number)
            .or_default()
            .push(stream);
    }

    let output = PartialOutput::new(out_path);
    let file = File::create(out_path)?;
    let mut encoder = zstd::stream::write::Encoder::new(BufWriter::new(file), 3)?;
    write_snapshot_header(&mut encoder, &header)?;
    let mut scratch: Vec<u8> = Vec::new();

    // Explicit-stack DFS from the volume root. Each item is a directory record
    // to expand, paired with the path that leads to it.
    let mut stack: Vec<(u64, String)> = vec![(root_record, String::new())];
    let mut visited: HashSet<u64> = HashSet::new();
    visited.insert(root_record);
    let mut entry_count: u64 = 0;
    let mut truncated = scan_truncated;

    'walk: while let Some((parent_record, parent_path)) = stack.pop() {
        if cancel.is_cancelled() {
            anyhow::bail!("snapshot build cancelled");
        }
        let Some(idxs) = by_parent.get(&parent_record) else {
            continue;
        };
        for &idx in idxs {
            if entry_count as usize >= max_entries {
                truncated = true;
                break 'walk;
            }
            let edge = &edges[idx];
            let child_path = if parent_path.is_empty() {
                format!("/{}", edge.node.name)
            } else {
                format!("{parent_path}/{}", edge.node.name)
            };
            // The grouping key the frontend queries with is exactly the child's
            // own parent_id — the volume root key for root's children, the
            // parent's node id otherwise — set by `collect_record_edges`.
            let parent_key = edge
                .node
                .parent_id
                .clone()
                .unwrap_or_else(|| volume_key.clone());
            let descend = edge.node.kind == NodeKind::Dir
                && !edge.node.deleted
                && visited.insert(edge.record_number);

            // Snapshot streams (named ADS) owned by this file or directory.
            // Flag the owner, then emit one pseudo-file row per stream
            // immediately after it so the streams sit directly beneath their
            // owner in the listing. Deleted owners are skipped (their streams
            // are recovery noise). The volume root carries no edge of its own,
            // so its streams are emitted separately after the walk.
            let owner_streams = (!edge.node.deleted)
                .then(|| streams_by_owner.get(&edge.record_number))
                .flatten();
            let mut node = edge.node.clone();
            if owner_streams.is_some_and(|s| !s.is_empty()) {
                node.has_snapshot = true;
            }
            let owner_id = node.id.clone();
            let line = SnapshotEntry {
                parent: parent_key.clone(),
                node,
                path: child_path.clone(),
            };
            write_snapshot_entry(&mut encoder, &line, &mut scratch)?;
            entry_count += 1;
            if entry_count.is_multiple_of(PROGRESS_EVERY) {
                on_progress(entry_count);
            }

            // One `ntfs-ads:` pseudo-file row per surfaced named stream, filed
            // under the same query parent as the owner (so it lists in this
            // directory) and pointing back at the owning file via `parent_id`.
            // Leaf files, never descended. See `parse_ntfs_ads_id` /
            // `read_data_stream` for the read path.
            if let Some(streams) = owner_streams {
                // `ntfs:part:N:rec` / `ntfs:lvm:N:M:rec` → `part:N:rec` tail.
                let id_tail = owner_id.strip_prefix("ntfs:").unwrap_or(&owner_id);
                for stream in streams {
                    if entry_count as usize >= max_entries {
                        truncated = true;
                        break 'walk;
                    }
                    let ads_id =
                        format!("ntfs-ads:{id_tail}:{}", hex::encode(stream.name.as_bytes()));
                    let ads_node = FsNode {
                        id: ads_id,
                        name: format!("{}:{} (数据流)", line.node.name, stream.name),
                        parent_id: Some(owner_id.clone()),
                        kind: NodeKind::File,
                        size: stream.size,
                        deleted: false,
                        inode: line.node.inode,
                        timestamps: line.node.timestamps.clone(),
                        link_target: None,
                        has_snapshot: false,
                    };
                    let ads_line = SnapshotEntry {
                        parent: parent_key.clone(),
                        path: format!("{child_path}:{}", stream.name),
                        node: ads_node,
                    };
                    write_snapshot_entry(&mut encoder, &ads_line, &mut scratch)?;
                    entry_count += 1;
                }
            }

            if descend {
                stack.push((edge.record_number, child_path));
            }
        }
    }

    // The volume root directory has no `$FILE_NAME` edge of its own, so its
    // named ADS never appear in the walk above. Emit them now as top-level
    // pseudo-rows filed under the volume root key — this is the classic hiding
    // spot (`\:secret.txt`). The id
    // encodes the root record so the read/stat path resolves the stream the
    // same way it does for a file's ADS.
    if let Some(root_streams) = streams_by_owner.get(&root_record) {
        let id_tail = format!("{volume_key}:{root_record}");
        for stream in root_streams {
            if entry_count as usize >= max_entries {
                truncated = true;
                break;
            }
            let ads_id = format!("ntfs-ads:{id_tail}:{}", hex::encode(stream.name.as_bytes()));
            let ads_node = FsNode {
                id: ads_id,
                // Leading `:` distinguishes a volume-root stream from an ordinary file.
                name: format!(":{} (数据流)", stream.name),
                parent_id: Some(volume_key.clone()),
                kind: NodeKind::File,
                size: stream.size,
                deleted: false,
                inode: Some(root_record),
                timestamps: Mactb::default(),
                link_target: None,
                has_snapshot: false,
            };
            let ads_line = SnapshotEntry {
                parent: volume_key.clone(),
                path: format!("/:{}", stream.name),
                node: ads_node,
            };
            write_snapshot_entry(&mut encoder, &ads_line, &mut scratch)?;
            entry_count += 1;
        }
    }

    encoder.finish()?;
    output.keep();
    on_progress(entry_count);

    let file_name = out_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| snapshot_file_name(&header.image_id, &volume_key));

    Ok(VolumeSnapshotBuilt {
        header,
        file_name,
        entry_count,
        truncated,
    })
}
