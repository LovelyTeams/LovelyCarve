//! Group complete native Btrfs filesystems before metadata-less RAID probing.
use super::*;
use crate::core::common::error::Failure;
use crate::core::evidence::partitions::btrfs::{read_member, MemberHeader};
use anyhow::ensure;
use std::collections::{BTreeSet, HashSet};

struct Member {
    image: Arc<DiskImage>,
    partition_id: u8,
    base: u64,
    header: MemberHeader,
}

pub(super) fn assemble(
    images: &[Arc<DiskImage>],
    consumed: &mut HashSet<String>,
    arrays: &mut Vec<RaidArray>,
    filesystems: &mut Vec<RaidFilesystem>,
    assembled: &mut Vec<AssembledArray>,
    diagnostics: &mut Vec<String>,
) {
    let mut sets: BTreeMap<[u8; 16], Vec<Member>> = BTreeMap::new();
    for image in images {
        if !is_block_evidence(image) || consumed.contains(image.id.as_str()) {
            continue;
        }
        for partition in image
            .partitions
            .iter()
            .filter(|part| part.kind == PartitionKind::Btrfs)
        {
            let sector_size = image.sector_size as u64;
            let Some(base) = partition.start_lba.checked_mul(sector_size) else {
                continue;
            };
            let Some(length) = partition.length_lba.checked_mul(sector_size) else {
                continue;
            };
            let length = length.min(image.len().saturating_sub(base));
            match read_member(image.as_ref(), base, length) {
                Ok(header) if header.num_devices > 1 => {
                    consumed.insert(image.id.to_string());
                    sets.entry(header.fsid).or_default().push(Member {
                        image: Arc::clone(image),
                        partition_id: partition.id,
                        base,
                        header,
                    });
                }
                Ok(_) => {}
                Err(error) => diagnostics.push(format!(
                    "btrfs member {}:{}: {error:#}",
                    image.id, partition.id
                )),
            }
        }
    }
    for (fsid, mut members) in sets {
        members.sort_unstable_by_key(|member| member.header.devid);
        match assemble_set(fsid, &members) {
            Ok((array, filesystem, image)) => {
                arrays.push(array);
                filesystems.push(filesystem);
                assembled.push(image);
            }
            Err(error) => diagnostics.push(format!("btrfs {}: {error:#}", hex::encode(fsid))),
        }
    }
}

fn assemble_set(
    fsid: [u8; 16],
    members: &[Member],
) -> anyhow::Result<(RaidArray, RaidFilesystem, AssembledArray)> {
    let primary = members[0].header;
    let mut seen_ids = BTreeSet::new();
    let mut seen_uuids = BTreeSet::new();
    for member in members {
        ensure!(
            member.header.same_commit(primary),
            Failure::corrupt("Btrfs members have different committed trees")
        );
        ensure!(
            seen_ids.insert(member.header.devid) && seen_uuids.insert(member.header.uuid),
            Failure::corrupt("duplicate Btrfs member ID or UUID")
        );
        ensure!(
            member.header.device_bytes.is_multiple_of(512),
            Failure::corrupt("Btrfs member size is not sector-aligned")
        );
    }
    ensure!(
        members.len() as u64 == primary.num_devices,
        Failure::unsupported(format!(
            "Btrfs needs all {} devices; {} available",
            primary.num_devices,
            members.len()
        ))
    );

    let fsid = hex::encode(fsid);
    let set_uuid = format!("btrfs:{fsid}:{}", primary.generation);
    let label = set_uuid.clone();
    let mut partition_start = 0u64;
    let mut partitions = Vec::with_capacity(members.len());
    let mut readers = Vec::with_capacity(members.len());
    let mut origins = Vec::with_capacity(members.len());
    for (index, member) in members.iter().enumerate() {
        let length = member.header.device_bytes;
        let sectors = length / 512;
        partitions.push(Partition {
            id: u8::try_from(index).context("too many Btrfs members")?,
            kind: PartitionKind::Btrfs,
            start_lba: partition_start,
            length_lba: sectors,
            label: Some(format!("Btrfs device {}", member.header.devid)),
            unique_guid: None,
            amiga_geometry: None,
            member_of: None,
        });
        partition_start = partition_start
            .checked_add(sectors)
            .context(Failure::corrupt("Btrfs grouped device size overflow"))?;
        readers.push(Some(StripedMember {
            reader: Arc::new(WholeDiskMember {
                image: Arc::clone(&member.image),
                byte_offset: member.base,
                byte_length: length,
            }),
            data_offset_bytes: 0,
            data_size_bytes: length,
        }));
        origins.push(RaidArrayMember {
            dev_number: index as u32,
            slot: Some(index as u32),
            device_uuid: hex::encode(member.header.uuid),
            data_size_sectors: sectors,
            reconstructed: false,
            state: RaidMemberState::Active,
            events: None,
            origin: RaidMemberOrigin::Whole {
                image_id: member.image.id.to_string(),
                partition_id: member.partition_id,
                byte_offset: member.base,
                byte_length: length,
            },
        });
    }
    // The synthetic evidence's raw geometry is a concatenation of device
    // ranges. The filesystem still resolves each chunk's own RAID profile.
    let source = Arc::new(source::ConcatVolume::new(readers, 0)?);
    let size_bytes = source.size_bytes();
    let array = RaidArray {
        set_uuid: set_uuid.clone(),
        set_name: format!("Btrfs {fsid}"),
        level: -1,
        layout: 0,
        chunksize_sectors: 0,
        raid_disks: members.len() as u32,
        data_offset_sectors: 0,
        assembled_bytes: size_bytes,
        members: origins,
        assembled_image_id: None,
        heuristic: false,
        metadata_version: "btrfs".to_owned(),
        degraded: false,
        degradable: false,
    };
    let fs = RaidFilesystem {
        set_uuid: set_uuid.clone(),
        kind: "btrfs",
        fsid: Some(fsid),
        total_bytes: Some(size_bytes),
        bytes_used: None,
        num_devices: Some(primary.num_devices),
        sector_size: None,
        node_size: None,
    };
    let image = AssembledArray {
        set_uuid,
        set_name: array.set_name.clone(),
        label,
        target: AssembledTarget::Array,
        source,
        size_bytes,
        fs_kind: PartitionKind::Btrfs,
        partitions: Some(partitions),
    };
    Ok((array, fs, image))
}
