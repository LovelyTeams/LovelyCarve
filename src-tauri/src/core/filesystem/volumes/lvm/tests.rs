use super::*;
use crate::core::evidence::partitions::lvm2::test_support;
use crate::core::filesystem::{evidence_session::EvidenceSession, tree::fs_tree::FsSession};
use std::{io::Write, sync::Arc};

#[test]
fn lvm_btrfs_is_a_directory_and_reads_by_its_discovered_nodes() {
    use crate::core::filesystem::{
        tree::fs_tree::children_for_image, volumes::btrfs::test_support as btrfs,
    };
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&test_support::fragmented(&btrfs::image()))
        .unwrap();
    let image = Arc::new(EvidenceSession::open(file.path()).unwrap());
    let nodes = children_for_image(&image, "part:0").unwrap();
    let volume = nodes.iter().find(|node| node.name == "root").unwrap();
    assert_eq!(volume.kind, NodeKind::Dir);
    let children = children_for_image(&image, &volume.id).unwrap();
    let entry = children
        .iter()
        .find(|node| node.name == "hello.txt")
        .unwrap();
    let mut session = FsSession::new(&image);
    assert_eq!(
        session.read_file_limited(&entry.id, usize::MAX).unwrap(),
        btrfs::CONTENT
    );
    assert_eq!(
        session.read_file_range(&entry.id, 6, 7).unwrap(),
        &btrfs::CONTENT[6..13]
    );
}

#[test]
fn lvm_raw_nodes_share_read_stream_range_and_original_runs() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&test_support::image(&test_support::text(), 1, false))
        .unwrap();
    let image = Arc::new(EvidenceSession::open(file.path()).unwrap());
    let mut session = FsSession::new(&image);
    let expected = [vec![0x11; 8192], vec![0x22; 8192]].concat();
    assert_eq!(
        session.read_file_limited("lvm:0:0", usize::MAX).unwrap(),
        expected
    );
    assert_eq!(
        session.read_file_range("lvm:0:0", 8190, 4).unwrap(),
        [0x11, 0x11, 0x22, 0x22]
    );
    let mut streamed = Vec::new();
    assert_eq!(
        session
            .stream_file("lvm:0:0", 3001, |bytes| {
                streamed.extend_from_slice(bytes);
                Ok(())
            })
            .unwrap(),
        16384
    );
    assert_eq!(streamed, expected);
    let stat = session.file_stat("lvm:0:0").unwrap();
    assert_eq!(stat.size, 16384);
    assert_eq!(stat.data_runs.len(), 2);
    let original: Vec<_> = stat
        .data_runs
        .iter()
        .flat_map(|run| {
            image
                .read_range(run.physical_offset, run.length as usize)
                .unwrap()
        })
        .collect();
    assert_eq!(original, expected);
    let volume = LvmVolume::open(&image, &image.partitions[0]).unwrap();
    let free = volume.vg_unalloc_nodes();
    assert_eq!(free.len(), 2);
    assert_eq!(free.iter().map(|node| node.size).sum::<u64>(), 12 * 4096);
    assert_eq!(
        volume.logical_volume_nodes().unwrap()[0].kind,
        NodeKind::File
    );
}

#[test]
fn lvm_4kn_geometry_uses_fixed_512_byte_metadata_units() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&test_support::image(&test_support::text(), 3, false))
        .unwrap();
    let mut image = DiskImage::open(file.path()).unwrap();
    image.sector_size = 4096;
    image.partitions[0].length_lba /= 8;
    let volume = LvmVolume::open(&image, &image.partitions[0]).unwrap();
    assert_eq!(volume.logical_volumes[0].size_bytes, 16384);
    assert_eq!(
        volume.logical_volumes[0].absolute_offset,
        test_support::PE_START as u64 + 12 * 4096
    );
}

#[test]
fn lvm_unsupported_and_missing_pv_volumes_remain_visible_and_fail_reads() {
    for text in [
        test_support::text().replace("type = \"striped\"", "type = \"thin\""),
        test_support::text().replace("\"pv1\", 12", "\"pv0\", 12"),
    ] {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&test_support::image(&text, 1, false))
            .unwrap();
        let image = Arc::new(EvidenceSession::open(file.path()).unwrap());
        let volume = LvmVolume::open(&image, &image.partitions[0]).unwrap();
        assert_eq!(volume.logical_volumes.len(), 1);
        assert_eq!(volume.logical_volumes[0].size_bytes, 16384);
        assert!(volume.logical_volumes[0].readable_layout().is_err());
        assert!(!volume.diagnostics.is_empty());
        assert!(volume.vg_unalloc_nodes().is_empty());
        let mut session = FsSession::new(&image);
        assert!(session.read_file_range("lvm:0:0", 0, 1).is_err());
    }
}

#[test]
fn lvm_source_runs_preserve_large_sparse_file_offsets_without_wrapping() {
    use crate::core::evidence::partitions::{test_support::BufSource, SectorSource};
    let source = BufSource(test_support::image(&test_support::text(), 1, false));
    let metadata =
        crate::core::evidence::partitions::lvm2::header::read_metadata(&source, 0, source.size())
            .unwrap();
    let group = &metadata.volume_group;
    let layout = linear_layout(
        group,
        &group.logical_volumes[0],
        &group.physical_volumes[1],
        0,
        source.size(),
    )
    .unwrap();
    let runs = physical_runs(
        Some(&layout),
        vec![FsDataRun {
            logical_offset: u64::MAX - 10,
            physical_offset: 8190,
            length: 4,
        }],
    )
    .unwrap();
    assert_eq!(
        runs.iter()
            .map(|run| run.logical_offset)
            .collect::<Vec<_>>(),
        [u64::MAX - 10, u64::MAX - 8]
    );
    assert!(physical_runs(
        Some(&layout),
        vec![FsDataRun {
            logical_offset: u64::MAX - 1,
            physical_offset: 8190,
            length: 4
        }]
    )
    .is_err());
}
