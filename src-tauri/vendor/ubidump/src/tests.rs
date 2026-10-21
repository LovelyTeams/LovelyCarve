use std::io::{Read, Seek, SeekFrom};
use std::sync::Arc;

use anyhow::Result;

use super::*;
use crate::ubifs::common::VTBL_VOLID;

const PEB: usize = 32768;
const DATA: usize = 4096;
const LEB: usize = PEB - DATA;

struct Bytes(Vec<u8>);
impl ReadAt for Bytes {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        Ok(self.0[offset as usize..offset as usize + length].to_vec())
    }
}

// Independent, bit-at-a-time UBI CRC oracle; does not use the reader's CRC.
fn checksum(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    crc
}

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}
fn seal(bytes: &mut [u8]) {
    let end = bytes.len() - 4;
    put32(bytes, end, checksum(&bytes[..end]));
}

fn peb(volume: u32, number: u32, sequence: u64, static_volume: bool, data: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0xff; PEB];
    let ec = &mut bytes[..64];
    ec.fill(0);
    ec[..4].copy_from_slice(b"UBI#");
    ec[4] = 1;
    put32(ec, 16, 2048);
    put32(ec, 20, DATA as u32);
    put32(ec, 24, 42);
    seal(ec);
    let vid = &mut bytes[2048..2112];
    vid.fill(0);
    vid[..4].copy_from_slice(b"UBI!");
    vid[4] = 1;
    vid[5] = if static_volume { 2 } else { 1 };
    put32(vid, 8, volume);
    put32(vid, 12, number);
    if static_volume {
        put32(vid, 20, data.len() as u32);
        put32(vid, 24, 2);
        put32(vid, 32, checksum(data));
    }
    vid[40..48].copy_from_slice(&sequence.to_be_bytes());
    seal(vid);
    bytes[DATA..DATA + data.len()].copy_from_slice(data);
    bytes
}

fn table(static_volume: bool) -> Vec<u8> {
    let mut bytes = vec![0; 128 * 172];
    for (index, record) in bytes.as_chunks_mut::<172>().0.iter_mut().enumerate() {
        if index == 2 {
            put32(record, 0, 3);
            put32(record, 4, 1);
            record[12] = if static_volume { 2 } else { 1 };
            record[14..16].copy_from_slice(&4u16.to_be_bytes());
            record[16..20].copy_from_slice(b"root");
        }
        seal(record);
    }
    bytes
}

fn fixture(static_volume: bool) -> Bytes {
    let table = table(static_volume);
    Bytes(
        [
            peb(VTBL_VOLID, 0, 0, false, &table),
            peb(VTBL_VOLID, 1, 0, false, &table),
            peb(
                2,
                1,
                1,
                static_volume,
                if static_volume { b"tail" } else { &[b'B'; LEB] },
            ),
            peb(2, 0, 1, static_volume, &[b'A'; LEB]),
        ]
        .concat(),
    )
}

#[test]
fn reconstructs_reordered_dynamic_lebs_and_erased_holes() {
    let bytes = fixture(false);
    let image = UbiImage::open(&bytes).unwrap();
    assert_eq!(image.peb_size, PEB as u32);
    let volume = &image.volumes[&2];
    assert_eq!(volume.name, "root");
    assert_eq!(volume.length, (3 * LEB) as u64);
    assert_eq!(
        volume.read_at(&bytes, (LEB - 3) as u64, 7).unwrap(),
        b"AAABBBB"
    );
    assert_eq!(
        volume.read_at(&bytes, (2 * LEB - 2) as u64, 5).unwrap(),
        [b'B', b'B', 255, 255, 255]
    );
    assert_eq!(
        volume.read_at(&bytes, volume.length - 2, 20).unwrap(),
        [255; 2]
    );
    assert!(volume.read_at(&bytes, u64::MAX, 20).unwrap().is_empty());
    let mut reader = UbiReader::new(bytes, Arc::clone(volume));
    reader.seek(SeekFrom::Start((LEB - 3) as u64)).unwrap();
    let mut data = [0; 7];
    reader.read_exact(&mut data).unwrap();
    assert_eq!(&data, b"AAABBBB");
    assert!(reader.seek(SeekFrom::Current(i64::MIN)).is_err());
}

#[test]
fn static_final_length_and_data_crc_are_verified() {
    let mut bytes = fixture(true);
    let image = UbiImage::open(&bytes).unwrap();
    let volume = &image.volumes[&2];
    assert_eq!(volume.length, (LEB + 4) as u64);
    assert_eq!(
        volume.read_at(&bytes, (LEB - 2) as u64, 20).unwrap(),
        b"AAtail"
    );
    bytes.0[2 * PEB + DATA] ^= 1;
    assert!(
        UbiImage::open(&bytes)
            .err()
            .unwrap()
            .to_string()
            .contains("data CRC")
    );
}

#[test]
fn newest_sequence_wins_independent_of_physical_order() {
    let mut bytes = fixture(false);
    bytes.0.extend(peb(2, 0, 9, false, &[b'C'; LEB]));
    bytes.0.extend(peb(2, 0, 2, false, &[b'D'; LEB]));
    let image = UbiImage::open(&bytes).unwrap();
    assert_eq!(image.volumes[&2].read_at(&bytes, 0, 5).unwrap(), b"CCCCC");
}

#[test]
fn duplicate_sequence_is_ambiguous() {
    let mut bytes = fixture(false);
    bytes.0.extend(peb(2, 0, 1, false, &[b'C'; LEB]));
    assert!(UbiImage::open(&bytes).is_err());
}

#[test]
fn backup_table_recovers_crc_failure_but_conflicting_valid_tables_fail() {
    let mut bytes = fixture(false);
    bytes.0[DATA] ^= 1;
    assert!(UbiImage::open(&bytes).is_ok());
    let mut bytes = fixture(false);
    bytes.0[DATA + 2 * 172 + 16] = b'x';
    seal(&mut bytes.0[DATA + 2 * 172..DATA + 3 * 172]);
    assert!(UbiImage::open(&bytes).is_err());
}

#[test]
fn malformed_names_and_short_records_do_not_panic() {
    use crate::ubifs::vtbl_record::UbiVtblRecord;
    let mut bytes = table(false);
    let record = &mut bytes[2 * 172..3 * 172];
    record[14..16].copy_from_slice(&u16::MAX.to_be_bytes());
    seal(record);
    assert!(UbiVtblRecord::parse(record).is_err());
    for length in 0..172 {
        assert!(UbiVtblRecord::parse(&record[..length]).is_err());
    }
}

#[test]
fn truncation_header_crc_and_missing_static_blocks_fail() {
    let mut bytes = fixture(false);
    bytes.0.pop();
    assert!(UbiImage::open(&bytes).is_err());
    let mut bytes = fixture(false);
    bytes.0[3 * PEB + 30] ^= 1;
    assert!(UbiImage::open(&bytes).is_err());
    let mut bytes = fixture(true);
    bytes.0.truncate(3 * PEB);
    assert!(UbiImage::open(&bytes).is_err());
}

#[test]
fn bounded_source_short_reads_fail() {
    struct Short;
    impl ReadAt for Short {
        fn size(&self) -> u64 {
            4096
        }
        fn read_at(&self, _: u64, _: usize) -> Result<Vec<u8>> {
            Ok(vec![0; 3])
        }
    }
    assert!(UbiImage::open(&Short).is_err());
}

#[test]
fn mixed_nonzero_image_sequences_fail_even_after_zero_sequence_headers() {
    let mut bytes = fixture(false);
    for physical in 0..2 {
        let ec = &mut bytes.0[physical * PEB..physical * PEB + 64];
        put32(ec, 24, 0);
        seal(ec);
    }
    let ec = &mut bytes.0[3 * PEB..3 * PEB + 64];
    put32(ec, 24, 99);
    seal(ec);
    assert!(
        UbiImage::open(&bytes)
            .err()
            .unwrap()
            .to_string()
            .contains("mixed UBI")
    );
}

#[test]
fn incomplete_updates_and_invalid_leb_ranges_fail() {
    let mut bytes = fixture(false);
    for physical in 0..2 {
        let record = &mut bytes.0[physical * PEB + DATA + 2 * 172..physical * PEB + DATA + 3 * 172];
        record[13] = 1;
        seal(record);
    }
    assert!(
        UbiImage::open(&bytes)
            .err()
            .unwrap()
            .to_string()
            .contains("incomplete update")
    );
    let mut bytes = fixture(false);
    let vid = &mut bytes.0[3 * PEB + 2048..3 * PEB + 2112];
    put32(vid, 12, u32::MAX);
    seal(vid);
    assert!(UbiImage::open(&bytes).is_err());
}
