//! Register matching Fusion members as bounded slices of one evidence source.

use super::*;
use crate::core::evidence::partitions::apfs::PartitionCursor;
use apfs::fusion::validate_fusion_pair;
use apfs::superblock::NxSuperblock;
use apfs::{ApfsVolume, FusionTier};
use std::collections::HashSet;

struct Member {
    image: Arc<DiskImage>,
    partition_id: u8,
    cursor: PartitionCursor,
    header: NxSuperblock,
    tier: FusionTier,
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
    for image in images.iter().filter(|image| is_block_evidence(image)) {
        if consumed.contains(image.id.as_str()) {
            continue;
        }
        for partition in image
            .partitions
            .iter()
            .filter(|part| part.kind == PartitionKind::ApfsContainer)
        {
            let mut cursor = match PartitionCursor::new(Arc::clone(image), partition) {
                Ok(cursor) => cursor,
                Err(error) => {
                    diagnostics.push(format!(
                        "APFS member {}:{}: {error:#}",
                        image.id, partition.id
                    ));
                    continue;
                }
            };
            let Ok(header) = cursor.superblock() else {
                continue;
            };
            let Some((uuid, tier)) = header.fusion_member() else {
                continue;
            };
            groups.entry(uuid).or_default().push(Member {
                image: Arc::clone(image),
                partition_id: partition.id,
                cursor,
                header,
                tier,
            });
        }
    }
    for (uuid, members) in groups {
        consumed.extend(members.iter().map(|member| member.image.id.to_string()));
        match assemble_pair(uuid, members) {
            Ok((array, filesystem, image)) => {
                arrays.push(array);
                filesystems.push(filesystem);
                assembled.push(image);
            }
            Err(error) => diagnostics.push(format!("APFS Fusion {}: {error:#}", hex::encode(uuid))),
        }
    }
}

fn assemble_pair(
    uuid: [u8; 16],
    members: Vec<Member>,
) -> anyhow::Result<(RaidArray, RaidFilesystem, AssembledArray)> {
    let [first, second]: [Member; 2] = members.try_into().map_err(|members: Vec<Member>| {
        anyhow::anyhow!("requires exactly two members, found {}", members.len())
    })?;
    let (main, tier2) = if first.tier == FusionTier::Main {
        (first, second)
    } else {
        (second, first)
    };
    validate_fusion_pair(
        &main.header,
        main.cursor.length_bytes,
        &tier2.header,
        tier2.cursor.length_bytes,
    )?;
    let fsid = hex::encode(uuid);
    let set_name = format!("APFS Fusion {fsid}");
    let main_origin = member_origin(&main, 0);
    let tier2_origin = member_origin(&tier2, 1);
    let mut start = 0u64;
    let mut reader = Vec::with_capacity(2);
    let mut partitions = Vec::with_capacity(2);
    for (id, member) in [(0, &main), (1, &tier2)] {
        let length = member.cursor.length_bytes;
        let sectors = length / 512;
        partitions.push(Partition {
            id,
            kind: PartitionKind::ApfsContainer,
            start_lba: start,
            length_lba: sectors,
            label: Some(if id == 0 {
                "APFS Fusion main".into()
            } else {
                "APFS Fusion tier 2".into()
            }),
            unique_guid: None,
            amiga_geometry: None,
            member_of: None,
        });
        start = start
            .checked_add(sectors)
            .context("APFS Fusion partition span overflow")?;
        reader.push(Some(StripedMember {
            reader: Arc::new(WholeDiskMember {
                image: Arc::clone(&member.image),
                byte_offset: member.cursor.base_offset,
                byte_length: length,
            }),
            data_offset_bytes: 0,
            data_size_bytes: length,
        }));
    }
    let info = ApfsVolume::container_source_info(main.cursor, Some(tier2.cursor))?;
    let set_uuid = format!("apfs-fusion:{fsid}:{}", info.transaction_id);
    let source = Arc::new(source::ConcatVolume::new(reader, 0)?);
    let size_bytes = source.size_bytes();
    let array = RaidArray {
        set_uuid: set_uuid.clone(),
        set_name: set_name.clone(),
        level: -1,
        layout: 0,
        chunksize_sectors: 0,
        raid_disks: 2,
        data_offset_sectors: 0,
        assembled_bytes: size_bytes,
        members: vec![main_origin, tier2_origin],
        assembled_image_id: None,
        heuristic: false,
        metadata_version: "apfs-fusion".into(),
        degraded: false,
        degradable: false,
    };
    let filesystem = RaidFilesystem {
        set_uuid: set_uuid.clone(),
        kind: "apfs",
        fsid: Some(hex::encode(info.uuid)),
        total_bytes: Some(info.size),
        bytes_used: None,
        num_devices: Some(2),
        sector_size: None,
        node_size: Some(info.block_size),
    };
    let image = AssembledArray {
        label: set_uuid.clone(),
        set_uuid,
        set_name,
        target: AssembledTarget::Array,
        source,
        size_bytes,
        fs_kind: PartitionKind::ApfsContainer,
        partitions: Some(partitions),
    };
    Ok((array, filesystem, image))
}

fn member_origin(member: &Member, id: u32) -> RaidArrayMember {
    RaidArrayMember {
        dev_number: id,
        slot: Some(id),
        device_uuid: hex::encode(member.header.fusion_uuid),
        data_size_sectors: member.cursor.length_bytes / 512,
        reconstructed: false,
        state: RaidMemberState::Active,
        events: Some(member.header.header.xid),
        origin: RaidMemberOrigin::Whole {
            image_id: member.image.id.to_string(),
            partition_id: member.partition_id,
            byte_offset: member.cursor.base_offset,
            byte_length: member.cursor.length_bytes,
        },
    }
}
