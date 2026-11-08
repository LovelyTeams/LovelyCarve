//! NTFS `$UsnJrnl:$J` change-journal (USN) parser.
//!
//! The Update Sequence Number journal records every create / delete / rename /
//! data-overwrite on the volume. Because entries survive even after the
//! subject file's MFT record is reused, the journal recovers per-file activity
//! history that a one-pass MFT scan cannot — the single highest-value NTFS
//! forensic artifact after the MFT itself.
//!
//! This module decodes the raw `$J` byte stream into [`UsnRecord`]s. It
//! delegates V2/V3 decoding to `ntfs-core` with bounded stream framing and
//! strict record/time validation. V4 extent records have no timestamp and are
//! skipped; zero-filled prefixes and inter-page padding do not end the scan.

use serde::{Deserialize, Serialize};

/// One decoded USN journal record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsnRecord {
    /// USN of this record (its byte offset within the journal namespace).
    pub usn: u64,
    /// MFT record number of the file this event concerns (low 48 bits of the
    /// file reference; the high 16 bits are the sequence number).
    pub file_record: u64,
    /// MFT record number of the parent directory at the time of the event.
    pub parent_record: u64,
    /// Event time as RFC3339 UTC, or `None` if the FILETIME was zero/invalid.
    pub timestamp: Option<String>,
    /// Decoded `Reason` flags (e.g. `["DATA_OVERWRITE", "CLOSE"]`).
    pub reasons: Vec<String>,
    /// File / directory name carried by the record (UTF-16LE on disk).
    pub name: String,
    /// Raw `FileAttributes` bitmask (FILE_ATTRIBUTE_*).
    pub file_attributes: u32,
}

fn u16_at(buf: &[u8], off: usize) -> Option<u16> {
    buf.get(off..off + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(buf: &[u8], off: usize) -> Option<u32> {
    buf.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn u64_at(buf: &[u8], off: usize) -> Option<u64> {
    buf.get(off..off + 8)
        .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

/// Windows FILETIME (100 ns ticks since 1601-01-01) → RFC3339 UTC.
fn filetime_to_rfc3339(ticks: i64) -> Option<String> {
    if ticks <= 0 {
        return None;
    }
    // FILETIME epoch (1601) precedes the Unix epoch (1970) by this many seconds.
    const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
    let secs = ticks / 10_000_000 - EPOCH_DIFF_SECS;
    let nanos = ((ticks % 10_000_000) * 100) as u32;
    chrono::DateTime::from_timestamp(secs, nanos).map(|dt| dt.to_rfc3339())
}

/// USN `Reason` bit → name. Mirrors the Windows `USN_REASON_*` constants.
const REASON_FLAGS: &[(u32, &str)] = &[
    (0x0000_0001, "DATA_OVERWRITE"),
    (0x0000_0002, "DATA_EXTEND"),
    (0x0000_0004, "DATA_TRUNCATION"),
    (0x0000_0010, "NAMED_DATA_OVERWRITE"),
    (0x0000_0020, "NAMED_DATA_EXTEND"),
    (0x0000_0040, "NAMED_DATA_TRUNCATION"),
    (0x0000_0100, "FILE_CREATE"),
    (0x0000_0200, "FILE_DELETE"),
    (0x0000_0400, "EA_CHANGE"),
    (0x0000_0800, "SECURITY_CHANGE"),
    (0x0000_1000, "RENAME_OLD_NAME"),
    (0x0000_2000, "RENAME_NEW_NAME"),
    (0x0000_4000, "INDEXABLE_CHANGE"),
    (0x0000_8000, "BASIC_INFO_CHANGE"),
    (0x0001_0000, "HARD_LINK_CHANGE"),
    (0x0002_0000, "COMPRESSION_CHANGE"),
    (0x0004_0000, "ENCRYPTION_CHANGE"),
    (0x0008_0000, "OBJECT_ID_CHANGE"),
    (0x0010_0000, "REPARSE_POINT_CHANGE"),
    (0x0020_0000, "STREAM_CHANGE"),
    (0x0040_0000, "TRANSACTED_CHANGE"),
    (0x0080_0000, "INTEGRITY_CHANGE"),
    (0x8000_0000, "CLOSE"),
];

fn decode_reasons(mask: u32) -> Vec<String> {
    REASON_FLAGS
        .iter()
        .filter(|(bit, _)| mask & bit != 0)
        .map(|(_, name)| (*name).to_string())
        .collect()
}

/// Parses one USN record starting at `buf[0]`. Returns the decoded record and
/// the record's total length (so the caller can advance). Returns `None` when
/// the record is malformed or has zero length (journal end / sparse gap).
fn parse_record(buf: &[u8]) -> Option<(UsnRecord, usize)> {
    let record_len = u32_at(buf, 0)? as usize;
    if record_len > buf.len() {
        return None;
    }
    let major = u16_at(buf, 4)?;

    // V4 describes changed extents and has neither a timestamp nor a name.
    // Never interpret it (or an unknown version) as a V2/V3 event.
    let (time_off, name_len_off, name_off_off, min_len) = match major {
        2 => (32, 56, 58, 60),
        3 => (48, 72, 74, 76),
        _ => return None,
    };

    // Zero length marks the end of populated data; anything shorter than this
    // version's fixed header cannot hold all fields we read below.
    if record_len < min_len || !record_len.is_multiple_of(8) {
        return None;
    }

    let buf = buf.get(..record_len)?;

    let name_len = u16_at(buf, name_len_off)? as usize;
    let name_off = u16_at(buf, name_off_off)? as usize;
    if !name_len.is_multiple_of(2)
        || name_off < min_len
        || !name_off.is_multiple_of(2)
        || name_off.checked_add(name_len)? > record_len
    {
        return None;
    }
    // ntfs-core decodes the binary format. Keep boundary/time validation here:
    // its default Unix epoch for invalid FILETIME must not enter our timeline.
    let record = match major {
        2 => ntfs_core::usn::parse_usn_record_v2(buf).ok()?,
        3 => ntfs_core::usn::parse_usn_record_v3(buf).ok()?,
        _ => return None,
    };
    let timestamp = filetime_to_rfc3339(u64_at(buf, time_off)? as i64);

    Some((
        UsnRecord {
            usn: u64::try_from(record.usn).ok()?,
            file_record: record.mft_entry,
            parent_record: record.parent_mft_entry,
            timestamp,
            reasons: decode_reasons(record.reason.bits()),
            name: record.filename,
            file_attributes: record.file_attributes.bits(),
        },
        record_len,
    ))
}

/// Incremental decoder for a logical `$J` stream. Carries partial records
/// across chunks, skips zero padding between journal pages and bounds memory.
#[derive(Default)]
pub struct UsnStreamDecoder {
    pending: Vec<u8>,
}

impl UsnStreamDecoder {
    pub fn push(
        &mut self,
        bytes: &[u8],
        mut visit: impl FnMut(UsnRecord) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.pending.extend_from_slice(bytes);
        let mut position = 0;
        while self.pending.len() - position >= 8 {
            let buffer = &self.pending[position..];
            let length = u32_at(buffer, 0).unwrap_or(0) as usize;
            // A journal record is much smaller than a 64 KiB page. Invalid
            // lengths cannot make a corrupt stream retain gigabytes of memory.
            if !(8..=65536).contains(&length) || !length.is_multiple_of(8) {
                position += 8;
                continue;
            }
            if length > buffer.len() {
                break;
            }
            if let Some((record, _)) = parse_record(&buffer[..length]) {
                visit(record)?;
            }
            // Also skip known-length V4/future records without inventing times.
            position += length;
        }
        self.pending.drain(..position);
        Ok(())
    }
}

/// Decodes a raw `$J` buffer, including sparse prefixes and page padding.
pub fn parse_usn_stream(buf: &[u8]) -> Vec<UsnRecord> {
    let mut out = Vec::new();
    let mut decoder = UsnStreamDecoder::default();
    // The collection callback cannot fail.
    if let Err(error) = decoder.push(buf, |record| {
        out.push(record);
        Ok(())
    }) {
        tracing::warn!("decode USN buffer failed: {error:#}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal valid USN_RECORD_V2 with the given name.
    fn make_v2(name: &str, reason: u32) -> Vec<u8> {
        let name_utf16: Vec<u8> = name.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let name_off = 60usize;
        let record_len = ((name_off + name_utf16.len()) + 7) & !7;
        let mut buf = vec![0u8; record_len];
        buf[0..4].copy_from_slice(&(record_len as u32).to_le_bytes());
        buf[4..6].copy_from_slice(&2u16.to_le_bytes()); // major version 2
        buf[8..16].copy_from_slice(&0x0002_0000_0000_002Au64.to_le_bytes()); // file ref (rec 42)
        buf[16..24].copy_from_slice(&0x0001_0000_0000_0005u64.to_le_bytes()); // parent ref (rec 5)
        buf[24..32].copy_from_slice(&123u64.to_le_bytes()); // usn
        buf[40..44].copy_from_slice(&reason.to_le_bytes());
        buf[56..58].copy_from_slice(&(name_utf16.len() as u16).to_le_bytes());
        buf[58..60].copy_from_slice(&(name_off as u16).to_le_bytes());
        buf[name_off..name_off + name_utf16.len()].copy_from_slice(&name_utf16);
        buf
    }

    #[test]
    fn parses_single_v2_record() {
        let buf = make_v2("evil.exe", 0x0000_0100 | 0x8000_0000); // FILE_CREATE | CLOSE
        let records = parse_usn_stream(&buf);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.name, "evil.exe");
        assert_eq!(r.file_record, 42);
        assert_eq!(r.parent_record, 5);
        assert_eq!(r.usn, 123);
        assert!(r.reasons.contains(&"FILE_CREATE".to_string()));
        assert!(r.reasons.contains(&"CLOSE".to_string()));
    }

    #[test]
    fn skips_sparse_prefix_and_parses_tail() {
        let mut buf = vec![0u8; 64]; // sparse zeroed prefix
        buf.extend_from_slice(&make_v2("late.txt", 0x0000_0200)); // FILE_DELETE
        let records = parse_usn_stream(&buf);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name, "late.txt");
        assert!(records[0].reasons.contains(&"FILE_DELETE".to_string()));
    }

    #[test]
    fn parses_multiple_records() {
        let mut buf = make_v2("a.txt", 0x0000_0100);
        buf.extend_from_slice(&make_v2("b.txt", 0x0000_0200));
        let records = parse_usn_stream(&buf);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "a.txt");
        assert_eq!(records[1].name, "b.txt");
    }

    /// Builds a USN_RECORD_V3 (16-byte FILE_ID_128 references; fields past the
    /// reference pair shifted +16 vs V2). The MFT record lives in the low 64
    /// bits of each 128-bit reference.
    fn make_v3(name: &str, reason: u32) -> Vec<u8> {
        let name_utf16: Vec<u8> = name.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let name_off = 76usize;
        let record_len = ((name_off + name_utf16.len()) + 7) & !7;
        let mut buf = vec![0u8; record_len];
        buf[0..4].copy_from_slice(&(record_len as u32).to_le_bytes());
        buf[4..6].copy_from_slice(&3u16.to_le_bytes()); // major version 3
                                                        // FileReferenceNumber (16 bytes): low 64 bits carry the MFT reference.
        buf[8..16].copy_from_slice(&0x0002_0000_0000_002Au64.to_le_bytes()); // rec 42
                                                                             // ParentFileReferenceNumber (16 bytes) at +24.
        buf[24..32].copy_from_slice(&0x0001_0000_0000_0005u64.to_le_bytes()); // rec 5
        buf[40..48].copy_from_slice(&123u64.to_le_bytes()); // usn
        buf[56..60].copy_from_slice(&reason.to_le_bytes());
        buf[72..74].copy_from_slice(&(name_utf16.len() as u16).to_le_bytes());
        buf[74..76].copy_from_slice(&(name_off as u16).to_le_bytes());
        buf[name_off..name_off + name_utf16.len()].copy_from_slice(&name_utf16);
        buf
    }

    #[test]
    fn parses_single_v3_record() {
        let buf = make_v3("modern.dll", 0x0000_0100 | 0x8000_0000); // FILE_CREATE | CLOSE
        let records = parse_usn_stream(&buf);
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.name, "modern.dll");
        // Regression guard: V2 offsets would misread these from inside the
        // 128-bit reference fields.
        assert_eq!(r.file_record, 42);
        assert_eq!(r.parent_record, 5);
        assert_eq!(r.usn, 123);
        assert!(r.reasons.contains(&"FILE_CREATE".to_string()));
        assert!(r.reasons.contains(&"CLOSE".to_string()));
    }

    #[test]
    fn parses_mixed_v2_and_v3_records() {
        let mut buf = make_v2("legacy.txt", 0x0000_0100);
        buf.extend_from_slice(&make_v3("modern.txt", 0x0000_0200));
        let records = parse_usn_stream(&buf);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].name, "legacy.txt");
        assert_eq!(records[1].name, "modern.txt");
        assert_eq!(records[1].parent_record, 5);
    }

    #[test]
    fn rejects_truncated_v3_header() {
        // A V3 record claiming 70 bytes is shorter than the 76-byte V3 header;
        // it must be rejected, not parsed with an out-of-bounds empty name.
        let mut buf = vec![0u8; 70];
        buf[0..4].copy_from_slice(&70u32.to_le_bytes());
        buf[4..6].copy_from_slice(&3u16.to_le_bytes());
        let records = parse_usn_stream(&buf);
        assert!(records.is_empty());
    }
    #[test]
    fn streams_records_across_every_chunk_boundary_and_page_padding() {
        let mut bytes = make_v2("before.txt", 0x100);
        bytes.resize(4096, 0);
        bytes.extend_from_slice(&make_v3("after.txt", 0x200));
        for chunk in [1, 7, 61, 100, 4096] {
            let mut decoder = UsnStreamDecoder::default();
            let mut names = Vec::new();
            for piece in bytes.chunks(chunk) {
                decoder
                    .push(piece, |record| {
                        names.push(record.name);
                        Ok(())
                    })
                    .expect("decode");
            }
            assert_eq!(names, vec!["before.txt", "after.txt"]);
        }
    }

    #[test]
    fn does_not_read_names_from_the_next_record_or_turn_v4_extents_into_dates() {
        let mut invalid = make_v2("bad.txt", 0x100);
        let length = invalid.len() as u16;
        invalid[58..60].copy_from_slice(&length.to_le_bytes());
        let mut v4 = make_v3("extent", 1);
        v4[4..6].copy_from_slice(&4u16.to_le_bytes());
        invalid.extend_from_slice(&v4);
        invalid.extend_from_slice(&make_v2("valid.txt", 0x100));
        let records = parse_usn_stream(&invalid);
        assert_eq!(
            records.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["valid.txt"]
        );
        assert_eq!(records[0].timestamp, None);
    }

    #[test]
    fn preserves_filetime_precision_when_adapting_the_library_record() {
        let mut bytes = make_v2("dated.txt", 0x100);
        bytes[32..40].copy_from_slice(&133_932_133_238_765_432u64.to_le_bytes());
        assert_eq!(
            parse_usn_stream(&bytes)[0].timestamp.as_deref(),
            Some("2025-06-01T01:02:03.876543200+00:00")
        );
    }
}
