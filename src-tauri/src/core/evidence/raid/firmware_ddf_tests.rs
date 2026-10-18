//! End-to-end DDF detection over synthetic member images.
use super::test_support::*;
use super::*;
use crate::core::evidence::raid::ddf::test_support::{
    ConfigFields, ContainerFields, PhysFields, VirtualFields, COPY_SECTORS,
};
use crate::core::evidence::raid::ddf::{PD_FAILED, PD_ONLINE, VD_MORPHING};
use crate::core::evidence::raid::source::test_support::{encode_raid10, encode_striped, pattern};
use crate::core::evidence::raid::source::{Raid10Layout, Raid5Layout, Raid6Layout, RaidLevel};

/// Room after the data for the anchor and one metadata copy.
const TAIL: usize = 32 * 1024;
const REFNUMS: [u32; 6] = [0x1001, 0x1002, 0x1003, 0x1004, 0x1005, 0x1006];
const VD: [u8; 24] = [0xA7; 24];

fn sectors(bytes: usize) -> u64 {
    (bytes / 512) as u64
}

fn container(disks: usize, name: &'static str) -> ContainerFields {
    ContainerFields::new(
        REFNUMS[..disks]
            .iter()
            .map(|&refnum| PhysFields::online(refnum))
            .collect(),
        vec![VirtualFields::new(VD, name)],
    )
}

/// A single-level configuration record over the first `disks` disks.
fn config(prl: u8, rlq: u8, disks: usize, component: usize, array_bytes: usize) -> ConfigFields {
    ConfigFields {
        array_blocks: sectors(array_bytes),
        ..ConfigFields::new(VD, prl, rlq, REFNUMS[..disks].to_vec(), sectors(component))
    }
}

/// Member image: `data` from sector `offset`, the metadata of disk `refnum`
/// at the end.
fn member(
    fields: &ContainerFields,
    refnum: u32,
    configs: &[ConfigFields],
    data: &[u8],
    offset: u64,
) -> Vec<u8> {
    let start = offset as usize * 512;
    let mut image = vec![0u8; start + data.len() + TAIL];
    image[start..start + data.len()].copy_from_slice(data);
    fields.place(&mut image, refnum, configs);
    image
}

fn members(
    fields: &ContainerFields,
    record: &ConfigFields,
    data: &[Vec<u8>],
    slots: &[usize],
) -> Vec<Arc<DiskImage>> {
    slots
        .iter()
        .map(|&slot| {
            let offset = record.lba_offsets[slot];
            disk(
                member(
                    fields,
                    REFNUMS[slot],
                    std::slice::from_ref(record),
                    &data[slot],
                    offset,
                ),
                None,
            )
        })
        .collect()
}

#[test]
fn parity_layouts_read_back_with_per_disk_offsets() {
    const COMPONENT: usize = 32 * 1024;
    for (prl, rlq, level) in [
        (0x05, 0x00, RaidLevel::Raid5(Raid5Layout::RightAsymmetric)),
        (0x05, 0x02, RaidLevel::Raid5(Raid5Layout::LeftAsymmetric)),
        (0x05, 0x03, RaidLevel::Raid5(Raid5Layout::LeftSymmetric)),
        (0x04, 0x01, RaidLevel::Raid4),
        (0x04, 0x00, RaidLevel::Raid5(Raid5Layout::Parity0)),
    ] {
        let logical = pattern(3 * COMPONENT, prl ^ rlq);
        let data = encode_striped(level, &logical, 4, CHUNK);
        let fields = container(4, "Parity");
        let record = ConfigFields {
            lba_offsets: vec![16, 16, 32, 64],
            ..config(prl, rlq, 4, COMPONENT, logical.len())
        };
        // Loaded in reverse; the reference numbers put them back.
        let images = members(&fields, &record, &data, &[3, 2, 1, 0]);
        let (stack, assembled) = detect_with_sources(&images);
        assert_eq!(
            stack.arrays.len(),
            1,
            "{prl:#x}/{rlq:#x}: {:?}",
            stack.diagnostics
        );
        let array = &stack.arrays[0];
        assert_eq!(array.metadata_version, "ddf 01.02.00");
        assert_eq!(array.set_name, "Parity");
        assert_eq!(array.members[0].slot, Some(3));
        assert_eq!(read_all(&assembled[0]), logical, "{prl:#x}/{rlq:#x}");
        assert!(assembled[0].label.starts_with("ddf:Parity-"));
    }
}

#[test]
fn raid6_rotations_rebuild_two_failed_disks() {
    const COMPONENT: usize = 16 * 1024;
    for (rlq, layout) in [
        (0x01, Raid6Layout::RotatingZeroRestart),
        (0x02, Raid6Layout::RotatingNRestart),
        (0x03, Raid6Layout::RotatingNContinue),
    ] {
        let logical = pattern(3 * COMPONENT, rlq);
        let data = encode_striped(RaidLevel::Raid6(layout), &logical, 5, CHUNK);
        let mut fields = container(5, "Dual");
        fields.disks[1].state = PD_ONLINE | PD_FAILED;
        fields.disks[3].state = PD_FAILED;
        let record = config(0x06, rlq, 5, COMPONENT, logical.len());
        // Only the surviving disks are loaded.
        let images = members(&fields, &record, &data, &[0, 2, 4]);
        let (stack, assembled) = detect_with_sources(&images);
        let array = &stack.arrays[0];
        assert_eq!(
            (array.level, array.layout),
            (6, layout.raw()),
            "{:?}",
            stack.diagnostics
        );
        assert!(array.degraded && !array.degradable);
        assert_eq!(read_all(&assembled[0]), logical, "rlq {rlq:#x}");
    }
}

#[test]
fn raid10_reads_mirrored_bvds_striped_together() {
    const COMPONENT: usize = 32 * 1024;
    let layout = Raid10Layout::from_raw(0x102).expect("n2");
    let (logical, data) = encode_raid10(layout, 4, CHUNK, COMPONENT);
    let fields = container(4, "R10");
    // The size field holds the whole volume, or (as some writers record
    // it) one BVD.
    for (srl, size_bytes) in [(0x00, logical.len()), (0x03, COMPONENT)] {
        let bvd = |seq: usize| ConfigFields {
            srl,
            sec_elmnt_count: 2,
            sec_elmnt_seq: seq as u8,
            array_blocks: sectors(size_bytes),
            ..ConfigFields::new(
                VD,
                0x01,
                0x00,
                REFNUMS[seq * 2..seq * 2 + 2].to_vec(),
                sectors(COMPONENT),
            )
        };
        // Each disk carries the record of its own BVD only.
        let images: Vec<_> = (0..4)
            .map(|slot| {
                disk(
                    member(&fields, REFNUMS[slot], &[bvd(slot / 2)], &data[slot], 0),
                    None,
                )
            })
            .collect();
        let (stack, assembled) = detect_with_sources(&images);
        let array = &stack.arrays[0];
        assert_eq!(
            (array.level, array.layout, array.raid_disks),
            (10, 0x102, 4),
            "{:?}",
            stack.diagnostics
        );
        assert_eq!(read_all(&assembled[0]), logical, "srl {srl:#x}");
    }
}

#[test]
fn raid1e_adjacent_mirroring_over_three_disks() {
    const COMPONENT: usize = 24 * 1024;
    let layout = Raid10Layout::from_raw(0x102).expect("n2");
    let (logical, data) = encode_raid10(layout, 3, CHUNK, COMPONENT);
    let fields = container(3, "R1E");
    let record = config(0x11, 0x00, 3, COMPONENT, logical.len());
    let images = members(&fields, &record, &data, &[1, 0, 2]);
    let (stack, assembled) = detect_with_sources(&images);
    assert_eq!(
        (stack.arrays[0].level, stack.arrays[0].layout),
        (10, 0x102),
        "{:?}",
        stack.diagnostics
    );
    assert_eq!(read_all(&assembled[0]), logical);
}

#[test]
fn striping_mirroring_and_concatenation_read_back() {
    const COMPONENT: usize = 16 * 1024;
    let striped = pattern(3 * COMPONENT, 21);
    let mirror = pattern(COMPONENT, 22);
    let parts: Vec<Vec<u8>> = (0..3).map(|seed| pattern(COMPONENT, 23 + seed)).collect();
    let cases = [
        (
            0x00,
            0x00,
            encode_striped(RaidLevel::Raid0, &striped, 3, CHUNK),
            striped.clone(),
        ),
        (0x01, 0x01, vec![mirror.clone(); 3], mirror.clone()),
        (0x1F, 0x00, parts.clone(), parts.concat()),
    ];
    for (prl, rlq, data, logical) in cases {
        let fields = container(3, "Plain");
        let record = config(prl, rlq, 3, COMPONENT, logical.len());
        let images = members(&fields, &record, &data, &[0, 1, 2]);
        let (stack, assembled) = detect_with_sources(&images);
        assert_eq!(
            assembled.len(),
            1,
            "{prl:#x}/{rlq:#x}: {:?}",
            stack.diagnostics
        );
        assert_eq!(read_all(&assembled[0]), logical, "{prl:#x}/{rlq:#x}");
    }
}

#[test]
fn disk_not_loaded_waits_for_consent() {
    const COMPONENT: usize = 32 * 1024;
    let logical = pattern(2 * COMPONENT, 31);
    let data = encode_striped(
        RaidLevel::Raid5(Raid5Layout::LeftSymmetric),
        &logical,
        3,
        CHUNK,
    );
    let fields = container(3, "Wait");
    let record = config(0x05, 0x03, 3, COMPONENT, logical.len());
    let images = members(&fields, &record, &data, &[0, 2]);
    let (stack, assembled) = detect_with_sources(&images);
    assert!(assembled.is_empty());
    assert!(stack.arrays[0].degradable, "{:?}", stack.diagnostics);

    let options = DetectOptions {
        degraded_sets: vec![stack.arrays[0].set_uuid.clone()],
    };
    let (stack, assembled) = detect_with_options(&images, &options);
    assert!(stack.arrays[0].degraded);
    assert_eq!(read_all(&assembled[0]), logical);
}

#[test]
fn member_behind_on_updates_is_stale() {
    const COMPONENT: usize = 32 * 1024;
    let volume = pattern(COMPONENT, 32);
    let fields = container(2, "Mirror");
    let mut old = fields.clone();
    old.seq -= 3;
    let record = config(0x01, 0x00, 2, COMPONENT, COMPONENT);
    let images = vec![
        disk(
            member(&fields, REFNUMS[0], &[record.clone()], &volume, 0),
            None,
        ),
        disk(
            member(&old, REFNUMS[1], &[record], &vec![0x44; COMPONENT], 0),
            None,
        ),
    ];
    let (stack, assembled) = detect_with_sources(&images);
    assert!(assembled.is_empty());
    let array = &stack.arrays[0];
    assert_eq!(
        slots(array),
        [
            (Some(0), RaidMemberState::Active),
            (Some(1), RaidMemberState::Stale)
        ],
        "{:?}",
        stack.diagnostics
    );
    let options = DetectOptions {
        degraded_sets: vec![array.set_uuid.clone()],
    };
    let (_, assembled) = detect_with_options(&images, &options);
    assert_eq!(read_all(&assembled[0]), volume);
}

#[test]
fn same_disk_loaded_twice_is_read_once() {
    const COMPONENT: usize = 32 * 1024;
    let volume = pattern(COMPONENT, 33);
    let fields = container(2, "Mirror");
    let record = config(0x01, 0x00, 2, COMPONENT, COMPONENT);
    let first = member(&fields, REFNUMS[0], &[record.clone()], &volume, 0);
    let images = vec![
        disk(first.clone(), None),
        disk(first, None),
        disk(member(&fields, REFNUMS[1], &[record], &volume, 0), None),
    ];
    let (stack, assembled) = detect_with_sources(&images);
    assert_eq!(
        slots(&stack.arrays[0]),
        [
            (Some(0), RaidMemberState::Active),
            (Some(0), RaidMemberState::Duplicate),
            (Some(1), RaidMemberState::Active)
        ],
        "{:?}",
        stack.diagnostics
    );
    assert_eq!(read_all(&assembled[0]), volume);
}

#[test]
fn member_with_corrupt_metadata_is_reported_and_left_out() {
    const COMPONENT: usize = 32 * 1024;
    let logical = pattern(2 * COMPONENT, 34);
    let data = encode_striped(
        RaidLevel::Raid5(Raid5Layout::LeftSymmetric),
        &logical,
        3,
        CHUNK,
    );
    let fields = container(3, "Broken");
    let record = config(0x05, 0x03, 3, COMPONENT, logical.len());
    let mut images = members(&fields, &record, &data, &[0, 1]);
    let mut broken = member(&fields, REFNUMS[2], &[record], &data[2], 0);
    let config_sector = broken.len() / 512 - 1 - COPY_SECTORS + 6;
    broken[config_sector * 512 + 72] ^= 0x01;
    images.push(disk(broken, None));
    let (stack, assembled) = detect_with_sources(&images);
    assert!(
        mentions(&stack.diagnostics, "configuration record 0 fails its CRC"),
        "{:?}",
        stack.diagnostics
    );
    let array = &stack.arrays[0];
    assert_eq!(array.members.len(), 2);
    assert!(array.degradable);
    assert!(assembled.is_empty());
}

#[test]
fn unsupported_levels_are_reported_not_assembled() {
    const COMPONENT: usize = 16 * 1024;
    // RAID5E, RAID1E offset mirroring, RAID3 and RAID5 BVDs striped (RAID50).
    for (prl, rlq, sec_count, shown) in [
        (0x15u8, 0x00u8, 1u8, 5),
        (0x11, 0x01, 1, 10),
        (0x03, 0x00, 1, 3),
        (0x05, 0x03, 2, 50),
    ] {
        let fields = container(6, "Exotic");
        let disks_per_bvd = if sec_count > 1 { 3 } else { 6 };
        let bvd = |seq: usize| ConfigFields {
            srl: if sec_count > 1 { 0x00 } else { 0xFF },
            sec_elmnt_count: sec_count,
            sec_elmnt_seq: seq as u8,
            ..ConfigFields::new(
                VD,
                prl,
                rlq,
                REFNUMS[seq * disks_per_bvd..(seq + 1) * disks_per_bvd].to_vec(),
                sectors(COMPONENT),
            )
        };
        let images: Vec<_> = (0..6)
            .map(|slot| {
                let record = bvd(slot / disks_per_bvd);
                disk(
                    member(&fields, REFNUMS[slot], &[record], &vec![0; COMPONENT], 0),
                    None,
                )
            })
            .collect();
        let (stack, assembled) = detect_with_sources(&images);
        assert!(assembled.is_empty(), "{prl:#x}/{rlq:#x}");
        assert_eq!(stack.arrays[0].level, shown, "{prl:#x}/{rlq:#x}");
        assert!(
            mentions(&stack.diagnostics, "not supported"),
            "{prl:#x}/{rlq:#x}: {:?}",
            stack.diagnostics
        );
    }
}

#[test]
fn reconfiguration_in_progress_is_reported_not_assembled() {
    const COMPONENT: usize = 16 * 1024;
    let volume = pattern(COMPONENT, 35);
    let mut fields = container(2, "Morph");
    fields.virtuals[0].state = VD_MORPHING;
    let record = config(0x01, 0x00, 2, COMPONENT, COMPONENT);
    let images = members(&fields, &record, &[volume.clone(), volume], &[0, 1]);
    let (stack, assembled) = detect_with_sources(&images);
    assert!(assembled.is_empty());
    assert!(
        mentions(&stack.diagnostics, "morphing"),
        "{:?}",
        stack.diagnostics
    );
}

#[test]
fn rebuild_from_unverified_parity_needs_consent() {
    const COMPONENT: usize = 32 * 1024;
    let logical = pattern(2 * COMPONENT, 36);
    let data = encode_striped(
        RaidLevel::Raid5(Raid5Layout::LeftSymmetric),
        &logical,
        3,
        CHUNK,
    );
    let mut fields = container(3, "Fresh");
    fields.virtuals[0].init_state = 0;
    fields.disks[2].state = PD_FAILED;
    let record = config(0x05, 0x03, 3, COMPONENT, logical.len());
    let images = members(&fields, &record, &data, &[0, 1]);
    let (stack, assembled) = detect_with_sources(&images);
    assert!(assembled.is_empty());
    assert!(stack.arrays[0].degradable);
    assert!(mentions(&stack.diagnostics, "not fully initialised"));

    let options = DetectOptions {
        degraded_sets: vec![stack.arrays[0].set_uuid.clone()],
    };
    let (stack, assembled) = detect_with_options(&images, &options);
    assert_eq!(read_all(&assembled[0]), logical);
    assert!(mentions(&stack.diagnostics, "may be inconsistent"));
}
