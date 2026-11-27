//! Multi-device filesystems whose member devices are partitions of one image.
//!
//! The Btrfs, bcachefs, ZFS, APFS Fusion and VMFS readers gather every member
//! device a filesystem has among the image's partitions, whichever member
//! they start from, so each member would present the same tree. The first
//! member in table order is the filesystem's volume. Every later member
//! records that partition in [`Partition::member_of`]: it stays listed with
//! its geometry for byte access, but is neither browsed, indexed, searched
//! nor mounted on its own.

use std::collections::hash_map::{Entry, HashMap};

use anyhow::Context;

use super::{
    apfs, bcachefs, btrfs, vmfs, zfs, Partition, PartitionKind, PartitionSlice, SectorSource,
};

/// What the member devices of one filesystem share, as its reader matches
/// sibling partitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum SetIdentity {
    /// Filesystem UUID of a Btrfs spanning more than one device.
    Btrfs([u8; 16]),
    /// External UUID of a bcachefs spanning more than one device.
    Bcachefs([u8; 16]),
    /// Pool GUID of a ZFS leaf device.
    Zfs(u64),
    /// Fusion set UUID of an APFS container tier.
    ApfsFusion([u8; 16]),
    /// Volume UUID and snapshot ID of an extent of a spanned VMFS datastore.
    Vmfs([u8; 16], u32),
}

/// Point every later member device of a multi-device filesystem at the first
/// member partition, which opens it. A partition whose header cannot be read
/// stays a volume of its own, as its reader would report the same failure.
pub fn mark_members(
    source: &(dyn SectorSource + Sync),
    partitions: &mut [Partition],
    sector_size: usize,
) {
    let mut first_members: HashMap<SetIdentity, u8> = HashMap::new();
    for index in 0..partitions.len() {
        let partition = &partitions[index];
        // A member device needs a sibling of its kind; skip the reads otherwise.
        let shares_kind = partitions
            .iter()
            .enumerate()
            .any(|(other, candidate)| other != index && candidate.kind == partition.kind);
        if !shares_kind {
            continue;
        }
        let id = partition.id;
        let identity = match member_identity(source, partition, sector_size) {
            Ok(Some(identity)) => identity,
            Ok(None) => continue,
            Err(error) => {
                tracing::debug!(
                    partition = id,
                    "multi-device member header unreadable: {error:#}"
                );
                continue;
            }
        };
        match first_members.entry(identity) {
            Entry::Occupied(first) => partitions[index].member_of = Some(*first.get()),
            Entry::Vacant(slot) => {
                slot.insert(id);
            }
        }
    }
}

/// The identity `partition` shares with the other member devices of its
/// filesystem, or `None` when the filesystem has a single device.
fn member_identity(
    source: &(dyn SectorSource + Sync),
    partition: &Partition,
    sector_size: usize,
) -> anyhow::Result<Option<SetIdentity>> {
    let sector = sector_size as u64;
    let base = partition
        .start_lba
        .checked_mul(sector)
        .context("member device offset overflow")?;
    let length = partition
        .length_lba
        .checked_mul(sector)
        .context("member device length overflow")?
        .min(source.size().saturating_sub(base));
    let device = PartitionSlice::new(source, base, length);
    Ok(match partition.kind {
        PartitionKind::Btrfs => {
            let header = btrfs::read_member(source, base, length)?;
            (header.num_devices > 1).then_some(SetIdentity::Btrfs(header.fsid))
        }
        PartitionKind::Bcachefs => {
            let header = bcachefs::read_member_at(source, partition, sector_size)?;
            (header.superblock.nr_devices > 1)
                .then_some(SetIdentity::Bcachefs(header.superblock.uuid))
        }
        PartitionKind::Zfs => Some(SetIdentity::Zfs(zfs::member_pool(&device)?)),
        PartitionKind::ApfsContainer => apfs::fusion_set(&device)?.map(SetIdentity::ApfsFusion),
        PartitionKind::Vmfs => vmfs::spanned_datastore(&device)?
            .map(|(volume, snapshot)| SetIdentity::Vmfs(volume, snapshot)),
        PartitionKind::Ntfs
        | PartitionKind::Refs
        | PartitionKind::Ext4
        | PartitionKind::Fat32
        | PartitionKind::Fat12
        | PartitionKind::Fat16
        | PartitionKind::ExFat
        | PartitionKind::Cdfs
        | PartitionKind::Squashfs
        | PartitionKind::Cramfs
        | PartitionKind::Ubifs
        | PartitionKind::Jffs2
        | PartitionKind::Yaffs2
        | PartitionKind::Yaffs1
        | PartitionKind::Erofs
        | PartitionKind::F2fs
        | PartitionKind::Qnx4
        | PartitionKind::Qnx6
        | PartitionKind::QnxEtfs
        | PartitionKind::QnxEfs
        | PartitionKind::Udf
        | PartitionKind::Hfs
        | PartitionKind::Mfs
        | PartitionKind::Littlefs
        | PartitionKind::Spiffs
        | PartitionKind::Hammer
        | PartitionKind::Hammer2
        | PartitionKind::Jfs
        | PartitionKind::Reiserfs
        | PartitionKind::Reiser4
        | PartitionKind::Amiga
        | PartitionKind::Befs
        | PartitionKind::Gfs1
        | PartitionKind::Gfs2
        | PartitionKind::Ocfs1
        | PartitionKind::Ocfs2
        | PartitionKind::Nilfs2
        | PartitionKind::Hpfs
        | PartitionKind::Vxfs
        | PartitionKind::AixJfs
        | PartitionKind::UnixwareBfs
        | PartitionKind::Xdvdfs
        | PartitionKind::Fatx
        | PartitionKind::Minix
        | PartitionKind::Minifs
        | PartitionKind::Romfs
        | PartitionKind::Ubi
        | PartitionKind::HfsPlus
        | PartitionKind::HfsX
        | PartitionKind::Ufs
        | PartitionKind::Xfs
        | PartitionKind::Lvm
        | PartitionKind::Bitlocker
        | PartitionKind::Filevault
        | PartitionKind::Luks
        | PartitionKind::Mdadm
        | PartitionKind::Extended
        | PartitionKind::Apm
        | PartitionKind::Bsd
        | PartitionKind::Swap
        | PartitionKind::BiosBoot
        | PartitionKind::Unknown => None,
    })
}

#[cfg(test)]
#[path = "device_sets_tests.rs"]
mod tests;
