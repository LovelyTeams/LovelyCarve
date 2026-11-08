use super::*;
use lamfold::{BlockCache, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use std::io::Read;

fn fixture(extended: bool) -> Vec<u8> {
    let compressed = if extended {
        include_bytes!("../tests/fixtures/udf201.zlib").as_slice()
    } else {
        include_bytes!("../tests/fixtures/udf102.zlib").as_slice()
    };
    let mut bytes = Vec::new();
    flate2::read::ZlibDecoder::new(compressed)
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

#[test]
fn udf_mkudffs_and_linux_kernel_reference_images() {
    for extended in [false, true] {
        let image = fixture(extended);
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let mut reader = Udf::open(SliceSource::new(&image), &mut context).unwrap();
        let root = reader.root();
        let entries = reader.read_dir(root, &mut context).unwrap();
        assert_eq!(entries.len(), 3);
        let big = reader
            .lookup(
                root,
                if extended { "big.bin" } else { "big.dat" },
                &mut context,
            )
            .unwrap()
            .unwrap();
        let expected: Vec<u8> = if extended {
            (0..70000)
                .map(|index| ((index * 11 + index / 257) & 255) as u8)
                .collect()
        } else {
            (0..5000).map(|index| (index % 256) as u8).collect()
        };
        let mut bytes = vec![0; expected.len()];
        assert_eq!(
            reader.read_at(big, 0, &mut bytes, &mut context).unwrap(),
            expected.len()
        );
        assert_eq!(bytes, expected);
        for offset in [
            0,
            2041,
            expected.len() - 7,
            expected.len(),
            expected.len() + 10,
        ] {
            let mut bytes = [0xaa; 123];
            let count = reader
                .read_at(big, offset as u64, &mut bytes, &mut context)
                .unwrap();
            assert_eq!(
                &bytes[..count],
                &expected[offset.min(expected.len())..(offset + 123).min(expected.len())]
            );
        }
        assert!(reader
            .timestamps(big, &mut context)
            .unwrap()
            .modified
            .is_some());
        let sub = reader.lookup(root, "sub", &mut context).unwrap().unwrap();
        assert!(reader
            .lookup(sub, "inner.txt", &mut context)
            .unwrap()
            .is_some());
    }
}

fn synthetic(block_size: usize) -> Vec<u8> {
    let compressed = match block_size {
        512 => include_bytes!("../tests/fixtures/synthetic-512.zlib").as_slice(),
        1024 => include_bytes!("../tests/fixtures/synthetic-1024.zlib").as_slice(),
        2048 => include_bytes!("../tests/fixtures/synthetic-2048.zlib").as_slice(),
        4096 => include_bytes!("../tests/fixtures/synthetic-4096.zlib").as_slice(),
        _ => panic!("test geometry"),
    };
    let mut bytes = Vec::new();
    flate2::read::ZlibDecoder::new(compressed)
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}
fn open(image: &[u8]) -> lamfold::Result<Udf<SliceSource<'_>>> {
    let mut cache = BlockCache::new(0);
    Udf::open(
        SliceSource::new(image),
        &mut SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        },
    )
}

#[test]
fn udf_geometry_holes_continuations_hardlinks_unicode_symlinks_and_time_roles() {
    for block_size in [512, 1024, 2048, 4096] {
        let image = synthetic(block_size);
        let mut reader = open(&image).unwrap();
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let root = reader.root();
        let entries = reader.read_dir(root, &mut context).unwrap();
        assert_eq!(entries.len(), 7); // deleted entry must not hide later entries
        assert!(entries.iter().any(|entry| entry.name == "中文.txt"));
        let big = reader
            .lookup(root, "big.bin", &mut context)
            .unwrap()
            .unwrap();
        assert_eq!(
            Some(big),
            reader.lookup(root, "alias.bin", &mut context).unwrap()
        );
        let expected: Vec<u8> = (0..4 * block_size + 37)
            .map(|index| {
                if index < block_size {
                    ((index * 17 + 3) & 255) as u8
                } else if index < 3 * block_size {
                    0
                } else {
                    (((index - 3 * block_size) * 19 + 5) & 255) as u8
                }
            })
            .collect();
        let mut actual = vec![0; expected.len()];
        assert_eq!(
            reader.read_at(big, 0, &mut actual, &mut context).unwrap(),
            expected.len()
        );
        assert_eq!(actual, expected);
        for offset in [
            block_size - 7,
            block_size * 2 - 7,
            block_size * 3 - 7,
            expected.len() - 3,
            expected.len(),
        ] {
            let mut bytes = [0x55; 37];
            let count = reader
                .read_at(big, offset as u64, &mut bytes, &mut context)
                .unwrap();
            assert_eq!(
                &bytes[..count],
                &expected[offset..(offset + 37).min(expected.len())]
            );
        }
        let link = reader.lookup(root, "link", &mut context).unwrap().unwrap();
        assert_eq!(
            reader.read_link(link, &mut context).unwrap(),
            Some(b"../big.bin".to_vec())
        );
        assert_eq!(reader.metadata(link, &mut context).unwrap().size, 10);
        let time = reader.timestamps(big, &mut context).unwrap();
        assert_eq!(time.modified, Some((1709191820, 123456000)));
        assert_eq!(time.created.is_some(), matches!(block_size, 512 | 4096));
        let unknown = reader
            .lookup(root, "unknown.txt", &mut context)
            .unwrap()
            .unwrap();
        assert!(reader
            .timestamps(unknown, &mut context)
            .unwrap()
            .modified
            .is_none());
        let huge = reader
            .lookup(root, "huge.bin", &mut context)
            .unwrap()
            .unwrap();
        let mut bytes = [0xaa; 37];
        assert_eq!(
            reader
                .read_at(huge, 0x3ffff000 - 17, &mut bytes, &mut context)
                .unwrap(),
            17
        );
        assert_eq!(&bytes[..17], &[0; 17]);
    }
}

fn repair_tag(image: &mut [u8], offset: usize) {
    let length = usize::from(descriptors::u16_at(image, offset + 10).unwrap());
    let crc = descriptors::crc16(&image[offset + 16..offset + 16 + length]);
    image[offset + 8..offset + 10].copy_from_slice(&crc.to_le_bytes());
    image[offset + 4] = 0;
    image[offset + 4] = image[offset..offset + 16]
        .iter()
        .fold(0u8, |sum, byte| sum.wrapping_add(*byte));
}

#[test]
fn udf_anchor_and_reserve_vds_recovery() {
    let mut image = synthetic(2048);
    image[256 * 2048 + 4] ^= 1;
    assert!(open(&image).is_ok()); // backup anchor at last / last-256
    image[32 * 2048 + 8] ^= 1;
    assert!(open(&image).is_ok()); // reserve sequence remains valid
    image[48 * 2048 + 8] ^= 1;
    assert!(open(&image).is_err());
}

#[test]
fn udf_rejects_foreign_partition_maps_truncation_and_corrupt_descriptors() {
    let original = synthetic(2048);
    for map_type in [0, 2, 3] {
        let mut image = original.clone();
        for block in [33, 49] {
            image[block * 2048 + 440] = map_type;
            repair_tag(&mut image, block * 2048);
        }
        assert!(matches!(
            open(&image),
            Err(lamfold::FoldError::Unsupported(_))
        ));
    }
    for length in [0, 1024, 256 * 2048, 400 * 2048, 599 * 2048] {
        assert!(open(&original[..length]).is_err());
    }
    // All anchors bad, with no checksum-consistent false positive elsewhere.
    let mut image = original.clone();
    for block in [256, 383, 639] {
        image[block * 2048 + 4] ^= 1;
    }
    assert!(open(&image).is_err());
    for (offset, value) in [
        (300 * 2048 + 12, 1),
        (301 * 2048 + 8, 0),
        (301 * 2048 + 20, 0),
    ] {
        let mut image = original.clone();
        image[offset] = value;
        assert!(open(&image).is_err());
    }
    // Type-1 map must name the actual physical partition.
    let mut image = original;
    for block in [33, 49] {
        image[block * 2048 + 444] = 8;
        repair_tag(&mut image, block * 2048);
    }
    assert!(open(&image).is_err());
}

fn directory_error(image: &[u8]) -> bool {
    let Ok(mut reader) = open(image) else {
        return true;
    };
    let mut cache = BlockCache::new(0);
    let root = reader.root();
    reader
        .read_dir(
            root,
            &mut SubstrateCtx {
                cache: &mut cache,
                verifier: &NoVerifier,
            },
        )
        .is_err()
}

#[test]
fn udf_rejects_bad_extent_chains_bounds_lengths_and_directory_records() {
    let original = synthetic(2048);
    for (relative, value) in [
        (180, 301u32), // root data outside partition, still inside source
        (56, 50000),   // logical length exceeds directory allocation map
    ] {
        let mut image = original.clone();
        image[301 * 2048 + relative..301 * 2048 + relative + 4]
            .copy_from_slice(&value.to_le_bytes());
        repair_tag(&mut image, 301 * 2048);
        assert!(directory_error(&image));
    }
    // Corrupt CRC of an AED, foreign long_ad ref, a shortened AD map and
    // a continuation pointing back to itself all fail before file exposure.
    for mode in 0..4 {
        let mut image = synthetic(512);
        let block = 360 * 512;
        match mode {
            0 => image[block + 8] ^= 1,
            1 => {
                image[block + 32] = 1;
                repair_tag(&mut image, block);
            }
            2 => {
                image[block + 20..block + 24].copy_from_slice(&16u32.to_le_bytes());
                repair_tag(&mut image, block);
            }
            3 => {
                image[block + 24..block + 28].copy_from_slice(&(0xc0000000u32 | 512).to_le_bytes());
                image[block + 28..block + 32].copy_from_slice(&60u32.to_le_bytes());
                image[block + 20..block + 24].copy_from_slice(&16u32.to_le_bytes());
                repair_tag(&mut image, block);
            }
            _ => unreachable!(),
        }
        assert!(directory_error(&image));
    }
    let mut image = original.clone();
    image[320 * 2048 + 8] ^= 1; // directory FID CRC
    assert!(directory_error(&image));
    let mut image = original;
    image[320 * 2048..320 * 2048 + 2].fill(0); // must not quietly stop on junk FID
    assert!(directory_error(&image));
}

#[test]
fn udf_name_and_timestamp_invalid_values_are_explicit() {
    assert!(descriptors::decode_name(&[16, 0]).is_err());
    assert!(descriptors::decode_name(&[16, 0xd8, 0]).is_err());
    assert!(descriptors::safe_component("..").is_err());
    assert!(descriptors::safe_component("bad/name").is_err());
    assert!(descriptors::safe_component("bad\0name").is_err());
    for value in [0, 13] {
        let mut stamp = [0; 12];
        stamp[..2].copy_from_slice(&0x1000u16.to_le_bytes());
        stamp[2..4].copy_from_slice(&2024u16.to_le_bytes());
        stamp[4] = value;
        stamp[5] = 1;
        assert!(descriptors::timestamp(&stamp).is_err());
    }
}
