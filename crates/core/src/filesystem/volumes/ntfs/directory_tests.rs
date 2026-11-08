//! Deleted directory indexes are not active directory listings.
use super::*;

#[test]
fn deleted_directory_without_index_lists_even_without_deleted_children() {
    let mut bytes = image();
    record(&mut bytes, 16, &[filename("removed", 1)]);
    bytes[MFT + 16 * 1024 + 22..MFT + 16 * 1024 + 24].copy_from_slice(&2u16.to_le_bytes());
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert!(volume.children(16).unwrap().is_empty());
    assert!(volume.children_with_deleted(16, None).unwrap().is_empty());
    assert!(volume
        .children_with_deleted(16, Some(&NtfsDeletedSnapshot::default()))
        .unwrap()
        .is_empty());
}

#[test]
fn deleted_directory_uses_mft_children_instead_of_damaged_index() {
    let mut bytes = image();
    record(
        &mut bytes,
        16,
        &[filename("removed", 1), named_resident(0x90, "$I30", &[0])],
    );
    bytes[MFT + 16 * 1024 + 22..MFT + 16 * 1024 + 24].copy_from_slice(&2u16.to_le_bytes());
    let mut child = filename("recovered.txt", 1);
    child[24..32].copy_from_slice(&(16u64 | (1 << 48)).to_le_bytes());
    record(&mut bytes, 17, &[child, resident(0x80, b"evidence")]);
    bytes[MFT + 17 * 1024 + 22..MFT + 17 * 1024 + 24].fill(0);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    let snapshot = volume.build_deleted_snapshot().unwrap();
    let children = volume.children_with_deleted(16, Some(&snapshot)).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].name, "recovered.txt");
    assert_eq!(children[0].inode, Some(17));
    assert!(children[0].deleted);
    assert_eq!(volume.read_file(17, 99).unwrap(), b"evidence");
}

#[test]
fn deleted_children_do_not_hide_live_directory_or_parent_record_errors() {
    let mut bytes = image();
    record(&mut bytes, 16, &[filename("live", 1)]);
    bytes[MFT + 16 * 1024 + 22..MFT + 16 * 1024 + 24].copy_from_slice(&3u16.to_le_bytes());
    let mut child = filename("removed.txt", 1);
    child[24..32].copy_from_slice(&(16u64 | (1 << 48)).to_le_bytes());
    record(&mut bytes, 17, &[child]);
    bytes[MFT + 17 * 1024 + 22..MFT + 17 * 1024 + 24].fill(0);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    let snapshot = volume.build_deleted_snapshot().unwrap();
    assert!(snapshot.by_parent.contains_key(&16));
    assert!(volume.children_with_deleted(16, Some(&snapshot)).is_err());

    // A surviving child reference cannot make an unreadable parent valid.
    bytes[MFT + 16 * 1024..MFT + 16 * 1024 + 4].copy_from_slice(b"BAAD");
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert!(volume.children_with_deleted(16, Some(&snapshot)).is_err());
}

#[test]
fn external_deleted_directory_image_regression() -> Result<()> {
    let Ok(path) = std::env::var("NTFS_DIRECTORY_IMAGE") else {
        return Ok(());
    };
    let image = DiskImage::open(&path)?;
    let partition = image
        .partitions
        .first()
        .ok_or_else(|| anyhow!("missing NTFS partition"))?;
    let volume = NtfsVolume::open(&image, partition)?;
    let mut reader = volume.reader();
    let file = volume.ntfs.file(&mut reader, 44964)?;
    assert!(file.is_directory());
    assert!(!file.flags().contains(NtfsFileFlags::IN_USE));
    assert!(matches!(
        file.directory_index(&mut reader),
        Err(ntfs::NtfsError::AttributeNotFound {
            ty: NtfsAttributeType::IndexRoot,
            ..
        })
    ));
    assert!(volume.children(44964)?.is_empty());
    let snapshot = volume.build_deleted_snapshot()?;
    let children = volume.children_with_deleted(44964, Some(&snapshot))?;
    assert_eq!(children.len(), snapshot.for_parent(44964).count());
    assert!(children.iter().all(|child| child.deleted));
    assert!(!volume.root_children()?.is_empty());
    Ok(())
}
