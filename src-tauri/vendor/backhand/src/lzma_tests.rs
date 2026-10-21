use super::*;
#[cfg(feature = "v3_lzma")]
use crate::traits::filesystem::BackhandInnerNode;
use std::sync::Arc;

const RAW: &[u8] = include_bytes!("../test_data/lzma-raw.block");
const ALONE: &[u8] = include_bytes!("../test_data/lzma-alone.block");
const PROPS: &[u8] = include_bytes!("../test_data/lzma-props.block");

#[test]
#[cfg(feature = "v3_lzma")]
fn lzma_independent_full_trees_read_both_endians_and_cross_blocks() {
    let fixtures: [(&[u8], bool); 8] = [
        (
            include_bytes!("../test_data/lzma-v3-raw-le.squashfs"),
            false,
        ),
        (include_bytes!("../test_data/lzma-v3-raw-be.squashfs"), true),
        (
            include_bytes!("../test_data/lzma-v3-alone-le.squashfs"),
            false,
        ),
        (
            include_bytes!("../test_data/lzma-v3-alone-be.squashfs"),
            true,
        ),
        (
            include_bytes!("../test_data/lzma-v3-props-le.squashfs"),
            false,
        ),
        (
            include_bytes!("../test_data/lzma-v3-props-be.squashfs"),
            true,
        ),
        (
            include_bytes!("../test_data/lzma-v3-gzip-le.squashfs"),
            false,
        ),
        (
            include_bytes!("../test_data/lzma-v3-gzip-be.squashfs"),
            true,
        ),
    ];
    let expected = b"Independent Rust LZMA fixture.\n".repeat(350);
    for (bytes, big) in fixtures {
        let kind = if big {
            crate::kind::BE_V3_0
        } else {
            crate::kind::LE_V3_0
        };
        let fs = crate::create_squashfs_from_kind(
            std::io::Cursor::new(bytes),
            0,
            crate::kind::Kind::from_const(kind).unwrap(),
        )
        .unwrap();
        let node = fs
            .files()
            .find(|node| node.fullpath == std::path::Path::new("/hello"))
            .unwrap();
        let BackhandInnerNode::File(file) = node.inner else {
            panic!("expected file")
        };
        assert_eq!(fs.file_data(&file).unwrap(), expected);
        for (offset, length) in [
            (0, 20),
            (4080, 60),
            (8180, 80),
            (10480, 100),
            (u64::MAX, 10),
        ] {
            let start = usize::try_from(offset)
                .unwrap_or(usize::MAX)
                .min(expected.len());
            let end = start.saturating_add(length).min(expected.len());
            assert_eq!(
                fs.file_data_range(&file, offset, length).unwrap(),
                expected[start..end]
            );
        }
    }
}

#[test]
fn lzma_bounded_decoders_require_complete_stream_and_preserve_output_on_failure() {
    let content = b"Independent Rust LZMA fixture.\n".repeat(350);
    for bytes in [RAW, ALONE, PROPS] {
        let cache = LzmaCache::new();
        let mut out = b"prefix".to_vec();
        decompress_adaptive(bytes, &mut out, &cache, 4096).unwrap();
        assert_eq!(&out[6..], &content[..4096]);
        for malformed in [&bytes[..bytes.len() - 1], &[bytes, &[1, 2, 3]].concat()] {
            let mut unchanged = b"prefix".to_vec();
            assert!(
                decompress_adaptive(malformed, &mut unchanged, &LzmaCache::new(), 4096).is_err(),
                "source {} bytes, malformed {} bytes",
                bytes.len(),
                malformed.len()
            );
            assert_eq!(unchanged, b"prefix");
        }
        let mut out = b"prefix".to_vec();
        assert!(decompress_adaptive(bytes, &mut out, &LzmaCache::new(), 4095).is_err());
        assert_eq!(out, b"prefix");
    }
    let mut too_large_dictionary = ALONE.to_vec();
    too_large_dictionary[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(decompress_standard(&too_large_dictionary, &mut Vec::new(), 4096).is_err());
    let mut wrong_length = ALONE.to_vec();
    wrong_length[5..13].copy_from_slice(&4000u64.to_le_bytes());
    assert!(decompress_standard(&wrong_length, &mut Vec::new(), 4096).is_err());
    let mut known_length = ALONE.to_vec();
    known_length[5..13].copy_from_slice(&4096u64.to_le_bytes());
    let mut out = Vec::new();
    decompress_standard(&known_length, &mut out, 4096).unwrap();
    assert_eq!(out, content[..4096]);
    assert!(
        decompress_adaptive(RAW, &mut Vec::new(), &LzmaCache::new(), MAX_BLOCK_SIZE + 1).is_err()
    );
}

#[test]
fn lzma_adaptive_prefix_and_shared_kind_cache_are_bounded() {
    let mut prefixed = vec![0x55; 10];
    prefixed.extend_from_slice(RAW);
    let cache = LzmaCache::new();
    let mut out = Vec::new();
    decompress_adaptive(&prefixed, &mut out, &cache, 4096).unwrap();
    let LzmaFormat::Adaptive(params) = cache.get() else {
        panic!("expected raw parameters")
    };
    assert_eq!(params.offset, 10);
    let kind = crate::kind::Kind::from_const(crate::kind::LE_V4_0).unwrap();
    kind.lzma_cache.set(LzmaFormat::Adaptive(params));
    assert!(Arc::ptr_eq(&kind.lzma_cache, &kind.clone().lzma_cache));
}
