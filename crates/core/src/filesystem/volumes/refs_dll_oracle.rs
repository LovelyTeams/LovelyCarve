//! Read-only ReFS volume adapter built on top of `librefs-13.dll` via
//! the FFI shim at [`crate::platform::ffi::librefs`].
//!
//! Mounting goes through `sys_device_open_callbacks` so the library reads
//! the partition lazily through `DiskImage::read_range` — no temp file,
//! no Dokan, the same callback-I/O pattern every other lovelycarve FS
//! adapter uses (see `core::filesystem::volumes::exfat::PartitionPart`).
//!
//! A `RefsVolume` mounts the partition once in [`RefsVolume::open`] and
//! reuses that live `LibRefsVolume` for every operation, so librefs' block
//! map and node LRU stay warm instead of being rebuilt per call (each
//! `fsapi_volume_mount` re-reads the boot sector + metadata pages straight
//! through the EWF reader — there is no `DiskImage`-level byte cache for E01).
//! Whole-volume snapshot walks additionally cache the `RefsVolume` itself
//! in [`crate::filesystem::tree::fs_tree::FsSession`], so one mount serves the
//! entire walk (mirroring the ext4 path) — re-mounting per directory was
//! the dominant cost of a ReFS snapshot build on a large E01.
//!
//! For whole-volume snapshots the preferred path is now
//! [`RefsVolume::walk_tree`] (a single `fsapi_volume_walk_tree` pass over the
//! entire metadata tree), driven by [`crate::catalog::snapshot::volume_snapshot_refs`] — it
//! reads each metadata node once instead of paying one targeted B-tree descent
//! per directory, which is ~3× faster on a large E01. The cached
//! `FsSession` path above remains the fallback when the loaded DLL
//! predates the `fsapi_volume_walk_tree` export.
//!
//! Node IDs encode the absolute UTF-8 path inside the volume as hex
//! bytes: `refs:part:{N}:{hex(/path/to/entry)}`. The empty-hex form
//! denotes the volume root and is never emitted directly — the partition
//! node itself stands in for the root, mirroring `volumes::exfat`.

use std::sync::Arc;

use anyhow::{anyhow, Result};
use chrono::{TimeZone, Utc};

use crate::evidence::crypto::bitlocker::UnlockedVolume;
use crate::evidence::image::find_partition;
use crate::evidence::image::{DiskImage, Partition};
use crate::filesystem::tree::fs_tree::{
    detect_mime_bytes, shannon_entropy, FsDataRun, FsNode, FsStat, Mactb, NodeKind,
};
use crate::platform::ffi::librefs::{
    DirEntry, NodeAttrs, NodeHandle, RefsBacking as LibRefsBacking, RefsVolume as LibRefsVolume,
    SystemTimestamp, WalkEntry, WalkResult,
};

const SAMPLE_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy)]
pub enum RefsSource {
    Partition(u8),
}

/// Parse a `refs:part:{N}:{hex(path)}` ID. Returns the source plus the
/// decoded UTF-8 path (with a leading `/`, or empty for the volume root).
pub fn parse_refs_id(id: &str) -> Option<(RefsSource, String)> {
    let mut parts = id.splitn(4, ':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some("refs"), Some("part"), Some(part_id), Some(hex_path)) => {
            let part_id = part_id.parse().ok()?;
            let path = if hex_path.is_empty() {
                String::new()
            } else {
                let bytes = hex::decode(hex_path).ok()?;
                String::from_utf8(bytes).ok()?
            };
            Some((RefsSource::Partition(part_id), path))
        }
        _ => None,
    }
}

/// Parse a `refs-snap:part:{N}:{hex(path)}:{hex(stream_name)}` ID — the
/// pseudo-file id minted for a ReFS file-snapshot stream surfaced as an
/// independent file row. Returns the source, the decoded owning-file path
/// (leading `/`), and the decoded stream name. Both the path and the stream
/// name are hex-encoded so arbitrary bytes / `:` in either are safe.
pub fn parse_refs_snap_id(id: &str) -> Option<(RefsSource, String, String)> {
    let mut parts = id.splitn(5, ':');
    match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some("refs-snap"), Some("part"), Some(part_id), Some(hex_path), Some(hex_stream)) => {
            let part_id = part_id.parse().ok()?;
            let path = String::from_utf8(hex::decode(hex_path).ok()?).ok()?;
            let stream = String::from_utf8(hex::decode(hex_stream).ok()?).ok()?;
            if path.is_empty() || stream.is_empty() {
                return None;
            }
            Some((RefsSource::Partition(part_id), path, stream))
        }
        _ => None,
    }
}

pub fn open_refs_source(image: &Arc<DiskImage>, source: RefsSource) -> Result<RefsVolume> {
    match source {
        RefsSource::Partition(part_id) => {
            let partition = find_partition(image, &part_id.to_string())?;
            RefsVolume::open(Arc::clone(image), partition)
        }
    }
}

pub struct RefsVolume {
    partition_id: u8,
    length_bytes: u64,
    /// Mounted once in [`RefsVolume::open`] and reused for every operation.
    /// Holding the live volume keeps librefs' per-volume block map and node
    /// LRU warm across calls instead of rebuilding them on each `children` /
    /// `read` (see the module header).
    volume: LibRefsVolume,
}

impl RefsVolume {
    pub fn open(image: Arc<DiskImage>, partition: &Partition) -> Result<Self> {
        let sector_size = image.sector_size as u64;
        let base_offset = partition.start_lba * sector_size;
        let length_bytes = partition.length_lba * sector_size;
        let backing: Box<dyn LibRefsBacking> = Box::new(ImageBacking {
            image,
            base_offset,
            length_bytes,
        });
        // Mount once. The single `fsapi_volume_mount` here also surfaces
        // boot-sector / FSRS-signature failures to the caller up front,
        // exactly as the old eager probe did — but without the second,
        // throwaway mount the per-method re-mount used to add.
        let volume = LibRefsVolume::open(backing)
            .map_err(|err| anyhow!("ReFS mount partition {}: {err}", partition.id))?;
        Ok(Self {
            partition_id: partition.id,
            length_bytes,
            volume,
        })
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }

    /// Whether the loaded `librefs-13.dll` supports the single-pass
    /// whole-tree walk. The snapshot fast path checks this and falls back to
    /// the generic per-directory walker when `false`.
    pub fn supports_walk_tree(&self) -> bool {
        self.volume.supports_walk_tree()
    }

    /// Enumerate every directory entry on the volume in one sequential
    /// metadata pass. Returns the raw [`WalkEntry`] edge list (keyed by parent
    /// / own object IDs) plus every `refsutil streamsnapshot` snapshot stream
    /// the same pass found — see [`crate::catalog::snapshot::volume_snapshot_refs`]. Errors
    /// if the DLL is too old.
    pub fn walk_tree(&self) -> Result<WalkResult> {
        self.volume
            .walk_tree()
            .map_err(|err| anyhow!("ReFS walk_tree partition {}: {err}", self.partition_id))
    }

    /// The partition id this volume was opened on (for node-id minting).
    pub fn partition_id(&self) -> u8 {
        self.partition_id
    }

    /// The `refs:part:{N}:{hex(path)}` node id for an absolute volume path
    /// (leading `/`), or the volume-root key (`part:{N}`) for the empty path.
    /// Exposed so the snapshot builder mints ids identical to the live
    /// [`children`](Self::children) path.
    pub fn node_id_for_path(&self, path: &str) -> String {
        self.node_id(path)
    }

    /// Lists the direct children of `path` inside the volume. `path` uses
    /// Unix-style separators with a leading `/`, or is empty for the
    /// volume root.
    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let parent_id = self.node_id(path);
        let volume = &self.volume;
        let (target_handle, owned) = self.resolve(volume, path)?;
        let entries = volume
            .list_directory(&target_handle, true)
            .map_err(|err| anyhow!("ReFS list {}: {err}", display_path(path)))?;
        if owned {
            volume.release_node(target_handle);
        }

        let parent_path = path.to_string();
        let mut out = Vec::with_capacity(entries.len());
        for entry in entries {
            if entry.name.is_empty() || entry.name == "." || entry.name == ".." {
                continue;
            }
            let timestamps = dirent_timestamps(&entry);
            let child_path = join_path(&parent_path, &entry.name);
            out.push(FsNode {
                id: self.node_id(&child_path),
                name: entry.name,
                parent_id: Some(parent_id.clone()),
                kind: if entry.is_directory {
                    NodeKind::Dir
                } else {
                    NodeKind::File
                },
                size: entry.size,
                deleted: false,
                inode: entry.inode,
                timestamps,
                link_target: None,
                has_snapshot: false,
            });
        }
        out.sort_by(|a, b| {
            let ak = u8::from(a.kind != NodeKind::Dir);
            let bk = u8::from(b.kind != NodeKind::Dir);
            ak.cmp(&bk)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(out)
    }

    pub fn read_file(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        if path.is_empty() {
            anyhow::bail!("cannot read ReFS volume root as a file");
        }
        let volume = &self.volume;
        let handle = self.lookup_owned(volume, path)?;
        let attrs = volume
            .get_attributes(&handle)
            .map_err(|err| anyhow!("ReFS stat {}: {err}", display_path(path)))?;
        if attrs.is_directory {
            volume.release_node(handle);
            anyhow::bail!(
                "ReFS path {} is a directory, not a file",
                display_path(path)
            );
        }
        let total = attrs.size as usize;
        let want = total.min(max_bytes);
        let bytes = volume
            .read(&handle, 0, want)
            .map_err(|err| anyhow!("ReFS read {}: {err}", display_path(path)));
        volume.release_node(handle);
        bytes
    }

    /// Stream the full file content of `path` to `visitor` in chunks of at
    /// most `chunk_size` bytes. Used by the Dokan read-only mount and the
    /// recursive export — both call us from byte 0 and never seek, so we
    /// just walk `fsapi_node_read` forward until EOF. Returns total bytes
    /// streamed.
    pub fn stream_file<F>(&self, path: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if path.is_empty() {
            anyhow::bail!("cannot stream ReFS volume root as a file");
        }
        let volume = &self.volume;
        let handle = self.lookup_owned(volume, path)?;
        let attrs = match volume.get_attributes(&handle) {
            Ok(a) => a,
            Err(err) => {
                volume.release_node(handle);
                return Err(anyhow!("ReFS stat {}: {err}", display_path(path)));
            }
        };
        if attrs.is_directory {
            volume.release_node(handle);
            anyhow::bail!(
                "ReFS path {} is a directory, not a file",
                display_path(path)
            );
        }

        // 64 KiB floor: smaller asks would cost one fsapi_node_read call
        // per IRP-sized chunk on big files (e.g. pagefile.sys, 1.3 GiB)
        // for no real benefit — librefs' I/O is already buffered.
        let chunk = chunk_size.max(64 * 1024);
        let total = attrs.size;
        let mut offset: u64 = 0;
        let mut streamed: u64 = 0;
        // Reuse one buffer across every chunk instead of letting `read`
        // allocate a fresh `Vec` per iteration — a 1.3 GiB pagefile.sys at
        // 64 KiB chunks would otherwise churn ~20k allocations.
        let mut buf = vec![0u8; chunk];
        let result = (|| {
            while offset < total {
                let remaining = total - offset;
                let want = remaining.min(chunk as u64) as usize;
                let written = volume
                    .read_into(&handle, offset, &mut buf[..want])
                    .map_err(|err| anyhow!("ReFS read {} @ {offset}: {err}", display_path(path)))?;
                if written == 0 {
                    break;
                }
                let advanced = written as u64;
                visitor(&buf[..written])?;
                offset += advanced;
                streamed += advanced;
            }
            Ok::<u64, anyhow::Error>(streamed)
        })();
        volume.release_node(handle);
        result
    }

    /// List the named data streams attached to `path` (regular ADS plus
    /// `refsutil streamsnapshot`-style `:$SNAPSHOT` streams). Returns
    /// an empty Vec for the volume root or a directory — only files
    /// carry named streams.
    pub fn list_streams(
        &self,
        path: &str,
    ) -> Result<Vec<crate::filesystem::volumes::ntfs::NtfsDataStream>> {
        if path.is_empty() {
            return Ok(Vec::new());
        }
        let volume = &self.volume;
        let handle = self.lookup_owned(volume, path)?;
        let entries = volume
            .list_streams(&handle)
            .map_err(|err| anyhow!("ReFS list_streams {}: {err}", display_path(path)));
        volume.release_node(handle);
        let entries = entries?;
        Ok(entries
            .into_iter()
            .map(|s| crate::filesystem::volumes::ntfs::NtfsDataStream {
                name: s.name,
                size: s.size,
                resident: s.resident,
            })
            .collect())
    }

    /// Read up to `max_bytes` from a single named stream of `path`. For
    /// `:$SNAPSHOT` streams this recovers the bytes the snapshot
    /// captured — the original pre-replacement content of the file.
    pub fn read_named_stream(
        &self,
        path: &str,
        stream_name: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        if path.is_empty() {
            anyhow::bail!("cannot read named stream on ReFS volume root");
        }
        if stream_name.is_empty() {
            anyhow::bail!("ReFS named-stream read requires a non-empty stream name");
        }
        let volume = &self.volume;
        let handle = self.lookup_owned(volume, path)?;
        let result = volume
            .read_stream(&handle, stream_name, 0, max_bytes)
            .map_err(|err| {
                anyhow!(
                    "ReFS read_stream {}:{stream_name}: {err}",
                    display_path(path)
                )
            });
        volume.release_node(handle);
        result
    }

    /// Stream a named stream of `path` chunk by chunk to `visitor`.
    /// Mirrors the same pattern as `stream_file` for the default
    /// `:$DATA`; used by [`crate::filesystem::tree::fs_tree::export_named_stream`]
    /// to dump multi-GB snapshots without buffering.
    pub fn stream_named_stream<F>(
        &self,
        path: &str,
        stream_name: &str,
        chunk_size: usize,
        mut visitor: F,
    ) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if path.is_empty() || stream_name.is_empty() {
            anyhow::bail!("ReFS stream_named_stream requires a non-empty path and stream name");
        }
        let volume = &self.volume;
        let handle = self.lookup_owned(volume, path)?;
        // 64 KiB floor matches stream_file; the fsapi_node_read_stream
        // call walks all extents per invocation, so very small chunks
        // become expensive on multi-GB streams.
        let chunk = chunk_size.max(64 * 1024);
        let mut offset: u64 = 0;
        let mut streamed: u64 = 0;
        let result = (|| {
            loop {
                let bytes = volume
                    .read_stream(&handle, stream_name, offset, chunk)
                    .map_err(|err| {
                        anyhow!(
                            "ReFS read_stream {}:{stream_name} @ {offset}: {err}",
                            display_path(path)
                        )
                    })?;
                if bytes.is_empty() {
                    break;
                }
                let advanced = bytes.len() as u64;
                visitor(&bytes)?;
                offset += advanced;
                streamed += advanced;
                // Library returns short reads when the stream ends.
                if bytes.len() < chunk {
                    break;
                }
            }
            Ok::<u64, anyhow::Error>(streamed)
        })();
        volume.release_node(handle);
        result
    }

    pub fn file_stat(&self, path: &str, node_id: &str) -> Result<FsStat> {
        if path.is_empty() {
            anyhow::bail!("cannot stat ReFS volume root as a file");
        }
        let volume = &self.volume;
        let handle = self.lookup_owned(volume, path)?;
        let attrs = volume
            .get_attributes(&handle)
            .map_err(|err| anyhow!("ReFS stat {}: {err}", display_path(path)));
        let attrs = match attrs {
            Ok(a) => a,
            Err(err) => {
                volume.release_node(handle);
                return Err(err);
            }
        };

        // Sample the first few KB for MIME / entropy / magic hints. Skip
        // for directories — the read would just fail.
        let sample = if attrs.is_directory {
            Vec::new()
        } else {
            let want = SAMPLE_BYTES.min(attrs.size as usize);
            if want == 0 {
                Vec::new()
            } else {
                volume.read(&handle, 0, want).unwrap_or_default()
            }
        };
        volume.release_node(handle);

        // librefs does not expose physical extents through fsapi, so we
        // leave data_runs empty. Hex view will fall back to the synthetic
        // mapping used for other extent-less adapters (FAT/exFAT).
        let runs: Vec<FsDataRun> = Vec::new();

        Ok(FsStat {
            node_id: node_id.to_string(),
            size: attrs.size,
            inode: if attrs.inode_number != 0 {
                Some(attrs.inode_number)
            } else {
                None
            },
            mode: if attrs.mode != 0 {
                Some((attrs.mode & 0xFFFF) as u16)
            } else {
                None
            },
            mime: detect_mime_bytes(&sample).to_string(),
            magic_hex: if sample.is_empty() {
                None
            } else {
                Some(
                    sample
                        .iter()
                        .take(16)
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            },
            entropy: if sample.is_empty() {
                None
            } else {
                Some(shannon_entropy(&sample))
            },
            data_runs: runs,
        })
    }

    /// Stat a named stream of `path` as if it were a standalone file — used by
    /// the snapshot pseudo-file (`refs-snap:` id). Mirrors [`file_stat`] but
    /// samples MIME / magic / entropy from the named stream's bytes and sizes
    /// from the stream. `data_runs` is empty (librefs exposes no per-stream
    /// physical extents, same as the default-stream path).
    pub fn named_stream_stat(
        &self,
        path: &str,
        stream_name: &str,
        node_id: &str,
    ) -> Result<FsStat> {
        if path.is_empty() || stream_name.is_empty() {
            anyhow::bail!("ReFS named-stream stat requires a non-empty path and stream name");
        }
        // Size: prefer the stream's reported size; fall back to the read length.
        let size = self
            .list_streams(path)?
            .into_iter()
            .find(|s| s.name == stream_name)
            .map(|s| s.size);
        let sample = self
            .read_named_stream(path, stream_name, SAMPLE_BYTES)
            .unwrap_or_default();
        let size = size.unwrap_or(sample.len() as u64);

        Ok(FsStat {
            node_id: node_id.to_string(),
            size,
            inode: None,
            mode: None,
            mime: detect_mime_bytes(&sample).to_string(),
            magic_hex: if sample.is_empty() {
                None
            } else {
                Some(
                    sample
                        .iter()
                        .take(16)
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            },
            entropy: if sample.is_empty() {
                None
            } else {
                Some(shannon_entropy(&sample))
            },
            data_runs: Vec::new(),
        })
    }

    /// Returns the on-disk size of the partition slice, in bytes. Used
    /// only by callers that need to know the underlying volume size
    /// without going through `fsapi_volume_get_attributes`.
    pub fn partition_length(&self) -> u64 {
        self.length_bytes
    }

    fn node_id(&self, path: &str) -> String {
        if path.is_empty() {
            format!("part:{}", self.partition_id)
        } else {
            format!(
                "refs:part:{}:{}",
                self.partition_id,
                hex::encode(path.as_bytes())
            )
        }
    }

    /// Resolve `path` to a node handle, treating the empty path / "/" as
    /// the root. Returns `(handle, owned)` — when `owned` is true the
    /// caller must call `release_node`; the root is borrowed and must
    /// not be released.
    fn resolve(&self, volume: &LibRefsVolume, path: &str) -> Result<(NodeHandle, bool)> {
        if path.is_empty() || path == "/" {
            return Ok((volume.root_node()?, false));
        }
        let handle = self.lookup_owned(volume, path)?;
        Ok((handle, true))
    }

    fn lookup_owned(&self, volume: &LibRefsVolume, path: &str) -> Result<NodeHandle> {
        let canonical = canonicalize_path(path);
        volume
            .lookup(&canonical)
            .map_err(|err| anyhow!("ReFS lookup {}: {err}", display_path(path)))
    }
}

/// `librefs::RefsBacking` impl for partition slices owned by a
/// `DiskImage`. Each `pread` resolves to a single
/// `DiskImage::read_range` against the absolute offset.
struct ImageBacking {
    image: Arc<DiskImage>,
    base_offset: u64,
    length_bytes: u64,
}

impl LibRefsBacking for ImageBacking {
    fn pread(&self, offset: u64, buf: &mut [u8]) -> Result<(), i32> {
        let want = buf.len();
        if want == 0 {
            return Ok(());
        }
        let end = offset.checked_add(want as u64).ok_or(5_i32)?;
        if end > self.length_bytes {
            return Err(5);
        }
        match self.image.read_range(self.base_offset + offset, want) {
            Ok(bytes) if bytes.len() == want => {
                buf.copy_from_slice(&bytes);
                Ok(())
            }
            _ => Err(5),
        }
    }

    fn size(&self) -> u64 {
        self.length_bytes
    }
}

/// `librefs::RefsBacking` impl that pipes I/O through an unlocked
/// BitLocker volume. ReFS-on-BitLocker is a supported Windows Server
/// layout (BitLocker To Go USB drives, encrypted Storage Spaces), so the
/// adapter exposes this path even though the existing test fixtures
/// don't exercise it yet.
pub struct BitlockerBacking {
    pub unlocked: Arc<UnlockedVolume>,
    pub length_bytes: u64,
}

impl LibRefsBacking for BitlockerBacking {
    fn pread(&self, offset: u64, buf: &mut [u8]) -> Result<(), i32> {
        let want = buf.len();
        if want == 0 {
            return Ok(());
        }
        let end = offset.checked_add(want as u64).ok_or(5_i32)?;
        if end > self.length_bytes {
            return Err(5);
        }
        match self.unlocked.read_at(offset, buf) {
            Ok(n) if n == want => Ok(()),
            _ => Err(5),
        }
    }

    fn size(&self) -> u64 {
        self.length_bytes
    }
}

fn canonicalize_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

fn display_path(path: &str) -> &str {
    if path.is_empty() {
        "/"
    } else {
        path
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() || parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

fn dirent_timestamps(entry: &DirEntry) -> Mactb {
    Mactb {
        modified: timestamp_to_rfc3339(entry.last_data_change_time),
        accessed: timestamp_to_rfc3339(entry.last_data_access_time),
        created: timestamp_to_rfc3339(entry.creation_time),
        mft_changed: None,
        birth: timestamp_to_rfc3339(entry.creation_time),
    }
}

/// [`Mactb`] for a whole-tree [`WalkEntry`], identical in shape to
/// [`dirent_timestamps`] so a snapshot built from the walk matches a live
/// directory listing byte-for-byte. Exposed for [`crate::catalog::snapshot::volume_snapshot_refs`].
pub fn walk_entry_timestamps(entry: &WalkEntry) -> Mactb {
    Mactb {
        modified: timestamp_to_rfc3339(entry.last_data_change_time),
        accessed: timestamp_to_rfc3339(entry.last_data_access_time),
        created: timestamp_to_rfc3339(entry.creation_time),
        mft_changed: None,
        birth: timestamp_to_rfc3339(entry.creation_time),
    }
}

/// Convert a `(tv_sec, tv_nsec)` pair anchored on the Unix epoch to RFC
/// 3339. The library returns `None` for "not available", which we round-
/// trip through `Option<SystemTimestamp>`.
fn timestamp_to_rfc3339(ts: Option<SystemTimestamp>) -> Option<String> {
    let ts = ts?;
    let secs = i64::try_from(ts.tv_sec).ok()?;
    Utc.timestamp_opt(secs, ts.tv_nsec)
        .single()
        .map(|dt| dt.to_rfc3339())
}

/// Wrap a node attribute snapshot into the `Mactb` shape the rest of
/// `fs_tree` consumes. Reused by `file_stat` for non-directory paths
/// where the dirent timestamps would be re-fetched anyway.
#[allow(dead_code)]
fn attrs_timestamps(attrs: &NodeAttrs) -> Mactb {
    Mactb {
        modified: timestamp_to_rfc3339(attrs.last_data_change_time),
        accessed: timestamp_to_rfc3339(attrs.last_data_access_time),
        created: timestamp_to_rfc3339(attrs.creation_time),
        mft_changed: timestamp_to_rfc3339(attrs.last_status_change_time),
        birth: timestamp_to_rfc3339(attrs.creation_time),
    }
}
