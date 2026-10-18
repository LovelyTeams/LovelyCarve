use super::*;
use crate::core::evidence::raid::mdadm_super::test_support::V1Fields;
use crate::core::evidence::raid::mdadm_super::{parse_v1, FEATURE_RESHAPE_ACTIVE};
use crate::core::evidence::raid::source::test_support::VecMember;

fn member(fields: &V1Fields, image: &str, data: Vec<u8>) -> AssembledMember {
    let sb = parse_v1(&fields.encode(), "1.2").expect("superblock");
    let length = data.len() as u64;
    AssembledMember {
        sb,
        origin: RaidMemberOrigin::Whole {
            image_id: image.into(),
            partition_id: 0,
            byte_offset: 0,
            byte_length: length,
        },
        reader: Arc::new(VecMember(data)),
    }
}

fn mirror_fields(dev_number: u32) -> V1Fields {
    V1Fields {
        dev_number,
        data_offset_sectors: 0,
        data_size_sectors: 8,
        size_sectors: 8,
        ..V1Fields::new(1, 2)
    }
}

#[test]
fn the_same_device_loaded_twice_is_read_once() {
    let data = vec![0x5A; 4096];
    let members = vec![
        member(&mirror_fields(0), "a", data.clone()),
        member(&mirror_fields(0), "a-copy", data.clone()),
        member(&mirror_fields(1), "b", data.clone()),
    ];
    let outcome = assemble_md_set("set", members, false).expect("outcome");
    let states: Vec<_> = outcome.array.members.iter().map(|m| m.state).collect();
    assert_eq!(
        states,
        [
            RaidMemberState::Active,
            RaidMemberState::Duplicate,
            RaidMemberState::Active
        ]
    );
    let volume = outcome.volume.expect("assembled");
    assert_eq!(volume.source.read(0, 4096).expect("read"), data);
}

#[test]
fn two_devices_claiming_one_slot_block_assembly() {
    let other = V1Fields {
        device_uuid: [0xCC; 16],
        ..mirror_fields(0)
    };
    let members = vec![
        member(&mirror_fields(0), "a", vec![1; 4096]),
        member(
            &V1Fields {
                events: 10,
                ..other
            },
            "b",
            vec![2; 4096],
        ),
        member(&mirror_fields(1), "c", vec![1; 4096]),
    ];
    let outcome = assemble_md_set("set", members, false).expect("outcome");
    assert!(outcome.volume.is_none());
    assert!(outcome
        .diagnostics
        .iter()
        .any(|message| message.contains("both claim slot 0")));
}

#[test]
fn reshape_in_progress_is_reported_not_assembled() {
    let fields = |dev| V1Fields {
        feature_map: FEATURE_RESHAPE_ACTIVE,
        ..mirror_fields(dev)
    };
    let members = vec![
        member(&fields(0), "a", vec![0; 4096]),
        member(&fields(1), "b", vec![0; 4096]),
    ];
    let outcome = assemble_md_set("set", members, true).expect("outcome");
    assert!(outcome.volume.is_none());
    assert!(outcome
        .diagnostics
        .iter()
        .any(|message| message.contains("reshape")));
}

#[test]
fn unsupported_level_stays_descriptive() {
    let fields = V1Fields {
        level: -4, // multipath
        ..mirror_fields(0)
    };
    let outcome =
        assemble_md_set("set", vec![member(&fields, "a", vec![0; 4096])], true).expect("outcome");
    assert!(outcome.volume.is_none());
    assert_eq!(outcome.array.level, -4);
}

#[test]
fn rebuilding_member_does_not_need_consent() {
    use crate::core::evidence::raid::mdadm_super::FEATURE_RECOVERY_OFFSET;
    let data = vec![0x33; 4096];
    let rebuilding = V1Fields {
        feature_map: FEATURE_RECOVERY_OFFSET,
        recovery_offset_sectors: 2,
        ..mirror_fields(1)
    };
    let members = vec![
        member(&mirror_fields(0), "a", data.clone()),
        member(&rebuilding, "b", vec![0; 4096]),
    ];
    let outcome = assemble_md_set("set", members, false).expect("outcome");
    assert_eq!(outcome.array.members[1].state, RaidMemberState::Rebuilding);
    assert!(outcome.array.degraded);
    let volume = outcome.volume.expect("assembled");
    assert_eq!(volume.source.read(0, 4096).expect("read"), data);
}

#[test]
fn capacity_estimates_follow_the_level() {
    let sb = |level, layout, raid_disks| {
        let fields = V1Fields {
            layout,
            size_sectors: 1000,
            data_size_sectors: 1200,
            chunk_sectors: 128,
            ..V1Fields::new(level, raid_disks)
        };
        parse_v1(&fields.encode(), "1.2").expect("superblock")
    };
    // RAID5/6 use whole chunks of the per-device size.
    assert_eq!(estimate_capacity(&sb(5, 2, 4)), 896 * 512 * 3);
    assert_eq!(estimate_capacity(&sb(6, 2, 4)), 896 * 512 * 2);
    assert_eq!(estimate_capacity(&sb(1, 0, 3)), 1000 * 512);
    assert_eq!(estimate_capacity(&sb(10, 0x102, 4)), 1000 * 512 * 2);
    assert_eq!(estimate_capacity(&sb(0, 0, 2)), 1200 * 512 * 2);
}
