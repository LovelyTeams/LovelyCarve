//! Locate and decrypt the CoreStorage encrypted-metadata region.
//!
//! Pipeline (see `docs/RESEARCH.md`, cross-checked against libfvde
//! `libfvde_metadata_read_type_0x0011`):
//! 1. Read the plaintext metadata region starting at
//!    `metadata_block_numbers[0] * block_size`; its first block has type
//!    `0x0011` and its payload carries `metadata_size` (region length) at
//!    payload offset 0 and a volume-groups-descriptor offset at payload 156.
//! 2. At that region-relative descriptor offset, read the encrypted-metadata
//!    block count (+8) and the 48-bit primary / secondary block numbers
//!    (+32 / +40). Verified on the fvdetest ground truth: count = 6144,
//!    primary = 2049.
//! 3. AES-XTS-128 decrypt the encrypted-metadata region: key1 = header key_data,
//!    key2 = physical-volume identifier, unit = 8192 bytes, tweak = 0-based unit
//!    index within the region.

use crate::error::FileVaultError;
use crate::read::{le_u16, le_u32, le_u64};
use crate::volume_header::VolumeHeader;
use crate::xts;

/// Block type of the first plaintext metadata block (the one whose payload holds
/// the volume-groups descriptor pointing at the encrypted metadata).
const BLOCK_TYPE_ENCRYPTED_METADATA_POINTER: u16 = 0x0011;
/// AES-XTS unit size for the encrypted metadata region.
const METADATA_UNIT_SIZE: usize = 8192;
/// Fixed 64-byte metadata-block header (checksum, version, type, serial,
/// transaction id, object id, number, block size, …).
pub const BLOCK_HEADER_SIZE: usize = 64;
/// Offset of the metadata-block header type field.
const BLOCK_HEADER_TYPE_OFFSET: usize = 10;
/// Payload offset (from the header end) of the u32 metadata region size.
const PAYLOAD_METADATA_SIZE_OFFSET: usize = 0;
/// Payload offset of the u32 volume-groups-descriptor offset (region-relative).
const PAYLOAD_VG_DESCRIPTOR_OFFSET: usize = 156;
/// Within the volume-groups descriptor: encrypted-metadata block count (u64).
const VG_COUNT_OFFSET: usize = 8;
/// Within the descriptor: 48-bit primary encrypted-metadata block number (u64,
/// top 16 bits are the physical-volume index and are masked off).
const VG_PRIMARY_BLOCK_OFFSET: usize = 32;
/// Mask keeping the low 48 bits of a CoreStorage block-number field.
const BLOCK_NUMBER_MASK: u64 = 0x0000_ffff_ffff_ffff;
/// A sane upper bound on the plaintext metadata region (blocks).
const MAX_METADATA_BLOCKS: u64 = 1 << 20;
/// A sane upper bound on the encrypted-metadata region (blocks).
const MAX_ENCRYPTED_METADATA_BLOCKS: u64 = 1 << 20;

/// The located encrypted-metadata region, plus everything needed to read it.
#[derive(Debug, Clone)]
pub struct EncryptedMetadataLocation {
    /// Byte offset of the primary encrypted-metadata region in the image.
    pub primary_offset: u64,
    /// Number of blocks in the region.
    pub block_count: u64,
    /// Region length in bytes (`block_count * block_size`), whole 8192 units.
    pub length: u64,
}

/// Byte offset and length of the plaintext metadata region (block `mbn[0]`).
#[must_use]
pub fn plaintext_metadata_region(header: &VolumeHeader) -> (u64, u64) {
    let block = header.metadata_block_numbers.first().copied().unwrap_or(0);
    let offset = block.saturating_mul(u64::from(header.block_size));
    (offset, 0)
}

/// Read the region size (bytes) from the plaintext metadata region's first
/// block payload, capping against an allocation bomb.
///
/// # Errors
/// [`FileVaultError::MetadataStructureMissing`] if the first block is not a
/// `0x0011` block, [`FileVaultError::OutOfRange`] if the size is absurd.
pub fn plaintext_metadata_size(
    header: &VolumeHeader,
    first_block: &[u8],
) -> Result<u64, FileVaultError> {
    let block_type = le_u16(first_block, BLOCK_HEADER_TYPE_OFFSET);
    if block_type != BLOCK_TYPE_ENCRYPTED_METADATA_POINTER {
        return Err(FileVaultError::MetadataStructureMissing {
            what: "0x0011 encrypted-metadata pointer block",
        });
    }
    let size = u64::from(le_u32(
        first_block,
        BLOCK_HEADER_SIZE + PAYLOAD_METADATA_SIZE_OFFSET,
    ));
    let block_size = u64::from(header.block_size);
    if block_size == 0 {
        return Err(FileVaultError::OutOfRange {
            what: "block size is zero",
        });
    }
    if size == 0 || size / block_size > MAX_METADATA_BLOCKS {
        return Err(FileVaultError::OutOfRange {
            what: "plaintext metadata region size",
        });
    }
    Ok(size)
}

/// Locate the encrypted-metadata region from the full plaintext metadata region.
///
/// `region` is the whole plaintext metadata (`metadata_size` bytes) beginning at
/// the `0x0011` block. The volume-groups-descriptor offset is read from the
/// first block's payload (offset 156) and is *region-relative*.
///
/// # Errors
/// [`FileVaultError::MetadataStructureMissing`] / [`FileVaultError::OutOfRange`]
/// on a malformed or out-of-range descriptor.
pub fn locate_encrypted_metadata(
    header: &VolumeHeader,
    region: &[u8],
) -> Result<EncryptedMetadataLocation, FileVaultError> {
    let block_type = le_u16(region, BLOCK_HEADER_TYPE_OFFSET);
    if block_type != BLOCK_TYPE_ENCRYPTED_METADATA_POINTER {
        return Err(FileVaultError::MetadataStructureMissing {
            what: "0x0011 encrypted-metadata pointer block",
        });
    }

    let descriptor_offset =
        le_u32(region, BLOCK_HEADER_SIZE + PAYLOAD_VG_DESCRIPTOR_OFFSET) as usize;

    // The descriptor must sit within the region with room for its fields.
    if descriptor_offset
        .checked_add(VG_PRIMARY_BLOCK_OFFSET + 8)
        .is_none_or(|end| end > region.len())
    {
        return Err(FileVaultError::MetadataStructureMissing {
            what: "volume-groups descriptor (offset out of region)",
        });
    }

    let block_count = le_u64(region, descriptor_offset + VG_COUNT_OFFSET);
    let primary_block =
        le_u64(region, descriptor_offset + VG_PRIMARY_BLOCK_OFFSET) & BLOCK_NUMBER_MASK;

    if block_count == 0 || block_count > MAX_ENCRYPTED_METADATA_BLOCKS {
        return Err(FileVaultError::OutOfRange {
            what: "encrypted-metadata block count",
        });
    }

    let block_size = u64::from(header.block_size);
    let primary_offset =
        primary_block
            .checked_mul(block_size)
            .ok_or(FileVaultError::OutOfRange {
                what: "encrypted-metadata primary offset",
            })?;
    let length = block_count
        .checked_mul(block_size)
        .ok_or(FileVaultError::OutOfRange {
            what: "encrypted-metadata length",
        })?;

    Ok(EncryptedMetadataLocation {
        primary_offset,
        block_count,
        length,
    })
}

/// Decrypt an encrypted-metadata region in place over 8192-byte AES-XTS-128
/// units, tweak = 0-based unit index. Only whole units are decrypted; a trailing
/// partial (never present for a valid region) is left untouched.
pub fn decrypt_metadata(header: &VolumeHeader, ciphertext: &mut [u8]) {
    xts::decrypt_units(
        ciphertext,
        &header.key_data,
        &header.physical_volume_identifier,
        METADATA_UNIT_SIZE,
        0,
    );
}

/// Block type of the segment-descriptor block (logical→physical map).
const BLOCK_TYPE_SEGMENT_DESCRIPTOR: u16 = 0x0305;
/// Payload offset of the first `0x0305` entry.
const SEG_FIRST_ENTRY_PAYLOAD_OFFSET: usize = 8;
/// Stride between `0x0305` entries.
const SEG_ENTRY_STRIDE: usize = 40;
/// Within a `0x0305` entry: logical block number (u64).
const SEG_LOGICAL_BLOCK_OFFSET: usize = 8;
/// Within an entry: number of blocks (u32).
const SEG_NUM_BLOCKS_OFFSET: usize = 16;
/// Within an entry: 48-bit physical block number (u64; top 16 bits masked).
const SEG_PHYSICAL_BLOCK_OFFSET: usize = 32;
/// A sane cap on segment-descriptor entries (guards a lying count).
const MAX_SEGMENTS: u32 = 1 << 16;

/// One logical→physical segment mapping decoded from a `0x0305` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentEntry {
    /// Logical block number where this segment begins.
    pub logical_block: u64,
    /// First physical block of the segment.
    pub physical_block: u64,
    /// Number of blocks in the segment.
    pub number_of_blocks: u32,
}

/// Parse the segment-descriptor (`0x0305`) block from decrypted metadata,
/// returning the (logical→physical) segment entries.
///
/// Finds the first `0x0305` block in `metadata`, reads its entry count
/// (capped), and decodes each 40-byte entry. Returns an empty vector if no
/// segment-descriptor block is present (a per-artifact miss).
#[must_use]
pub fn parse_segments(metadata: &[u8], block_size: usize) -> Vec<SegmentEntry> {
    let Some(block_offset) =
        find_block_of_type(metadata, block_size, BLOCK_TYPE_SEGMENT_DESCRIPTOR)
    else {
        return Vec::new();
    };
    let payload = block_offset + BLOCK_HEADER_SIZE;
    let count = le_u32(metadata, payload).min(MAX_SEGMENTS);

    let mut out = Vec::new();
    for i in 0..count as usize {
        let entry = payload + SEG_FIRST_ENTRY_PAYLOAD_OFFSET + i * SEG_ENTRY_STRIDE;
        // Stop if the entry would run past this block's payload — a lying
        // `number_of_entries` must not read into the next block or out of bounds.
        if entry + SEG_ENTRY_STRIDE > block_offset + block_size {
            break;
        }
        let physical_block =
            le_u64(metadata, entry + SEG_PHYSICAL_BLOCK_OFFSET) & BLOCK_NUMBER_MASK;
        out.push(SegmentEntry {
            logical_block: le_u64(metadata, entry + SEG_LOGICAL_BLOCK_OFFSET),
            physical_block,
            number_of_blocks: le_u32(metadata, entry + SEG_NUM_BLOCKS_OFFSET),
        });
    }
    out
}

/// Locate a block of `block_type` in `metadata`, returning its byte offset.
#[must_use]
fn find_block_of_type(metadata: &[u8], block_size: usize, block_type: u16) -> Option<usize> {
    if block_size == 0 {
        return None; // cov:unreachable: block_size comes from a validated header (>=512)
    }
    let mut offset = 0;
    while offset + block_size <= metadata.len() {
        if le_u16(metadata, offset + BLOCK_HEADER_TYPE_OFFSET) == block_type {
            return Some(offset);
        }
        offset += block_size;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(block_size: u32, mbn0: u64) -> VolumeHeader {
        VolumeHeader {
            block_size,
            bytes_per_sector: 512,
            physical_volume_size: 0,
            metadata_block_numbers: [mbn0, 0, 0, 0],
            key_data: [0u8; 16],
            physical_volume_identifier: [0u8; 16],
            logical_volume_group_identifier: [0; 16],
        }
    }

    /// Build a plaintext metadata region: block 0 is a 0x0011 pointer whose
    /// payload carries metadata_size@0 and the descriptor offset@156; the
    /// descriptor (at a region offset) carries count@+8 and primary@+32.
    fn build_region(block_size: usize, count: u64, primary: u64) -> Vec<u8> {
        let mut region = vec![0u8; block_size * 4];
        // Block 0 header: type 0x0011 at offset 10.
        region[10..12].copy_from_slice(&BLOCK_TYPE_ENCRYPTED_METADATA_POINTER.to_le_bytes());
        // Payload starts at BLOCK_HEADER_SIZE (64).
        let payload = BLOCK_HEADER_SIZE;
        // metadata_size = whole region.
        region[payload..payload + 4].copy_from_slice(&((block_size as u32) * 4).to_le_bytes());
        // Descriptor offset (region-relative): put it at 2*block_size.
        let desc = 2 * block_size;
        region[payload + PAYLOAD_VG_DESCRIPTOR_OFFSET..payload + PAYLOAD_VG_DESCRIPTOR_OFFSET + 4]
            .copy_from_slice(&(desc as u32).to_le_bytes());
        region[desc + VG_COUNT_OFFSET..desc + VG_COUNT_OFFSET + 8]
            .copy_from_slice(&count.to_le_bytes());
        // primary with a physical-volume index in the top 16 bits to prove masking.
        let primary_field = primary | (0x1234u64 << 48);
        region[desc + VG_PRIMARY_BLOCK_OFFSET..desc + VG_PRIMARY_BLOCK_OFFSET + 8]
            .copy_from_slice(&primary_field.to_le_bytes());
        region
    }

    #[test]
    fn locates_encrypted_metadata_masking_volume_index() {
        let h = header(4096, 1);
        let region = build_region(4096, 6144, 2049);
        let loc = locate_encrypted_metadata(&h, &region).unwrap();
        assert_eq!(loc.block_count, 6144);
        assert_eq!(loc.primary_offset, 2049 * 4096);
        assert_eq!(loc.length, 6144 * 4096);
    }

    #[test]
    fn rejects_wrong_first_block_type() {
        let h = header(4096, 1);
        let mut region = build_region(4096, 6144, 2049);
        region[10..12].copy_from_slice(&0x0010u16.to_le_bytes());
        assert!(matches!(
            locate_encrypted_metadata(&h, &region),
            Err(FileVaultError::MetadataStructureMissing { .. })
        ));
    }

    #[test]
    fn rejects_absurd_block_count() {
        let h = header(4096, 1);
        let region = build_region(4096, u64::from(u32::MAX), 2049);
        assert!(matches!(
            locate_encrypted_metadata(&h, &region),
            Err(FileVaultError::OutOfRange { .. })
        ));
    }

    #[test]
    fn descriptor_offset_out_of_region_is_missing() {
        let h = header(4096, 1);
        let mut region = build_region(4096, 6144, 2049);
        let payload = BLOCK_HEADER_SIZE;
        // Point the descriptor beyond the region.
        let beyond = region.len() as u32 + 100;
        region[payload + PAYLOAD_VG_DESCRIPTOR_OFFSET..payload + PAYLOAD_VG_DESCRIPTOR_OFFSET + 4]
            .copy_from_slice(&beyond.to_le_bytes());
        assert!(matches!(
            locate_encrypted_metadata(&h, &region),
            Err(FileVaultError::MetadataStructureMissing { .. })
        ));
    }

    #[test]
    fn plaintext_size_reads_from_pointer_block() {
        let h = header(4096, 1);
        let region = build_region(4096, 6144, 2049);
        let size = plaintext_metadata_size(&h, &region[..4096]).unwrap();
        assert_eq!(size, 4096 * 4);
    }

    #[test]
    fn plaintext_size_rejects_non_pointer_block() {
        let h = header(4096, 1);
        let block = vec![0u8; 4096];
        assert!(matches!(
            plaintext_metadata_size(&h, &block),
            Err(FileVaultError::MetadataStructureMissing { .. })
        ));
    }

    #[test]
    fn plaintext_region_offset_is_block_scaled() {
        let h = header(4096, 3);
        assert_eq!(plaintext_metadata_region(&h), (3 * 4096, 0));
    }

    #[test]
    fn parses_single_segment() {
        let block_size = 4096usize;
        let mut meta = vec![0u8; block_size];
        meta[10..12].copy_from_slice(&BLOCK_TYPE_SEGMENT_DESCRIPTOR.to_le_bytes());
        let payload = BLOCK_HEADER_SIZE;
        meta[payload..payload + 4].copy_from_slice(&1u32.to_le_bytes()); // one entry
        let entry = payload + SEG_FIRST_ENTRY_PAYLOAD_OFFSET;
        meta[entry + SEG_LOGICAL_BLOCK_OFFSET..entry + SEG_LOGICAL_BLOCK_OFFSET + 8]
            .copy_from_slice(&0u64.to_le_bytes());
        meta[entry + SEG_NUM_BLOCKS_OFFSET..entry + SEG_NUM_BLOCKS_OFFSET + 4]
            .copy_from_slice(&40960u32.to_le_bytes());
        let phys = 16384u64 | (0x1234u64 << 48);
        meta[entry + SEG_PHYSICAL_BLOCK_OFFSET..entry + SEG_PHYSICAL_BLOCK_OFFSET + 8]
            .copy_from_slice(&phys.to_le_bytes());
        let segs = parse_segments(&meta, block_size);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].physical_block, 16384);
        assert_eq!(segs[0].number_of_blocks, 40960);
        assert_eq!(segs[0].logical_block, 0);
    }

    #[test]
    fn no_segment_block_yields_empty() {
        let meta = vec![0u8; 4096];
        assert!(parse_segments(&meta, 4096).is_empty());
    }

    #[test]
    fn decrypt_metadata_roundtrips() {
        let mut h = header(4096, 1);
        h.key_data = [0x11u8; 16];
        h.physical_volume_identifier = [0x22u8; 16];
        let plain: Vec<u8> = (0..8192u32).map(|i| (i & 0xff) as u8).collect();
        let mut buf = plain.clone();
        crate::xts::encrypt_units(
            &mut buf,
            &h.key_data,
            &h.physical_volume_identifier,
            8192,
            0,
        );
        decrypt_metadata(&h, &mut buf);
        assert_eq!(buf, plain);
    }

    #[test]
    fn plaintext_size_rejects_zero_block_size() {
        let h = header(0, 1);
        let region = build_region(4096, 6144, 2049);
        assert!(matches!(
            plaintext_metadata_size(&h, &region[..4096]),
            Err(FileVaultError::OutOfRange {
                what: "block size is zero"
            })
        ));
    }

    #[test]
    fn plaintext_size_rejects_absurd_size() {
        // block_size 512 with size u32::MAX -> size/512 > MAX_METADATA_BLOCKS.
        let h = header(512, 1);
        let mut block = vec![0u8; 4096];
        block[10..12].copy_from_slice(&BLOCK_TYPE_ENCRYPTED_METADATA_POINTER.to_le_bytes());
        block[BLOCK_HEADER_SIZE..BLOCK_HEADER_SIZE + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(matches!(
            plaintext_metadata_size(&h, &block),
            Err(FileVaultError::OutOfRange {
                what: "plaintext metadata region size"
            })
        ));
    }

    #[test]
    fn parse_segments_zero_block_size_is_empty() {
        // find_block_of_type is reached with block_size == 0 -> None -> empty.
        assert!(parse_segments(&[0u8; 4096], 0).is_empty());
    }

    #[test]
    fn parse_segments_stops_on_lying_count() {
        // A 0x0305 block claiming more entries than the block can hold must stop
        // at the block boundary, not read past it.
        let block_size = 4096usize;
        let mut meta = vec![0u8; block_size];
        meta[10..12].copy_from_slice(&BLOCK_TYPE_SEGMENT_DESCRIPTOR.to_le_bytes());
        let payload = BLOCK_HEADER_SIZE;
        // Claim 1000 entries; only (4096-64-8)/40 ~= 100 fit.
        meta[payload..payload + 4].copy_from_slice(&1000u32.to_le_bytes());
        let segs = parse_segments(&meta, block_size);
        // Never panics; yields at most what fits.
        assert!(segs.len() < 1000);
    }
}
