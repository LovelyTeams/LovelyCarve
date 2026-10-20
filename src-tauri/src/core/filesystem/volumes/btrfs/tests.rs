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
    let chunks = [ChunkMapping {
        logical: 4096,
        physical: 7168,
        length: 8192,
    }];
    let walker = TreeWalker::new(&image, 0, &chunks, 4096, Some(&layout));
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
fn decode_label_strips_trailing_nuls() {
    let mut buf = [0u8; 16];
    buf[..5].copy_from_slice(b"my-fs");
    assert_eq!(decode_label(&buf).as_deref(), Some("my-fs"));
    assert_eq!(decode_label(&[0u8; 16]), None);
}
