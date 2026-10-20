//! Volume Shadow Copy discovery and session-only snapshot images.
//!
//! [`list`] reports the shadow copies stored on each NTFS volume of an image —
//! plain NTFS partitions and unlocked BitLocker volumes — and [`open`] exposes
//! one of them as a read-only session image (`vss://…`). Its bytes are
//! reconstructed on demand by the vendored `vsc-core` chain reader from the
//! live volume and the copy-on-write stores; nothing is written to disk. Reads
//! that touch unreadable evidence sectors fail instead of returning zeros.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::{Arc, Weak};

use anyhow::{bail, ensure, Context};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use vsc::{SnapshotLayout, StoreDescriptor, StoreInfo, VssVolume};

use crate::core::evidence::crypto::bitlocker::UnlockedVolume;
use crate::core::evidence::image::{DerivedImage, DiskImage};
use crate::core::evidence::partitions::{
    detect_partitions_with_meta, Partition, PartitionKind, SectorSource,
};

/// Path prefix of shadow-copy session images.
pub const SESSION_PREFIX: &str = "vss://";

/// The Unix epoch as a Windows FILETIME (100 ns ticks since 1601).
const UNIX_EPOCH_FILETIME: u64 = 116_444_736_000_000_000;

/// One shadow copy of an NTFS volume.
#[derive(Debug, Clone, Serialize)]
pub struct ShadowCopyInfo {
    /// Position in creation order, oldest first.
    pub index: usize,
    pub store_id: String,
    pub shadow_copy_id: Option<String>,
    pub shadow_copy_set_id: Option<String>,
    /// Raw creation FILETIME and its Unix-milliseconds form.
    pub creation_filetime: u64,
    pub created_unix_ms: Option<i64>,
    /// Volume size when the snapshot was taken.
    pub volume_size: u64,
    pub snapshot_context: Option<u32>,
    pub attributes: Option<u32>,
    pub persistent: bool,
    pub client_accessible: bool,
    pub operating_machine: Option<String>,
    pub service_machine: Option<String>,
    /// Why the shadow copy cannot be opened, when its store has no pointer to
    /// its copy-on-write data.
    pub unavailable_reason: Option<String>,
    /// Session image already open for this shadow copy; filled in by the
    /// command layer.
    pub open_image_id: Option<String>,
}

/// The shadow copies found on one NTFS volume.
#[derive(Debug, Clone, Serialize)]
pub struct ShadowVolumeInfo {
    pub partition_id: u8,
    pub volume_size: u64,
    /// Oldest first. Empty when the volume carries the VSS header but no
    /// shadow copies (none taken, or all deleted).
    pub shadow_copies: Vec<ShadowCopyInfo>,
    /// Set when the volume's VSS metadata could not be read.
    pub error: Option<String>,
}

/// Where a shadow-copy session image comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShadowCopyOrigin {
    pub parent_image_id: String,
    pub partition_id: u8,
    pub index: usize,
    pub store_id: String,
    pub shadow_copy_id: Option<String>,
    pub created_unix_ms: Option<i64>,
}

/// The live bytes of an NTFS volume that may carry shadow copies.
#[derive(Clone)]
enum LiveVolume {
    /// A plain partition, read straight from the image.
    Partition {
        image: Weak<DiskImage>,
        offset: u64,
        length: u64,
    },
    /// An unlocked BitLocker volume, read through its decryptor.
    Bitlocker(Arc<UnlockedVolume>),
}

impl LiveVolume {
    /// The volume of `partition`, or `None` when it cannot hold NTFS shadow
    /// copies (another file system, or a still-locked BitLocker volume).
    fn for_partition(
        image: &Arc<DiskImage>,
        partition: &Partition,
    ) -> anyhow::Result<Option<Self>> {
        match partition.kind {
            PartitionKind::Ntfs => {
                let sector = image.sector_size as u64;
                let offset = partition
                    .start_lba
                    .checked_mul(sector)
                    .context("partition offset overflows")?;
                let length = partition
                    .length_lba
                    .checked_mul(sector)
                    .context("partition size overflows")?;
                ensure!(
                    offset
                        .checked_add(length)
                        .is_some_and(|end| end <= image.len()),
                    "partition {} lies outside the image",
                    partition.id
                );
                Ok(Some(Self::Partition {
                    image: Arc::downgrade(image),
                    offset,
                    length,
                }))
            }
            PartitionKind::Bitlocker => {
                Ok(image.bitlocker_unlocked(partition.id).map(Self::Bitlocker))
            }
            _ => Ok(None),
        }
    }

    fn len(&self) -> u64 {
        match self {
            Self::Partition { length, .. } => *length,
            Self::Bitlocker(volume) => volume.size(),
        }
    }

    /// Fill `buffer` from the volume at `offset`, or fail.
    fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        let in_range = offset
            .checked_add(buffer.len() as u64)
            .is_some_and(|end| end <= self.len());
        if !in_range {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "shadow copy read runs past the volume",
            ));
        }
        match self {
            Self::Partition {
                image,
                offset: base,
                ..
            } => {
                let image = image
                    .upgrade()
                    .ok_or_else(|| io::Error::other("shadow copy evidence closed"))?;
                let absolute = base + offset;
                let filled = image
                    .read_into(absolute, buffer)
                    .map_err(io::Error::other)?;
                if filled != buffer.len()
                    || image.has_bad_region_in_range(absolute, buffer.len() as u64)
                {
                    return Err(io::Error::other(
                        "shadow copy evidence contains unreadable sectors",
                    ));
                }
                Ok(())
            }
            Self::Bitlocker(volume) => {
                let mut filled = 0;
                while filled < buffer.len() {
                    let read = volume
                        .read_at(offset + filled as u64, &mut buffer[filled..])
                        .map_err(io::Error::other)?;
                    if read == 0 {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "BitLocker volume ended early",
                        ));
                    }
                    filled += read;
                }
                Ok(())
            }
        }
    }
}

/// `Read + Seek` over a [`LiveVolume`], the interface `VssVolume` parses.
struct LiveCursor {
    volume: LiveVolume,
    position: u64,
    cancellation: Option<CancellationToken>,
}

impl LiveCursor {
    fn new(volume: LiveVolume, cancellation: Option<CancellationToken>) -> Self {
        Self {
            volume,
            position: 0,
            cancellation,
        }
    }
}

impl Read for LiveCursor {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if self
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(io::Error::other("shadow copy operation cancelled"));
        }
        let remaining = self.volume.len().saturating_sub(self.position);
        let count = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        if count == 0 {
            return Ok(0);
        }
        self.volume
            .read_exact_at(self.position, &mut buffer[..count])?;
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for LiveCursor {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let position = match from {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.volume.len()) + i128::from(delta),
        };
        self.position = u64::try_from(position)
            .map_err(|_| io::Error::other("invalid shadow copy volume seek"))?;
        Ok(self.position)
    }
}

/// The reconstructed bytes of one shadow copy.
struct SnapshotSource {
    layout: SnapshotLayout,
    live: LiveVolume,
}

impl SectorSource for SnapshotSource {
    fn size(&self) -> u64 {
        self.layout.volume_size()
    }

    fn read(&self, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
        let available = self.size().saturating_sub(offset).min(length as u64) as usize;
        let mut bytes = vec![0; available];
        if available > 0 {
            self.layout
                .read_at(offset, &mut bytes, &mut |volume_offset, out: &mut [u8]| {
                    self.live.read_exact_at(volume_offset, out)
                })
                .with_context(|| {
                    format!("failed to reconstruct shadow copy bytes at {offset:#x}")
                })?;
        }
        Ok(bytes)
    }
}

fn filetime_to_unix_ms(filetime: u64) -> Option<i64> {
    let ticks = filetime.checked_sub(UNIX_EPOCH_FILETIME)?;
    i64::try_from(ticks / 10_000).ok()
}

fn describe_copy(
    index: usize,
    store: &StoreDescriptor,
    info: Result<StoreInfo, vsc::VssError>,
) -> ShadowCopyInfo {
    let (info, unavailable_reason) = match info {
        Ok(info) => (Some(info), None),
        Err(error) => (None, Some(error.to_string())),
    };
    let attributes = info.as_ref().map(|info| info.attributes);
    ShadowCopyInfo {
        index,
        store_id: store.store_id_string(),
        shadow_copy_id: info.as_ref().map(StoreInfo::shadow_copy_id_string),
        shadow_copy_set_id: info.as_ref().map(StoreInfo::shadow_copy_set_id_string),
        creation_filetime: store.creation_time,
        created_unix_ms: filetime_to_unix_ms(store.creation_time),
        volume_size: store.volume_size,
        snapshot_context: info.as_ref().map(|info| info.snapshot_context),
        attributes: attributes.map(vsc::AttributeFlags::bits),
        persistent: attributes.is_some_and(vsc::AttributeFlags::is_persistent),
        client_accessible: attributes.is_some_and(vsc::AttributeFlags::is_client_accessible),
        operating_machine: info.as_ref().map(|info| info.operating_machine.clone()),
        service_machine: info.as_ref().map(|info| info.service_machine.clone()),
        unavailable_reason,
        open_image_id: None,
    }
}

/// The shadow copies of one volume, or `None` when it carries no VSS header.
fn describe_volume(partition_id: u8, live: LiveVolume) -> anyhow::Result<Option<ShadowVolumeInfo>> {
    let volume_size = live.len();
    let mut volume = VssVolume::open(LiveCursor::new(live, None))?;
    if !volume.has_vss_header() {
        return Ok(None);
    }
    let stores = volume.stores().to_vec();
    let shadow_copies = stores
        .iter()
        .enumerate()
        .map(|(index, store)| describe_copy(index, store, volume.store_info(index)))
        .collect();
    Ok(Some(ShadowVolumeInfo {
        partition_id,
        volume_size,
        shadow_copies,
        error: None,
    }))
}

/// List the shadow copies of every NTFS volume of `image`. Volumes without a
/// VSS header are omitted; one whose metadata cannot be read is reported with
/// its error.
pub fn list(image: &Arc<DiskImage>) -> Vec<ShadowVolumeInfo> {
    let mut volumes = Vec::new();
    for partition in &image.partitions {
        let live = match LiveVolume::for_partition(image, partition) {
            Ok(Some(live)) => live,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(
                    "shadow copy probe skipped partition {}: {error:#}",
                    partition.id
                );
                continue;
            }
        };
        let volume_size = live.len();
        match describe_volume(partition.id, live) {
            Ok(Some(volume)) => volumes.push(volume),
            Ok(None) => {}
            Err(error) => volumes.push(ShadowVolumeInfo {
                partition_id: partition.id,
                volume_size,
                shadow_copies: Vec::new(),
                error: Some(format!("{error:#}")),
            }),
        }
    }
    volumes
}

/// Session-path prefix shared by every image opened for this shadow copy.
pub fn session_path_prefix(parent_image_id: &str, partition_id: u8, store_id: &str) -> String {
    format!("{SESSION_PREFIX}{parent_image_id}/{partition_id}/{store_id}/")
}

fn session_label(origin: &ShadowCopyOrigin) -> String {
    let created = origin
        .created_unix_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map(|time| time.format("_%Y-%m-%d_%H-%M-%S_UTC").to_string())
        .unwrap_or_default();
    format!(
        "分区{}_卷影副本{}{created}",
        origin.partition_id,
        origin.index + 1
    )
}

/// Reconstruct shadow copy `index` (creation order, oldest first) of the NTFS
/// volume in `partition_id` as a read-only session image.
pub fn open(
    image: &Arc<DiskImage>,
    partition_id: u8,
    index: usize,
    cancellation: &CancellationToken,
) -> anyhow::Result<DiskImage> {
    let partition = image
        .partitions
        .iter()
        .find(|partition| partition.id == partition_id)
        .with_context(|| format!("partition {partition_id} not found"))?;
    let live = LiveVolume::for_partition(image, partition)?.with_context(|| {
        format!("partition {partition_id} is not a readable NTFS volume (unlock BitLocker first)")
    })?;
    let mut volume = VssVolume::open(LiveCursor::new(live.clone(), Some(cancellation.clone())))?;
    ensure!(
        volume.has_vss_header(),
        "partition {partition_id} has no shadow copies"
    );
    let store = volume.stores().get(index).cloned().with_context(|| {
        format!(
            "shadow copy {} not found on partition {partition_id}",
            index + 1
        )
    })?;
    let shadow_copy_id = Some(volume.store_info(index)?.shadow_copy_id_string());
    let layout = volume.snapshot_layout(index)?;
    if cancellation.is_cancelled() {
        bail!("shadow copy open cancelled");
    }

    let source = Arc::new(SnapshotSource { layout, live });
    let detected = detect_partitions_with_meta(source.as_ref(), 512)?;
    let mut snapshot = DiskImage::from_assembled("vss".into(), source, detected.partitions, 512)?;
    let origin = ShadowCopyOrigin {
        parent_image_id: image.id.clone(),
        partition_id,
        index,
        store_id: store.store_id_string(),
        shadow_copy_id,
        created_unix_ms: filetime_to_unix_ms(store.creation_time),
    };
    snapshot.path = format!(
        "{}{}",
        session_path_prefix(&image.id, partition_id, &origin.store_id),
        session_label(&origin)
    )
    .into();
    snapshot.table_scheme = detected.scheme;
    snapshot.disk_id = detected.disk_id;
    snapshot.derived = Some(DerivedImage::ShadowCopy {
        parent: image.id.clone(),
        origin,
    });
    Ok(snapshot)
}
