#[test]
fn lvm_fragmented_xfs_cursor_crosses_segments_and_seeks_from_end() {
    use super::*;
    use crate::evidence::partitions::lvm2::test_support;
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&test_support::image(&test_support::text(), 1, false))
        .unwrap();
    let image = Arc::new(DiskImage::open(file.path()).unwrap());
    let volume = LvmVolume::open(&image, &image.partitions[0]).unwrap();
    let (base_offset, layout) = volume.logical_volumes[0].reader_geometry().unwrap();
    let mut cursor = PartitionCursor {
        image,
        base_offset,
        layout,
        length_bytes: 16384,
        position: 0,
    };
    cursor.seek(SeekFrom::Start(8190)).unwrap();
    let mut bytes = [0; 4];
    cursor.read_exact(&mut bytes).unwrap();
    assert_eq!(bytes, [0x11, 0x11, 0x22, 0x22]);
    cursor.seek(SeekFrom::End(-2)).unwrap();
    assert_eq!(cursor.read(&mut bytes).unwrap(), 2);
    assert_eq!(&bytes[..2], &[0x22, 0x22]);
}
use super::*;
use crate::evidence::image::PartitionKind;

#[test]
fn synthetic_xfs_dispatch_preserves_names_and_logical_reads() {
    use crate::filesystem::{evidence_session::EvidenceSession, tree::fs_tree::FsSession};
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(include_bytes!(
        "../../../../../../vendor/xfs-fuse/test_data/minimal-v4.bin"
    ))
    .unwrap();
    let image = Arc::new(EvidenceSession::open(file.path()).unwrap());
    let partition = image
        .partitions
        .iter()
        .find(|partition| partition.kind == PartitionKind::Xfs)
        .unwrap();
    let volume = XfsVolume::open_partition(Arc::clone(&image), partition).unwrap();
    let children = volume.root_children().unwrap();
    let raw_nodes: Vec<_> = children.iter().filter(|node| node.name == "�").collect();
    assert_eq!(raw_nodes.len(), 2);
    assert_ne!(raw_nodes[0].id, raw_nodes[1].id);
    assert!(
        parse_xfs_id(&raw_nodes[0].id).unwrap().1.ends_with(&[0xff])
            || parse_xfs_id(&raw_nodes[0].id).unwrap().1.ends_with(&[0xfe])
    );
    let link = children.iter().find(|node| node.name == "link").unwrap();
    assert_eq!(link.link_target.as_deref(), Some("data"));
    assert!(link.timestamps.birth.is_none());
    assert_eq!(volume.read_file("/link", 2).unwrap(), b"da");
    let data = children.iter().find(|node| node.name == "data").unwrap();
    let mut session = FsSession::new(&image);
    let full = session.read_file_limited(&data.id, usize::MAX).unwrap();
    let mut streamed = Vec::new();
    assert_eq!(
        session
            .stream_file(&data.id, 7, |bytes| {
                streamed.extend_from_slice(bytes);
                Ok(())
            })
            .unwrap(),
        full.len() as u64
    );
    assert_eq!(streamed, full);
    let direct = crate::filesystem::fs_dispatch::dispatch(&image, &data.id)
        .unwrap()
        .unwrap();
    for (offset, length) in [(4093, 8), (8190, 9000), (16380, 5000), (50000, 5)] {
        let start = (offset as usize).min(full.len());
        let expected = &full[start..start.saturating_add(length).min(full.len())];
        assert_eq!(
            direct
                .read_range(&data.id, offset, length)
                .unwrap()
                .unwrap(),
            expected
        );
        assert_eq!(
            session.read_file_range(&data.id, offset, length).unwrap(),
            expected
        );
    }
    assert_eq!(
        session.read_file_limited(&raw_nodes[0].id, 4).unwrap(),
        b"AAAA"
    );
    assert_eq!(session.read_file_limited(&link.id, 4).unwrap(), b"data");
    let stat = session.file_stat(&data.id).unwrap();
    assert_eq!(stat.mode, Some(0x81a4));
    assert_eq!(stat.data_runs.len(), 2);
    assert_eq!(stat.data_runs[0].physical_offset, 8 * 4096);
    assert_eq!(stat.data_runs[1].logical_offset, 4 * 4096);
    assert_eq!(stat.data_runs[1].length, 4096 - 7);
    assert_eq!(
        data.timestamps.accessed.as_deref(),
        Some("1969-12-31T23:59:58.000000123+00:00")
    );
}

#[test]
fn invalid_xfs_open_returns_error_and_cursor_seek_does_not_wrap() {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new().unwrap();
    let mut bytes = vec![0; 512];
    bytes[..4].copy_from_slice(b"XFSB");
    file.write_all(&bytes).unwrap();
    let image = Arc::new(DiskImage::open(file.path()).unwrap());
    assert!(XfsVolume::open(
        Arc::clone(&image),
        XfsSource::Partition(0),
        0,
        512,
        "part:0".into()
    )
    .is_err());
    let mut cursor = PartitionCursor {
        image,
        layout: None,
        base_offset: 0,
        length_bytes: 512,
        position: u64::MAX,
    };
    assert!(cursor.seek(SeekFrom::Current(1)).is_err());
    assert_eq!(cursor.position, u64::MAX);
}

#[test]
fn fragmented_xfs_layout_reads_and_projects_physical_runs() {
    use crate::evidence::partitions::lvm2::layout::ByteSegment;
    use std::io::Write;
    let fixture = include_bytes!("../../../../../../vendor/xfs-fuse/test_data/minimal-v4.bin");
    let split = 8 * 4096 + 128;
    let first = 8192usize;
    let second = 180224usize;
    let mut backing = vec![0; second + fixture.len() - split];
    backing[first..first + split].copy_from_slice(&fixture[..split]);
    backing[second..].copy_from_slice(&fixture[split..]);
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&backing).unwrap();
    let image = Arc::new(DiskImage::open(file.path()).unwrap());
    let layout = LinearLayout {
        size: fixture.len() as u64,
        segments: vec![
            ByteSegment {
                logical_offset: 0,
                physical_offset: first as u64,
                size: split as u64,
            },
            ByteSegment {
                logical_offset: split as u64,
                physical_offset: second as u64,
                size: (fixture.len() - split) as u64,
            },
        ],
    };
    let cursor = PartitionCursor {
        image,
        base_offset: 0,
        layout: Some(layout.clone()),
        length_bytes: fixture.len() as u64,
        position: 0,
    };
    let volume = XfsVolume {
        source: XfsSource::Lvm {
            part_id: 0,
            lv_index: 0,
        },
        root_parent_id: "lvm:0:0".into(),
        volume: Mutex::new(Volume::open(cursor).unwrap()),
        base_offset: 0,
        layout: Some(layout),
    };
    assert_eq!(
        volume.read_file_range("/data", 120, 20).unwrap(),
        vec![b'A'; 20]
    );
    let stat = volume.file_stat("/data", "test").unwrap();
    assert_eq!(stat.data_runs.len(), 3);
    assert_eq!(
        (stat.data_runs[0].physical_offset, stat.data_runs[0].length),
        ((first + 8 * 4096) as u64, 128)
    );
    assert_eq!(
        (
            stat.data_runs[1].physical_offset,
            stat.data_runs[1].logical_offset
        ),
        (second as u64, 128)
    );
    let full = volume.read_file("/data", usize::MAX).unwrap();
    let mut stream = Vec::new();
    volume
        .stream_file("/data", 100, |bytes| {
            stream.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();
    assert_eq!(full, stream);
}

/// Drive the adapter against a real XFS image when the path is supplied
/// via `LOVELYCARVE_XFS_TEST`. Walks the root, samples the first regular
/// file, and confirms `read_file` returns bytes.
#[test]
fn lists_real_xfs_root_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_XFS_TEST") else {
        return;
    };
    let image = Arc::new(
        crate::filesystem::evidence_session::EvidenceSession::open(path)
            .expect("XFS image should open"),
    );
    let xfs_partition = image
        .partitions
        .iter()
        .find(|p| p.kind == PartitionKind::Xfs)
        .cloned()
        .expect("expected at least one XFS partition");
    let volume = XfsVolume::open_partition(Arc::clone(&image), &xfs_partition).expect("open XFS");
    let children = volume.root_children().expect("list XFS root");
    assert!(!children.is_empty(), "XFS root should expose entries");
    if let Some(first_file) = children.iter().find(|node| node.kind == NodeKind::File) {
        let path = format!("/{}", first_file.name);
        let bytes = volume.read_file(&path, 8192).expect("XFS file read");
        assert!(!bytes.is_empty(), "XFS file should yield bytes");
    }
}

/// Regression guard for the xfs-fuse panic on freed/unsupported inodes.
/// A directory entry that points at a zeroed inode (e.g. a runtime
/// `/var/tmp/systemd-private-*` dir on a real CentOS image) used to abort
/// the whole walk with `panic!("Inode type (0o0) not yet supported.")`
/// inside `getattr`; it must now degrade to an `Err`-and-skip so a
/// volume-snapshot / tree browse completes. Drives the live dispatcher
/// (the exact path the GUI takes), bounded so a large volume still
/// finishes the check quickly. Env-gated on `LOVELYCARVE_XFS_TEST`.
#[test]
fn walks_real_xfs_tree_without_panicking_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_XFS_TEST") else {
        return;
    };
    let image = Arc::new(
        crate::filesystem::evidence_session::EvidenceSession::open(path)
            .expect("XFS image should open"),
    );
    let xfs_partition = image
        .partitions
        .iter()
        .find(|p| p.kind == PartitionKind::Xfs)
        .cloned()
        .expect("expected at least one XFS partition");

    let mut stack = vec![format!("part:{}", xfs_partition.id)];
    let mut visited = std::collections::HashSet::new();
    let mut count = 0usize;
    let mut skipped_dirs = 0usize;
    while let Some(id) = stack.pop() {
        if count >= 200_000 {
            break;
        }
        // A dir whose own inode can't be parsed returns Err here (no panic);
        // skip it and keep walking, exactly as the snapshot builder does.
        let children = match crate::filesystem::tree::fs_tree::children_for_image(&image, &id) {
            Ok(children) => children,
            Err(_) => {
                skipped_dirs += 1;
                continue;
            }
        };
        for child in children {
            count += 1;
            if child.kind == NodeKind::Dir && visited.insert(child.id.clone()) {
                stack.push(child.id);
            }
        }
    }
    assert!(
        count > 100,
        "should enumerate a non-trivial XFS tree without panicking, saw {count}"
    );
    let _ = skipped_dirs;
}

#[test]
fn reads_real_lvm_xfs_after_plain_xfs_when_env_path_is_set() {
    let Some(path) = std::env::var_os("LOVELYCARVE_XFS_LVM_TEST") else {
        return;
    };
    let image = Arc::new(
        crate::filesystem::evidence_session::EvidenceSession::open(path)
            .expect("XFS image should open"),
    );
    let xfs_partition = image
        .partitions
        .iter()
        .find(|partition| partition.kind == PartitionKind::Xfs)
        .cloned()
        .expect("expected a plain XFS partition");
    let boot_volume =
        XfsVolume::open_partition(Arc::clone(&image), &xfs_partition).expect("open boot XFS");
    assert!(
        !boot_volume
            .root_children()
            .expect("list boot XFS root")
            .is_empty(),
        "boot XFS root should expose entries"
    );

    let lvm_partition = image
        .partitions
        .iter()
        .find(|partition| partition.kind == PartitionKind::Lvm)
        .cloned()
        .expect("expected an LVM partition");
    let lvm = LvmVolume::open(&image, &lvm_partition).expect("open LVM");
    let mut root_volume = None;
    for logical_volume in lvm
        .logical_volumes
        .iter()
        .filter(|volume| volume.kind == PartitionKind::Xfs)
    {
        let volume = open_xfs_source(
            &image,
            XfsSource::Lvm {
                part_id: lvm_partition.id,
                lv_index: logical_volume.index,
            },
        )
        .expect("open LVM XFS volume");
        let root = volume.root_children().expect("list LVM XFS root");
        if root
            .iter()
            .any(|node| node.kind == NodeKind::Dir && node.name == "etc")
        {
            root_volume = Some(volume);
            break;
        }
    }
    let volume = root_volume.expect("expected an LVM XFS root containing /etc");
    let etc = volume.children("/etc").expect("list LVM XFS /etc");
    assert!(!etc.is_empty(), "/etc should expose entries");
    if let Some(first_file) = etc
        .iter()
        .find(|node| node.kind == NodeKind::File && node.size > 0 && node.size < 8192)
    {
        let path = format!("/etc/{}", first_file.name);
        let bytes = volume
            .read_file(&path, 8192)
            .unwrap_or_else(|err| panic!("read {path} through LVM XFS: {err}"));
        assert!(!bytes.is_empty(), "{path} should yield bytes");
    }
}
