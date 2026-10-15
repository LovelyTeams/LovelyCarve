use crate::error::{EwfError, Result};
use safe_read::{le_u32, le_u64};

/// EWF v1 magic signature: `"EVF\x09\x0d\x0a\xff\x00"` (8 bytes).
pub const EVF_SIGNATURE: [u8; 8] = [0x45, 0x56, 0x46, 0x09, 0x0d, 0x0a, 0xff, 0x00];

/// EWF-L01 magic signature: `"LVF\x09\x0d\x0a\xff\x00"` (8 bytes).
///
/// EnCase's *Logical* Evidence File. Same v1 container and section machinery as
/// an E01, but what it carries is a tree of file ENTRIES in an `ltree` section
/// rather than disk sectors -- there is no partition table and no filesystem.
pub const LVF_SIGNATURE: [u8; 8] = [0x4c, 0x56, 0x46, 0x09, 0x0d, 0x0a, 0xff, 0x00];

/// Which EWF v1 container a segment file is.
///
/// The header parser reports this rather than assuming, because the two share
/// every byte of layout after the signature and differ entirely in meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EwfKind {
    /// `EVF` — a physical/bitstream image (E01).
    Physical,
    /// `LVF` — a logical evidence file (L01).
    Logical,
}

/// Size of the EWF v1 file header in bytes.
pub const FILE_HEADER_SIZE: usize = 13;

/// Size of an EWF v1 section descriptor in bytes (16-byte type + 8 next +
/// 8 size + 40 pad + 4 adler-32).
pub const SECTION_DESCRIPTOR_SIZE: usize = 76;

/// Byte offset of the stored adler-32 within a section descriptor; the checksum
/// covers descriptor bytes `[0..SECTION_DESCRIPTOR_CRC_OFFSET]`.
pub const SECTION_DESCRIPTOR_CRC_OFFSET: usize = 72;

/// `ewf_data_t` (volume/disk) body length when the trailing adler-32 is present.
/// The checksum covers bytes `[0..VOLUME_BODY_CRC_OFFSET]` and is stored at
/// `[VOLUME_BODY_CRC_OFFSET..VOLUME_BODY_CRC_OFFSET+4]`.
pub const VOLUME_BODY_FULL_SIZE: usize = 1052;

/// Byte offset of the stored adler-32 within a full `ewf_data_t` body.
pub const VOLUME_BODY_CRC_OFFSET: usize = 1048;

/// Number of bytes a table-section header occupies before its entries
/// (`entry_count`(4) + pad(4) + `base_offset`(8) + pad(4) + adler-32(4)).
pub const TABLE_HEADER_SIZE: usize = 24;

/// Byte offset of the stored adler-32 within a table header; it covers table
/// header bytes `[0..TABLE_HEADER_CRC_OFFSET]`.
pub const TABLE_HEADER_CRC_OFFSET: usize = 20;

/// Default LRU cache size (number of decompressed chunks to keep).
pub(crate) const DEFAULT_LRU_SIZE: usize = 100;

/// Compute the adler-32 checksum used by every EWF v1 section CRC.
///
/// EWF stores a little-endian adler-32 over a defined prefix of each
/// descriptor / volume body / table header. This is the single entry point so
/// the reader and any consumer (e.g. an integrity auditor) agree bit-for-bit.
#[must_use]
pub fn adler32(data: &[u8]) -> u32 {
    adler2::adler32_slice(data)
}

// ---------------------------------------------------------------------------
// EWF File Header (13 bytes)
// ---------------------------------------------------------------------------

/// Parsed EWF v1 file header. Present at offset 0 of every segment file.
///
/// Layout (little-endian):
/// | Offset | Size | Field          |
/// |--------|------|----------------|
/// | 0      | 8    | EVF signature  |
/// | 8      | 1    | Fields_start   |
/// | 9      | 2    | Segment number |
/// | 11     | 2    | Fields_end     |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EwfFileHeader {
    pub segment_number: u16,
    /// Which container this segment belongs to.
    pub kind: EwfKind,
}

impl EwfFileHeader {
    /// Parse a file header from a byte slice (must be >= 13 bytes).
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < FILE_HEADER_SIZE {
            return Err(EwfError::BufferTooShort {
                expected: FILE_HEADER_SIZE,
                got: buf.len(),
            });
        }
        // Report the container, never infer it: the two signatures are the only
        // thing distinguishing a bitstream image from a file-entry tree, and
        // every byte after them is identical.
        let kind = match &buf[0..8] {
            b if b == EVF_SIGNATURE => EwfKind::Physical,
            b if b == LVF_SIGNATURE => EwfKind::Logical,
            other => {
                let mut found = [0u8; 8];
                found.copy_from_slice(other);
                return Err(EwfError::InvalidSignature { found });
            }
        };
        let segment_number = u16::from_le_bytes([buf[9], buf[10]]);
        Ok(Self {
            segment_number,
            kind,
        })
    }
}

// ---------------------------------------------------------------------------
// Section Descriptor (76 bytes)
// ---------------------------------------------------------------------------

/// Parsed EWF v1 section descriptor. Forms a linked list within each segment.
///
/// Layout (little-endian):
/// | Offset | Size | Field       |
/// |--------|------|-------------|
/// | 0      | 16   | Type (NUL-padded string) |
/// | 16     | 8    | Next (absolute file offset) |
/// | 24     | 8    | SectionSize |
/// | 72     | 4    | Checksum    |
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionDescriptor {
    /// Section type string (e.g. "header", "volume", "table", "sectors", "done").
    pub section_type: String,
    /// Absolute file offset of the next section descriptor (0 = end of chain).
    pub next: u64,
    /// Size of this section's data (including the 76-byte descriptor itself).
    pub section_size: u64,
    /// Absolute file offset where this descriptor was found.
    pub offset: u64,
    /// Stored adler-32 from descriptor bytes `[72..76]`, covering `[0..72]`.
    pub stored_crc: u32,
}

impl SectionDescriptor {
    /// Parse a section descriptor from a 76-byte buffer.
    /// `offset` is the absolute file position where this descriptor starts.
    pub fn parse(buf: &[u8], offset: u64) -> Result<Self> {
        if buf.len() < SECTION_DESCRIPTOR_SIZE {
            return Err(EwfError::BufferTooShort {
                expected: SECTION_DESCRIPTOR_SIZE,
                got: buf.len(),
            });
        }
        // Type: 16 bytes, NUL-terminated ASCII
        let type_end = buf[..16].iter().position(|&b| b == 0).unwrap_or(16);
        let section_type = String::from_utf8_lossy(&buf[..type_end]).into_owned();
        let next = le_u64(buf, 16);
        let section_size = le_u64(buf, 24);
        let stored_crc = le_u32(buf, SECTION_DESCRIPTOR_CRC_OFFSET);
        Ok(Self {
            section_type,
            next,
            section_size,
            offset,
            stored_crc,
        })
    }

    /// Byte range of the descriptor that the stored adler-32 covers: `[0..72]`.
    #[must_use]
    pub fn crc_covers() -> std::ops::Range<usize> {
        0..SECTION_DESCRIPTOR_CRC_OFFSET
    }

    /// Recompute the descriptor adler-32 over `raw[0..72]` and compare it to the
    /// stored value. `raw` is the 76-byte descriptor this was parsed from.
    /// Returns `false` if `raw` is too short to cover the checksummed range.
    #[must_use]
    pub fn verify_crc(&self, raw: &[u8]) -> bool {
        raw.get(Self::crc_covers())
            .is_some_and(|covered| adler32(covered) == self.stored_crc)
    }
}

// ---------------------------------------------------------------------------
// Volume Section (parsed from "volume" or "disk" section data)
// ---------------------------------------------------------------------------

/// Image geometry extracted from the EWF volume section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EwfVolume {
    pub chunk_count: u32,
    pub sectors_per_chunk: u32,
    pub bytes_per_sector: u32,
    pub sector_count: u64,
    /// `media_type` byte at `ewf_data_t[0]`.
    pub media_type: u8,
    /// `set_identifier` GUID at `ewf_data_t[64..80]` (all-zero = absent).
    pub set_identifier: [u8; 16],
    /// Stored body adler-32 at `[1048..1052]`, present only when the parsed body
    /// is at least `VOLUME_BODY_FULL_SIZE` (1052) bytes — `None` otherwise.
    pub stored_crc: Option<u32>,
}

impl EwfVolume {
    /// Parse volume data from bytes following a "volume"/"disk" section descriptor.
    ///
    /// Reads the geometry fields from the first 24 bytes; `media_type`,
    /// `set_identifier`, and the body CRC are read when `buf` is long enough,
    /// otherwise they default (zero / `None`).
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < 24 {
            return Err(EwfError::BufferTooShort {
                expected: 24,
                got: buf.len(),
            });
        }
        let media_type = buf[0];
        let chunk_count = le_u32(buf, 4);
        let sectors_per_chunk = le_u32(buf, 8);
        let bytes_per_sector = le_u32(buf, 12);
        let sector_count = le_u64(buf, 16);
        let mut set_identifier = [0u8; 16];
        if let Some(s) = buf.get(64..80) {
            set_identifier.copy_from_slice(s);
        }
        let stored_crc = if buf.len() >= VOLUME_BODY_FULL_SIZE {
            Some(le_u32(buf, VOLUME_BODY_CRC_OFFSET))
        } else {
            None
        };
        Ok(Self {
            chunk_count,
            sectors_per_chunk,
            bytes_per_sector,
            sector_count,
            media_type,
            set_identifier,
            stored_crc,
        })
    }

    /// Chunk size in bytes (`sectors_per_chunk` * `bytes_per_sector`).
    #[must_use]
    pub fn chunk_size(&self) -> u64 {
        u64::from(self.sectors_per_chunk) * u64::from(self.bytes_per_sector)
    }

    /// Total image size in bytes (`bytes_per_sector` * `sector_count`).
    ///
    /// Saturates instead of overflowing: `sector_count` is an untrusted `u64`
    /// from the image, so a malformed geometry could otherwise overflow the
    /// multiplication and panic in a debug build.
    #[must_use]
    pub fn total_size(&self) -> u64 {
        u64::from(self.bytes_per_sector).saturating_mul(self.sector_count)
    }

    /// Byte range of the body that the stored adler-32 covers: `[0..1048]`.
    #[must_use]
    pub fn crc_covers() -> std::ops::Range<usize> {
        0..VOLUME_BODY_CRC_OFFSET
    }

    /// Recompute the body adler-32 over `raw[0..1048]` and compare to the stored
    /// value. Returns `None` when no CRC was stored (body shorter than 1052),
    /// else `Some(matches)`.
    #[must_use]
    pub fn verify_crc(&self, raw: &[u8]) -> Option<bool> {
        let stored = self.stored_crc?;
        Some(
            raw.get(Self::crc_covers())
                .is_some_and(|covered| adler32(covered) == stored),
        )
    }
}

// ---------------------------------------------------------------------------
// Table Entry (4 bytes) and Chunk metadata
// ---------------------------------------------------------------------------

/// A single table entry: 4-byte bitfield where bit 31 = compressed, bits 0-30 = offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableEntry {
    pub compressed: bool,
    pub chunk_offset: u32,
}

impl TableEntry {
    /// Parse a table entry from a 4-byte little-endian value.
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < 4 {
            return Err(EwfError::BufferTooShort {
                expected: 4,
                got: buf.len(),
            });
        }
        let raw = le_u32(buf, 0);
        let compressed = (raw >> 31) != 0;
        let chunk_offset = raw & 0x7FFF_FFFF;
        Ok(Self {
            compressed,
            chunk_offset,
        })
    }
}

/// The 24-byte header that precedes a "table"/"table2" section's entries.
///
/// Layout (little-endian): `entry_count(4) | pad(4) | base_offset(8) |
/// pad(4) | adler-32(4)`. The checksum covers header bytes `[0..20]` and is
/// stored at `[20..24]`; a stored value of 0 means the writer omitted it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TableHeader {
    /// Number of 4-byte chunk-offset entries that follow this header.
    pub entry_count: u32,
    /// Base file offset added to each entry's relative chunk offset.
    pub base_offset: u64,
    /// Stored adler-32 over `[0..20]`; 0 = the writer did not store one.
    pub stored_crc: u32,
}

impl TableHeader {
    /// Parse a table header from the bytes at the start of a table section's
    /// body (immediately after its section descriptor). Requires all 24 bytes,
    /// including the CRC field at `[20..24]`.
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < TABLE_HEADER_SIZE {
            return Err(EwfError::BufferTooShort {
                expected: TABLE_HEADER_SIZE,
                got: buf.len(),
            });
        }
        Ok(Self {
            entry_count: le_u32(buf, 0),
            base_offset: le_u64(buf, 8),
            stored_crc: le_u32(buf, TABLE_HEADER_CRC_OFFSET),
        })
    }

    /// Byte range of the header that the stored adler-32 covers: `[0..20]`.
    #[must_use]
    pub fn crc_covers() -> std::ops::Range<usize> {
        0..TABLE_HEADER_CRC_OFFSET
    }

    /// Recompute the header adler-32 over `raw[0..20]` and compare to the stored
    /// value. Returns `None` when no CRC was stored (`stored_crc == 0`), else
    /// `Some(matches)`.
    #[must_use]
    pub fn verify_crc(&self, raw: &[u8]) -> Option<bool> {
        if self.stored_crc == 0 {
            return None;
        }
        Some(
            raw.get(Self::crc_covers())
                .is_some_and(|covered| adler32(covered) == self.stored_crc),
        )
    }
}

/// Internal chunk metadata: where to find and how to read one chunk of image data.
///
/// Packed to 16 bytes (from 32) because the in-RAM chunk table dominates memory
/// for large images — one entry per ~32 KB, so a 2 TB image is ~67M entries
/// (~1 GB packed vs ~2 GB unpacked). The `compressed` flag rides in the offset's
/// high bit (a file offset is always < 2^63), and `size`/`segment_idx` fit `u32`
/// (a chunk is capped at 128 MB on disk; segment counts are small). Access goes
/// through the methods so the packing stays an implementation detail.
#[derive(Debug, Clone)]
pub(crate) struct Chunk {
    /// Bit 63 = zlib-compressed flag; bits 0..63 = file offset within the segment.
    offset_packed: u64,
    /// Low 28 bits: segment index. High nibble: EWF2 marker and three flags.
    segment_idx: u32,
    /// On-disk size: compressed length if compressed, else `chunk_size`.
    size: u32,
}

impl Chunk {
    const COMPRESSED_BIT: u64 = 1 << 63;

    pub(crate) fn new(segment_idx: usize, compressed: bool, offset: u64, size: u64) -> Self {
        // A real EWF file offset always fits in 63 bits (the top bit is the
        // compressed flag). A malformed/crafted offset is masked below rather
        // than asserted — never panic on untrusted input; the resulting bounded
        // offset is caught by downstream range checks.
        let flag = if compressed { Self::COMPRESSED_BIT } else { 0 };
        Self {
            offset_packed: (offset & !Self::COMPRESSED_BIT) | flag,
            segment_idx: segment_idx as u32,
            size: size as u32,
        }
    }

    pub(crate) fn with_v2_flags(mut self, flags: u32) -> Self {
        self.segment_idx |= (8 | (flags & 7)) << 28;
        self
    }
    pub(crate) fn v2_flags(&self) -> Option<u32> {
        (self.segment_idx & (1 << 31) != 0).then_some((self.segment_idx >> 28) & 7)
    }

    pub(crate) fn segment_idx(&self) -> usize {
        (self.segment_idx & 0x0fff_ffff) as usize
    }

    pub(crate) fn compressed(&self) -> bool {
        self.offset_packed & Self::COMPRESSED_BIT != 0
    }

    pub(crate) fn offset(&self) -> u64 {
        self.offset_packed & !Self::COMPRESSED_BIT
    }

    pub(crate) fn size(&self) -> u64 {
        u64::from(self.size)
    }

    pub(crate) fn set_size(&mut self, size: u64) {
        self.size = size as u32;
    }
}

#[cfg(test)]
mod crc_tests {
    use super::*;

    #[test]
    fn adler32_matches_published_vectors() {
        assert_eq!(adler32(b""), 0x0000_0001);
        assert_eq!(adler32(b"abc"), 0x024D_0127);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    /// Build a 76-byte section descriptor with a correct stored CRC over `[0..72]`.
    fn descriptor(type_name: &[u8]) -> [u8; SECTION_DESCRIPTOR_SIZE] {
        let mut d = [0u8; SECTION_DESCRIPTOR_SIZE];
        d[..type_name.len()].copy_from_slice(type_name);
        d[16..24].copy_from_slice(&1000u64.to_le_bytes()); // next
        d[24..32].copy_from_slice(&170u64.to_le_bytes()); // section_size
        let crc = adler32(&d[..SECTION_DESCRIPTOR_CRC_OFFSET]);
        d[SECTION_DESCRIPTOR_CRC_OFFSET..].copy_from_slice(&crc.to_le_bytes());
        d
    }

    #[test]
    fn descriptor_verify_crc_accepts_correct_and_rejects_tampered() {
        let d = descriptor(b"volume");
        let desc = SectionDescriptor::parse(&d, 13).unwrap();
        assert_eq!(SectionDescriptor::crc_covers(), 0..72);
        assert!(desc.verify_crc(&d), "correct CRC must verify");

        // Flip a covered byte: stored_crc no longer matches the recomputed CRC.
        let mut tampered = d;
        tampered[24] ^= 0xFF;
        // stored_crc is still the original; recompute over the tampered prefix.
        assert!(!desc.verify_crc(&tampered), "tampered body must fail");
    }

    #[test]
    fn descriptor_verify_crc_false_when_raw_too_short() {
        let d = descriptor(b"done");
        let desc = SectionDescriptor::parse(&d, 0).unwrap();
        assert!(!desc.verify_crc(&d[..50]));
    }

    #[test]
    fn volume_crc_present_only_for_full_body() {
        // 94-byte body (reader's path): no stored CRC.
        let short = [0u8; 94];
        let vol = EwfVolume::parse(&short).unwrap();
        assert_eq!(vol.stored_crc, None);
        assert_eq!(vol.verify_crc(&short), None);

        // Full 1052-byte body with a correct CRC at [1048..1052].
        let mut full = vec![0u8; VOLUME_BODY_FULL_SIZE];
        full[0] = 0x01; // media_type
        full[4..8].copy_from_slice(&5u32.to_le_bytes()); // chunk_count
        let crc = adler32(&full[..VOLUME_BODY_CRC_OFFSET]);
        full[VOLUME_BODY_CRC_OFFSET..].copy_from_slice(&crc.to_le_bytes());
        let vol = EwfVolume::parse(&full).unwrap();
        assert_eq!(vol.media_type, 0x01);
        assert_eq!(vol.chunk_count, 5);
        assert_eq!(vol.stored_crc, Some(crc));
        assert_eq!(EwfVolume::crc_covers(), 0..1048);
        assert_eq!(vol.verify_crc(&full), Some(true));

        full[4] ^= 0xFF; // tamper chunk_count
        assert_eq!(vol.verify_crc(&full), Some(false));
    }

    #[test]
    fn volume_total_size_saturates_on_overflow() {
        // Malformed geometry from an untrusted image: bytes_per_sector * sector_count
        // overflows u64. total_size() must saturate, never panic.
        let mut buf = [0u8; 24];
        buf[12..16].copy_from_slice(&u32::MAX.to_le_bytes()); // bytes_per_sector
        buf[16..24].copy_from_slice(&u64::MAX.to_le_bytes()); // sector_count
        let vol = EwfVolume::parse(&buf).unwrap();
        assert_eq!(vol.total_size(), u64::MAX);
    }

    #[test]
    fn table_header_crc_none_when_stored_zero() {
        // entry_count=3, base_offset=200, stored_crc=0 → no check.
        let mut hdr = [0u8; TABLE_HEADER_SIZE];
        hdr[0..4].copy_from_slice(&3u32.to_le_bytes());
        hdr[8..16].copy_from_slice(&200u64.to_le_bytes());
        let th = TableHeader::parse(&hdr).unwrap();
        assert_eq!(th.entry_count, 3);
        assert_eq!(th.base_offset, 200);
        assert_eq!(th.stored_crc, 0);
        assert_eq!(th.verify_crc(&hdr), None);
    }

    #[test]
    fn table_header_crc_verifies_when_stored() {
        let mut hdr = [0u8; TABLE_HEADER_SIZE];
        hdr[0..4].copy_from_slice(&7u32.to_le_bytes());
        hdr[8..16].copy_from_slice(&4096u64.to_le_bytes());
        // Independent fixed vector: 20 header bytes, CRC at byte 20.
        hdr[20..24].copy_from_slice(&0x0150_0018u32.to_le_bytes());
        let th = TableHeader::parse(&hdr).unwrap();
        assert_eq!(th.stored_crc, 0x0150_0018);
        assert_eq!(TableHeader::crc_covers(), 0..20);
        assert_eq!(th.verify_crc(&hdr), Some(true));

        // The four reserved bytes after base_offset are also covered.
        hdr[16] ^= 1;
        assert_eq!(th.verify_crc(&hdr), Some(false));
        hdr[16] ^= 1;
        hdr[0] ^= 0xFF;
        assert_eq!(th.verify_crc(&hdr), Some(false));
    }

    /// RED: a real L01 begins `LVF`, and the header parser must accept it and
    /// say which container it is.
    ///
    /// Everything after the signature is byte-identical to an E01 header, which
    /// is exactly why this must be explicit: the two are indistinguishable by
    /// layout and completely different in meaning. An E01 holds disk sectors; an
    /// L01 holds a tree of file entries and has no filesystem at all. A reader
    /// that guesses will hand a caller sectors that are not sectors.
    #[test]
    fn file_header_accepts_a_logical_evidence_signature() {
        let mut hdr = [0u8; FILE_HEADER_SIZE];
        hdr[0..8].copy_from_slice(&LVF_SIGNATURE);
        hdr[8] = 0x01;
        hdr[9..11].copy_from_slice(&7u16.to_le_bytes());

        let h = EwfFileHeader::parse(&hdr).expect("an LVF header is a valid EWF v1 header");
        assert_eq!(h.segment_number, 7, "segment number is read the same way");
        assert_eq!(
            h.kind,
            EwfKind::Logical,
            "and the container must be reported as LOGICAL, never guessed"
        );
    }

    /// RED: an E01 must still be reported as physical, so the new branch cannot
    /// silently reclassify every existing image.
    #[test]
    fn file_header_still_reports_physical_for_evf() {
        let mut hdr = [0u8; FILE_HEADER_SIZE];
        hdr[0..8].copy_from_slice(&EVF_SIGNATURE);
        hdr[8] = 0x01;
        hdr[9..11].copy_from_slice(&1u16.to_le_bytes());
        let h = EwfFileHeader::parse(&hdr).expect("EVF still parses");
        assert_eq!(h.kind, EwfKind::Physical);
    }

    /// RED: anything that is neither signature is still refused. Widening the
    /// parser must not turn it into one that accepts arbitrary bytes.
    #[test]
    fn file_header_still_rejects_an_unknown_signature() {
        let mut hdr = [0u8; FILE_HEADER_SIZE];
        hdr[0..8].copy_from_slice(b"NOTEWF\x00\x00");
        assert!(
            EwfFileHeader::parse(&hdr).is_err(),
            "an unknown signature must still be refused"
        );
    }
}
