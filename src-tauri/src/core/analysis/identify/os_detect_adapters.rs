//! Wires the [`OsFingerprint`](crate::core::analysis::identify::os_detect::OsFingerprint) trait
//! onto every filesystem the dispatcher in `core::filesystem::tree::fs_tree` already knows
//! how to walk. Instead of duplicating per-format path-resolution code,
//! the single [`Fingerprint`] adapter resolves a POSIX-style path by
//! walking [`fs_tree::children_for_image`] component-by-component and
//! reading via [`fs_tree::read_file_limited`]. That gives free coverage
//! of NTFS, Ext4, FAT, exFAT, HFS+, APFS, XFS, Btrfs, UFS and SquashFS,
//! plus transparent BitLocker-unlocked NTFS and LVM2 logical volumes —
//! every filesystem `fs_tree` understands becomes detectable.
//!
//! The image-level entry point [`detect_image`] iterates every partition,
//! expands LVM containers into their logical volumes, marks BitLocker
//! partitions whose volume is still locked, runs the detector chain, and
//! returns a per-partition report.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::core::analysis::identify::os_detect::{self, DetectedOs, OsFingerprint};
use crate::core::evidence::image::{Partition, PartitionKind};
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{self, FsNode, NodeKind};
use crate::core::filesystem::volumes::lvm::LvmVolume;

/// Per-file read ceiling for fingerprint probes. Most fingerprints are
/// tiny (`/etc/os-release`, `SystemVersion.plist`, `boot.cfg` are all
/// under 50 KiB), but the Windows registry hive
/// `Windows/System32/config/SOFTWARE` runs 30–500 MiB on real installs
/// and notatin needs the whole thing to follow cell-offset pointers to
/// `Microsoft\Windows NT\CurrentVersion`. A cap below ~200 MiB silently
/// breaks Windows detection on long-used systems — the parser sees
/// truncated bytes, fails to follow the offset, and returns no match.
const READ_CAP_BYTES: usize = 256 * 1024 * 1024;

/// Result of detecting an OS on one filesystem-bearing slice (a partition
/// or a single LVM logical volume). `bitlocker_locked == true` is the
/// "needs unlock" hint the UI surfaces; in that case `filesystem_opened`
/// is `false` and `detections` is empty.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OsDetectionPartition {
    pub partition_id: u8,
    pub partition_label: String,
    pub partition_kind: String,
    pub filesystem_opened: bool,
    pub error: Option<String>,
    pub detections: Vec<DetectedOs>,
    pub bitlocker_locked: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OsDetectionResult {
    pub image_id: String,
    pub partitions: Vec<OsDetectionPartition>,
}

/// Generic [`OsFingerprint`] over any node in the `fs_tree` dispatcher.
/// `root_id` is the parent-id understood by `fs_tree::children_for_image`
/// — `part:N` for a regular partition, `lvm:N:M` for one logical volume
/// inside an LVM2 container, or a synthetic archive root (`tar:` / `zip:`
/// / …). Shared with `core::analysis::identify::autodetect`, which probes the same roots with
/// its own marker tables.
pub(crate) struct Fingerprint {
    image: Arc<EvidenceSession>,
    root_id: String,
}

impl Fingerprint {
    pub(crate) fn new(image: Arc<EvidenceSession>, root_id: String) -> Self {
        Self { image, root_id }
    }

    /// Read a file with an explicit byte cap (the trait's `read` uses the
    /// hive-sized [`READ_CAP_BYTES`]).
    pub(crate) fn read_limited(&self, path: &str, cap: usize) -> anyhow::Result<Option<Vec<u8>>> {
        let Some(node) = self.lookup(path)? else {
            return Ok(None);
        };
        if node.kind != NodeKind::File {
            return Ok(None);
        }
        Ok(Some(fs_tree::read_file_limited(
            &self.image,
            &node.id,
            cap,
        )?))
    }

    fn lookup(&self, path: &str) -> anyhow::Result<Option<FsNode>> {
        let components = path.split('/').filter(|s| !s.is_empty());
        let mut current_id = self.root_id.clone();
        let mut current_node: Option<FsNode> = None;
        for component in components {
            let children = fs_tree::children_for_image(&self.image, &current_id)?;
            let Some(found) = children
                .into_iter()
                .find(|node| node.name.eq_ignore_ascii_case(component))
            else {
                return Ok(None);
            };
            current_id = found.id.clone();
            current_node = Some(found);
        }
        Ok(current_node)
    }
}

impl OsFingerprint for Fingerprint {
    fn read(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        self.read_limited(path, READ_CAP_BYTES)
    }

    fn exists(&self, path: &str) -> anyhow::Result<bool> {
        Ok(self.lookup(path)?.is_some())
    }

    fn is_dir(&self, path: &str) -> anyhow::Result<bool> {
        Ok(self
            .lookup(path)?
            .map(|node| node.kind == NodeKind::Dir)
            .unwrap_or(false))
    }
}

pub fn detect_image(image: &Arc<EvidenceSession>) -> anyhow::Result<Vec<OsDetectionPartition>> {
    let mut out = Vec::new();
    for partition in &image.partitions {
        match partition.kind {
            PartitionKind::Lvm => match LvmVolume::open(image, partition) {
                Ok(lvm) => {
                    if lvm.logical_volumes.is_empty() {
                        out.push(unsupported_partition_entry(partition));
                    } else {
                        for lv in &lvm.logical_volumes {
                            out.push(detect_logical_volume(image, partition, lv));
                        }
                    }
                }
                Err(err) => out.push(OsDetectionPartition {
                    partition_id: partition.id,
                    partition_label: partition_label(partition),
                    partition_kind: "lvm".to_string(),
                    filesystem_opened: false,
                    error: Some(err.to_string()),
                    detections: Vec::new(),
                    bitlocker_locked: false,
                }),
            },
            PartitionKind::Bitlocker if image.bitlocker_unlocked(partition.id).is_none() => {
                out.push(OsDetectionPartition {
                    partition_id: partition.id,
                    partition_label: partition_label(partition),
                    partition_kind: "bitlocker".to_string(),
                    filesystem_opened: false,
                    error: None,
                    detections: Vec::new(),
                    bitlocker_locked: true,
                });
            }
            kind if supports_os_detect(kind) => {
                out.push(detect_partition(image, partition));
            }
            _ => out.push(unsupported_partition_entry(partition)),
        }
    }
    Ok(out)
}

fn detect_partition(image: &Arc<EvidenceSession>, partition: &Partition) -> OsDetectionPartition {
    let fingerprint = Fingerprint {
        image: Arc::clone(image),
        root_id: format!("part:{}", partition.id),
    };
    // Trial-list the root so we report FS-open failures distinctly from
    // "FS opened, no fingerprint matched". Keeping the listing lets the
    // detector skip the work entirely on a corrupt FS.
    match fs_tree::children_for_image(image, &fingerprint.root_id) {
        Ok(_) => {
            let detections = os_detect::detect(&fingerprint);
            OsDetectionPartition {
                partition_id: partition.id,
                partition_label: partition_label(partition),
                partition_kind: kind_str(partition.kind).to_string(),
                filesystem_opened: true,
                error: None,
                detections,
                bitlocker_locked: matches!(partition.kind, PartitionKind::Bitlocker),
            }
        }
        Err(err) => OsDetectionPartition {
            partition_id: partition.id,
            partition_label: partition_label(partition),
            partition_kind: kind_str(partition.kind).to_string(),
            filesystem_opened: false,
            error: Some(err.to_string()),
            detections: Vec::new(),
            bitlocker_locked: false,
        },
    }
}

fn detect_logical_volume(
    image: &Arc<EvidenceSession>,
    partition: &Partition,
    lv: &crate::core::filesystem::volumes::lvm::LvmLogicalVolume,
) -> OsDetectionPartition {
    if !supports_os_detect(lv.kind) {
        return OsDetectionPartition {
            partition_id: partition.id,
            partition_label: format!("LV {}/{} · {}", partition.id, lv.index, lv.name),
            partition_kind: format!("lvm:{}", kind_str(lv.kind)),
            filesystem_opened: false,
            error: None,
            detections: Vec::new(),
            bitlocker_locked: false,
        };
    }
    let fingerprint = Fingerprint {
        image: Arc::clone(image),
        root_id: format!("lvm:{}:{}", partition.id, lv.index),
    };
    match fs_tree::children_for_image(image, &fingerprint.root_id) {
        Ok(_) => {
            let detections = os_detect::detect(&fingerprint);
            OsDetectionPartition {
                partition_id: partition.id,
                partition_label: format!("LV {}/{} · {}", partition.id, lv.index, lv.name),
                partition_kind: format!("lvm:{}", kind_str(lv.kind)),
                filesystem_opened: true,
                error: None,
                detections,
                bitlocker_locked: false,
            }
        }
        Err(err) => OsDetectionPartition {
            partition_id: partition.id,
            partition_label: format!("LV {}/{} · {}", partition.id, lv.index, lv.name),
            partition_kind: format!("lvm:{}", kind_str(lv.kind)),
            filesystem_opened: false,
            error: Some(err.to_string()),
            detections: Vec::new(),
            bitlocker_locked: false,
        },
    }
}

fn unsupported_partition_entry(partition: &Partition) -> OsDetectionPartition {
    OsDetectionPartition {
        partition_id: partition.id,
        partition_label: partition_label(partition),
        partition_kind: kind_str(partition.kind).to_string(),
        filesystem_opened: false,
        error: None,
        detections: Vec::new(),
        bitlocker_locked: false,
    }
}

fn partition_label(partition: &Partition) -> String {
    if let Some(label) = partition.label.as_ref() {
        return label.clone();
    }
    format!(
        "Partition {} · {} · LBA {}",
        partition.id,
        kind_str(partition.kind),
        partition.start_lba
    )
}

/// Filesystems whose `fs_tree::children_for_image` walk + path-component
/// matching reaches the kind of OS-fingerprint files we look for.
pub(crate) fn supports_os_detect(kind: PartitionKind) -> bool {
    matches!(
        kind,
        PartitionKind::Ntfs
            | PartitionKind::Refs
            | PartitionKind::Ext4
            | PartitionKind::Btrfs
            | PartitionKind::Fat32
            | PartitionKind::Fat12
            | PartitionKind::Fat16
            | PartitionKind::ExFat
            | PartitionKind::Cdfs
            | PartitionKind::Squashfs
            | PartitionKind::ApfsContainer
            | PartitionKind::HfsPlus
            | PartitionKind::HfsX
            | PartitionKind::Ufs
            | PartitionKind::Xfs
            | PartitionKind::Bitlocker
    )
}

/// Stable lowercase identifier mirroring the `Partition` IPC field used
/// throughout the rest of the codebase. Kept here so changes to one
/// don't drift away from the other.
pub(crate) fn kind_str(kind: PartitionKind) -> &'static str {
    match kind {
        PartitionKind::Ntfs => "ntfs",
        PartitionKind::Refs => "refs",
        PartitionKind::Ext4 => "ext4",
        PartitionKind::Btrfs => "btrfs",
        PartitionKind::Lvm => "lvm",
        PartitionKind::Fat32 => "fat32",
        PartitionKind::Fat12 => "fat12",
        PartitionKind::Fat16 => "fat16",
        PartitionKind::ExFat => "exfat",
        PartitionKind::Cdfs => "cdfs",
        PartitionKind::Squashfs => "squashfs",
        PartitionKind::Ubi => "ubi",
        PartitionKind::Bitlocker => "bitlocker",
        PartitionKind::BiosBoot => "biosboot",
        PartitionKind::ApfsContainer => "apfscontainer",
        PartitionKind::HfsPlus => "hfsplus",
        PartitionKind::HfsX => "hfsx",
        PartitionKind::Luks => "luks",
        PartitionKind::Filevault => "filevault",
        PartitionKind::Apm => "apm",
        PartitionKind::Bsd => "bsd",
        PartitionKind::Swap => "swap",
        PartitionKind::Extended => "extended",
        PartitionKind::Mdadm => "mdadm",
        PartitionKind::Ufs => "ufs",
        PartitionKind::Xfs => "xfs",
        PartitionKind::Zfs => "zfs",
        PartitionKind::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::analysis::identify::os_detect::OsFamily;

    #[test]
    fn supports_table_matches_kind_str_table() {
        // Sanity check: every kind we claim to support has a stable
        // string label. Catches drift if PartitionKind grows a new
        // variant and someone forgets to extend either table.
        let supported = [
            PartitionKind::Ntfs,
            PartitionKind::Ext4,
            PartitionKind::Btrfs,
            PartitionKind::Fat32,
            PartitionKind::Fat12,
            PartitionKind::Fat16,
            PartitionKind::ExFat,
            PartitionKind::Cdfs,
            PartitionKind::Squashfs,
            PartitionKind::ApfsContainer,
            PartitionKind::HfsPlus,
            PartitionKind::HfsX,
            PartitionKind::Ufs,
            PartitionKind::Xfs,
            PartitionKind::Bitlocker,
        ];
        for kind in supported {
            assert!(supports_os_detect(kind), "{kind:?} should be supported");
            assert_ne!(kind_str(kind), "unknown", "{kind:?} should have a label");
        }
    }

    /// Env-gated regression guard for the SOFTWARE-hive-truncation bug:
    /// set `LOVELYCARVE_OS_DETECT_WIN_TEST` to a raw/dd/E01 image of a
    /// real Windows install and the test asserts at least one partition
    /// detects family `Windows`. The original bug — a 4 MiB read cap —
    /// silently broke this on any system whose hive's `CurrentVersion`
    /// cell sat past the cap. Reproduce the pre-fix behaviour by
    /// dropping `READ_CAP_BYTES` back to `4 * 1024 * 1024`.
    #[test]
    fn detects_windows_on_real_image_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_OS_DETECT_WIN_TEST") else {
            return;
        };
        let image = std::sync::Arc::new(
            crate::core::filesystem::evidence_session::EvidenceSession::open(path)
                .expect("Windows test image should open"),
        );
        let partitions = detect_image(&image).expect("detect_image must succeed");
        let windows_hit = partitions
            .iter()
            .flat_map(|p| p.detections.iter())
            .find(|d| d.family == OsFamily::Windows);
        assert!(
            windows_hit.is_some(),
            "expected at least one partition to be identified as Windows; \
             got: {partitions:#?}"
        );
    }
}
