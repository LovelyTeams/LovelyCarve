use std::sync::Arc;

use crate::evidence::image::find_partition;
use crate::evidence::image::{ImageFormat, Partition, PartitionKind};
use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::{ChildrenOptions, FsNode, Mactb, NodeKind};
use crate::filesystem::volumes::lvm::{parse_lvm_id, LvmVolume};
use crate::filesystem::volumes::ntfs::{parse_ntfs_id, NtfsVolume};
use crate::filesystem::volumes::ntfs::{NtfsDeletedSnapshot, NtfsSource};
use crate::filesystem::volumes::ntfs_usn::UsnRecord;

const BDE_LOCKED_PREFIX: &str = "bde-locked:part:";
const NTFS_ROOT_RECORD: u64 = 5;

pub(crate) fn open_ntfs_for_source(
    image: &Arc<EvidenceSession>,
    source: NtfsSource,
) -> anyhow::Result<NtfsVolume<'_>> {
    let partition_id = match source {
        NtfsSource::Partition(id) => id,
        NtfsSource::Lvm { part_id, .. } => part_id,
    };
    let image = image.filesystem_session(&format!("part:{partition_id}"));
    match source {
        NtfsSource::Partition(partition_id) => {
            let partition = find_partition(image, &partition_id.to_string())?;
            if partition.kind == PartitionKind::Bitlocker {
                let unlocked = image.bitlocker_unlocked(partition_id).ok_or_else(|| {
                    anyhow::anyhow!(
                        "BitLocker partition {partition_id} is locked; unlock it before reading"
                    )
                })?;
                return NtfsVolume::open_bitlocker(unlocked, partition_id);
            }
            NtfsVolume::open(image, partition)
        }
        NtfsSource::Lvm { part_id, lv_index } => {
            let partition = find_partition(image, &part_id.to_string())?;
            NtfsVolume::open_lvm(image, partition, lv_index)
        }
    }
}

/// Reads the NTFS `$UsnJrnl:$J` change journal for the partition or LVM
/// volume named by `parent_id` (`"part:N"` / `"lvm:N:M"`, or any `ntfs:`-
/// prefixed node id on that volume). `max_bytes` caps how much of the journal
/// is read. Returns an empty vec if the volume has no change journal.
pub fn ntfs_usn_journal(
    image: &Arc<EvidenceSession>,
    parent_id: &str,
    max_bytes: usize,
) -> anyhow::Result<Vec<UsnRecord>> {
    let source = if let Some((source, _record)) = parse_ntfs_id(parent_id) {
        source
    } else if let Some(rest) = parent_id.strip_prefix("lvm:") {
        let mut parts = rest.split(':');
        let part_id: u8 = parts
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("invalid lvm volume key: {parent_id}"))?;
        let lv_index: usize = parts
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("invalid lvm volume key: {parent_id}"))?;
        NtfsSource::Lvm { part_id, lv_index }
    } else {
        let part_id: u8 = parent_id
            .strip_prefix("part:")
            .unwrap_or(parent_id)
            .parse()
            .map_err(|_| anyhow::anyhow!("not an NTFS volume key: {parent_id}"))?;
        NtfsSource::Partition(part_id)
    };
    let volume = open_ntfs_for_source(image, source)?;
    volume.read_usn_journal(max_bytes)
}

/// If `volume_key` (`"part:N"` or `"lvm:N:M"`) names an NTFS volume root —
/// including an unlocked BitLocker partition — open it. Returns `None` for any
/// other key so a caller (e.g. the volume-snapshot builder) can fall back to
/// the generic per-directory walker. `Some(Err(..))` means the key IS an NTFS
/// volume but opening it failed; the error is propagated, not swallowed.
///
/// This shares the exact open path (`open_ntfs_for_source`) used by live
/// directory listings, so the BitLocker / LVM handling stays in one place.
pub fn open_ntfs_volume_for_key<'a>(
    image: &'a Arc<EvidenceSession>,
    volume_key: &str,
) -> Option<anyhow::Result<NtfsVolume<'a>>> {
    let image = image.filesystem_session(volume_key);
    if let Some(part_id_str) = volume_key.strip_prefix("part:") {
        let part_id = part_id_str.parse::<u8>().ok()?;
        let partition = find_partition(image, &part_id.to_string()).ok()?;
        match partition.kind {
            PartitionKind::Ntfs => {
                Some(open_ntfs_for_source(image, NtfsSource::Partition(part_id)))
            }
            PartitionKind::Bitlocker if image.bitlocker_unlocked(part_id).is_some() => {
                Some(open_ntfs_for_source(image, NtfsSource::Partition(part_id)))
            }
            _ => None,
        }
    } else if let Some((part_id, lv_index)) = parse_lvm_id(volume_key) {
        let partition = find_partition(image, part_id).ok()?;
        let lvm = LvmVolume::open(image, partition).ok()?;
        match lvm.logical_volumes.get(lv_index) {
            Some(lv) if lv.kind == PartitionKind::Ntfs => Some(open_ntfs_for_source(
                image,
                NtfsSource::Lvm {
                    part_id: partition.id,
                    lv_index,
                },
            )),
            _ => None,
        }
    } else {
        None
    }
}

pub(crate) fn ntfs_root_children(
    image: &Arc<EvidenceSession>,
    partition: &Partition,
    options: ChildrenOptions,
) -> anyhow::Result<Vec<FsNode>> {
    let source = NtfsSource::Partition(partition.id);
    let volume = open_ntfs_for_source(image, source)?;
    ntfs_children(image, &volume, source, NTFS_ROOT_RECORD, options)
}

pub(crate) fn ntfs_lvm_root_children(
    image: &Arc<EvidenceSession>,
    partition: &Partition,
    lv_index: usize,
    options: ChildrenOptions,
) -> anyhow::Result<Vec<FsNode>> {
    let source = NtfsSource::Lvm {
        part_id: partition.id,
        lv_index,
    };
    let volume = open_ntfs_for_source(image, source)?;
    ntfs_children(image, &volume, source, NTFS_ROOT_RECORD, options)
}

fn ntfs_children(
    image: &Arc<EvidenceSession>,
    volume: &NtfsVolume<'_>,
    source: NtfsSource,
    record_number: u64,
    options: ChildrenOptions,
) -> anyhow::Result<Vec<FsNode>> {
    if !options.include_deleted_ntfs {
        return volume.children(record_number);
    }
    let snapshot = ntfs_deleted_snapshot_for_listing(image, volume, source);
    volume.children_with_deleted(record_number, snapshot.as_deref())
}

/// Pick the right deleted-records strategy for a synchronous directory
/// listing. The previous behaviour built the entire MFT snapshot inline on
/// every first listing — on a 1.86 TiB NTFS partition that costs several
/// minutes, freezing the UI behind "正在读取目录…". Today we:
///
///   * reuse a cached snapshot when available;
///   * for image-file sources (E01 / VMDK / raw / …) still build inline so
///     deleted entries appear from the very first listing, the way users
///     expect for forensic images that already paid the MFT-scan tax at
///     load time;
///   * for live block devices (`PhysicalDisk`), kick the scan off in a
///     background task and return `None` to the current listing so it
///     completes in milliseconds. Once the background scan finishes, the
///     `Arc<NtfsDeletedSnapshot>` is installed into the image cache and
///     subsequent directory listings pick it up transparently.
pub(crate) fn ntfs_deleted_snapshot_for_listing(
    image: &Arc<EvidenceSession>,
    volume: &NtfsVolume<'_>,
    source: NtfsSource,
) -> Option<Arc<NtfsDeletedSnapshot>> {
    if let Some(cached) = image.ntfs_deleted_snapshot(source) {
        return Some(cached);
    }
    if matches!(image.info().source_format, ImageFormat::PhysicalDisk) {
        spawn_ntfs_deleted_snapshot(Arc::clone(image), source);
        return None;
    }
    ensure_ntfs_deleted_snapshot(image, volume, source)
}

/// One-shot background scan that opens its own NTFS volume handle, builds
/// the deleted-records snapshot, and installs it back into the
/// `EvidenceSession` cache. Idempotent — if the snapshot is already present when
/// the task starts (e.g. a prior listing finished its own scan) the task
/// exits immediately. Safe to call repeatedly.
///
/// Uses `std::thread::spawn` rather than tokio so callers don't need a
/// runtime in scope — the example binaries probe this code path
/// synchronously and tokio's `spawn_blocking` would panic there.
fn spawn_ntfs_deleted_snapshot(image: Arc<EvidenceSession>, source: NtfsSource) {
    std::thread::spawn(move || {
        if image.ntfs_deleted_snapshot(source).is_some() {
            return;
        }
        tracing::info!("NTFS background deleted-scan begin for {source:?}");
        let volume = match open_ntfs_for_source(&image, source) {
            Ok(volume) => volume,
            Err(err) => {
                tracing::warn!("NTFS background scan: open {source:?} failed: {err}");
                return;
            }
        };
        match volume.build_deleted_snapshot() {
            Ok(snapshot) => {
                image.install_ntfs_deleted_snapshot(source, Arc::new(snapshot));
                tracing::info!("NTFS background deleted-scan done for {source:?}");
            }
            Err(err) => tracing::warn!("NTFS background scan {source:?} failed: {err}"),
        }
    });
}

/// Lazily populates the per-source deleted-records snapshot on the
/// `EvidenceSession` cache and returns an `Arc` to it. Returns `None` if the scan
/// fails — callers fall back to active-only listings without bubbling up.
fn ensure_ntfs_deleted_snapshot(
    image: &Arc<EvidenceSession>,
    volume: &NtfsVolume<'_>,
    source: NtfsSource,
) -> Option<Arc<NtfsDeletedSnapshot>> {
    if let Some(cached) = image.ntfs_deleted_snapshot(source) {
        return Some(cached);
    }
    match volume.build_deleted_snapshot() {
        Ok(snapshot) => {
            let arc = Arc::new(snapshot);
            image.install_ntfs_deleted_snapshot(source, Arc::clone(&arc));
            Some(arc)
        }
        Err(err) => {
            tracing::warn!("NTFS deleted-records scan failed for {source:?}: {err}");
            None
        }
    }
}

/// Spawns a background task that pre-builds the deleted-records snapshot for
/// every NTFS volume on `image` whose source is reachable right now — plain
/// NTFS partitions, BitLocker partitions that have already been unlocked,
/// and NTFS volumes that live inside LVM2 logical volumes. Called from
/// `image_load` (covers plain NTFS + LVM-backed NTFS) and from
/// `bitlocker_unlock` (covers a partition that just became reachable). The
/// FsTree view sees deleted entries inline as soon as the user clicks into
/// the partition. Failures are logged and swallowed — the lazy fallback in
/// [`ensure_ntfs_deleted_snapshot`] still works if the background scan never
/// finishes (e.g. process exits early).
pub fn prefetch_ntfs_deleted_snapshots(image: Arc<EvidenceSession>) {
    let mut targets: Vec<NtfsSource> = Vec::new();
    for partition in &image.partitions {
        match partition.kind {
            PartitionKind::Ntfs => targets.push(NtfsSource::Partition(partition.id)),
            PartitionKind::Bitlocker if image.bitlocker_unlocked(partition.id).is_some() => {
                targets.push(NtfsSource::Partition(partition.id));
            }
            PartitionKind::Lvm => {
                let Ok(lvm) = LvmVolume::open(&image, partition) else {
                    continue;
                };
                for lv in &lvm.logical_volumes {
                    if lv.kind == PartitionKind::Ntfs {
                        targets.push(NtfsSource::Lvm {
                            part_id: partition.id,
                            lv_index: lv.index,
                        });
                    }
                }
            }
            _ => {}
        }
    }
    if targets.is_empty() {
        return;
    }
    tokio::task::spawn_blocking(move || {
        for source in targets {
            if image.ntfs_deleted_snapshot(source).is_some() {
                continue;
            }
            let volume = match open_ntfs_for_source(&image, source) {
                Ok(volume) => volume,
                Err(err) => {
                    tracing::warn!("NTFS prefetch: open {source:?} failed: {err}");
                    continue;
                }
            };
            match volume.build_deleted_snapshot() {
                Ok(snapshot) => {
                    image.install_ntfs_deleted_snapshot(source, Arc::new(snapshot));
                }
                Err(err) => tracing::warn!(
                    "NTFS prefetch: deleted-records scan failed for {source:?}: {err}"
                ),
            }
        }
    });
}

/// Returns the children of a BitLocker partition. If the volume has been
/// unlocked via `EvidenceSession::unlock_bitlocker_partition`, this enumerates the
/// underlying NTFS root (with deleted MFT records inlined). Otherwise it
/// returns a single placeholder node that the frontend recognises and turns
/// into an unlock prompt.
pub(crate) fn bitlocker_partition_children(
    image: &Arc<EvidenceSession>,
    partition: &Partition,
    options: ChildrenOptions,
) -> anyhow::Result<Vec<FsNode>> {
    if image.bitlocker_unlocked(partition.id).is_some() {
        return ntfs_root_children(image, partition, options);
    }
    Ok(vec![FsNode {
        id: format!("{BDE_LOCKED_PREFIX}{}", partition.id),
        name: "BitLocker 加密卷（未解锁）".to_string(),
        parent_id: Some(format!("part:{}", partition.id)),
        kind: NodeKind::Unallocated,
        size: 0,
        deleted: false,
        inode: None,
        timestamps: Mactb::default(),
        link_target: None,
        has_snapshot: false,
    }])
}
