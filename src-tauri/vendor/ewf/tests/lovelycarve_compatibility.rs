//! Regression coverage for the local integration patches. Fixtures are created
//! from explicit format layouts, independent of the reader's parsers.
use std::io::Write;

use ewf::{EwfReader, SegmentSource};

fn zlib(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_le_bytes).collect()
}

fn v1_section(output: &mut Vec<u8>, kind: &str, body: &[u8]) {
    let offset = output.len();
    let mut descriptor = [0; 76];
    descriptor[..kind.len()].copy_from_slice(kind.as_bytes());
    descriptor[16..24].copy_from_slice(&((offset + 76 + body.len()) as u64).to_le_bytes());
    descriptor[24..32].copy_from_slice(&((76 + body.len()) as u64).to_le_bytes());
    let checksum = ewf::sections::adler32(&descriptor[..72]);
    descriptor[72..].copy_from_slice(&checksum.to_le_bytes());
    output.extend_from_slice(&descriptor);
    output.extend_from_slice(body);
}

fn v1(stored: &[u8], compressed: bool) -> Vec<u8> {
    let mut output = b"EVF\x09\x0d\x0a\xff\0\x01\x01\0\0\0".to_vec();
    let header = utf16("\u{feff}3\nmain\nc\tn\te\tmd\tsn\n案件\t001\t张三\tModel\tSN-123\n");
    v1_section(&mut output, "header2", &zlib(&header));
    let mut volume = [0; 94];
    volume[0] = 1;
    volume[36] = 2; // Media flags are not the compression level.
    volume[52] = 1;
    volume[4..8].copy_from_slice(&1u32.to_le_bytes());
    volume[8..12].copy_from_slice(&8u32.to_le_bytes());
    volume[12..16].copy_from_slice(&4096u32.to_le_bytes());
    volume[16..24].copy_from_slice(&8u64.to_le_bytes());
    v1_section(&mut output, "volume", &volume);
    let data_offset = (output.len() + 76) as u64;
    v1_section(&mut output, "sectors", stored);
    let mut table = vec![0; 28];
    table[..4].copy_from_slice(&1u32.to_le_bytes());
    table[8..16].copy_from_slice(&data_offset.to_le_bytes());
    table[24..28].copy_from_slice(&(if compressed { 0x80000000u32 } else { 0 }).to_le_bytes());
    v1_section(&mut output, "table", &table);
    v1_section(&mut output, "done", &[]);
    output
}

fn read_v1(bytes: Vec<u8>) -> Result<Vec<u8>, ewf::EwfError> {
    let reader = EwfReader::open_from_sources(vec![SegmentSource::from_bytes(bytes)])?;
    // Request the bulk path even for tiny malformed/final-chunk fixtures.
    let mut output = vec![0; (reader.total_size() as usize).max(1024 * 1024)];
    let count = reader.read_at(&mut output, 0)?;
    output.truncate(count);
    Ok(output)
}

#[test]
fn v1_preserves_4kn_and_utf16_acquisition_fields() {
    let bytes = v1(&zlib(&vec![42; 32768]), true);
    let reader = EwfReader::open_from_sources(vec![SegmentSource::from_bytes(bytes)]).unwrap();
    let metadata = reader.metadata();
    assert_eq!(metadata.compression.as_deref(), Some("快速压缩"));
    assert_eq!(
        (reader.bytes_per_sector(), reader.total_size()),
        (4096, 32768)
    );
    assert_eq!(
        (
            metadata.case_number.as_deref(),
            metadata.examiner.as_deref(),
            metadata.model.as_deref(),
            metadata.serial_number.as_deref()
        ),
        (Some("案件"), Some("张三"), Some("Model"), Some("SN-123"))
    );
}

#[test]
fn v1_detects_raw_corruption_and_short_or_oversized_decodes() {
    let expected = vec![42; 32768];
    let mut raw = expected.clone();
    raw.extend_from_slice(&ewf::sections::adler32(&raw).to_le_bytes());
    assert_eq!(read_v1(v1(&raw, false)).unwrap(), expected);
    raw[1024] ^= 1;
    assert!(read_v1(v1(&raw, false)).is_err());
    assert!(read_v1(v1(&zlib(&[42; 16]), true)).is_err());
    assert!(read_v1(v1(&zlib(&vec![42; 32769]), true)).is_err());
    let mut corrupt = zlib(&expected);
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    assert!(read_v1(v1(&corrupt, true)).is_err());
}

fn v2_section(output: &mut Vec<u8>, previous: &mut u64, kind: u32, data: &[u8]) -> u64 {
    let data_offset = output.len() as u64;
    output.extend_from_slice(data);
    let offset = output.len() as u64;
    let mut descriptor = [0; 64];
    descriptor[..4].copy_from_slice(&kind.to_le_bytes());
    descriptor[8..16].copy_from_slice(&previous.to_le_bytes());
    descriptor[16..24].copy_from_slice(&(data.len() as u64).to_le_bytes());
    descriptor[24..28].copy_from_slice(&64u32.to_le_bytes());
    output.extend_from_slice(&descriptor);
    *previous = offset;
    data_offset
}

fn v2(stored: &[u8], method: u16, flags: u32) -> tempfile::NamedTempFile {
    let mut output = b"EVF2\x0d\x0a\x81\0\x02\x01".to_vec();
    output.extend_from_slice(&method.to_le_bytes());
    output.extend_from_slice(&1u32.to_le_bytes());
    output.resize(32, 0);
    let mut previous = 0;
    let device = utf16("2\nmain\nb\tsc\tts\tmd\tsn\n4096\t8\t8\tDisk\tSN\n");
    let device = if method == 2 {
        let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::best());
        encoder.write_all(&device).unwrap();
        encoder.finish().unwrap()
    } else {
        device
    };
    v2_section(&mut output, &mut previous, 1, &device);
    let offset = v2_section(&mut output, &mut previous, 3, stored);
    let mut table = vec![0; 48];
    table[8..12].copy_from_slice(&1u32.to_le_bytes());
    table[32..40].copy_from_slice(&offset.to_le_bytes());
    table[40..44].copy_from_slice(&(stored.len() as u32).to_le_bytes());
    table[44..48].copy_from_slice(&flags.to_le_bytes());
    v2_section(&mut output, &mut previous, 4, &table);
    v2_section(&mut output, &mut previous, 15, &[]);
    let mut file = tempfile::Builder::new().suffix(".Ex01").tempfile().unwrap();
    file.write_all(&output).unwrap();
    file
}

#[test]
fn v2_bzip2_pattern_fill_and_checksums() {
    let expected: Vec<_> = (0..32768).map(|index| (index % 251) as u8).collect();
    let mut encoder = bzip2::write::BzEncoder::new(Vec::new(), bzip2::Compression::best());
    encoder.write_all(&expected).unwrap();
    let compressed = encoder.finish().unwrap();
    let file = v2(&compressed, 2, 1);
    let reader = EwfReader::open(file.path()).unwrap();
    assert_eq!(
        (
            reader.bytes_per_sector(),
            reader.metadata().model.as_deref()
        ),
        (4096, Some("Disk"))
    );
    let mut output = vec![0; 32768];
    reader.read_at(&mut output, 0).unwrap();
    assert_eq!(output, expected);
    let pattern = b"12345678";
    let file = v2(pattern, 1, 5);
    EwfReader::open(file.path())
        .unwrap()
        .read_at(&mut output, 0)
        .unwrap();
    assert_eq!(output, pattern.repeat(4096));
    let mut raw = expected.clone();
    raw.extend_from_slice(&ewf::sections::adler32(&raw).to_le_bytes());
    let file = v2(&raw, 1, 2);
    EwfReader::open(file.path())
        .unwrap()
        .read_at(&mut output, 0)
        .unwrap();
    assert_eq!(output, expected);
    raw[0] ^= 1;
    let file = v2(&raw, 1, 2);
    assert!(EwfReader::open(file.path())
        .unwrap()
        .read_at(&mut output, 0)
        .is_err());
}

#[test]
fn v1_final_raw_chunk_accepts_partial_and_padded_storage() {
    for stored_len in [4096, 32768] {
        let mut stored = vec![42; stored_len];
        stored.extend_from_slice(&ewf::sections::adler32(&stored).to_le_bytes());
        let mut image = v1(&stored, false);
        let volume = image
            .windows(6)
            .position(|bytes| bytes == b"volume")
            .unwrap()
            + 76;
        image[volume + 16..volume + 24].copy_from_slice(&1u64.to_le_bytes());
        assert_eq!(read_v1(image).unwrap(), vec![42; 4096]);
    }
}
