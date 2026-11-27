//! Assemble OpenZFS pool members without substituting a linear disk for DVA mapping.

use super::*;
use crate::core::evidence::partitions::zfs::PartitionSource;
use std::collections::HashSet;
use zfs_rs::{label::Label, pool::PoolLayout};

struct Member {
    image: Arc<DiskImage>,
    partition_id: u8,
    source: PartitionSource,
    label: Label,
}

pub(super) fn assemble(
    images: &[Arc<DiskImage>],
    consumed: &mut HashSet<String>,
    arrays: &mut Vec<RaidArray>,
    filesystems: &mut Vec<RaidFilesystem>,
    assembled: &mut Vec<AssembledArray>,
    diagnostics: &mut Vec<String>,
) {
    let mut pools: BTreeMap<u64, Vec<Member>> = BTreeMap::new();
    for image in images.iter().filter(|image| is_block_evidence(image)) {
        if consumed.contains(image.id.as_str()) {
            continue;
        }
        for partition in image
            .partitions
            .iter()
            .filter(|part| part.kind == PartitionKind::Zfs)
        {
            match read_member(image, partition) {
                Ok(member) => pools
                    .entry(member.label.identity.pool_guid)
                    .or_default()
                    .push(member),
                Err(error) => diagnostics.push(format!(
                    "ZFS member {}:{}: {error:#}",
                    image.id, partition.id
                )),
            }
        }
    }
    for (guid, mut members) in pools {
        members.sort_unstable_by_key(|member| member.label.identity.leaf_guid);
        let layout = match PoolLayout::from_labels(members.iter().map(|member| &member.label)) {
            Ok(layout) if layout.single_leaf() => continue,
            Ok(layout) => layout,
            Err(error) => {
                diagnostics.push(format!("ZFS pool {guid}: {error}"));
                consumed.extend(members.iter().map(|member| member.image.id.to_string()));
                continue;
            }
        };
        consumed.extend(members.iter().map(|member| member.image.id.to_string()));
        match assemble_pool(&layout, &members) {
            Ok((array, filesystem, image)) => {
                arrays.push(array);
                filesystems.push(filesystem);
                assembled.push(image);
            }
            Err(error) => diagnostics.push(format!("ZFS pool {guid}: {error:#}")),
        }
    }
}

fn read_member(image: &Arc<DiskImage>, partition: &Partition) -> anyhow::Result<Member> {
    let source = PartitionSource::new(image, partition)?;
    let label = source.label()?;
    Ok(Member {
        image: Arc::clone(image),
        partition_id: partition.id,
        source,
        label,
    })
}

fn assemble_pool(
    layout: &PoolLayout,
    members: &[Member],
) -> anyhow::Result<(RaidArray, RaidFilesystem, AssembledArray)> {
    let fsid = format!("{:016x}", layout.guid);
    let txg = layout.transaction();
    let set_uuid = format!("zfs:{fsid}:{txg}");
    let set_name = format!("ZFS {}", layout.name);
    let mut partitions = Vec::with_capacity(members.len());
    let mut readers = Vec::with_capacity(members.len());
    let mut origins = Vec::with_capacity(members.len());
    let mut start = 0u64;
    for (index, member) in members.iter().enumerate() {
        let id = u8::try_from(index).context("too many ZFS member partitions")?;
        let length = member.source.length;
        let sectors = length / 512;
        let leaf = format!("{:016x}", member.label.identity.leaf_guid);
        partitions.push(Partition {
            id,
            kind: PartitionKind::Zfs,
            start_lba: start,
            length_lba: sectors,
            label: Some(format!("ZFS leaf {leaf}")),
            unique_guid: None,
            amiga_geometry: None,
            member_of: None,
        });
        start = start
            .checked_add(sectors)
            .context("ZFS member span overflow")?;
        readers.push(Some(StripedMember {
            reader: Arc::new(WholeDiskMember {
                image: Arc::clone(&member.image),
                byte_offset: member.source.base,
                byte_length: length,
            }),
            data_offset_bytes: 0,
            data_size_bytes: length,
        }));
        origins.push(RaidArrayMember {
            dev_number: u32::from(id),
            slot: Some(u32::from(id)),
            device_uuid: leaf,
            data_size_sectors: sectors,
            reconstructed: false,
            state: RaidMemberState::Active,
            events: member.label.config.as_ref().map(|config| config.txg),
            origin: RaidMemberOrigin::Whole {
                image_id: member.image.id.to_string(),
                partition_id: member.partition_id,
                byte_offset: member.source.base,
                byte_length: length,
            },
        });
    }
    let source = Arc::new(source::ConcatVolume::new(readers, 0)?);
    let size_bytes = source.size_bytes();
    let array = RaidArray {
        set_uuid: set_uuid.clone(),
        set_name: set_name.clone(),
        level: -1,
        layout: 0,
        chunksize_sectors: 0,
        raid_disks: members.len() as u32,
        data_offset_sectors: 0,
        assembled_bytes: size_bytes,
        members: origins,
        assembled_image_id: None,
        heuristic: false,
        metadata_version: "zfs".into(),
        degraded: false,
        degradable: false,
    };
    let filesystem = RaidFilesystem {
        set_uuid: set_uuid.clone(),
        kind: "zfs",
        fsid: Some(fsid),
        total_bytes: None,
        bytes_used: None,
        num_devices: Some(members.len() as u64),
        sector_size: None,
        node_size: None,
    };
    let image = AssembledArray {
        label: set_uuid.clone(),
        set_uuid,
        set_name,
        target: AssembledTarget::Array,
        source,
        size_bytes,
        fs_kind: PartitionKind::Zfs,
        partitions: Some(partitions),
    };
    Ok((array, filesystem, image))
}
