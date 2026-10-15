use super::*;

#[test]
fn matches_windows_compressed_fixture_across_adaptive_token_boundaries() {
    let expected: Vec<u8> = (0..16384)
        .map(|index| {
            if (index / 4096) % 3 == 0 {
                (index % 17) as u8
            } else {
                (index ^ (index >> 4) ^ (index >> 9)) as u8
            }
        })
        .collect();
    assert_eq!(
        decompress(include_bytes!("testdata/lznt1_windows.bin"), expected.len()).unwrap(),
        expected
    );
}

#[test]
fn decodes_literals_overlapping_matches_and_raw_blocks() {
    // A literal A followed by an offset-1 match of 4095 bytes.
    let compressed = [3, 0xb0, 2, b'A', 0xfc, 0x0f];
    assert_eq!(decompress(&compressed, 4096).unwrap(), vec![b'A'; 4096]);
    let mut two_blocks = compressed.to_vec();
    two_blocks.extend_from_slice(&[2, 0x30, b'B', b'C', b'D', 0, 0]);
    let mut expected = vec![b'A'; 4096];
    expected.extend_from_slice(b"BCD");
    assert_eq!(decompress(&two_blocks, 8192).unwrap(), expected);
}

#[test]
fn ignores_storage_padding_after_a_complete_compression_unit() {
    // A full 4 KiB block followed by one alignment byte. Windows LZNT1
    // decoding stops at the output size without parsing a new block header.
    let input = [3, 0xb0, 2, b'A', 0xfc, 0x0f, 0];
    assert_eq!(decompress(&input, 4096).unwrap(), vec![b'A'; 4096]);
    // The same byte is still a truncated header if another block is needed.
    assert!(decompress(&input, 8192).is_err());
}

#[test]
fn rejects_truncated_invalid_and_oversized_blocks() {
    for input in [
        vec![0x00],
        vec![0x03, 0xb0, 0x00, b'A'],
        vec![0x00, 0xb0, 0x01],
        vec![0x02, 0xb0, 0x01, 0x00, 0x00],
        vec![0x00, 0x10, b'A'],
        vec![0x03, 0xb0, 0x02, b'A', 0xfd, 0x0f],
    ] {
        assert!(decompress(&input, 4096).is_err(), "{input:?}");
    }
    assert!(decompress(&[0x02, 0x30, b'A', b'B', b'C'], 2).is_err());
}

#[test]
fn rejects_matches_into_previous_blocks() {
    let input = [0x00, 0x30, b'A', 0x02, 0xb0, 0x01, 0x00, 0x00];
    assert!(decompress(&input, 8192).is_err());
}
