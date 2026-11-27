use super::*;

#[test]
fn lvm_fragmented_btrfs_tree_node_crosses_pe_segment_boundary() {
    use crate::core::evidence::partitions::lvm2::{
        header::read_metadata, layout::linear_layout, test_support,
    };
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&test_support::image(&test_support::text(), 1, false))
        .unwrap();
    let image = DiskImage::open(file.path()).unwrap();
    let metadata = read_metadata(&image, 0, image.len()).unwrap();
    let group = &metadata.volume_group;
    let layout = linear_layout(
        group,
        &group.logical_volumes[0],
        &group.physical_volumes[1],
        0,
        image.len(),
    )
    .unwrap();
    let chunks = [ChunkMapping::linear(4096, 7168, 8192)];
    let walker = TreeWalker::new(&image, &chunks, 4096, Some(&layout));
    assert_eq!(
        walker.read_node(4096).unwrap(),
        [vec![0x11; 1024], vec![0x22; 3072]].concat()
    );
}

#[test]
fn cmp_key_orders_lexicographically() {
    let a = Key {
        objectid: 1u64.into(),
        key_type: 1,
        offset: 0u64.into(),
    };
    let b = Key {
        objectid: 1u64.into(),
        key_type: 1,
        offset: 1u64.into(),
    };
    let c = Key {
        objectid: 1u64.into(),
        key_type: 2,
        offset: 0u64.into(),
    };
    let d = Key {
        objectid: 2u64.into(),
        key_type: 0,
        offset: 0u64.into(),
    };
    assert_eq!(cmp_key(&a, &a), Ordering::Equal);
    assert_eq!(cmp_key(&a, &b), Ordering::Less);
    assert_eq!(cmp_key(&b, &c), Ordering::Less);
    assert_eq!(cmp_key(&c, &d), Ordering::Less);
}

#[test]
fn parse_btrfs_id_round_trip() {
    let (source, subvol, ino) = parse_btrfs_id("btrfs:part:3:5:256").expect("parse");
    assert!(matches!(source, BtrfsSource::Partition(3)));
    assert_eq!(subvol, 5);
    assert_eq!(ino, 256);

    let (source, subvol, ino) = parse_btrfs_id("btrfs:lvm:2:7:5:256").expect("parse lvm");
    assert!(matches!(
        source,
        BtrfsSource::Lvm {
            part_id: 2,
            lv_index: 7
        }
    ));
    assert_eq!(subvol, 5);
    assert_eq!(ino, 256);

    assert!(parse_btrfs_id("btrfs:part:3:256").is_none());
    assert!(parse_btrfs_id("btrfs:part:3").is_none());
    assert!(parse_btrfs_id("btrfs:lvm:2:7:5:256:extra").is_none());
    assert!(parse_btrfs_id("btrfs:lvm:2:7:5").is_none());
    assert!(parse_btrfs_id("ext4:part:3:5:256").is_none());
}

#[test]
fn partition_root_subvolume_filter_skips_default_and_default_refs() {
    let independent = SubvolEntry {
        bytenr: 1,
        level: 0,
        name: Some("snapshot".to_string()),
        parent_subvols: Vec::new(),
    };
    let mounted_in_default = SubvolEntry {
        parent_subvols: vec![FS_TREE_OBJECTID],
        ..independent.clone()
    };
    // 嵌套子卷：挂在另一个（存在的）非默认子卷下，例如 @rootfs/root。
    let mounted_in_subvol = SubvolEntry {
        parent_subvols: vec![256],
        ..independent.clone()
    };
    // 悬空引用：父子卷的 ROOT_ITEM 已不存在，等同无主。
    let dangling_parent = SubvolEntry {
        parent_subvols: vec![999],
        ..independent.clone()
    };
    let subvolumes: BTreeMap<u64, SubvolEntry> = [
        (FS_TREE_OBJECTID, independent.clone()),
        (256, mounted_in_default.clone()),
        (257, mounted_in_subvol.clone()),
        (301, independent.clone()),
        (302, dangling_parent.clone()),
    ]
    .into_iter()
    .collect();

    assert!(!should_list_subvol_at_partition_root(
        FS_TREE_OBJECTID,
        FS_TREE_OBJECTID,
        &independent,
        &subvolumes,
    ));
    assert!(!should_list_subvol_at_partition_root(
        256,
        FS_TREE_OBJECTID,
        &mounted_in_default,
        &subvolumes,
    ));
    assert!(!should_list_subvol_at_partition_root(
        257,
        FS_TREE_OBJECTID,
        &mounted_in_subvol,
        &subvolumes,
    ));
    assert!(should_list_subvol_at_partition_root(
        301,
        FS_TREE_OBJECTID,
        &independent,
        &subvolumes,
    ));
    assert!(should_list_subvol_at_partition_root(
        302,
        FS_TREE_OBJECTID,
        &dangling_parent,
        &subvolumes,
    ));
    assert_eq!(
        root_subvolume_name(301, &independent),
        "Subvol_301_snapshot"
    );
}

#[test]
fn decompress_none_is_identity() {
    let bytes = b"abc".as_slice();
    let out = decompress(COMPRESS_NONE, bytes, bytes.len(), 4096).expect("decompress");
    assert_eq!(out, bytes);
}

#[test]
fn decompress_lzo_reads_btrfs_wrapper() {
    let segment = [0x12, 0, 0x20, 0, 0xdf, 0, 0, 0x11, 0, 0];
    let mut frame = Vec::new();
    let total_len = LZO_LEN_SIZE + LZO_LEN_SIZE + segment.len();
    frame.extend_from_slice(&(total_len as u32).to_le_bytes());
    frame.extend_from_slice(&(segment.len() as u32).to_le_bytes());
    frame.extend_from_slice(&segment);

    let out = decompress(COMPRESS_LZO, &frame, 512, 4096).expect("decompress");
    assert_eq!(out, vec![0u8; 512]);
}

#[test]
fn decompress_lzo_rejects_truncated_wrapper() {
    let mut frame = Vec::new();
    frame.extend_from_slice(&18u32.to_le_bytes());
    frame.extend_from_slice(&10u32.to_le_bytes());

    assert!(decompress(COMPRESS_LZO, &frame, 512, 4096).is_err());
}

#[test]
fn btrfs_zstd_ignores_padding_after_the_frame_and_zeroes_the_tail_sector() {
    let expected = vec![b'Z'; 2300];
    let mut frame = zstd::bulk::compress(&expected, 1).unwrap();
    frame.resize(4096, 0);
    assert_eq!(
        decompress(COMPRESS_ZSTD, &frame, expected.len(), 4096).unwrap(),
        expected
    );
    let regular = decompress_regular(COMPRESS_ZSTD, &frame, 4096, 4096).unwrap();
    assert_eq!(regular[..expected.len()], expected);
    assert!(regular[expected.len()..].iter().all(|&byte| byte == 0));
    assert!(decompress_regular(COMPRESS_ZSTD, &frame, 8192, 4096).is_err());
}

#[test]
fn decode_label_strips_trailing_nuls() {
    let mut buf = [0u8; 16];
    buf[..5].copy_from_slice(b"my-fs");
    assert_eq!(decode_label(&buf).as_deref(), Some("my-fs"));
    assert_eq!(decode_label(&[0u8; 16]), None);
}

#[test]
fn btrfs_grouped_raid0_uses_the_member_uuid_and_preserves_sparse_and_physical_runs() {
    use crate::core::evidence::raid::detect_with_sources;
    use std::sync::Arc;

    let member_bytes = test_support::striped_members();
    let directory = tempfile::tempdir().unwrap();
    let images: Vec<_> = member_bytes
        .iter()
        .enumerate()
        .map(|(i, bytes)| {
            let path = directory.path().join(format!("stripe-{i}.img"));
            std::fs::write(&path, bytes).unwrap();
            Arc::new(DiskImage::open(path).unwrap())
        })
        .collect();
    assert!(open_btrfs_source(&images[0], BtrfsSource::Partition(0)).is_err());
    let (incomplete, groups) = detect_with_sources(&images[..1]);
    assert!(groups.is_empty());
    assert!(incomplete
        .diagnostics
        .iter()
        .any(|error| error.contains("1 available")));
    let (stack, groups) = detect_with_sources(&images);
    assert_eq!(groups.len(), 1, "{:?}", stack.diagnostics);
    assert_eq!(stack.arrays[0].metadata_version, "btrfs");
    let group = &groups[0];
    let image = DiskImage::from_assembled(
        group.label.clone(),
        Arc::clone(&group.source),
        group.partitions.clone().unwrap(),
        512,
    )
    .unwrap();
    let volume = open_btrfs_source(&image, BtrfsSource::Partition(0)).unwrap();
    let inode = volume.read_inode(5, 257).unwrap();
    let expected = [vec![0; 4096], vec![0x7a; 4096], vec![0x6b; 4096]].concat();
    assert_eq!(
        volume.read_inode_data(5, &inode, usize::MAX).unwrap(),
        expected
    );
    assert_eq!(
        volume.read_inode_range(5, &inode, 8189, 11).unwrap(),
        expected[8189..8200]
    );
    let mut streamed = Vec::new();
    assert_eq!(
        volume
            .stream_inode_data(5, &inode, 4096, |bytes| {
                streamed.extend_from_slice(bytes);
                Ok(())
            })
            .unwrap(),
        expected.len() as u64
    );
    assert_eq!(streamed, expected);
    let runs = volume.data_runs(5, &inode).unwrap();
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0].logical_offset, 4096);
    assert_eq!(runs[1].logical_offset, 8192);
    assert_eq!(runs[0].physical_offset, 0x4f000);
    assert_eq!(runs[1].physical_offset, 0xc0000);
}

#[test]
fn assembled_btrfs_set_is_one_volume_and_lists_its_second_device_as_a_member() {
    use crate::core::filesystem::fs_all::{vol_id_for_key, FsAll};
    use crate::core::filesystem::tree::fs_tree::{children_for_image, NodeKind};

    let directory = tempfile::tempdir().unwrap();
    let session = test_support::assembled_striped_set(directory.path());
    let members: Vec<_> = session
        .partitions
        .iter()
        .map(|partition| partition.member_of)
        .collect();
    assert_eq!(members, vec![None, Some(0)]);
    let roots: Vec<_> = children_for_image(&session, "")
        .unwrap()
        .into_iter()
        .map(|node| (node.id, node.kind))
        .collect();
    assert_eq!(
        roots,
        vec![
            ("part:0".to_owned(), NodeKind::Dir),
            ("unsupported:1".to_owned(), NodeKind::Unallocated),
        ]
    );
    // The volume holds the file once, and the raw span of both devices.
    let device = 512 * 1024;
    let volume: Vec<_> = children_for_image(&session, "part:0")
        .unwrap()
        .into_iter()
        .map(|node| (node.name, node.id))
        .collect();
    assert_eq!(
        volume
            .iter()
            .filter(|(name, _)| name == "hello.txt")
            .count(),
        1
    );
    let unallocated: Vec<_> = volume
        .iter()
        .filter(|(_, id)| id.starts_with("unalloc:"))
        .cloned()
        .collect();
    assert_eq!(
        unallocated,
        vec![
            (
                "$未分配簇1".to_owned(),
                format!("unalloc:part:0:0x0:{device:#x}")
            ),
            (
                "$未分配簇2".to_owned(),
                format!("unalloc:part:0:{device:#x}:{device:#x}")
            ),
        ]
    );
    for member in ["part:1", "unsupported:1"] {
        let error = children_for_image(&session, member).unwrap_err();
        assert!(format!("{error:#}").contains("part:0"), "{error:#}");
    }
    let volumes: Vec<_> = FsAll::for_image(&session)
        .entries()
        .iter()
        .map(|entry| entry.volume_key.clone())
        .collect();
    assert_eq!(volumes, vec!["part:0"]);
    assert!(vol_id_for_key(&session, "part:1").is_err());
}
