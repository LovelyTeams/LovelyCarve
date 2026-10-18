use super::test_support::*;
use super::*;
use crate::core::evidence::partitions::test_support::BufSource;

fn mirror_mpb() -> MpbFields {
    let disks = vec![
        DiskFields {
            serial: "WD-WCC4E0000001",
            total_blocks: 4096,
            status: DISK_CONFIGURED,
        },
        DiskFields {
            serial: "WD-WCC4E0000002",
            total_blocks: 4096,
            status: DISK_CONFIGURED,
        },
    ];
    let map = MapFields::new(1, 2, 256, 2048);
    MpbFields::new(disks, vec![VolumeFields::new("Volume0", map, 2048)])
}

fn image_with(mpb: &MpbFields, len: usize, sector_size: usize) -> Vec<u8> {
    let mut image = vec![0u8; len];
    mpb.place(&mut image, sector_size);
    image
}

fn decoded(image: Vec<u8>) -> ImsmSuper {
    match probe(&BufSource(image)).expect("probe") {
        ImsmProbe::Valid(mpb) => mpb,
        other => panic!("expected a valid MPB, got {other:?}"),
    }
}

#[test]
fn decodes_disks_volumes_and_maps() {
    let mpb = decoded(image_with(&mirror_mpb(), 4096 * 512, 512));
    assert_eq!(mpb.version, "1.3.00");
    assert_eq!((mpb.sector_size, mpb.generation_num), (512, 40));
    assert_eq!(mpb.disks.len(), 2);
    assert_eq!(mpb.disks[1].serial_text(), "WD-WCC4E0000002");
    assert_eq!(mpb.disks[1].total_sectors, 4096);
    assert_eq!(mpb.unsupported_attributes(), 0);
    let volume = &mpb.volumes[0];
    assert_eq!(volume.name_text(), "Volume0");
    assert_eq!(volume.size_sectors, 2048);
    assert!(!volume.migrating && volume.source_map.is_none());
    assert_eq!(
        volume.map,
        ImsmMap {
            pba_of_lba0: 0,
            blocks_per_member: 2048,
            num_data_stripes: 4,
            blocks_per_strip: 256,
            map_state: MAP_NORMAL,
            raw_level: 1,
            num_members: 2,
            num_domains: 2,
            ord: vec![0, 1],
        }
    );
    assert_eq!(
        volume.map.component_sectors(volume.size_sectors),
        Some(2048)
    );
}

#[test]
fn corrupt_checksum_is_rejected() {
    let mut image = image_with(&mirror_mpb(), 4096 * 512, 512);
    let anchor = image.len() - 1024;
    image[anchor + 0xD8 + 16] ^= 0x01; // a disk's size, checksum left as is
    match probe(&BufSource(image)).expect("probe") {
        ImsmProbe::Invalid(reason) => assert!(reason.contains("checksum"), "{reason}"),
        other => panic!("expected an invalid MPB, got {other:?}"),
    }
}

#[test]
fn no_anchor_and_impossible_sizes() {
    assert_eq!(
        probe(&BufSource(vec![0u8; 64 * 1024])).expect("probe"),
        ImsmProbe::Absent
    );
    let mut image = image_with(&mirror_mpb(), 4096 * 512, 512);
    let anchor = image.len() - 1024;
    image[anchor + 0x24..anchor + 0x28].copy_from_slice(&100u32.to_le_bytes());
    assert!(matches!(
        probe(&BufSource(image)).expect("probe"),
        ImsmProbe::Invalid(_)
    ));
}

/// An MPB longer than one sector continues in the sectors before the anchor;
/// reading them from anywhere else breaks the checksum.
#[test]
fn multi_sector_mpb_is_read_from_before_the_anchor() {
    let mut fields = mirror_mpb();
    fields.padding = 1500;
    let second = VolumeFields::new("Data", MapFields::new(0, 2, 128, 1024), 2048);
    fields.volumes.push(second);
    let image = image_with(&fields, 4096 * 512, 512);
    let mpb = decoded(image);
    assert_eq!(mpb.volumes.len(), 2);
    assert_eq!(mpb.volumes[1].name_text(), "Data");
    assert_eq!(mpb.volumes[1].index, 1);
}

#[test]
fn four_k_metadata_is_converted_to_512_byte_sectors() {
    let mut fields = mirror_mpb();
    for disk in &mut fields.disks {
        disk.total_blocks = 512;
    }
    fields.volumes[0] = VolumeFields::new("Volume0", MapFields::new(1, 2, 32, 256), 256);
    fields.volumes[0].map.pba_of_lba0 = 2;
    let image = image_with(&fields, 512 * 4096, 4096);
    let mpb = decoded(image);
    assert_eq!(mpb.sector_size, 4096);
    assert_eq!(mpb.disks[0].total_sectors, 4096);
    let map = &mpb.volumes[0].map;
    assert_eq!(
        (map.pba_of_lba0, map.blocks_per_strip, map.blocks_per_member),
        (16, 256, 2048)
    );
    assert_eq!(mpb.volumes[0].size_sectors, 2048);
    // The stripe count is not a block count and keeps its value.
    assert_eq!(map.num_data_stripes, 4);
}

#[test]
fn migrating_volume_carries_its_source_map() {
    let mut fields = mirror_mpb();
    let mut source = MapFields::new(1, 2, 256, 2048);
    source.map_state = MAP_DEGRADED;
    source.ord = vec![0, 1 | ORD_REBUILD];
    let volume = &mut fields.volumes[0];
    volume.migr_state = 1;
    volume.migr_type = MIGR_REBUILD;
    volume.source_map = Some(source);
    let mpb = decoded(image_with(&fields, 4096 * 512, 512));
    let volume = &mpb.volumes[0];
    assert!(volume.migrating);
    assert_eq!(volume.migr_type, MIGR_REBUILD);
    assert!(volume.current_map().rebuilding(1));
    assert!(!volume.map.rebuilding(1));
    assert_eq!(volume.current_map().disk_index(1), Some(1));
}

#[test]
fn verify_with_fix_flag_reads_as_repair() {
    let mut fields = mirror_mpb();
    let volume = &mut fields.volumes[0];
    volume.migr_state = 1;
    volume.migr_type = MIGR_VERIFY;
    volume.status = DEV_VERIFY_AND_FIX;
    volume.source_map = Some(volume.map.clone());
    let mpb = decoded(image_with(&fields, 4096 * 512, 512));
    assert_eq!(mpb.volumes[0].migr_type, MIGR_REPAIR);
}

#[test]
fn serial_normalisation_matches_the_firmware() {
    let serial = normalize_serial("  WD-WCC4E1234567 ").expect("serial");
    assert_eq!(&serial[..15], b"WD-WCC4E1234567");
    assert_eq!(serial[15], 0);
    // Longer serials keep their last 16 characters; ':' becomes ';'.
    let long = normalize_serial("S3Z9NB0K123456X:AB").expect("serial");
    assert_eq!(&long, b"Z9NB0K123456X;AB");
    assert_eq!(normalize_serial(" \t "), None);
    let mut padded = serial;
    padded[15] = 0;
    assert!(serial_eq(&serial, &padded));
    assert!(!serial_eq(&serial, &long));
}

#[test]
fn volume_identity_depends_on_family_index_and_name() {
    let mut fields = mirror_mpb();
    fields.volumes.push(VolumeFields::new(
        "Volume0",
        MapFields::new(0, 2, 128, 1024),
        2048,
    ));
    let mpb = decoded(image_with(&fields, 4096 * 512, 512));
    let first = mpb.volume_uuid(&mpb.volumes[0]);
    assert_eq!(first, mpb.volume_uuid(&mpb.volumes[0]));
    assert_ne!(first, mpb.volume_uuid(&mpb.volumes[1]));
}

#[test]
fn component_size_falls_back_to_the_volume_size() {
    let map = ImsmMap {
        pba_of_lba0: 0,
        blocks_per_member: 0,
        num_data_stripes: 16,
        blocks_per_strip: 128,
        map_state: MAP_NORMAL,
        raw_level: 5,
        num_members: 3,
        num_domains: 1,
        ord: vec![0, 1, 2],
    };
    // 16 stripes of 128 sectors per member, two data members.
    assert_eq!(map.component_sectors(4096), Some(2048));
    // A volume grown without updating the stripe count.
    assert_eq!(map.component_sectors(40_000), Some(19_968));
    let raid10 = ImsmMap {
        raw_level: 1,
        num_members: 4,
        ..map
    };
    assert_eq!(raid10.level(), 10);
    assert_eq!(raid10.data_members(), Some(2));
}
