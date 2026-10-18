//! End-to-end IMSM detection over synthetic member images.
use super::test_support::*;
use super::*;
use crate::core::evidence::partitions::lvm2::test_support as lvm;
use crate::core::evidence::raid::imsm::test_support::{
    DiskFields, MapFields, MpbFields, VolumeFields,
};
use crate::core::evidence::raid::imsm::{
    DISK_CONFIGURED, DISK_FAILED, MAP_DEGRADED, MIGR_GEN_MIGR, MIGR_REBUILD, ORD_REBUILD,
};
use crate::core::evidence::raid::source::test_support::{encode_raid10, encode_striped, pattern};
use crate::core::evidence::raid::source::{Raid10Layout, Raid5Layout, RaidLevel};

const STRIP: u16 = (CHUNK / 512) as u16;
/// Room after the data for the MPB at the end of a member.
const TAIL: usize = 16 * 1024;
const SERIALS: [&str; 4] = ["SERIAL-A", "SERIAL-B", "SERIAL-C", "SERIAL-D"];

fn sectors(bytes: usize) -> u64 {
    (bytes / 512) as u64
}

fn table(count: usize, len: usize) -> Vec<DiskFields> {
    SERIALS[..count]
        .iter()
        .map(|serial| DiskFields {
            serial,
            total_blocks: sectors(len),
            status: DISK_CONFIGURED,
        })
        .collect()
}

/// Member image: `data` from sector 0, the MPB at the end.
fn member(mpb: &MpbFields, data: &[u8], len: usize) -> Vec<u8> {
    let mut image = vec![0u8; len];
    image[..data.len()].copy_from_slice(data);
    mpb.place(&mut image, 512);
    image
}

/// A healthy three-disk RAID5 volume and its member data.
struct Raid5Set {
    mpb: MpbFields,
    data: Vec<Vec<u8>>,
    logical: Vec<u8>,
    len: usize,
}

const RAID5_COMPONENT: usize = 64 * 1024;

fn raid5_set() -> Raid5Set {
    let logical = pattern(2 * RAID5_COMPONENT, 1);
    let data = encode_striped(
        RaidLevel::Raid5(Raid5Layout::LeftAsymmetric),
        &logical,
        3,
        CHUNK,
    );
    let len = RAID5_COMPONENT + TAIL;
    let map = MapFields::new(5, 3, STRIP, sectors(RAID5_COMPONENT));
    let volume = VolumeFields::new("Volume0", map, sectors(logical.len()));
    Raid5Set {
        mpb: MpbFields::new(table(3, len), vec![volume]),
        data,
        logical,
        len,
    }
}

fn mirror_mpb(component: usize, len: usize) -> MpbFields {
    let map = MapFields::new(1, 2, STRIP, sectors(component));
    MpbFields::new(
        table(2, len),
        vec![VolumeFields::new("Mirror", map, sectors(component))],
    )
}

#[test]
fn raid5_members_are_placed_by_drive_serial() {
    let set = raid5_set();
    // Loaded out of slot order; the serials put them back.
    let images: Vec<_> = [2usize, 0, 1]
        .iter()
        .map(|&slot| {
            disk(
                member(&set.mpb, &set.data[slot], set.len),
                Some(SERIALS[slot]),
            )
        })
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    assert_eq!(stack.arrays.len(), 1, "{:?}", stack.diagnostics);
    let array = &stack.arrays[0];
    assert_eq!((array.level, array.layout, array.raid_disks), (5, 0, 3));
    assert_eq!(array.metadata_version, "imsm 1.3.00");
    assert_eq!(array.set_name, "Volume0");
    assert_eq!(array.chunksize_sectors, u32::from(STRIP));
    assert!(!array.degraded && !array.degradable);
    assert_eq!(
        slots(array),
        [
            (Some(2), RaidMemberState::Active),
            (Some(0), RaidMemberState::Active),
            (Some(1), RaidMemberState::Active)
        ]
    );
    assert_eq!(array.members[0].device_uuid, "SERIAL-C");
    let device = device_of(&assembled, array);
    assert!(device.label.starts_with("imsm:Volume0-"));
    assert_eq!(read_all(device), set.logical);
}

#[test]
fn striped_volume_with_unmatched_images_is_listed_not_assembled() {
    const COMPONENT: usize = 32 * 1024;
    let logical = pattern(2 * COMPONENT, 2);
    let data = encode_striped(RaidLevel::Raid0, &logical, 2, CHUNK);
    let len = COMPONENT + TAIL;
    let map = MapFields::new(0, 2, STRIP, sectors(COMPONENT));
    let mpb = MpbFields::new(
        table(2, len),
        vec![VolumeFields::new("Stripe", map, sectors(logical.len()))],
    );
    let images = |serials: bool| -> Vec<_> {
        (0..2)
            .map(|slot| {
                disk(
                    member(&mpb, &data[slot], len),
                    serials.then_some(SERIALS[slot]),
                )
            })
            .collect()
    };

    let (stack, assembled) = detect_with_sources(&images(false));
    assert!(assembled.is_empty());
    // Neither assembled nor handed to the metadata-less search.
    assert_eq!(stack.arrays.len(), 1);
    let array = &stack.arrays[0];
    assert_eq!(
        slots(array),
        [(None, RaidMemberState::Unidentified); 2],
        "{:?}",
        stack.diagnostics
    );
    assert!(!array.degradable);
    assert!(mentions(&stack.diagnostics, "slot order is unknown"));

    let (stack, assembled) = detect_with_sources(&images(true));
    assert_eq!(read_all(device_of(&assembled, &stack.arrays[0])), logical);
}

#[test]
fn raid1_mirrors_without_serials_stand_in_for_each_other() {
    const COMPONENT: usize = 48 * 1024;
    let volume = pattern(COMPONENT, 3);
    let len = COMPONENT + TAIL;
    let mpb = mirror_mpb(COMPONENT, len);
    let images: Vec<_> = (0..2)
        .map(|_| disk(member(&mpb, &volume, len), None))
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    assert_eq!(read_all(&assembled[0]), volume, "{:?}", stack.diagnostics);
    assert!(!stack.arrays[0].degraded);
    assert!(mentions(&stack.diagnostics, "in-sync RAID1 mirror"));

    // Once a disk is recorded as failed, an unmatched image may be that disk.
    let mut failed = mpb.clone();
    failed.disks[1].status |= DISK_FAILED;
    failed.volumes[0].map.map_state = MAP_DEGRADED;
    let images: Vec<_> = (0..2)
        .map(|_| disk(member(&failed, &volume, len), None))
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    assert!(assembled.is_empty());
    assert!(mentions(&stack.diagnostics, "slot order is unknown"));
}

#[test]
fn raid10_members_are_placed_by_unique_disk_size() {
    const COMPONENT: usize = 32 * 1024;
    let layout = Raid10Layout::from_raw(0x102).expect("n2");
    let (logical, data) = encode_raid10(layout, 4, CHUNK, COMPONENT);
    let lens: Vec<usize> = (0..4).map(|slot| COMPONENT + TAIL + slot * 4096).collect();
    let disks = (0..4)
        .map(|slot| DiskFields {
            serial: SERIALS[slot],
            total_blocks: sectors(lens[slot]),
            status: DISK_CONFIGURED,
        })
        .collect();
    // Older firmware records RAID10 as level 1 over four members.
    let map = MapFields::new(1, 4, STRIP, sectors(COMPONENT));
    let mpb = MpbFields::new(
        disks,
        vec![VolumeFields::new("R10", map, sectors(logical.len()))],
    );
    let images: Vec<_> = [3usize, 1, 0, 2]
        .iter()
        .map(|&slot| disk(member(&mpb, &data[slot], lens[slot]), None))
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    let array = &stack.arrays[0];
    assert_eq!(
        (array.level, array.layout, array.raid_disks),
        (10, 0x102, 4),
        "{:?}",
        stack.diagnostics
    );
    assert_eq!(read_all(&assembled[0]), logical);
}

#[test]
fn failed_disk_is_rebuilt_from_parity_without_asking() {
    let set = raid5_set();
    let mut current = set.mpb.clone();
    current.generation_num += 1;
    current.disks[2].status |= DISK_FAILED;
    current.volumes[0].map.map_state = MAP_DEGRADED;
    // The failed disk kept the copy from before it failed.
    let images = vec![
        disk(member(&current, &set.data[0], set.len), Some(SERIALS[0])),
        disk(member(&current, &set.data[1], set.len), Some(SERIALS[1])),
        disk(
            member(&set.mpb, &vec![0xEE; RAID5_COMPONENT], set.len),
            Some(SERIALS[2]),
        ),
    ];
    let (stack, assembled) = detect_with_sources(&images);
    let array = &stack.arrays[0];
    assert!(
        array.degraded && !array.degradable,
        "{:?}",
        stack.diagnostics
    );
    assert_eq!(slots(array)[2], (Some(2), RaidMemberState::Faulty));
    assert_eq!(read_all(&assembled[0]), set.logical);
}

#[test]
fn missing_member_still_expected_waits_for_consent() {
    let set = raid5_set();
    let images: Vec<_> = (0..2)
        .map(|slot| {
            disk(
                member(&set.mpb, &set.data[slot], set.len),
                Some(SERIALS[slot]),
            )
        })
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    assert!(assembled.is_empty());
    assert!(stack.arrays[0].degradable, "{:?}", stack.diagnostics);

    let options = DetectOptions {
        degraded_sets: vec![stack.arrays[0].set_uuid.clone()],
    };
    let (stack, assembled) = detect_with_options(&images, &options);
    assert!(stack.arrays[0].degraded);
    assert!(assembled[0].label.ends_with("~without-2"));
    assert_eq!(read_all(&assembled[0]), set.logical);
}

#[test]
fn disk_being_rebuilt_is_not_read() {
    const COMPONENT: usize = 32 * 1024;
    let volume = pattern(COMPONENT, 4);
    let len = COMPONENT + TAIL;
    let mut mpb = mirror_mpb(COMPONENT, len);
    let fields = &mut mpb.volumes[0];
    let mut source = fields.map.clone();
    source.map_state = MAP_DEGRADED;
    source.ord = vec![0, 1 | ORD_REBUILD];
    fields.migr_state = 1;
    fields.migr_type = MIGR_REBUILD;
    fields.source_map = Some(source);
    let images = vec![
        disk(member(&mpb, &volume, len), Some(SERIALS[0])),
        disk(member(&mpb, &vec![0x5A; COMPONENT], len), Some(SERIALS[1])),
    ];
    let (stack, assembled) = detect_with_sources(&images);
    let array = &stack.arrays[0];
    assert_eq!(
        slots(array),
        [
            (Some(0), RaidMemberState::Active),
            (Some(1), RaidMemberState::Rebuilding)
        ],
        "{:?}",
        stack.diagnostics
    );
    assert!(array.degraded);
    assert_eq!(read_all(&assembled[0]), volume);
}

#[test]
fn member_that_missed_updates_is_stale() {
    const COMPONENT: usize = 32 * 1024;
    let volume = pattern(COMPONENT, 5);
    let len = COMPONENT + TAIL;
    let mpb = mirror_mpb(COMPONENT, len);
    let mut old = mpb.clone();
    old.generation_num -= 3;
    let images = vec![
        disk(member(&mpb, &volume, len), Some(SERIALS[0])),
        disk(member(&old, &vec![0x33; COMPONENT], len), Some(SERIALS[1])),
    ];
    let (stack, assembled) = detect_with_sources(&images);
    assert!(assembled.is_empty());
    let array = &stack.arrays[0];
    assert!(array.degradable);
    assert_eq!(slots(array)[1], (Some(1), RaidMemberState::Stale));
    assert!(mentions(&stack.diagnostics, "is stale"));

    let options = DetectOptions {
        degraded_sets: vec![array.set_uuid.clone()],
    };
    let (_, assembled) = detect_with_options(&images, &options);
    assert_eq!(read_all(&assembled[0]), volume);
}

#[test]
fn volume_in_migration_is_reported_not_assembled() {
    let mut set = raid5_set();
    let fields = &mut set.mpb.volumes[0];
    fields.migr_state = 1;
    fields.migr_type = MIGR_GEN_MIGR;
    fields.source_map = Some(MapFields::new(0, 2, STRIP, sectors(RAID5_COMPONENT)));
    let images: Vec<_> = (0..3)
        .map(|slot| {
            disk(
                member(&set.mpb, &set.data[slot], set.len),
                Some(SERIALS[slot]),
            )
        })
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    assert!(assembled.is_empty());
    assert_eq!(stack.arrays[0].level, 5);
    assert!(
        mentions(&stack.diagnostics, "migration"),
        "{:?}",
        stack.diagnostics
    );
}

#[test]
fn member_with_corrupt_metadata_is_reported_and_left_out() {
    const COMPONENT: usize = 32 * 1024;
    let volume = pattern(COMPONENT, 6);
    let len = COMPONENT + TAIL;
    let mpb = mirror_mpb(COMPONENT, len);
    let mut broken = member(&mpb, &volume, len);
    broken[len - 1024 + 0x2C] ^= 0x01; // generation, checksum left as is
    let images = vec![
        disk(member(&mpb, &volume, len), Some(SERIALS[0])),
        disk(broken, Some(SERIALS[1])),
    ];
    let (stack, assembled) = detect_with_sources(&images);
    assert!(
        mentions(&stack.diagnostics, "checksum"),
        "{:?}",
        stack.diagnostics
    );
    let array = &stack.arrays[0];
    assert_eq!(array.members.len(), 1);
    assert!(array.degradable);
    assert!(assembled.is_empty());
}

#[test]
fn two_volumes_share_the_same_disks() {
    const FIRST: usize = 32 * 1024;
    const SECOND: usize = 16 * 1024;
    let striped_logical = pattern(2 * FIRST, 7);
    let striped = encode_striped(RaidLevel::Raid0, &striped_logical, 2, CHUNK);
    let mirror = pattern(SECOND, 8);
    let len = FIRST + SECOND + TAIL;
    let first = VolumeFields::new(
        "Fast",
        MapFields::new(0, 2, STRIP, sectors(FIRST)),
        sectors(2 * FIRST),
    );
    let mut second_map = MapFields::new(1, 2, STRIP, sectors(SECOND));
    second_map.pba_of_lba0 = sectors(FIRST);
    let second = VolumeFields::new("Safe", second_map, sectors(SECOND));
    let mpb = MpbFields::new(table(2, len), vec![first, second]);
    let images: Vec<_> = (0..2)
        .map(|slot| {
            let data = [striped[slot].clone(), mirror.clone()].concat();
            disk(member(&mpb, &data, len), Some(SERIALS[slot]))
        })
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    assert_eq!(stack.arrays.len(), 2, "{:?}", stack.diagnostics);
    let named = |name: &str| {
        stack
            .arrays
            .iter()
            .find(|array| array.set_name == name)
            .expect("volume")
    };
    assert_ne!(named("Fast").set_uuid, named("Safe").set_uuid);
    assert_eq!(
        read_all(device_of(&assembled, named("Fast"))),
        striped_logical
    );
    assert_eq!(read_all(device_of(&assembled, named("Safe"))), mirror);
    assert_eq!(named("Safe").data_offset_sectors, sectors(FIRST));
}

#[test]
fn metadata_written_with_4k_sectors() {
    const COMPONENT: usize = 32 * 1024;
    let logical = pattern(2 * COMPONENT, 9);
    let data = encode_striped(RaidLevel::Raid0, &logical, 2, CHUNK);
    let len = COMPONENT + 4 * 4096;
    let disks = SERIALS[..2]
        .iter()
        .map(|serial| DiskFields {
            serial,
            total_blocks: (len / 4096) as u64,
            status: DISK_CONFIGURED,
        })
        .collect();
    // Block counts in 4096-byte units: one-block strips.
    let map = MapFields::new(0, 2, 1, (COMPONENT / 4096) as u64);
    let mpb = MpbFields::new(
        disks,
        vec![VolumeFields::new(
            "Volume0",
            map,
            (2 * COMPONENT / 4096) as u64,
        )],
    );
    let images: Vec<_> = (0..2)
        .map(|slot| {
            let mut image = vec![0u8; len];
            image[..COMPONENT].copy_from_slice(&data[slot]);
            mpb.place(&mut image, 4096);
            disk(image, Some(SERIALS[slot]))
        })
        .collect();
    let (stack, assembled) = detect_with_sources(&images);
    assert_eq!(
        stack.arrays[0].chunksize_sectors,
        u32::from(STRIP),
        "{:?}",
        stack.diagnostics
    );
    assert_eq!(read_all(&assembled[0]), logical);
}

/// A member disk shows its volume's partitions; the PV seen through them is
/// the PV of the assembled volume, not two more.
#[test]
fn pvs_seen_through_member_disks_are_not_counted_again() {
    let volume = lvm::image(&lvm::text(), 1, false);
    let component = volume.len();
    let len = component + TAIL;
    let mpb = mirror_mpb(component, len);
    let pv = Partition {
        id: 1,
        kind: PartitionKind::Lvm,
        start_lba: 0,
        length_lba: sectors(component),
        label: None,
        unique_guid: None,
    };
    let images: Vec<_> = (0..2)
        .map(|slot| {
            disk_with(
                member(&mpb, &volume, len),
                Some(SERIALS[slot]),
                vec![pv.clone()],
            )
        })
        .collect();
    let (stack, _) = detect_with_sources(&images);
    assert_eq!(stack.volume_groups.len(), 1, "{:?}", stack.diagnostics);
    let members: Vec<_> = stack.volume_groups[0]
        .members
        .iter()
        .map(|member| member.image_id.clone())
        .collect();
    assert_eq!(members, [array_source_id(&stack.arrays[0].set_uuid)]);
    assert!(!mentions(&stack.diagnostics, "ambiguous"));
}
