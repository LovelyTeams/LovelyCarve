use super::*;
use std::io::Write;

const CLUSTER: usize = 4096;
const UNIT: usize = CLUSTER * 16;
const MFT: usize = CLUSTER * 4;

#[path = "capability_tests.rs"]
mod capabilities;

fn record(image: &mut [u8], number: usize, attributes: &[Vec<u8>]) {
    let record = &mut image[MFT + number * 1024..MFT + (number + 1) * 1024];
    record.fill(0);
    record[..4].copy_from_slice(b"FILE");
    record[4..6].copy_from_slice(&48u16.to_le_bytes());
    record[6..8].copy_from_slice(&3u16.to_le_bytes());
    record[16..18].copy_from_slice(&1u16.to_le_bytes());
    record[20..22].copy_from_slice(&56u16.to_le_bytes());
    record[22..24].copy_from_slice(&1u16.to_le_bytes());
    record[28..32].copy_from_slice(&1024u32.to_le_bytes());
    let mut offset = 56;
    for attribute in attributes {
        record[offset..offset + attribute.len()].copy_from_slice(attribute);
        offset += attribute.len();
    }
    record[offset..offset + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    record[24..28].copy_from_slice(&((offset + 4) as u32).to_le_bytes());
    record[48..50].copy_from_slice(&[0xaa, 0x55]);
    for (index, end) in [510, 1022].into_iter().enumerate() {
        let original = [record[end], record[end + 1]];
        record[50 + index * 2..52 + index * 2].copy_from_slice(&original);
        record[end..end + 2].copy_from_slice(&[0xaa, 0x55]);
    }
}

fn non_resident(name: &str, runs: &[u8], length: u64, initialized: u64, allocated: u64) -> Vec<u8> {
    let name: Vec<_> = name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let run_offset = (72 + name.len()).next_multiple_of(8);
    let size = (run_offset + runs.len()).next_multiple_of(8);
    let mut attribute = vec![0; size];
    attribute[..4].copy_from_slice(&0x80u32.to_le_bytes());
    attribute[4..8].copy_from_slice(&(size as u32).to_le_bytes());
    attribute[8] = 1;
    attribute[9] = (name.len() / 2) as u8;
    attribute[10..12].copy_from_slice(&72u16.to_le_bytes());
    attribute[12..14].copy_from_slice(&1u16.to_le_bytes());
    attribute[24..32].copy_from_slice(&15u64.to_le_bytes());
    attribute[32..34].copy_from_slice(&(run_offset as u16).to_le_bytes());
    attribute[34] = 4;
    attribute[40..48].copy_from_slice(&allocated.to_le_bytes());
    attribute[48..56].copy_from_slice(&length.to_le_bytes());
    attribute[56..64].copy_from_slice(&initialized.to_le_bytes());
    attribute[72..72 + name.len()].copy_from_slice(&name);
    attribute[run_offset..run_offset + runs.len()].copy_from_slice(runs);
    attribute
}

fn resident(ty: u32, data: &[u8]) -> Vec<u8> {
    let size = (24 + data.len()).next_multiple_of(8);
    let mut attribute = vec![0; size];
    attribute[..4].copy_from_slice(&ty.to_le_bytes());
    attribute[4..8].copy_from_slice(&(size as u32).to_le_bytes());
    attribute[16..20].copy_from_slice(&(data.len() as u32).to_le_bytes());
    attribute[20..22].copy_from_slice(&24u16.to_le_bytes());
    attribute[24..24 + data.len()].copy_from_slice(data);
    attribute
}

fn image() -> Vec<u8> {
    let mut image = vec![0; CLUSTER * 128];
    image[..11].copy_from_slice(b"\xeb\x52\x90NTFS    ");
    image[11..13].copy_from_slice(&512u16.to_le_bytes());
    image[13] = 8;
    image[40..48].copy_from_slice(&1024u64.to_le_bytes());
    image[48..56].copy_from_slice(&4u64.to_le_bytes());
    image[64] = (-10i8) as u8;
    image[68] = (-12i8) as u8;
    image[510..512].copy_from_slice(&[0x55, 0xaa]);
    let mut mft = non_resident("", &[0x11, 8, 4, 0], 32768, 32768, 32768);
    mft[12..14].fill(0);
    record(&mut image, 0, &[mft]);
    let mut upcase = non_resident("", &[0x11, 32, 80, 0], 131072, 131072, 131072);
    upcase[12..14].fill(0);
    record(&mut image, 10, &[upcase]);
    for code in 0..=u16::MAX {
        let uppercase = if (b'a' as u16..=b'z' as u16).contains(&code) {
            code - 32
        } else {
            code
        };
        let offset = 80 * CLUSTER + usize::from(code) * 2;
        image[offset..offset + 2].copy_from_slice(&uppercase.to_le_bytes());
    }
    image
}

fn repeated_unit(byte: u8) -> Vec<u8> {
    (0..16)
        .flat_map(|_| [3, 0xb0, 2, byte, 0xfc, 0x0f])
        .collect()
}

fn open_image(bytes: &[u8]) -> (tempfile::NamedTempFile, DiskImage) {
    let mut file = tempfile::Builder::new().suffix(".img").tempfile().unwrap();
    file.write_all(bytes).unwrap();
    let image = DiskImage::open(file.path()).unwrap();
    (file, image)
}

#[test]
fn lvm_fragmented_ntfs_full_range_stream_and_deleted_provenance() {
    use crate::evidence::partitions::lvm2::test_support::fragmented;
    let mut logical = image();
    let expected: Vec<_> = (0..2 * CLUSTER).map(|index| (index % 251) as u8).collect();
    // Data crosses the midpoint of the two LVM segments.
    logical[63 * CLUSTER..65 * CLUSTER].copy_from_slice(&expected);
    let mut attribute = non_resident(
        "",
        &[0x11, 2, 63, 0],
        expected.len() as u64,
        expected.len() as u64,
        expected.len() as u64,
    );
    attribute[12..14].fill(0);
    record(&mut logical, 16, &[attribute]);
    let (_file, image) = open_image(&fragmented(&logical));
    let volume = NtfsVolume::open_lvm(&image, &image.partitions[0], 0).unwrap();
    assert_eq!(volume.read_file(16, usize::MAX).unwrap(), expected);
    assert_eq!(
        volume.read_range(16, 4090, 20).unwrap(),
        expected[4090..4110]
    );
    let mut streamed = Vec::new();
    volume
        .stream_file(16, 4093, |bytes| {
            streamed.extend_from_slice(bytes);
            Ok(())
        })
        .unwrap();
    assert_eq!(streamed, expected);
    let mut reader = volume.backing.reader();
    let record = volume.ntfs.file(&mut reader, 16).unwrap();
    let runs = volume.capture_data_runs(&record, &mut reader, expected.len() as u64);
    assert_eq!(runs.len(), 2);
    let original: Vec<_> = runs
        .into_iter()
        .flat_map(|(offset, length)| image.read_range(offset, length as usize).unwrap())
        .collect();
    assert_eq!(original, expected);
}

#[test]
fn compressed_raw_sparse_and_partial_units_share_all_read_paths() {
    let mut image = image();
    let runs = [
        0x11, 1, 32, 0x01, 15, 0x11, 16, 8, 0x01, 16, 0x11, 1, 20, 0x01, 15, 0,
    ];
    let length = (UNIT * 3 + 5000) as u64;
    let mut default = non_resident("", &runs, length, length, (UNIT * 4) as u64);
    default[24..32].copy_from_slice(&63u64.to_le_bytes());
    let named = non_resident("secret", &runs, length, length, (UNIT * 4) as u64);
    record(&mut image, 16, &[default, named]);
    let compressed = repeated_unit(b'A');
    image[32 * CLUSTER..32 * CLUSTER + compressed.len()].copy_from_slice(&compressed);
    image[40 * CLUSTER..40 * CLUSTER + UNIT].fill(b'B');
    let compressed = repeated_unit(b'C');
    image[60 * CLUSTER..60 * CLUSTER + compressed.len()].copy_from_slice(&compressed);
    let mut expected = vec![b'A'; UNIT];
    expected.extend(vec![b'B'; UNIT]);
    expected.extend(vec![0; UNIT]);
    expected.extend(vec![b'C'; 5000]);
    let (_file, image) = open_image(&image);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert_eq!(volume.read_file(16, usize::MAX).unwrap(), expected);
    assert_eq!(
        volume.read_data_stream(16, "secret", usize::MAX).unwrap(),
        expected
    );
    let mut streamed = Vec::new();
    assert_eq!(
        volume
            .stream_file(16, 77777, |bytes| {
                streamed.extend_from_slice(bytes);
                Ok(())
            })
            .unwrap(),
        length
    );
    assert_eq!(streamed, expected);
    for (offset, length) in [
        (0, 7),
        (UNIT - 5, 20),
        (UNIT * 2 - 3, 40),
        (UNIT * 3 - 2, 5005),
        (17, UNIT * 3),
        (expected.len(), 20),
    ] {
        assert_eq!(
            volume.read_range(16, offset as u64, length).unwrap(),
            expected[offset..expected.len().min(offset + length)]
        );
    }
    assert_eq!(
        volume.file_stat(16, "test").unwrap().magic_hex,
        Some("41 41 41 41 41 41 41 41 41 41 41 41 41 41 41 41".into())
    );
    assert_eq!(
        volume.named_stream_stat(16, "secret", "test").unwrap().size,
        expected.len() as u64
    );
}

#[test]
fn joins_compressed_unit_and_sparse_padding_across_attribute_list() {
    let mut image = image();
    let mut entries = Vec::new();
    for (number, vcn) in [(16u64, 0u64), (17, 1)] {
        let mut entry = vec![0; 32];
        entry[..4].copy_from_slice(&0x80u32.to_le_bytes());
        entry[4..6].copy_from_slice(&32u16.to_le_bytes());
        entry[8..16].copy_from_slice(&vcn.to_le_bytes());
        entry[16..24].copy_from_slice(&(number | (1 << 48)).to_le_bytes());
        entries.extend(entry);
    }
    let mut first = non_resident("", &[0x11, 1, 32, 0], 3883, 3883, UNIT as u64);
    first[24..32].fill(0);
    let mut list = resident(0x20, &entries);
    list[14..16].copy_from_slice(&1u16.to_le_bytes());
    record(&mut image, 16, &[list, first]);
    let mut continuation = non_resident("", &[0x01, 15, 0], 0, 0, 0);
    continuation[16..24].copy_from_slice(&1u64.to_le_bytes());
    continuation[12..14].fill(0); // Only the first extent needs the compressed flag.
    record(&mut image, 17, &[continuation]);
    let compressed = repeated_unit(b'J');
    image[32 * CLUSTER..32 * CLUSTER + compressed.len()].copy_from_slice(&compressed);
    let (_file, image) = open_image(&image);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert_eq!(volume.read_file(16, usize::MAX).unwrap(), vec![b'J'; 3883]);
}

#[test]
fn initialized_tail_is_zero_and_corrupt_units_fail() {
    let mut bytes = image();
    let attribute = non_resident("", &[0x11, 1, 32, 0x01, 15, 0], 6000, 1000, UNIT as u64);
    record(&mut bytes, 16, &[attribute]);
    let compressed = repeated_unit(b'A');
    bytes[32 * CLUSTER..32 * CLUSTER + compressed.len()].copy_from_slice(&compressed);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    let mut expected = vec![b'A'; 1000];
    expected.resize(6000, 0);
    assert_eq!(volume.read_file(16, usize::MAX).unwrap(), expected);
    assert_eq!(volume.read_range(16, 990, 20).unwrap(), expected[990..1010]);
    bytes[32 * CLUSTER..32 * CLUSTER + 6].copy_from_slice(&[3, 0xb0, 2, b'A', 0xff, 0x0f]);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    assert!(volume.read_file(16, usize::MAX).is_err());
    assert!(volume.read_range(16, 0, 10).is_err());
    assert!(volume.stream_file(16, UNIT, |_| Ok(())).is_err());
}

#[test]
fn concatenates_fragmented_compressed_clusters_before_decoding() {
    let mut bytes = image();
    let attribute = non_resident(
        "",
        &[0x11, 1, 32, 0x11, 1, 8, 0x01, 14, 0],
        5096,
        5096,
        UNIT as u64,
    );
    record(&mut bytes, 16, &[attribute]);
    let mut stored = vec![0xff, 0x3f];
    stored.extend(vec![b'A'; 4096]);
    stored.extend_from_slice(&0x33e7u16.to_le_bytes());
    stored.extend(vec![b'B'; 1000]);
    bytes[32 * CLUSTER..33 * CLUSTER].copy_from_slice(&stored[..CLUSTER]);
    bytes[40 * CLUSTER..40 * CLUSTER + stored.len() - CLUSTER].copy_from_slice(&stored[CLUSTER..]);
    let (_file, image) = open_image(&bytes);
    let volume = NtfsVolume::open(&image, &image.partitions[0]).unwrap();
    let mut expected = vec![b'A'; 4096];
    expected.extend(vec![b'B'; 1000]);
    assert_eq!(volume.read_file(16, usize::MAX).unwrap(), expected);
    assert_eq!(
        volume.read_range(16, 4090, 20).unwrap(),
        expected[4090..4110]
    );
}
