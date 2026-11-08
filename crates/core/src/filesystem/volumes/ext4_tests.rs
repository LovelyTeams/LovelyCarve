//! Application-level content/metadata regressions for the ext capability audit.

use super::ext4::{Ext4Source, Ext4Volume};
use crate::evidence::image::DiskImage;
use std::{io::Write, sync::Arc};

const FIXTURE: &[u8] = include_bytes!("../../../../../vendor/ext4-view/test_data/audit_ext.bin");

#[test]
fn lvm_fragmented_ext4_navigation_full_range_stream_and_physical_runs() {
    use crate::evidence::partitions::lvm2::test_support::fragmented;
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&fragmented(FIXTURE)).unwrap();
    let image = Arc::new(DiskImage::open(file.path()).unwrap());
    let source = Ext4Source::Lvm {
        part_id: 0,
        lv_index: 0,
    };
    let volume = super::ext4::open_ext4_source(&image, source).unwrap();
    assert!(!volume.children(2, true).unwrap().is_empty());
    for number in [12, 15, 19] {
        let inode = volume.read_inode(number).unwrap();
        let full = volume.read_inode_data(&inode, usize::MAX).unwrap();
        let mut streamed = Vec::new();
        volume
            .stream_inode_data(&inode, 4103, |bytes| {
                streamed.extend_from_slice(bytes);
                Ok(())
            })
            .unwrap();
        assert_eq!(streamed, full);
        for offset in [0, 60, 1020, 8190, full.len()] {
            let from = offset.min(full.len());
            let to = (offset + 4097).min(full.len());
            assert_eq!(
                volume
                    .read_inode_range(&inode, offset as u64, 4097)
                    .unwrap(),
                full[from..to]
            );
        }
        for run in volume.physical_data_runs(&inode).unwrap() {
            let count =
                run.length
                    .min(inode.size.saturating_sub(run.logical_offset)) as usize;
            if count > 0 {
                assert_eq!(
                    image.read_range(run.physical_offset, count).unwrap(),
                    full[run.logical_offset as usize..run.logical_offset as usize + count]
                );
            }
        }
    }
    assert_eq!(volume.extended_attributes(16).unwrap()[0].value.len(), 1024);
    assert_eq!(
        volume
            .symlink_target(&volume.read_inode(18).unwrap())
            .unwrap(),
        format!("/{}", "p".repeat(79))
    );
}

#[test]
fn ext4_all_read_paths_agree_for_inline_sparse_indirect_and_unwritten_data() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(FIXTURE).unwrap();
    let image = Arc::new(DiskImage::open(file.path()).unwrap());
    let volume = Ext4Volume::open(
        image,
        Ext4Source::Partition(0),
        0,
        FIXTURE.len() as u64,
        "part:0".into(),
    )
    .unwrap();
    for (number, expected) in [
        (12, (0..80).collect::<Vec<u8>>()),
        (
            15,
            (1..=12)
                .flat_map(|byte| vec![byte; 1024])
                .chain(vec![b'L'; 1024])
                .chain(vec![0; 1024])
                .collect(),
        ),
        (19, [vec![0; 1024], vec![b'W'; 1024]].concat()),
    ] {
        let inode = volume.read_inode(number).unwrap();
        assert_eq!(
            volume.read_inode_data(&inode, usize::MAX).unwrap(),
            expected
        );
        let mut streamed = Vec::new();
        assert_eq!(
            volume
                .stream_inode_data(&inode, 4096, |bytes| {
                    streamed.extend_from_slice(bytes);
                    Ok(())
                })
                .unwrap(),
            expected.len() as u64
        );
        assert_eq!(streamed, expected);
        for (start, length) in [
            (0, 0),
            (57, 10),
            (60, 40),
            (1020, 20),
            (12 * 1024 - 3, 1030),
            (expected.len(), 1),
        ] {
            let from = start.min(expected.len());
            let to = start.saturating_add(length).min(expected.len());
            assert_eq!(
                volume
                    .read_inode_range(&inode, start as u64, length)
                    .unwrap(),
                expected[from..to]
            );
        }
        assert_eq!(
            volume.read_inode_range(&inode, u64::MAX, 10).unwrap(),
            Vec::<u8>::new()
        );
    }
    assert!(volume
        .data_runs(&volume.read_inode(12).unwrap())
        .unwrap()
        .is_empty());
    assert_eq!(
        volume
            .data_runs(&volume.read_inode(15).unwrap())
            .unwrap()
            .last()
            .unwrap()
            .logical_offset,
        12 * 1024
    );
    assert_eq!(
        volume
            .children(13, true)
            .unwrap()
            .into_iter()
            .map(|node| node.name)
            .collect::<Vec<_>>(),
        ["base", "tail"]
    );
    assert_eq!(volume.extended_attributes(16).unwrap()[0].value.len(), 1024);
    assert_eq!(
        volume
            .symlink_target(&volume.read_inode(18).unwrap())
            .unwrap(),
        format!("/{}", "p".repeat(79))
    );
}

#[test]
fn ext4_allocated_data_cannot_cross_the_evidence_volume() {
    let mut bytes = FIXTURE.to_vec();
    let inode = 4096 + 18 * 256;
    bytes[inode + 64 + 8..inode + 64 + 12].copy_from_slice(&64u32.to_le_bytes());
    bytes.extend(vec![b'X'; 1024]); // adjacent partition bytes are present but out of bounds
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&bytes).unwrap();
    let image = Arc::new(DiskImage::open(file.path()).unwrap());
    let volume = Ext4Volume::open(
        image,
        Ext4Source::Partition(0),
        0,
        FIXTURE.len() as u64,
        "part:0".into(),
    )
    .unwrap();
    let inode = volume.read_inode(19).unwrap();
    assert!(volume.read_inode_data(&inode, 2048).is_err());
    assert!(volume.stream_inode_data(&inode, 4096, |_| Ok(())).is_err());
}
