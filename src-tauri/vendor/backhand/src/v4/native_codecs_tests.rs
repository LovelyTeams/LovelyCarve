//! Independent Python/liblzma and Zstandard fixtures; no writer round trip.
use super::native_codecs::*;

fn payload() -> Vec<u8> {
    (0..=255).cycle().take(16384).collect()
}

#[cfg(feature = "xz")]
#[test]
fn xz_independent_checks_and_all_squashfs_bcj_filters() {
    for bytes in [
        include_bytes!("../../test_data/native-plain.xz").as_slice(),
        include_bytes!("../../test_data/native-x86.xz"),
        include_bytes!("../../test_data/native-powerpc.xz"),
        include_bytes!("../../test_data/native-ia64.xz"),
        include_bytes!("../../test_data/native-arm.xz"),
        include_bytes!("../../test_data/native-armthumb.xz"),
        include_bytes!("../../test_data/native-sparc.xz"),
        include_bytes!("../../test_data/native-crc64.xz"),
        include_bytes!("../../test_data/native-sha256.xz"),
        include_bytes!("../../test_data/native-no-check.xz"),
        include_bytes!("../../test_data/native-delta-x86.xz"),
        include_bytes!("../../test_data/native-with-sizes.xz"),
    ] {
        let mut output = b"prefix".to_vec();
        decode_xz(bytes, &mut output).unwrap();
        assert_eq!(&output[6..], payload());
        assert_eq!(&output[..6], b"prefix");
    }
}

#[cfg(feature = "xz")]
#[test]
fn xz_rejects_truncation_trailing_corruption_limits_and_forged_index() {
    let valid = include_bytes!("../../test_data/native-plain.xz");
    let mut invalid: Vec<Vec<u8>> = (0..valid.len()).map(|end| valid[..end].to_vec()).collect();
    invalid.push([valid.as_slice(), b"\0\0\0\0"].concat());
    invalid.push([valid.as_slice(), valid.as_slice()].concat());
    for position in 0..valid.len() {
        let mut bytes = valid.to_vec();
        bytes[position] ^= 1;
        invalid.push(bytes);
    }
    invalid.push(include_bytes!("../../test_data/native-large-output.xz").to_vec());
    invalid.push(include_bytes!("../../test_data/native-large-dictionary.xz").to_vec());
    invalid.push(include_bytes!("../../test_data/native-bad-index.xz").to_vec());
    invalid.push(include_bytes!("../../test_data/native-bad-footer.xz").to_vec());
    invalid.push(include_bytes!("../../test_data/native-bad-block-size.xz").to_vec());
    for (index, bytes) in invalid.iter().enumerate() {
        let mut output = b"prefix".to_vec();
        assert!(
            decode_xz(bytes, &mut output).is_err(),
            "invalid fixture {index}"
        );
        assert_eq!(output, b"prefix");
    }
}

#[cfg(feature = "xz")]
#[test]
fn native_xz_writer_remains_readable_with_alternative_bcj_flags() {
    let mut empty = Vec::new();
    decode_xz(
        include_bytes!("../../test_data/native-empty.xz"),
        &mut empty,
    )
    .unwrap();
    assert!(empty.is_empty());
    let mut multiple = Vec::new();
    decode_xz(
        include_bytes!("../../test_data/native-multi-block.xz"),
        &mut multiple,
    )
    .unwrap();
    assert_eq!(multiple, payload().repeat(2));
    use crate::FilesystemCompressor;
    use crate::compression::{
        CompressionAction, CompressionOptions, Compressor, DefaultCompressor, Xz,
    };
    use crate::v4::compressor::XzFilter;
    let bytes = DefaultCompressor
        .compress(
            &payload(),
            FilesystemCompressor {
                id: Compressor::Xz,
                options: Some(CompressionOptions::Xz(Xz {
                    dictionary_size: 65536,
                    filters: XzFilter::new(0x3f),
                    bit_opts: None,
                    fb: None,
                })),
                extra: None,
            },
            65536,
        )
        .unwrap();
    let mut output = Vec::new();
    decode_xz(&bytes, &mut output).unwrap();
    assert_eq!(output, payload());
}

#[cfg(feature = "zstd")]
#[test]
fn zstd_independent_checksum_full_consumption_and_allocation_bounds() {
    let valid = include_bytes!("../../test_data/native-checksum.zst");
    let mut output = b"prefix".to_vec();
    decode_zstd(valid, &mut output).unwrap();
    assert_eq!(&output[6..], payload());
    let mut invalid: Vec<Vec<u8>> = (0..valid.len()).map(|end| valid[..end].to_vec()).collect();
    invalid.push([valid.as_slice(), b"extra"].concat());
    invalid.push([valid.as_slice(), valid.as_slice()].concat());
    for position in valid.len() - 4..valid.len() {
        let mut bytes = valid.to_vec();
        bytes[position] ^= 1;
        invalid.push(bytes);
    }
    invalid.push(include_bytes!("../../test_data/native-large-output.zst").to_vec());
    invalid.push(include_bytes!("../../test_data/native-large-window.zst").to_vec());
    for (index, bytes) in invalid.iter().enumerate() {
        let mut output = b"prefix".to_vec();
        assert!(
            decode_zstd(bytes, &mut output).is_err(),
            "invalid fixture {index}"
        );
        assert_eq!(output, b"prefix");
    }
}
