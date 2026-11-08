use super::*;
use lamfold::{BlockCache, FoldFrontend, NoVerifier, SliceSource, SubstrateCtx};
use std::{io::Read, path::Path};

fn fixture(name: &str) -> Vec<u8> {
    let compressed = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(format!("{name}.zlib")),
    )
    .unwrap();
    let mut bytes = Vec::new();
    flate2::read::ZlibDecoder::new(compressed.as_slice())
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}
fn open(image: &[u8]) -> lamfold::Result<Minix<SliceSource<'_>>> {
    let mut cache = BlockCache::new(0);
    Minix::open(
        SliceSource::new(image),
        &mut SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        },
    )
}

#[test]
fn minix_util_linux_mkfs_fsck_reference_for_all_five_layouts_and_both_endians() {
    for (suffix, version, names) in [
        ("v1c14", Version::V1, 14),
        ("v1c30", Version::V1, 30),
        ("v2c14", Version::V2, 14),
        ("v2c30", Version::V2, 30),
        ("v3c60", Version::V3, 60),
    ] {
        for endian in ["", ".BE"] {
            let image = fixture(&format!("fsck-images-{suffix}{endian}"));
            let header = Superblock::parse(&image[1024..]).unwrap();
            assert_eq!(
                (header.version, header.name_length, header.endian),
                (
                    version,
                    names,
                    if endian.is_empty() {
                        Endian::Little
                    } else {
                        Endian::Big
                    }
                )
            );
            let mut reader = open(&image).unwrap();
            let mut cache = BlockCache::new(0);
            let mut context = SubstrateCtx {
                cache: &mut cache,
                verifier: &NoVerifier,
            };
            assert!(reader.read_dir(1, &mut context).unwrap().is_empty());
            assert_eq!(
                reader.timestamps(1, &mut context).unwrap().modified,
                Some((1438460212, 0))
            );
        }
    }
}

const SYNTHETIC: [&str; 7] = [
    "v1-14-1024-0-le",
    "v1-30-1024-1-be",
    "v2-14-1024-0-be",
    "v2-30-1024-2-le",
    "v3-60-1024-0-le",
    "v3-60-2048-0-be",
    "v3-60-4096-0-le",
];

#[test]
fn minix_full_and_ranged_data_holes_multiblock_zones_and_all_indirect_levels() {
    for name in SYNTHETIC {
        let image = fixture(name);
        let header = Superblock::parse(&image[1024..]).unwrap();
        let zone_size = header.zone_size() as usize;
        let pointers = header.pointers_per_block() as usize;
        let mut reader = open(&image).unwrap();
        let mut cache = BlockCache::new(0);
        let mut context = SubstrateCtx {
            cache: &mut cache,
            verifier: &NoVerifier,
        };
        let entries = reader.read_dir(1, &mut context).unwrap();
        assert!(entries.iter().any(|entry| entry.name == "text.txt")); // beyond deleted slot
        let big = reader.lookup(1, "big.bin", &mut context).unwrap().unwrap();
        assert_eq!(
            Some(big),
            reader.lookup(1, "alias.bin", &mut context).unwrap()
        );
        let metadata = reader.metadata(big, &mut context).unwrap();
        let expected: Vec<u8> = (0..metadata.size as usize)
            .map(|index| {
                let logical_zone = index / zone_size;
                let factor = if logical_zone == 0 {
                    13
                } else if logical_zone == 2 || logical_zone == 7 {
                    17
                } else if logical_zone == 7 + pointers + 2 {
                    19
                } else {
                    return 0;
                };
                (((index % zone_size) * factor + 7) & 255) as u8
            })
            .collect();
        let mut actual = vec![0; expected.len()];
        assert_eq!(
            reader.read_at(big, 0, &mut actual, &mut context).unwrap(),
            expected.len()
        );
        assert_eq!(actual, expected, "{name}");
        for offset in [
            zone_size - 7,
            7 * zone_size - 7,
            (7 + pointers) * zone_size - 7,
            expected.len() - 7,
            expected.len(),
            expected.len() + 9,
        ] {
            let mut bytes = [0x55; 117];
            let count = reader
                .read_at(big, offset as u64, &mut bytes, &mut context)
                .unwrap();
            assert_eq!(
                &bytes[..count],
                &expected[offset.min(expected.len())..(offset + 117).min(expected.len())],
                "{name} offset {offset}"
            );
        }
        if let Some(triple) = reader.lookup(1, "triple.bin", &mut context).unwrap() {
            let offset = (7 + pointers + pointers * pointers) as u64 * zone_size as u64;
            let mut bytes = [0xff; 74];
            assert_eq!(
                reader
                    .read_at(triple, offset - 37, &mut bytes, &mut context)
                    .unwrap(),
                74
            );
            assert_eq!(&bytes[..37], &[0; 37]);
            assert_eq!(
                &bytes[37..],
                &(0..37)
                    .map(|index| (index * 19 + 7) as u8)
                    .collect::<Vec<_>>()
            );
        }
        let time = reader.timestamps(big, &mut context).unwrap();
        assert_eq!(time.modified, Some((1700000001, 0)));
        assert_eq!(
            time.accessed,
            (header.version != Version::V1).then_some((1700000000, 0))
        );
        assert_eq!(
            time.changed,
            (header.version != Version::V1).then_some((1700000002, 0))
        );
        assert!(time.created.is_none());
        let link = reader.lookup(1, "link", &mut context).unwrap().unwrap();
        assert_eq!(
            reader.read_link(link, &mut context).unwrap(),
            Some(b"big.bin".to_vec())
        );
        let sub = reader.lookup(1, "sub", &mut context).unwrap().unwrap();
        let nested = reader
            .lookup(sub, "nested.txt", &mut context)
            .unwrap()
            .unwrap();
        let mut content = [0; 11];
        assert_eq!(
            reader
                .read_at(nested, 0, &mut content, &mut context)
                .unwrap(),
            11
        );
        assert_eq!(&content, b"hello minix");
    }
}

fn directory_error(image: &[u8]) -> bool {
    let Ok(mut reader) = open(image) else {
        return true;
    };
    let mut cache = BlockCache::new(0);
    reader
        .read_dir(
            1,
            &mut SubstrateCtx {
                cache: &mut cache,
                verifier: &NoVerifier,
            },
        )
        .is_err()
}

#[test]
fn minix_rejects_truncation_geometry_bitmap_capacity_and_free_live_inodes() {
    let original = fixture(SYNTHETIC[4]);
    for length in [0, 1024, 1055, 16384, original.len() - 1] {
        assert!(open(&original[..length]).is_err());
    }
    for (offset, value) in [
        (1024 + 6, 0),
        (1024 + 8, 0),
        (1024 + 10, 1),
        (1024 + 12, 255),
        (1024 + 28, 512),
    ] {
        let mut image = original.clone();
        image[offset..offset + 2].copy_from_slice(&(value as u16).to_le_bytes());
        assert!(open(&image).is_err());
    }
    let mut image = original.clone();
    image[1024..1028].copy_from_slice(&9000u32.to_le_bytes());
    assert!(open(&image).is_err());
    let mut image = original.clone();
    image[1038..1040].copy_from_slice(&0x0100u16.to_le_bytes());
    assert!(matches!(
        open(&image),
        Err(lamfold::FoldError::Unsupported(_))
    ));
    let mut image = original.clone();
    image[1054] = 1;
    assert!(matches!(
        open(&image),
        Err(lamfold::FoldError::Unsupported(_))
    ));
    let mut image = original.clone();
    image[2048] &= !2; // root inode allocation
    assert!(open(&image).is_err());
    let mut image = original;
    image[2048] &= !4; // live big.bin inode
    assert!(directory_error(&image));
}

#[test]
fn minix_rejects_free_outside_and_metadata_aliased_data_and_indirect_cycles() {
    let original = fixture(SYNTHETIC[4]);
    for (offset, value, logical) in [
        (4160 + 24, 256u32, 0u64),         // past volume
        (4160 + 24, 7, 0),                 // reserved metadata zone
        (4160 + 24, 200, 0),               // free data zone
        (12 * 1024, 12, 7 * 1024),         // single points to itself as data
        (13 * 1024, 13, (7 + 256) * 1024), // double points to itself
    ] {
        let mut image = original.clone();
        image[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        let mut reader = open(&image).unwrap();
        let mut cache = BlockCache::new(0);
        assert!(reader
            .read_at(
                2,
                logical,
                &mut [0; 1],
                &mut SubstrateCtx {
                    cache: &mut cache,
                    verifier: &NoVerifier
                }
            )
            .is_err());
    }
    let mut image = original;
    image[3072] &= !(1 << 3); // root is first(8); file starts zone10 => bitmap3
    let mut reader = open(&image).unwrap();
    let mut cache = BlockCache::new(0);
    assert!(reader
        .read_at(
            2,
            0,
            &mut [0; 1],
            &mut SubstrateCtx {
                cache: &mut cache,
                verifier: &NoVerifier
            }
        )
        .is_err());
}

#[test]
fn minix_v3_derived_first_zone_and_directory_corruption() {
    let mut image = fixture(SYNTHETIC[4]);
    // A zero first-zone field derives the metadata minimum instead of reading
    // file extents from zone zero. Reserved padding need not hold file data.
    image[1034..1036].fill(0);
    assert_eq!(
        Superblock::parse(&image[1024..]).unwrap().first_data_zone,
        5
    );
    let reference = fixture("fsck-images-v3c60");
    let mut derived = reference.clone();
    derived[1034..1036].fill(0);
    let mut reader = open(&derived).unwrap();
    let mut cache = BlockCache::new(0);
    assert!(reader
        .read_dir(
            1,
            &mut SubstrateCtx {
                cache: &mut cache,
                verifier: &NoVerifier
            }
        )
        .unwrap()
        .is_empty());
    for (offset, value) in [
        (8 * 1024, 2u8),
        (8 * 1024 + 4, b'/'),
        (8 * 1024 + 64 * 5 + 4, 0xff),
    ] {
        let mut image = fixture(SYNTHETIC[4]);
        image[offset] = value;
        assert!(directory_error(&image));
    }
}
