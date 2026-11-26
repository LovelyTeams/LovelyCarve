//! Bind on-disk device IDs to bounded ranges of the evidence byte source.
use super::*;
use crate::core::evidence::partitions::btrfs::{
    read_member_superblock, superblock_offset, MemberHeader,
};
use crate::core::evidence::partitions::lvm2::layout::LinearLayout;
use crate::core::evidence::partitions::PartitionKind;
use crate::core::filesystem::volumes::lvm::read_backing_range;
use anyhow::ensure;

pub(super) struct DeviceSet {
    members: BTreeMap<u64, (MemberHeader, u64)>,
}

impl DeviceSet {
    pub(super) fn open(
        image: &DiskImage,
        layout: Option<&LinearLayout>,
        source: BtrfsSource,
        base: u64,
        length: u64,
    ) -> anyhow::Result<(Vec<u8>, Self)> {
        let offset = superblock_offset(base, length)?;
        let bytes = read_backing_range(image, layout, offset, 4096)?;
        let primary = MemberHeader::parse(&bytes, length)?;
        let mut members = BTreeMap::from([(primary.devid, (primary, base))]);
        if primary.num_devices > 1 {
            let BtrfsSource::Partition(partition_id) = source else {
                bail!(Failure::unsupported(
                    "btrfs: multi-device logical volume requires a grouped source"
                ));
            };
            for partition in image.partitions.iter().filter(|partition| {
                partition.id != partition_id && partition.kind == PartitionKind::Btrfs
            }) {
                let sector_size = image.sector_size as u64;
                let base = partition
                    .start_lba
                    .checked_mul(sector_size)
                    .context(Failure::corrupt("btrfs: member start overflow"))?;
                let length = partition
                    .length_lba
                    .checked_mul(sector_size)
                    .context(Failure::corrupt("btrfs: member size overflow"))?
                    .min(image.len().saturating_sub(base));
                let Ok(superblock) = read_member_superblock(image, base, length) else {
                    continue;
                };
                if !MemberHeader::identifies(&superblock, primary.fsid) {
                    continue;
                }
                let candidate = MemberHeader::parse(&superblock, length)?;
                ensure!(
                    candidate.same_commit(primary),
                    Failure::corrupt("btrfs: members have different committed trees")
                );
                ensure!(
                    !members.contains_key(&candidate.devid)
                        && members
                            .values()
                            .all(|(member, _)| member.uuid != candidate.uuid),
                    Failure::corrupt("btrfs: duplicate member device ID or UUID")
                );
                members.insert(candidate.devid, (candidate, base));
            }
            ensure!(
                members.len() as u64 == primary.num_devices,
                Failure::unsupported(format!(
                    "btrfs: {} of {} devices available",
                    members.len(),
                    primary.num_devices
                ))
            );
        }
        Ok((bytes, Self { members }))
    }

    pub(super) fn resolve(&self, stripe: &Stripe, length: u64) -> anyhow::Result<u64> {
        let devid = stripe.devid.get();
        let &(member, base) = self.members.get(&devid).with_context(|| {
            Failure::corrupt(format!("btrfs: chunk references unknown device {devid}"))
        })?;
        ensure!(
            member.uuid == stripe.dev_uuid,
            Failure::corrupt("btrfs: stripe device UUID mismatch")
        );
        let offset = stripe.offset.get();
        ensure!(
            offset
                .checked_add(length)
                .is_some_and(|end| end <= member.device_bytes),
            Failure::corrupt("btrfs: stripe allocation leaves its device")
        );
        base.checked_add(offset)
            .filter(|start| start.checked_add(length).is_some())
            .context(Failure::corrupt("btrfs: stripe physical offset overflow"))
    }
}
