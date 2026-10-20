//! End-to-end md / LVM detection over synthetic member images.
use super::*;
use crate::core::evidence::partitions::lvm2::header::checksum;
use crate::core::evidence::partitions::lvm2::test_support as lvm;
use crate::core::evidence::raid::mdadm_super::test_support::{V090Fields, V1Fields};
use crate::core::evidence::raid::mdadm_super::{format_uuid, v090_super_sector, ROLE_FAULTY};
use crate::core::evidence::raid::source::test_support::{encode_raid10, encode_striped, pattern};
use crate::core::evidence::raid::source::{Raid10Layout, Raid5Layout, Raid6Layout, RaidLevel};

struct MemSource(Vec<u8>);

impl SectorSource for MemSource {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        let start = (offset as usize).min(self.0.len());
        let end = (start + len).min(self.0.len());
        Ok(self.0[start..end].to_vec())
    }
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
}

fn disk_with(bytes: Vec<u8>, partitions: Vec<Partition>) -> Arc<DiskImage> {
    Arc::new(
        DiskImage::from_assembled("member".into(), Arc::new(MemSource(bytes)), partitions, 512)
            .expect("member image"),
    )
}

fn disk(bytes: Vec<u8>) -> Arc<DiskImage> {
    disk_with(bytes, Vec::new())
}

const CHUNK: usize = 4096;
const DATA_OFFSET_SECTORS: u64 = 16;

#[test]
fn observing_detection_preserves_results_and_reports_stage_local_bounds() {
    let geometry = geometry(1, 0, 2, 512);
    let content = pattern(512 * 512, 7);
    let images = (0..2)
        .map(|slot| disk(v12_member(&device(&geometry, slot), &content)))
        .collect::<Vec<_>>();
    let options = DetectOptions::default();
    let baseline = detect_with_options(&images, &options);
    let progress = std::sync::Mutex::new(Vec::new());
    let observed = detect_with_options_observed(&images, &options, &|event| {
        progress.lock().unwrap().push(event);
    });
    assert_eq!(
        serde_json::to_value(&baseline.0).unwrap(),
        serde_json::to_value(&observed.0).unwrap()
    );
    assert_eq!(baseline.1.len(), observed.1.len());
    let progress = progress.into_inner().unwrap();
    assert_eq!(progress.first().unwrap().stage, DetectStage::LvmMetadata);
    assert!(progress
        .iter()
        .any(|event| event.stage == DetectStage::MdArrays
            && event.current == 1
            && event.total == Some(1)));
    assert!(progress
        .iter()
        .any(|event| event.stage == DetectStage::Heuristic && event.total.is_none()));
    assert!(progress
        .iter()
        .all(|event| event.total.is_none_or(|total| event.current <= total)));
}

/// Array geometry shared by every member's superblock.
fn geometry(level: i32, layout: u32, raid_disks: u32, size_sectors: u64) -> V1Fields {
    V1Fields {
        layout,
        chunk_sectors: (CHUNK / 512) as u32,
        size_sectors,
        ..V1Fields::new(level, raid_disks)
    }
}

fn device(base: &V1Fields, dev_number: u32) -> V1Fields {
    V1Fields {
        dev_number,
        device_uuid: [dev_number as u8 + 1; 16],
        ..base.clone()
    }
}

/// 1.2 member: superblock 4 KiB from the start, data after 8 KiB.
fn v12_member(fields: &V1Fields, data: &[u8]) -> Vec<u8> {
    let fields = V1Fields {
        super_offset_sectors: 8,
        data_offset_sectors: DATA_OFFSET_SECTORS,
        data_size_sectors: data.len() as u64 / 512,
        ..fields.clone()
    };
    let offset = DATA_OFFSET_SECTORS as usize * 512;
    let mut image = vec![0u8; offset + data.len()];
    image[4096..8192].copy_from_slice(&fields.encode());
    image[offset..].copy_from_slice(data);
    image
}

/// 1.0 member: data from sector 0, superblock 8–12 KiB before the end.
fn v10_member(fields: &V1Fields, data: &[u8]) -> Vec<u8> {
    let size = data.len() + 16 * 1024;
    let sector = (size as u64 / 512 - 16) & !7;
    let fields = V1Fields {
        super_offset_sectors: sector,
        data_offset_sectors: 0,
        data_size_sectors: data.len() as u64 / 512,
        ..fields.clone()
    };
    let mut image = vec![0u8; size];
    image[..data.len()].copy_from_slice(data);
    let at = sector as usize * 512;
    image[at..at + 4096].copy_from_slice(&fields.encode());
    image
}

/// 0.90 member: data from sector 0, superblock in the last 64 KiB block.
fn v090_member(fields: &V090Fields, data: &[u8]) -> Vec<u8> {
    let size = data.len().next_multiple_of(65536) + 65536;
    let sector = v090_super_sector(size as u64 / 512).expect("superblock fits");
    let mut image = vec![0u8; size];
    image[..data.len()].copy_from_slice(data);
    let at = sector as usize * 512;
    image[at..at + 4096].copy_from_slice(&fields.encode(false));
    image
}

/// Volume bytes that probe as a bare ext4 filesystem.
fn ext4_volume(len: usize, seed: u8) -> Vec<u8> {
    let mut bytes = pattern(len, seed);
    bytes[..4096].fill(0);
    bytes[0x438..0x43A].copy_from_slice(&[0x53, 0xEF]);
    bytes
}

fn read_all(array: &AssembledArray) -> Vec<u8> {
    array
        .source
        .read(0, array.size_bytes as usize)
        .expect("read assembled device")
}

fn set_uuid() -> String {
    format_uuid(&[0xAA; 16])
}

/// Three 1.2 members of a left-symmetric RAID5 carrying ext4.
fn raid5_members(logical: &[u8]) -> (V1Fields, Vec<Vec<u8>>) {
    let base = geometry(5, 2, 3, 256);
    let data = encode_striped(
        RaidLevel::Raid5(Raid5Layout::LeftSymmetric),
        logical,
        3,
        CHUNK,
    );
    (base, data)
}

#[test]
fn v12_raid5_assembles_and_labels_its_volume() {
    let logical = ext4_volume(256 * 1024, 1);
    let (base, data) = raid5_members(&logical);
    let images: Vec<_> = (0..3)
        .map(|dev| disk(v12_member(&device(&base, dev), &data[dev as usize])))
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    assert_eq!(stack.arrays.len(), 1, "{:?}", stack.diagnostics);
    let array = &stack.arrays[0];
    assert_eq!((array.level, array.raid_disks), (5, 3));
    assert_eq!(array.metadata_version, "1.2");
    assert_eq!(array.assembled_bytes, logical.len() as u64);
    assert!(!array.degraded && !array.degradable);
    assert!(array
        .members
        .iter()
        .all(|member| member.state == RaidMemberState::Active));
    assert_eq!(assembled.len(), 1);
    assert_eq!(read_all(&assembled[0]), logical);
    assert_eq!(
        assembled[0].partitions,
        Some(vec![Partition {
            id: 0,
            kind: PartitionKind::Ext4,
            start_lba: 0,
            length_lba: 512,
            label: Some("ext4 (md · host:0)".into()),
            unique_guid: None,
        }])
    );
    assert_eq!(stack.filesystems[0].kind, "ext4");
    assert_eq!(assembled[0].label, format!("md:host:0-{}", set_uuid()));
}

#[test]
fn v10_raid6_assembles_without_a_member_md_recorded_as_failed() {
    let logical = pattern(256 * 1024, 2);
    let base = V1Fields {
        dev_roles: vec![0, 1, 2, ROLE_FAULTY],
        ..geometry(6, 2, 4, 256)
    };
    let data = encode_striped(
        RaidLevel::Raid6(Raid6Layout::LeftSymmetric),
        &logical,
        4,
        CHUNK,
    );
    let images: Vec<_> = (0..3)
        .map(|dev| disk(v10_member(&device(&base, dev), &data[dev as usize])))
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    let array = &stack.arrays[0];
    assert_eq!(array.metadata_version, "1.0");
    assert!(array.degraded, "{:?}", stack.diagnostics);
    assert_eq!(assembled.len(), 1);
    assert_eq!(read_all(&assembled[0]), logical);
}

#[test]
fn v090_raid1_reads_the_mirrored_volume() {
    let volume = ext4_volume(192 * 1024, 3);
    let images: Vec<_> = (0..2)
        .map(|this_disk| {
            let fields = V090Fields {
                size_kib: 192,
                this_disk,
                ..V090Fields::new(1, 2)
            };
            disk(v090_member(&fields, &volume))
        })
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    let array = &stack.arrays[0];
    assert_eq!((array.level, array.metadata_version.as_str()), (1, "0.90"));
    assert_eq!(array.set_name, "md0");
    assert_eq!(array.assembled_bytes, volume.len() as u64);
    assert_eq!(read_all(&assembled[0]), volume);
}

#[test]
fn absent_member_still_listed_waits_for_the_examiner() {
    let logical = pattern(256 * 1024, 4);
    let (base, data) = raid5_members(&logical);
    let images: Vec<_> = (0..2)
        .map(|dev| disk(v12_member(&device(&base, dev), &data[dev as usize])))
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    assert!(assembled.is_empty());
    assert!(stack.arrays[0].degradable);
    assert!(stack.arrays[0].assembled_image_id.is_none());

    let options = DetectOptions {
        degraded_sets: vec![set_uuid()],
    };
    let (stack, assembled) = detect_with_options(&images, &options);
    assert!(stack.arrays[0].degraded);
    assert!(!stack.arrays[0].degradable);
    assert_eq!(read_all(&assembled[0]), logical);
    // A degraded reconstruction is its own device, distinct from the complete one.
    assert_eq!(
        assembled[0].label,
        format!("md:host:0-{}~without-2", set_uuid())
    );
}

#[test]
fn stale_member_is_rebuilt_from_parity_not_read() {
    let logical = pattern(256 * 1024, 5);
    let (base, mut data) = raid5_members(&logical);
    data[2].fill(0xEE);
    let images: Vec<_> = (0..3)
        .map(|dev| {
            let fields = V1Fields {
                events: if dev == 2 { 3 } else { 10 },
                ..device(&base, dev)
            };
            disk(v12_member(&fields, &data[dev as usize]))
        })
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    assert!(assembled.is_empty());
    assert!(stack.arrays[0].degradable);
    assert_eq!(stack.arrays[0].members[2].state, RaidMemberState::Stale);
    assert!(stack
        .diagnostics
        .iter()
        .any(|message| message.contains("is stale")));

    let options = DetectOptions {
        degraded_sets: vec![set_uuid()],
    };
    let (stack, assembled) = detect_with_options(&images, &options);
    assert_eq!(read_all(&assembled[0]), logical);
    assert!(stack.arrays[0].members[2].reconstructed);
}

/// md accepts a member one event behind (a crash between superblock writes).
#[test]
fn member_one_event_behind_is_current() {
    let logical = pattern(256 * 1024, 6);
    let (base, data) = raid5_members(&logical);
    let images: Vec<_> = (0..3)
        .map(|dev| {
            let fields = V1Fields {
                events: if dev == 1 { 9 } else { 10 },
                ..device(&base, dev)
            };
            disk(v12_member(&fields, &data[dev as usize]))
        })
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    assert!(!stack.arrays[0].degraded);
    assert_eq!(read_all(&assembled[0]), logical);
}

#[test]
fn raid10_near_layout_assembles() {
    let layout = Raid10Layout::from_raw(0x102).expect("n2");
    let (logical, data) = encode_raid10(layout, 4, CHUNK, 128 * 1024);
    let base = geometry(10, 0x102, 4, 256);
    let images: Vec<_> = (0..4)
        .map(|dev| disk(v12_member(&device(&base, dev), &data[dev as usize])))
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    assert_eq!(stack.arrays[0].level, 10);
    assert_eq!(read_all(&assembled[0]), logical);
}

#[test]
fn linear_array_concatenates_members_of_different_sizes() {
    let first = pattern(128 * 1024, 7);
    let second = pattern(192 * 1024, 8);
    let base = V1Fields {
        chunk_sectors: 0,
        ..V1Fields::new(-1, 2)
    };
    let images = vec![
        disk(v12_member(&device(&base, 0), &first)),
        disk(v12_member(&device(&base, 1), &second)),
    ];
    let (stack, assembled) = detect_with_sources(&images);
    assert_eq!(stack.arrays[0].level, -1);
    assert_eq!(read_all(&assembled[0]), [first, second].concat());
}

/// A 0.90 superblock at the end of a partition that ends with its disk is
/// visible from both; the partition is the member.
#[test]
fn tail_superblock_belongs_to_the_innermost_partition() {
    let volume = pattern(192 * 1024, 9);
    let member = |this_disk| {
        v090_member(
            &V090Fields {
                size_kib: 192,
                this_disk,
                ..V090Fields::new(1, 2)
            },
            &volume,
        )
    };
    let partition_bytes = member(0);
    let mut disk_bytes = vec![0u8; 128 * 512];
    disk_bytes.extend_from_slice(&partition_bytes);
    let partition = Partition {
        id: 1,
        kind: PartitionKind::Mdadm,
        start_lba: 128,
        length_lba: partition_bytes.len() as u64 / 512,
        label: None,
        unique_guid: None,
    };
    let images = vec![disk_with(disk_bytes, vec![partition]), disk(member(1))];
    let (stack, assembled) = detect_with_sources(&images);
    let array = &stack.arrays[0];
    assert_eq!(array.members.len(), 2, "{:?}", stack.diagnostics);
    assert!(array.members.iter().any(|member| matches!(
        member.origin,
        RaidMemberOrigin::Whole {
            partition_id: 1,
            byte_offset: 65536,
            ..
        }
    )));
    assert_eq!(read_all(&assembled[0]), volume);
}

#[test]
fn corrupt_superblock_is_reported_and_ignored() {
    let volume = pattern(128 * 1024, 10);
    let base = geometry(1, 0, 2, 256);
    let mut broken = v12_member(&device(&base, 1), &volume);
    broken[4096 + 92] ^= 0x01; // raid_disks, without fixing the checksum
    let images = vec![disk(v12_member(&device(&base, 0), &volume)), disk(broken)];
    let (stack, assembled) = detect_with_sources(&images);
    assert!(stack
        .diagnostics
        .iter()
        .any(|message| message.contains("fails its checksum")));
    assert_eq!(stack.arrays[0].members.len(), 1);
    assert!(assembled.is_empty());
    assert!(stack.arrays[0].degradable);
}

/// Physical volume fixture whose label carries `identifier`.
fn pv(text: &str, identifier: &str) -> Vec<u8> {
    let mut bytes = lvm::image(text, 1, false);
    bytes[544..576].copy_from_slice(identifier.as_bytes());
    let crc = checksum(&bytes[532..1024]);
    bytes[528..532].copy_from_slice(&crc.to_le_bytes());
    bytes
}

/// Two single-disk RAID1 arrays, each one PV of a volume group whose LV spans
/// both — the shape NAS hybrid volumes take.
#[test]
fn lv_spanning_pvs_on_two_arrays_becomes_its_own_device() {
    let text = lvm::text().replace("\"pv1\", 0", "\"pv0\", 0");
    let arrays = [
        ([0xA1u8; 16], pv(&text, lvm::PV_ID)),
        ([0xB2u8; 16], pv(&text, lvm::OTHER_PV_ID)),
    ];
    let images: Vec<_> = arrays
        .iter()
        .map(|(uuid, content)| {
            let fields = V1Fields {
                set_uuid: *uuid,
                ..geometry(1, 0, 1, content.len() as u64 / 512)
            };
            disk(v12_member(&fields, content))
        })
        .collect();
    let (mut stack, assembled) = detect_with_sources(&images);
    assert_eq!(stack.arrays.len(), 2, "{:?}", stack.diagnostics);
    for array in assembled
        .iter()
        .filter(|array| array.target == AssembledTarget::Array)
    {
        let kinds: Vec<_> = array.partitions.iter().flatten().map(|p| p.kind).collect();
        assert_eq!(kinds, vec![PartitionKind::Lvm]);
    }
    let lv = assembled
        .iter()
        .find(|array| {
            array.target
                == AssembledTarget::LogicalVolume {
                    vg_uuid: stack.volume_groups[0].vg_uuid.clone(),
                    lv_name: "root".into(),
                }
        })
        .expect("cross-array LV");
    assert_eq!(read_all(lv), [vec![0x11; 8192], vec![0x22; 8192]].concat());

    // Registration names the arrays' images; PVs then point at them.
    for (array, image_id) in stack.arrays.iter_mut().zip(["image-a", "image-b"]) {
        array.assembled_image_id = Some(image_id.into());
    }
    resolve_array_sources(&mut stack);
    let mut pv_images: Vec<_> = stack.volume_groups[0]
        .members
        .iter()
        .map(|member| member.image_id.as_str())
        .collect();
    pv_images.sort_unstable();
    assert_eq!(pv_images, ["image-a", "image-b"]);
}
