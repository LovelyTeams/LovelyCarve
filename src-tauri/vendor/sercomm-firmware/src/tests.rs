use super::*;
use std::io::Read;

const DEFLATE: &[u8] = include_bytes!("../../../test_data/firmware/synthetic-sercomm-deflate.img");

fn stored() -> Vec<u8> {
    let bytes = include_bytes!("../../../test_data/firmware/synthetic-sercomm-stored.img.zlib");
    let mut output = Vec::new();
    flate2::read::ZlibDecoder::new(&bytes[..])
        .read_to_end(&mut output)
        .unwrap();
    output
}
fn reseal(bytes: &mut [u8]) {
    bytes[511] = 0;
    bytes[511] = 0u8.wrapping_sub(bytes.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
}
fn coordinates(bytes: &[u8]) -> (usize, usize, usize) {
    let zip = &bytes[SEAL..];
    let (start, length, _, _, _, _) = zip_member(zip).unwrap();
    let central = SEAL + le32(&zip[zip.len() - 22..], 16).unwrap() as usize;
    (SEAL + start, length, central)
}
fn update_stored_crc(bytes: &mut [u8], start: usize, length: usize, central: usize) {
    let crc = crc32fast::hash(&bytes[start..start + length]);
    bytes[SEAL + 14..SEAL + 18].copy_from_slice(&crc.to_le_bytes());
    bytes[central + 16..central + 20].copy_from_slice(&crc.to_le_bytes());
    reseal(bytes);
}

#[test]
fn independent_zip_modes_keep_source_and_decoded_geometry_separate() {
    let stored = stored();
    for bytes in [DEFLATE, stored.as_slice()] {
        let source = bytes.to_vec();
        let image = open(bytes, &CancellationToken::new()).unwrap();
        assert_eq!(image.member_name(), "Synthetic.bin");
        assert_eq!(image.logical_length(), 393216);
        assert!(image.source_extent().end <= bytes.len() as u64);
        assert_ne!(
            image.source_extent().end - image.source_extent().start,
            image.logical_length()
        );
        assert_eq!(image.read_range(0, 4).unwrap(), b"UBI#");
        assert_eq!(image.read_range(393215, 99).unwrap().len(), 1);
        assert!(image.read_range(393216, 1).unwrap().is_empty());
        assert!(image.read_range(u64::MAX, usize::MAX).unwrap().is_empty());
        assert_eq!(bytes, source);
    }
}

#[test]
fn truncation_bad_seals_trailing_bytes_and_cancel_are_errors() {
    for length in [
        0,
        6,
        10,
        69,
        70,
        511,
        512,
        600,
        DEFLATE.len() / 2,
        DEFLATE.len() - 1,
    ] {
        assert!(
            open(&DEFLATE[..length], &CancellationToken::new()).is_err(),
            "length {length}"
        );
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(open(DEFLATE, &cancel).is_err());
    for position in [0, 7, 59, 63, 70, 511, 600] {
        let mut bad = DEFLATE.to_vec();
        bad[position] ^= 1;
        assert!(open(&bad, &CancellationToken::new()).is_err());
    }
    let mut extra = DEFLATE.to_vec();
    extra.push(0);
    reseal(&mut extra);
    assert!(open(&extra, &CancellationToken::new()).is_err());
}

#[test]
fn valid_seal_cannot_hide_zip_crc_lengths_or_local_header_disagreement() {
    let original = stored();
    let (start, length, central) = coordinates(&original);
    for position in [
        start + 10,
        SEAL + 6,
        SEAL + 14,
        SEAL + 18,
        SEAL + 22,
        SEAL + 30,
        central + 8,
        central + 16,
        central + 20,
        central + 24,
        central + 42,
    ] {
        let mut bad = original.clone();
        bad[position] ^= 1;
        reseal(&mut bad);
        assert!(
            open(&bad, &CancellationToken::new()).is_err(),
            "position {position}"
        );
    }
    let mut oversized = original;
    let expected = (MAX_BYTES as u32 + 1).to_le_bytes();
    oversized[SEAL + 22..SEAL + 26].copy_from_slice(&expected);
    oversized[central + 24..central + 28].copy_from_slice(&expected);
    reseal(&mut oversized);
    assert!(open(&oversized, &CancellationToken::new()).is_err());
    assert!(length > ROOTFS);
}

#[test]
fn checksummed_zip_still_checks_profile_footer_padding_and_ubi() {
    let original = stored();
    let (start, length, central) = coordinates(&original);
    for relative in [
        0,
        KERNEL,
        KERNEL + 30,
        KERNEL + 1000,
        ROOTFS,
        ROOTFS + 60,
        length - FOOTER + 11,
        length - 1,
    ] {
        let mut bad = original.clone();
        bad[start + relative] ^= 1;
        update_stored_crc(&mut bad, start, length, central);
        assert!(
            open(&bad, &CancellationToken::new()).is_err(),
            "relative {relative}"
        );
    }
}

#[test]
fn exact_deflate_end_and_output_length_are_required() {
    let (start, length, method, expected, crc, _) = zip_member(&DEFLATE[SEAL..]).unwrap();
    let bytes = &DEFLATE[SEAL + start..SEAL + start + length];
    for cut in [0, 1, length / 2, length - 2, length - 1] {
        assert!(
            inflate(
                &bytes[..cut],
                method,
                expected,
                crc,
                &CancellationToken::new()
            )
            .is_err()
        );
    }
    let mut extra = bytes.to_vec();
    extra.push(0);
    assert!(inflate(&extra, method, expected, crc, &CancellationToken::new()).is_err());
    assert!(inflate(bytes, method, expected - 1, crc, &CancellationToken::new()).is_err());
    assert!(inflate(bytes, method, expected, crc ^ 1, &CancellationToken::new()).is_err());
}

#[test]
#[ignore = "set SERCOMM_FIRMWARE_EXTERNAL to semicolon-separated sealed NAND ZIP firmware"]
fn external_nand_views_preserve_physical_extents_and_validate_ubi() {
    let paths = std::env::var("SERCOMM_FIRMWARE_EXTERNAL").unwrap();
    let controls =
        std::env::var("SERCOMM_FIRMWARE_ROOTFS").expect("set matching rootfs.bin control paths");
    let paths: Vec<_> = paths.split(';').collect();
    let controls: Vec<_> = controls.split(';').collect();
    assert_eq!(paths.len(), controls.len());
    for (path, control) in paths.into_iter().zip(controls) {
        let bytes = std::fs::read(path).unwrap();
        let image = open(&bytes, &CancellationToken::new()).unwrap();
        assert!(image.source_extent().end <= bytes.len() as u64);
        assert_eq!(image.read_range(0, 4).unwrap(), b"UBI#");
        assert!(image.logical_length() > 0);
        assert_eq!(
            image
                .read_range(0, image.logical_length() as usize)
                .unwrap(),
            std::fs::read(control).unwrap()
        );
    }
}
