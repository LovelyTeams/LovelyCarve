//! Additional libvshadow API and integrity regressions, using synthetic bytes.

use super::*;
use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

#[test]
fn signature_probe_restores_cursor_and_does_not_parse_catalog() {
    let mut image = build_image(true);
    image[CATALOG_OFF as usize] = 0;
    let mut cursor = Cursor::new(image);
    cursor.set_position(123);
    assert!(crate::check_volume_signature(&mut cursor).unwrap());
    assert_eq!(cursor.position(), 123);
    assert!(!crate::check_volume_signature(&mut Cursor::new(vec![0; 20])).unwrap());
}

#[test]
fn volume_header_versions_and_raw_fields() {
    let mut image = build_image(true);
    wr(&mut image, 0x1e00 + 16, &2u32.to_le_bytes());
    wr(&mut image, 0x1e00 + 56, &12345u64.to_le_bytes());
    wr(&mut image, 0x1e00 + 64, &STORE_ID);
    wr(&mut image, 0x1e00 + 80, &SET_ID);
    let volume = VssVolume::open(Cursor::new(image.clone())).unwrap();
    let header = volume.header().unwrap();
    assert_eq!((header.version, header.maximum_size), (2, 12345));
    assert_eq!(
        (header.volume_identifier, header.storage_volume_identifier),
        (STORE_ID, SET_ID)
    );
    for (field, value) in [(16, 3u32), (20, 2u32)] {
        let mut corrupt = image.clone();
        wr(&mut corrupt, 0x1e00 + field, &value.to_le_bytes());
        assert!(VssVolume::open(Cursor::new(corrupt)).is_err());
    }
}

#[test]
fn catalog_rejects_wrong_version_duplicate_and_orphan_identities() {
    let image = build_image(true);
    let catalog = CATALOG_OFF as usize;
    let mut bad = image.clone();
    wr(&mut bad, catalog + 16, &2u32.to_le_bytes());
    assert!(VssVolume::open(Cursor::new(bad)).is_err());
    let mut bad = image.clone();
    wr(&mut bad, catalog + 384, &4u64.to_le_bytes());
    assert!(VssVolume::open(Cursor::new(bad)).is_err());
    for entry in [128, 256] {
        let mut bad = image.clone();
        let duplicate = bad[catalog + entry..catalog + entry + 128].to_vec();
        wr(&mut bad, catalog + 384, &duplicate);
        assert!(VssVolume::open(Cursor::new(bad)).is_err());
    }
    let mut bad = image;
    wr(&mut bad, catalog + 256 + 16, &SET_ID);
    assert!(VssVolume::open(Cursor::new(bad)).is_err());
}

#[test]
fn truncated_headers_and_short_underlying_reads_are_errors() {
    let mut image = vec![0; 0x1e00 + 16];
    wr(&mut image, 0x1e00, &VSS_IDENTIFIER);
    assert!(matches!(
        VssVolume::open(Cursor::new(image)),
        Err(VssError::CorruptVolumeMetadata { .. })
    ));
    // Seek advertises the complete volume; the evidence backend fails halfway
    // through the catalog block, independently of the metadata bounds checks.
    struct ShortSource(Cursor<Vec<u8>>);
    impl Read for ShortSource {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            if self.0.position() >= CATALOG_OFF + 1024 {
                return Ok(0);
            }
            let count = buffer
                .len()
                .min((CATALOG_OFF + 1024 - self.0.position()) as usize);
            self.0.read(&mut buffer[..count])
        }
    }
    impl Seek for ShortSource {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            self.0.seek(position)
        }
    }
    assert!(matches!(
        VssVolume::open(ShortSource(Cursor::new(build_image(true)))),
        Err(VssError::Io(_))
    ));
    let image = build_chain_image(&single_store(vec![plain(4, 32)], vec![]), &[]);
    let layout = VssVolume::open(Cursor::new(image))
        .unwrap()
        .snapshot_layout(0)
        .unwrap();
    assert!(layout
        .read_at(block(4), &mut [0; 10], &mut |_, _| {
            Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "truncated evidence",
            ))
        })
        .is_err());
}

#[test]
fn catalog_limit_returns_an_error_instead_of_a_partial_success() {
    // Generate a long chain on demand so this test needs one block, not a
    // 64 MiB fixture. Every block is valid but the catalog exceeds its budget.
    struct LongCatalog {
        position: u64,
        header: Vec<u8>,
    }
    impl Read for LongCatalog {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            buffer.fill(0);
            if self.position == 0x1e00 {
                buffer.copy_from_slice(&self.header[0x1e00..0x1e00 + buffer.len()]);
            } else {
                wr(buffer, 0, &VSS_IDENTIFIER);
                wr(buffer, 16, &1u32.to_le_bytes());
                wr(buffer, 20, &2u32.to_le_bytes());
                wr(buffer, 40, &(self.position + RBS as u64).to_le_bytes());
            }
            self.position += buffer.len() as u64;
            Ok(buffer.len())
        }
    }
    impl Seek for LongCatalog {
        fn seek(&mut self, from: SeekFrom) -> std::io::Result<u64> {
            let end = ((crate::catalog::MAX_CATALOG_BLOCKS + 2) * RBS) as u64;
            let position = match from {
                SeekFrom::Start(offset) => i128::from(offset),
                SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
                SeekFrom::End(delta) => i128::from(end) + i128::from(delta),
            };
            self.position =
                u64::try_from(position).map_err(|_| std::io::ErrorKind::InvalidInput)?;
            Ok(self.position)
        }
    }
    assert!(matches!(
        VssVolume::open(LongCatalog {
            position: 0,
            header: build_image(false)
        }),
        Err(VssError::CorruptVolumeMetadata {
            reason: "catalog exceeds the block limit",
            ..
        })
    ));
}

#[test]
fn catalog_pairs_reordered_entries_across_blocks_and_preserves_pointer_fields() {
    let mut image = build_image(true);
    let catalog = CATALOG_OFF as usize;
    let pointer = image[catalog + 256..catalog + 384].to_vec();
    image[catalog + 256..catalog + 384].fill(0);
    let second = STORE_BITMAP_OFF as usize;
    store_block_hdr(&mut image, second as u64, 2, 0);
    wr(&mut image, catalog + 40, &(second as u64).to_le_bytes());
    wr(&mut image, second + 128, &pointer);
    wr(&mut image, second + 128 + 40, &0x1234u64.to_le_bytes());
    wr(
        &mut image,
        second + 128 + 56,
        &0x1234_5678_9abc_def0_u64.to_le_bytes(),
    );
    wr(&mut image, second + 128 + 64, &0x9876u64.to_le_bytes());
    let volume = VssVolume::open(Cursor::new(image.clone())).unwrap();
    let descriptor = &volume.stores()[0];
    assert!(descriptor.has_in_volume_data());
    assert_eq!(descriptor.store_block_range_list_offset, Some(0x1234));
    assert_eq!(descriptor.ntfs_file_reference, Some(0x1234_5678_9abc_def0));
    assert_eq!(descriptor.allocated_size, Some(0x9876));
    // Move the type-2 descriptor behind the type-3 pointer.
    let snapshot = image[catalog + 128..catalog + 256].to_vec();
    image[catalog + 128..catalog + 256].fill(0);
    wr(&mut image, second + 256, &snapshot);
    let volume = VssVolume::open(Cursor::new(image)).unwrap();
    assert_eq!(volume.stores()[0].store_header_offset, Some(STORE_HDR_OFF));
    assert!(!VssVolume::open(Cursor::new(build_image(false)))
        .unwrap()
        .stores()[0]
        .has_in_volume_data());
}

#[test]
fn store_information_ignores_unreliable_size_but_checks_header_and_strings() {
    for size in [0, 1, u64::MAX] {
        let mut image = build_image(true);
        wr(&mut image, STORE_HDR_OFF as usize + 48, &size.to_le_bytes());
        let info = VssVolume::open(Cursor::new(image))
            .unwrap()
            .store_info(0)
            .unwrap();
        assert_eq!(info.shadow_copy_id_string(), SHADOW_ID_STR);
        assert_eq!(info.operating_machine, OP_MACHINE);
    }
    for (field, value) in [(0, 0u32), (16, 2u32), (20, 3u32)] {
        let mut image = build_image(true);
        wr(
            &mut image,
            STORE_HDR_OFF as usize + field,
            &value.to_le_bytes(),
        );
        assert!(VssVolume::open(Cursor::new(image))
            .unwrap()
            .store_info(0)
            .is_err());
    }
    for length in [1u16, u16::MAX - 1] {
        let mut image = build_image(true);
        wr(
            &mut image,
            STORE_HDR_OFF as usize + 128 + 64,
            &length.to_le_bytes(),
        );
        assert!(VssVolume::open(Cursor::new(image))
            .unwrap()
            .store_info(0)
            .is_err());
    }
    assert!(StoreInfo::parse_checked(&[0; 67]).is_err());
    let mut image = build_image(true);
    let service_length = STORE_HDR_OFF as usize + 128 + 66 + utf16le(OP_MACHINE).len();
    wr(&mut image, service_length, &u16::MAX.to_le_bytes());
    assert!(VssVolume::open(Cursor::new(image))
        .unwrap()
        .store_info(0)
        .is_err());
}

#[test]
fn raw_blocks_keep_replaced_ignored_and_overlay_descriptors() {
    let descriptors = vec![
        plain(4, 32),
        plain(4, 33),
        overlay(4, 34, 5),
        not_used(plain(5, 35)),
    ];
    let image = build_chain_image(&single_store(descriptors.clone(), vec![]), &[]);
    let mut volume = VssVolume::open(Cursor::new(image)).unwrap();
    let raw = volume.store_blocks(0).unwrap();
    assert_eq!(raw.len(), 4);
    assert_eq!(volume.store_map(0).unwrap().block_count(), 1);
    assert_eq!(raw[0].store_offset, descriptors[0].store);
    assert_eq!(raw[2].allocation_bitmap, 5);
    assert!(raw[3].flags.is_not_used());
    assert!(volume.store_blocks(1).is_err());
}

#[test]
fn unaligned_descriptors_cannot_alias_another_original_block() {
    for field in [0, 16] {
        let mut image = build_chain_image(&single_store(vec![plain(4, 32)], vec![]), &[]);
        wr(
            &mut image,
            list_block(0) * RBS + 128 + field,
            &1u64.to_le_bytes(),
        );
        assert!(VssVolume::open(Cursor::new(image))
            .unwrap()
            .snapshot_layout(0)
            .is_err());
    }
    let mut image = build_chain_image(&single_store(vec![forwarder(4, 5)], vec![]), &[]);
    wr(
        &mut image,
        list_block(0) * RBS + 128 + 8,
        &1u64.to_le_bytes(),
    );
    assert!(VssVolume::open(Cursor::new(image))
        .unwrap()
        .snapshot_layout(0)
        .is_err());
}

#[test]
fn range_list_chain_and_integrity() {
    let mut image = build_chain_image(&single_store(vec![], vec![]), &[]);
    let pointer = R_CATALOG * RBS + 256;
    wr(&mut image, pointer + 40, &block(60).to_le_bytes());
    store_block_hdr(&mut image, block(60), 5, block(61));
    store_block_hdr(&mut image, block(61), 5, 0);
    for (number, store, relative) in [(60, 40, 0), (61, 41, 1)] {
        let record = number * RBS + 128;
        wr(&mut image, record, &block(store).to_le_bytes());
        wr(&mut image, record + 8, &block(relative).to_le_bytes());
        wr(&mut image, record + 16, &(RBS as u64).to_le_bytes());
    }
    let mut volume = VssVolume::open(Cursor::new(image.clone())).unwrap();
    assert_eq!(volume.store_ranges(0).unwrap().len(), 2);
    assert!(volume.store_ranges(1).is_err());
    for (offset, bytes) in [
        (60 * RBS + 40, block(60).to_le_bytes()),
        (60 * RBS + 128 + 16, u64::MAX.to_le_bytes()),
        (60 * RBS + 128, 1u64.to_le_bytes()),
    ] {
        let mut bad = image.clone();
        wr(&mut bad, offset, &bytes);
        assert!(VssVolume::open(Cursor::new(bad))
            .unwrap()
            .store_ranges(0)
            .is_err());
    }
    let mut volume = VssVolume::open(Cursor::new(build_chain_image(
        &single_store(vec![], vec![]),
        &[],
    )))
    .unwrap();
    assert_eq!(volume.store_ranges(0).unwrap(), []);
}

fn set_ntfs_geometry(image: &mut [u8], size: usize, sector: usize) {
    let mut boot = [0; 512];
    wr(&mut boot, 3, b"NTFS    ");
    wr(&mut boot, 11, &(sector as u16).to_le_bytes());
    wr(&mut boot, 40, &((size / sector - 1) as u64).to_le_bytes());
    wr(&mut boot, 510, &[0x55, 0xaa]);
    wr(image, 0, &boot);
    wr(image, size - sector, &boot);
}

#[test]
fn corrupt_catalog_size_uses_verified_ntfs_geometry_without_erasing_raw_size() {
    for sector in [512, 1024, 2048, 4096] {
        let mut image = build_chain_image(&single_store(vec![], vec![]), &[]);
        let size = image.len();
        set_ntfs_geometry(&mut image, size, sector);
        wr(
            &mut image,
            R_CATALOG * RBS + 128 + 8,
            &u64::MAX.to_le_bytes(),
        );
        let mut volume = VssVolume::open(Cursor::new(image.clone())).unwrap();
        assert_eq!(volume.stores()[0].volume_size, u64::MAX);
        assert_eq!(volume.snapshot_size().unwrap(), size as u64);
        assert_eq!(
            volume.snapshot_layout(0).unwrap().volume_size(),
            size as u64
        );
        image[size - sector + 40] ^= 1;
        assert!(VssVolume::open(Cursor::new(image))
            .unwrap()
            .snapshot_layout(0)
            .is_err());
    }
}

#[test]
fn inconsistent_store_sizes_require_ntfs_and_geometry_overflow_fails() {
    let mut image = two_store_image(vec![], vec![], &[]);
    wr(&mut image, R_CATALOG * RBS + 384 + 8, &1u64.to_le_bytes());
    assert!(VssVolume::open(Cursor::new(image.clone()))
        .unwrap()
        .snapshot_layout(0)
        .is_err());
    let size = image.len();
    set_ntfs_geometry(&mut image, size, 512);
    assert_eq!(
        VssVolume::open(Cursor::new(image.clone()))
            .unwrap()
            .snapshot_size()
            .unwrap(),
        size as u64
    );
    wr(&mut image, 40, &u64::MAX.to_le_bytes());
    assert!(VssVolume::open(Cursor::new(image))
        .unwrap()
        .snapshot_size()
        .is_err());
}

#[test]
fn smaller_snapshot_can_read_saved_data_beyond_its_logical_end() {
    let mut image = build_chain_image(&single_store(vec![plain(4, 32)], vec![]), &[(32, 0x5a)]);
    wr(
        &mut image,
        R_CATALOG * RBS + 128 + 8,
        &block(16).to_le_bytes(),
    );
    let mut volume = VssVolume::open(Cursor::new(image)).unwrap();
    assert_eq!(volume.snapshot_size().unwrap(), block(16));
    let mut snapshot = volume.snapshot(0).unwrap();
    assert_eq!(snapshot.read_block(block(4)).unwrap(), [0x5a; RBS]);
    assert!(snapshot.read_block(block(16)).is_err());
}

#[test]
fn sequential_seek_and_positioned_reads_agree_at_eof() {
    let image = build_chain_image(
        &single_store(vec![plain(4, 32), overlay(4, 33, 5)], vec![]),
        &[(32, 0x5a), (33, 0xbb)],
    );
    let mut volume = VssVolume::open(Cursor::new(image)).unwrap();
    let mut snapshot = volume.snapshot(0).unwrap();
    let mut expected = vec![0; 900];
    snapshot.read_at(block(4) + 400, &mut expected).unwrap();
    snapshot.seek(SeekFrom::Start(block(4) + 400)).unwrap();
    let mut actual = vec![0; 900];
    snapshot.read_exact(&mut actual).unwrap();
    assert_eq!(actual, expected);
    let position = snapshot.stream_position().unwrap();
    snapshot.read_at(0, &mut [0; 1]).unwrap();
    assert_eq!(snapshot.stream_position().unwrap(), position);
    snapshot.seek(SeekFrom::End(-7)).unwrap();
    assert_eq!(snapshot.read(&mut [0; 100]).unwrap(), 7);
    assert_eq!(snapshot.read(&mut [0; 1]).unwrap(), 0);
    assert!(snapshot.seek(SeekFrom::Current(i64::MIN)).is_err());
    assert_eq!(snapshot.stream_position().unwrap(), R_IMG_LEN as u64);
    snapshot.seek(SeekFrom::Start(u64::MAX)).unwrap();
    assert_eq!(snapshot.read(&mut [0; 1]).unwrap(), 0);
    assert!(snapshot.seek(SeekFrom::Current(1)).is_err());
}

#[test]
fn long_store_chain_is_iterative_and_source_bounds_do_not_depend_on_callback() {
    let map = Arc::new(crate::StoreMap::default());
    let layout = crate::SnapshotLayout::new(1, vec![map; 20_000]);
    let mut output = [0];
    layout
        .read_at(0, &mut output, &mut |_, out| {
            out.fill(7);
            Ok(())
        })
        .unwrap();
    assert_eq!(output, [7]);
    let image = build_chain_image(&single_store(vec![plain(4, 99_999)], vec![]), &[]);
    let layout = VssVolume::open(Cursor::new(image))
        .unwrap()
        .snapshot_layout(0)
        .unwrap();
    let mut called = false;
    assert!(layout
        .read_at(block(4), &mut output, &mut |_, _| {
            called = true;
            Ok(())
        })
        .is_err());
    assert!(!called);
}
