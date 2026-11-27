//! Bind a complete bcachefs member set to one registered evidence source.

use super::*;
use crate::core::evidence::partitions::bcachefs::{read_member, DeviceHeader};
use std::collections::HashSet;

struct Member {
    image: Arc<DiskImage>,
    partition_id: u8,
    header: DeviceHeader,
}

pub(super) fn assemble(
    images: &[Arc<DiskImage>],
    consumed: &mut HashSet<String>,
    arrays: &mut Vec<RaidArray>,
    filesystems: &mut Vec<RaidFilesystem>,
    assembled: &mut Vec<AssembledArray>,
    diagnostics: &mut Vec<String>,
) {
    let mut groups: BTreeMap<[u8; 16], Vec<Member>> = BTreeMap::new();
    for image in images {
        if !is_block_evidence(image) || consumed.contains(image.id.as_str()) {
            continue;
        }
        for partition in image
            .partitions
            .iter()
            .filter(|part| part.kind == PartitionKind::Bcachefs)
        {
            match read_member(image, partition) {
                Ok(header) if header.superblock.nr_devices > 1 => {
                    consumed.insert(image.id.to_string());
                    groups
                        .entry(header.superblock.uuid)
                        .or_default()
                        .push(Member {
                            image: Arc::clone(image),
                            partition_id: partition.id,
                            header,
                        });
                }
                Ok(_) => {}
                Err(error) => diagnostics.push(format!(
                    "bcachefs member {}:{}: {error:#}",
                    image.id, partition.id
                )),
            }
        }
    }
    for (uuid, mut members) in groups {
        members.sort_unstable_by_key(|member| member.header.superblock.dev_idx);
        match assemble_members(uuid, &members) {
            Ok((array, filesystem, image)) => {
                arrays.push(array);
                filesystems.push(filesystem);
                assembled.push(image);
            }
            Err(error) => diagnostics.push(format!("bcachefs {}: {error:#}", hex::encode(uuid))),
        }
    }
}

fn assemble_members(
    uuid: [u8; 16],
    members: &[Member],
) -> anyhow::Result<(RaidArray, RaidFilesystem, AssembledArray)> {
    let headers: Vec<_> = members
        .iter()
        .map(|member| member.header.superblock.clone())
        .collect();
    let primary = &headers[bcachefs_reader::select_superblock(&headers)?];
    let fsid = hex::encode(uuid);
    let set_uuid = format!("bcachefs:{fsid}:{}", primary.seq);
    let set_name = format!("bcachefs {fsid}");
    let mut partitions = Vec::with_capacity(members.len());
    let mut readers = Vec::with_capacity(members.len());
    let mut origins = Vec::with_capacity(members.len());
    let mut partition_start = 0u64;
    for (slot, member) in members.iter().enumerate() {
        let DeviceHeader {
            base,
            length,
            uuid,
            superblock,
        } = &member.header;
        let sectors = length / 512;
        partitions.push(Partition {
            id: superblock.dev_idx,
            kind: PartitionKind::Bcachefs,
            start_lba: partition_start,
            length_lba: sectors,
            label: Some(format!("bcachefs device {}", superblock.dev_idx)),
            unique_guid: None,
            amiga_geometry: None,
            member_of: None,
        });
        partition_start = partition_start
            .checked_add(sectors)
            .context("bcachefs array length overflow")?;
        readers.push(Some(StripedMember {
            reader: Arc::new(WholeDiskMember {
                image: Arc::clone(&member.image),
                byte_offset: *base,
                byte_length: *length,
            }),
            data_offset_bytes: 0,
            data_size_bytes: *length,
        }));
        origins.push(RaidArrayMember {
            dev_number: u32::from(superblock.dev_idx),
            slot: Some(slot as u32),
            device_uuid: hex::encode(uuid),
            data_size_sectors: sectors,
            reconstructed: false,
            state: RaidMemberState::Active,
            events: Some(superblock.seq),
            origin: RaidMemberOrigin::Whole {
                image_id: member.image.id.to_string(),
                partition_id: member.partition_id,
                byte_offset: *base,
                byte_length: *length,
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
        metadata_version: "bcachefs".into(),
        degraded: false,
        degradable: false,
    };
    let fs = RaidFilesystem {
        set_uuid: set_uuid.clone(),
        kind: "bcachefs",
        fsid: Some(fsid),
        total_bytes: Some(size_bytes),
        bytes_used: None,
        num_devices: Some(members.len() as u64),
        sector_size: Some(u32::from(primary.block_size) * 512),
        node_size: Some(primary.btree_node_size() * 512),
    };
    let image = AssembledArray {
        label: set_uuid.clone(),
        set_uuid,
        set_name,
        target: AssembledTarget::Array,
        source,
        size_bytes,
        fs_kind: PartitionKind::Bcachefs,
        partitions: Some(partitions),
    };
    Ok((array, fs, image))
}
