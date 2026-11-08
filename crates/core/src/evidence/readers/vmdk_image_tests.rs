use super::*;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor/vmdk/tests/data")
        .join(name)
}

#[test]
fn vmdk_committed_images_decode_and_seek_to_expected_bytes() {
    for (name, size, modulo) in [
        ("minimal.vmdk", 1024 * 1024, 1),
        ("mono_flat.vmdk", 1024 * 1024, 1),
        ("flat.vmdk", 1024 * 1024, 1),
        ("tw_sparse.vmdk", 4 * 1024 * 1024, 1),
        ("tw_sparse_data.vmdk", 4 * 1024 * 1024, 256),
        ("compressed_stream_opt.vmdk", 65536, 64),
    ] {
        let mut reader = VmdkHandle::open(&fixture(name)).unwrap();
        let expected: Vec<u8> = (0..size).map(|index| (index % modulo) as u8).collect();
        let mut actual = vec![0xff; size];
        assert_eq!(reader.read_at(0, &mut actual).unwrap(), size, "{name}");
        assert_eq!(actual, expected, "{name}");
        for offset in [1, 511, 4093, 65530] {
            let mut bytes = [0; 100];
            let count = reader.read_at(offset, &mut bytes).unwrap();
            assert_eq!(
                &bytes[..count],
                &expected[offset as usize..offset as usize + count],
                "{name}"
            );
        }
        assert_eq!(reader.read_at(size as u64, &mut actual).unwrap(), 0);
    }
}

#[test]
fn vmdk_unicode_descriptor_offsets_extents_and_snapshots() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("磁盘.vmdk");
    std::fs::write(
        directory.path().join("数据.bin"),
        [vec![1; 512], vec![2; 512]].concat(),
    )
    .unwrap();
    std::fs::write(&path, "\u{feff}\n  # Disk DescriptorFile\ncreateType = \"monolithicFlat\"\nRW 1 FLAT \"数据.bin\" 1 # start at sector 1\nRW 1 ZERO\nRW 1 FLAT \"数据.bin\" 0\n").unwrap();
    let mut reader = VmdkHandle::open(&path).unwrap();
    let mut data = vec![0; 1536];
    reader.read_at(0, &mut data).unwrap();
    assert_eq!(data, [vec![2; 512], vec![0; 512], vec![1; 512]].concat());
    assert_eq!(reader.file_snapshots().len(), 2);
    assert_eq!(
        reader.files_on_disk_bytes(),
        Some(std::fs::metadata(path).unwrap().len() + 1024)
    );
}

fn sparse_layer(cid: u32, parent: Option<(u32, &str)>, entries: [u32; 2]) -> Vec<u8> {
    let mut data = vec![0; 24 * 512];
    data[..4].copy_from_slice(b"KDMV");
    data[4..8].copy_from_slice(&1u32.to_le_bytes());
    data[8..12].copy_from_slice(&4u32.to_le_bytes()); // explicit zero-grain support
    data[12..20].copy_from_slice(&16u64.to_le_bytes());
    data[20..28].copy_from_slice(&8u64.to_le_bytes());
    data[28..36].copy_from_slice(&1u64.to_le_bytes()); // descriptor
    data[36..44].copy_from_slice(&1u64.to_le_bytes());
    data[44..48].copy_from_slice(&4u32.to_le_bytes());
    data[56..64].copy_from_slice(&2u64.to_le_bytes()); // GD
    let (parent_cid, hint) = parent.unwrap_or((u32::MAX, ""));
    let descriptor = format!("# Disk DescriptorFile\ncreateType=\"monolithicSparse\"\nCID={cid:08x}\nparentCID={parent_cid:08x}\nparentFileNameHint=\"{hint}\"\n");
    data[512..512 + descriptor.len()].copy_from_slice(descriptor.as_bytes());
    data[1024..1028].copy_from_slice(&3u32.to_le_bytes());
    for (index, entry) in entries.iter().enumerate() {
        data[1536 + index * 4..1540 + index * 4].copy_from_slice(&entry.to_le_bytes());
    }
    data[4096..8192].fill(0x41);
    data[8192..12288].fill(0x42);
    data
}

#[test]
fn vmdk_snapshot_inherits_holes_but_zero_marker_masks_parent() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().join("base.vmdk");
    let delta = directory.path().join("delta.vmdk");
    std::fs::write(&base, sparse_layer(1, None, [8, 16])).unwrap();
    std::fs::write(&delta, sparse_layer(2, Some((1, "base.vmdk")), [0, 1])).unwrap();
    let mut reader = VmdkHandle::open(&delta).unwrap();
    let mut data = vec![0xff; 8192];
    reader.read_at(0, &mut data).unwrap();
    assert_eq!(data, [vec![0x41; 4096], vec![0; 4096]].concat());
    assert_eq!(reader.file_snapshots().len(), 2);
}

#[test]
fn vmdk_missing_mismatched_and_cyclic_parents_fail() {
    let directory = tempfile::tempdir().unwrap();
    let base = directory.path().join("base.vmdk");
    let delta = directory.path().join("delta.vmdk");
    std::fs::write(&delta, sparse_layer(2, Some((1, "base.vmdk")), [0, 0])).unwrap();
    assert!(VmdkHandle::open(&delta).is_err());
    std::fs::write(&base, sparse_layer(3, None, [8, 16])).unwrap();
    assert!(VmdkHandle::open(&delta)
        .unwrap_err()
        .to_string()
        .contains("CID mismatch"));
    std::fs::write(&base, sparse_layer(1, Some((2, "delta.vmdk")), [8, 16])).unwrap();
    assert!(VmdkHandle::open(&delta)
        .unwrap_err()
        .to_string()
        .contains("cycle"));
}

#[test]
fn vmdk_missing_extent_is_an_open_error_and_truncation_is_a_read_fault() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("disk.vmdk");
    std::fs::write(
        &path,
        "# Disk DescriptorFile\ncreateType=\"monolithicFlat\"\nRW 2 FLAT \"extent.bin\" 0\n",
    )
    .unwrap();
    assert!(VmdkHandle::open(&path).is_err());
    std::fs::write(directory.path().join("extent.bin"), [0x41; 512]).unwrap();
    let image = crate::evidence::image::DiskImage::open(&path).unwrap();
    let (_, report) = image.collect_read_faults(|| image.read_range(512, 512));
    assert!(report.read_faults_detected);
}

#[test]
fn vmdk_multi_extent_delta_is_rejected_explicitly() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("delta.vmdk");
    std::fs::write(
        directory.path().join("part.vmdk"),
        sparse_layer(2, None, [0, 0]),
    )
    .unwrap();
    std::fs::write(&path, "# Disk DescriptorFile\ncreateType=\"twoGbMaxExtentSparse\"\nCID=00000002\nparentCID=00000001\nparentFileNameHint=\"base.vmdk\"\nRW 16 SPARSE \"part.vmdk\"\n").unwrap();
    assert!(VmdkHandle::open(&path)
        .unwrap_err()
        .to_string()
        .contains("multi-extent delta"));
}
