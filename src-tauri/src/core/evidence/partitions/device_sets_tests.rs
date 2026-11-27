use super::*;
use crate::core::evidence::partitions::test_support::BufSource;
use crate::core::filesystem::volumes::btrfs::test_support;

fn partition(id: u8, kind: PartitionKind, start_lba: u64, length_lba: u64) -> Partition {
    Partition {
        id,
        kind,
        start_lba,
        length_lba,
        label: None,
        unique_guid: None,
        amiga_geometry: None,
        member_of: None,
    }
}

/// Device-ordered member images laid out back to back, with their partitions.
fn layout(volumes: &[Vec<u8>], kind: PartitionKind) -> (BufSource, Vec<Partition>) {
    let mut bytes = Vec::new();
    let mut partitions = Vec::new();
    for (id, volume) in volumes.iter().enumerate() {
        let start = bytes.len() as u64 / 512;
        partitions.push(partition(id as u8, kind, start, volume.len() as u64 / 512));
        bytes.extend_from_slice(volume);
    }
    (BufSource(bytes), partitions)
}

fn members_of(partitions: &[Partition]) -> Vec<Option<u8>> {
    partitions
        .iter()
        .map(|partition| partition.member_of)
        .collect()
}

#[test]
fn later_btrfs_devices_point_at_the_first_member_partition() {
    let [first, second] = test_support::striped_members();
    for volumes in [
        vec![first.clone(), second.clone()],
        // Table order decides, not the device ID: device 2 comes first here.
        vec![second, first],
    ] {
        let (source, mut partitions) = layout(&volumes, PartitionKind::Btrfs);
        mark_members(&source, &mut partitions, 512);
        assert_eq!(members_of(&partitions), vec![None, Some(0)]);
        assert!(partitions[0].opens_volume() && !partitions[1].opens_volume());
    }
}

#[test]
fn unrelated_partitions_stay_volumes_between_members() {
    let [first, second] = test_support::striped_members();
    // A single-device Btrfs between the members is a volume of its own.
    let (source, mut partitions) = layout(
        &[first, test_support::image(), second],
        PartitionKind::Btrfs,
    );
    mark_members(&source, &mut partitions, 512);
    assert_eq!(members_of(&partitions), vec![None, None, Some(0)]);
}

#[test]
fn copies_of_a_single_device_filesystem_stay_separate_volumes() {
    let volume = test_support::image();
    let (source, mut partitions) = layout(&[volume.clone(), volume], PartitionKind::Btrfs);
    mark_members(&source, &mut partitions, 512);
    assert_eq!(members_of(&partitions), vec![None, None]);
}

#[test]
fn unreadable_member_headers_leave_the_partitions_untouched() {
    let [first, _] = test_support::striped_members();
    let (source, mut partitions) = layout(&[first, vec![0; 512 * 1024]], PartitionKind::Btrfs);
    mark_members(&source, &mut partitions, 512);
    assert_eq!(members_of(&partitions), vec![None, None]);
}
