//! Per-image filesystem array (`fs_all`): `vol_id` is its index.
//!
//! Every filesystem in an image has an array position. The walker keeps string volume
//! keys (`part:N`, `lvm:N:M`, or a synthetic root for archive / folder
//! evidence) as the walker identity; this module fixes the mapping between
//! those keys and the integer `vol_id` persisted in `filedb`.
//!
//! Order is deterministic: partitions in table order, each LVM container
//! immediately followed by its logical volumes, then the synthetic root for
//! evidence without a partition table, followed by metadata-only Docker
//! container roots. Enumerating these roots never composes their layers.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::core::evidence::partitions::PartitionKind;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{self, NodeKind};
use crate::core::filesystem::volumes::lvm::LvmVolume;

/// Synthetic volume key for evidence that exposes a single root instead of a
/// partition table (docker / folder / tar / zip / 7z).
pub const ROOT_VOLUME_KEY: &str = "root";

/// One entry of the filesystem array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    pub vol_id: i64,
    pub volume_key: String,
    pub kind: PartitionKind,
    /// Node-id prefix family used by [`crate::core::filesystem::meta_addr`] when
    /// reconstructing ids: `ntfs`, `ext4`, `btrfs`, or `path`.
    pub fs_prefix: &'static str,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FsAll {
    entries: Vec<FsEntry>,
}

impl FsAll {
    /// Enumerate the filesystems of a loaded image. LVM containers whose
    /// metadata cannot be parsed keep their container entry and log a warning;
    /// their logical volumes are simply absent.
    pub fn for_image(image: &Arc<EvidenceSession>) -> Self {
        let mut entries = Vec::new();
        let sector = image.sector_size as u64;
        for partition in &image.info().partitions {
            let offset = partition.start_lba * sector;
            let size = partition.length_lba * sector;
            entries.push(FsEntry {
                vol_id: entries.len() as i64,
                volume_key: format!("part:{}", partition.id),
                kind: partition.kind,
                fs_prefix: fs_prefix_for_kind(partition.kind),
                offset,
                size,
            });
            if partition.kind != PartitionKind::Lvm {
                continue;
            }
            let reader = image.filesystem_session(&format!("part:{}", partition.id));
            let partition = reader
                .partitions
                .iter()
                .find(|candidate| candidate.id == partition.id)
                .unwrap_or(partition);
            match LvmVolume::open(reader, partition) {
                Ok(lvm) => {
                    for (lv_index, lv) in lvm.logical_volumes.iter().enumerate() {
                        entries.push(FsEntry {
                            vol_id: entries.len() as i64,
                            volume_key: format!("lvm:{}:{lv_index}", partition.id),
                            kind: lv.kind,
                            fs_prefix: fs_prefix_for_kind(lv.kind),
                            offset: lv.absolute_offset,
                            size: lv.size_bytes,
                        });
                    }
                }
                Err(err) => {
                    tracing::warn!(
                        "fs_all: skip logical volumes of part:{}: {err:#}",
                        partition.id
                    );
                }
            }
        }
        if entries.is_empty() {
            entries.push(FsEntry {
                vol_id: 0,
                volume_key: ROOT_VOLUME_KEY.to_string(),
                kind: PartitionKind::Unknown,
                fs_prefix: "path",
                offset: 0,
                size: image.len(),
            });
        }
        // Virtual container volumes follow all physical volumes. Discovering
        // their metadata does not merge or index their filesystems.
        for volume_key in crate::core::filesystem::docker::host_docker::volume_keys(image) {
            entries.push(FsEntry {
                vol_id: entries.len() as i64,
                volume_key,
                kind: PartitionKind::Unknown,
                fs_prefix: "path",
                offset: 0,
                size: image.len(),
            });
        }
        Self { entries }
    }

    pub fn entries(&self) -> &[FsEntry] {
        &self.entries
    }

    pub fn entry(&self, vol_id: i64) -> Option<&FsEntry> {
        usize::try_from(vol_id)
            .ok()
            .and_then(|index| self.entries.get(index))
    }

    pub fn vol_id_of(&self, volume_key: &str) -> Option<i64> {
        let key = if volume_key.trim().is_empty() {
            ROOT_VOLUME_KEY
        } else {
            volume_key
        };
        self.entries
            .iter()
            .find(|entry| entry.volume_key == key)
            .map(|entry| entry.vol_id)
    }

    pub fn volume_key_of(&self, vol_id: i64) -> Option<&str> {
        self.entry(vol_id).map(|entry| entry.volume_key.as_str())
    }
}

/// The node whose children are the top-level entries of a volume. Partition,
/// logical-volume and Docker-container volumes list their own root
/// directory; the root volume of a logical source (folder, archive, AD1,
/// Docker image) holds one synthetic node for the whole source, and the
/// source's entries sit below that node.
pub fn directory_root(image: &Arc<EvidenceSession>, volume_key: &str) -> anyhow::Result<String> {
    let logical = image.docker.is_some()
        || image.folder.is_some()
        || image.tar.is_some()
        || image.zip.is_some()
        || image.sevenz.is_some()
        || image.ad1.is_some();
    if volume_key != ROOT_VOLUME_KEY || !logical {
        return Ok(volume_key.to_owned());
    }
    let nodes = fs_tree::children_for_image(image, ROOT_VOLUME_KEY)?;
    match nodes.as_slice() {
        [source] if source.kind == NodeKind::Dir => Ok(source.id.clone()),
        _ => anyhow::bail!("the logical evidence root lists {} nodes", nodes.len()),
    }
}

/// Resolve `vol_id` for a volume key on a loaded image, erroring when the key
/// is not part of the filesystem array.
pub fn vol_id_for_key(image: &Arc<EvidenceSession>, volume_key: &str) -> anyhow::Result<i64> {
    FsAll::for_image(image)
        .vol_id_of(volume_key)
        .ok_or_else(|| anyhow::anyhow!("volume {volume_key} is not in the filesystem array"))
}

pub fn fs_prefix_for_kind(kind: PartitionKind) -> &'static str {
    match kind {
        PartitionKind::Ntfs | PartitionKind::Bitlocker => "ntfs",
        PartitionKind::Ext4 => "ext4",
        PartitionKind::Btrfs => "btrfs",
        PartitionKind::Refs
        | PartitionKind::Fat32
        | PartitionKind::Fat12
        | PartitionKind::Fat16
        | PartitionKind::ExFat
        | PartitionKind::Cdfs
        | PartitionKind::Squashfs
        | PartitionKind::Ubi
        | PartitionKind::Lvm
        | PartitionKind::BiosBoot
        | PartitionKind::ApfsContainer
        | PartitionKind::HfsPlus
        | PartitionKind::HfsX
        | PartitionKind::Filevault
        | PartitionKind::Luks
        | PartitionKind::Apm
        | PartitionKind::Bsd
        | PartitionKind::Swap
        | PartitionKind::Extended
        | PartitionKind::Mdadm
        | PartitionKind::Ufs
        | PartitionKind::Xfs
        | PartitionKind::Zfs
        | PartitionKind::Unknown => "path",
    }
}
