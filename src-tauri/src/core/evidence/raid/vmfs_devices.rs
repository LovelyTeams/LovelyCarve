//! Register a complete VMware datastore with each original extent member bounded.

use super::*;
use crate::core::evidence::partitions::partition_cursor::PartitionCursor;
use std::collections::HashSet;
use vmfs_reader::{Filesystem, MemberInfo, Volume};

struct Member {
    image: Arc<DiskImage>,
    partition_id: u8,
    cursor: PartitionCursor,
    header: MemberInfo,
}

pub(super) fn assemble(
    images: &[Arc<DiskImage>],
    consumed: &mut HashSet<String>,
    arrays: &mut Vec<RaidArray>,
    filesystems: &mut Vec<RaidFilesystem>,
    assembled: &mut Vec<AssembledArray>,
    diagnostics: &mut Vec<String>,
) {
    let mut groups: BTreeMap<([u8; 16], u32), Vec<Member>> = BTreeMap::new();
    for image in images.iter().filter(|image| is_block_evidence(image)) {
        if consumed.contains(image.id.as_str()) {
            continue;
        }
        for part in image
            .partitions
            .iter()
            .filter(|part| part.kind == PartitionKind::Vmfs)
        {
            let result = PartitionCursor::new(Arc::clone(image), part).and_then(|mut cursor| {
                Ok(vmfs_reader::probe_member(&mut cursor)?.map(|header| (cursor, header)))
            });
            match result {
                Ok(Some((cursor, header))) if header.member_count > 1 => {
                    groups
                        .entry((header.volume_uuid, header.snapshot_id))
                        .or_default()
                        .push(Member {
                            image: Arc::clone(image),
                            partition_id: part.id,
                            cursor,
                            header,
                        });
                }
                Ok(_) => {}
                Err(error) => {
                    diagnostics.push(format!("VMFS member {}:{}: {error:#}", image.id, part.id))
                }
            }
        }
    }
    for ((uuid, snapshot), mut members) in groups {
        consumed.extend(members.iter().map(|member| member.image.id.to_string()));
        members.sort_unstable_by_key(|member| member.header.first_extent);
        match assemble_datastore(uuid, snapshot, members) {
            Ok((array, filesystem, image)) => {
                arrays.push(array);
                filesystems.push(filesystem);
                assembled.push(image);
            }
            Err(error) => diagnostics.push(format!(
                "VMFS {} snapshot {snapshot}: {error:#}",
                hex::encode(uuid)
            )),
        }
    }
}

fn assemble_datastore(
    uuid: [u8; 16],
    snapshot: u32,
    members: Vec<Member>,
) -> anyhow::Result<(RaidArray, RaidFilesystem, AssembledArray)> {
    let fsid = hex::encode(uuid);
    let mut partitions = Vec::with_capacity(members.len());
    let mut readers = Vec::with_capacity(members.len());
    let mut origins = Vec::with_capacity(members.len());
    let mut cursors = Vec::with_capacity(members.len());
    let mut start = 0u64;
    for (slot, member) in members.into_iter().enumerate() {
        let slot = u8::try_from(slot).context("too many VMFS members")?;
        let length = member.cursor.length_bytes;
        let base = member.cursor.base_offset;
        let sectors = length / 512;
        anyhow::ensure!(
            length.is_multiple_of(512),
            "VMFS member slice is not sector aligned"
        );
        partitions.push(Partition {
            id: slot,
            kind: PartitionKind::Vmfs,
            start_lba: start,
            length_lba: sectors,
            label: Some(format!("VMFS extent {}", member.header.first_extent)),
            unique_guid: None,
            amiga_geometry: None,
        });
        start = start
            .checked_add(sectors)
            .context("VMFS member slices overflow")?;
        origins.push(RaidArrayMember {
            dev_number: u32::from(slot),
            slot: Some(u32::from(slot)),
            device_uuid: hex::encode(member.header.member_uuid),
            data_size_sectors: sectors,
            reconstructed: false,
            state: RaidMemberState::Active,
            events: Some(member.header.volume_generation),
            origin: RaidMemberOrigin::Whole {
                image_id: member.image.id.to_string(),
                partition_id: member.partition_id,
                byte_offset: base,
                byte_length: length,
            },
        });
        readers.push(Some(StripedMember {
            reader: Arc::new(WholeDiskMember {
                image: member.image,
                byte_offset: base,
                byte_length: length,
            }),
            data_offset_bytes: 0,
            data_size_bytes: length,
        }));
        cursors.push(member.cursor);
    }
    let volume = Volume::open_members(cursors)?;
    let logical_bytes = volume.size();
    let filesystem = Filesystem::open(volume)?;
    let geometry = filesystem.geometry();
    let version = match geometry.version {
        vmfs_reader::Version::Vmfs5 => 5,
        vmfs_reader::Version::Vmfs6 => 6,
    };
    let generation = origins[0].events.context("missing VMFS generation")?;
    let set_uuid = format!("vmfs{version}:{fsid}:{snapshot}:{generation}");
    let set_name = if geometry.label.is_empty() {
        format!("VMFS {fsid}")
    } else {
        geometry.label.clone()
    };
    let source = Arc::new(source::ConcatVolume::new(readers, 0)?);
    let size_bytes = source.size_bytes();
    let devices = origins.len() as u32;
    let array = RaidArray {
        set_uuid: set_uuid.clone(),
        set_name: set_name.clone(),
        level: -1,
        layout: 0,
        chunksize_sectors: 0,
        raid_disks: devices,
        data_offset_sectors: 0,
        assembled_bytes: size_bytes,
        members: origins,
        assembled_image_id: None,
        heuristic: false,
        metadata_version: format!("vmfs{version}"),
        degraded: false,
        degradable: false,
    };
    let filesystem = RaidFilesystem {
        set_uuid: set_uuid.clone(),
        kind: "vmfs".into(),
        fsid: Some(fsid),
        total_bytes: Some(logical_bytes),
        bytes_used: None,
        num_devices: Some(u64::from(devices)),
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
        fs_kind: PartitionKind::Vmfs,
        partitions: Some(partitions),
    };
    Ok((array, filesystem, image))
}
