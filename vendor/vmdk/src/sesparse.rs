//! seSparse (Space-Efficient Sparse) extent reader — vSphere 6.5+ VMFS6 snapshots.
//!
//! Detected by `SESPARSE` extent type in a text descriptor (not by file magic).
//! Two fixed 512-byte headers:
//!   - Constant header: magic `0x00000000CAFEBABE`
//!   - Volatile header: magic `0x00000000CAFECAFE`
//!
//! All fields are `u64` little-endian. GTEs are 8 bytes each.
//! Grain size MUST be 8 sectors (4 KiB). Grain table size MUST be 64 sectors
//! (= 4096 entries × 8 bytes ÷ 512 = 64 sectors per GT).
//!
//! Reference: QEMU `vmdk.c` `vmdk_open_se_sparse()`;
//! strict version check: `version == 0x0000_0002_0000_0001`.

use std::io::{Read, Seek, SeekFrom};

use crate::error::VmdkError;

/// Constant-header magic (`0x0000_0000_CAFE_BABE`, little-endian).
pub const SE_CONST_MAGIC: u64 = 0x0000_0000_CAFE_BABE;

/// Required version field in the constant header.
pub const SE_VERSION: u64 = 0x0000_0002_0000_0001;

/// Grain size in sectors — MUST be exactly 8 for seSparse.
pub const SE_GRAIN_SECTORS: u64 = 8;

/// Grain table size in sectors — MUST be exactly 64 (4096 entries × 8 B ÷ 512).
pub const SE_GT_SECTORS: u64 = 64;

/// Number of GTEs per grain table: 64 sectors × 512 bytes ÷ 8 bytes-per-GTE.
pub const SE_GTES_PER_GT: u64 = 4096;

const SECTOR_SIZE: u64 = 512;

// ── Grain-entry encoding (see sesparse-encoding memory; QEMU block/vmdk.c) ────
/// L1 (GD) allocated marker: high 32 bits of an allocated GD entry.
pub const SE_GD_ALLOC_MASK: u64 = 0xffff_ffff_0000_0000;
pub const SE_GD_ALLOC_FLAG: u64 = 0x1000_0000_0000_0000;
/// L1 low 32 bits hold the grain-table index.
pub const SE_GD_INDEX_MASK: u64 = 0x0000_0000_ffff_ffff;
/// L2 (GTE) top-nibble type field.
pub const SE_GTE_TYPE_MASK: u64 = 0xf000_0000_0000_0000;
pub const SE_GTE_TYPE_ALLOCATED: u64 = 0x3000_0000_0000_0000;
pub const SE_GTE_TYPE_UNMAPPED: u64 = 0x1000_0000_0000_0000; // read as zero
pub const SE_GTE_TYPE_ZERO: u64 = 0x2000_0000_0000_0000; // read as zero

/// Decode an allocated L2 (GTE) entry into a grain index (bit-rotated layout).
pub fn se_gte_grain_index(gte: u64) -> u64 {
    ((gte & 0x0fff_0000_0000_0000) >> 48) | ((gte & 0x0000_ffff_ffff_ffff) << 12)
}

/// Parsed seSparse constant header (first 512 bytes of the extent file).
pub struct SeConstHeader {
    pub capacity: u64,      // virtual disk size in sectors
    pub grain_size: u64,    // must be 8
    pub gd_offset: u64,     // grain directory sector offset
    pub gt_offset: u64,     // start of grain tables (sectors)
    pub grains_offset: u64, // start of grain data (sectors)
}

impl SeConstHeader {
    /// Parse the first 512 bytes of a seSparse extent file.
    pub fn parse(data: &[u8]) -> Result<Self, VmdkError> {
        if data.len() < 208 {
            return Err(VmdkError::FileTooSmall);
        }
        let magic = u64::from_le_bytes(data[0..8].try_into().expect("8 bytes"));
        if magic != SE_CONST_MAGIC {
            return Err(VmdkError::BadMagic);
        }
        let version = u64::from_le_bytes(data[8..16].try_into().expect("8 bytes"));
        if version != SE_VERSION {
            return Err(VmdkError::UnsupportedVersion(version as u32));
        }
        let capacity = u64::from_le_bytes(data[16..24].try_into().expect("8 bytes"));
        let grain_size = u64::from_le_bytes(data[24..32].try_into().expect("8 bytes"));
        if grain_size != SE_GRAIN_SECTORS {
            return Err(VmdkError::FieldOutOfRange {
                field: "grain_size",
                value: grain_size,
                reason: "must equal the seSparse fixed grain size (8 sectors)",
            });
        }
        let grain_table_size = u64::from_le_bytes(data[32..40].try_into().expect("8 bytes"));
        if grain_table_size != SE_GT_SECTORS {
            return Err(VmdkError::FieldOutOfRange {
                field: "grain_table_size",
                value: grain_table_size,
                reason: "must equal the seSparse fixed grain-table size",
            });
        }
        // Grain directory offset @128; grain tables @144; grain data @192 (all sectors).
        let gd_offset = u64::from_le_bytes(data[128..136].try_into().expect("8 bytes"));
        let gt_offset = u64::from_le_bytes(data[144..152].try_into().expect("8 bytes"));
        let grains_offset = u64::from_le_bytes(data[192..200].try_into().expect("8 bytes"));

        Ok(SeConstHeader {
            capacity,
            grain_size,
            gd_offset,
            gt_offset,
            grains_offset,
        })
    }
}

/// Open a seSparse extent file, loading the grain directory into memory.
///
/// Returns `(grain_dir, grain_size_bytes, grains_offset_sectors)`.
/// `grain_dir[i]` holds the raw L1 entry (nibble-encoded) for that GD slot.
pub(crate) fn open_sesparse<R: Read + Seek>(
    mut reader: R,
) -> Result<(Vec<u64>, u64, u64), VmdkError> {
    let mut hdr_bytes = [0u8; 512];
    reader.read_exact(&mut hdr_bytes)?;
    let hdr = SeConstHeader::parse(&hdr_bytes)?;

    let grain_size_bytes = hdr.grain_size * SECTOR_SIZE;

    // The number of GD entries = ceil(num_grains / GTES_PER_GT).
    let num_grains = hdr.capacity.div_ceil(hdr.grain_size);
    let num_gts = num_grains.div_ceil(SE_GTES_PER_GT);
    // At least one GD entry even for a sub-grain-table-sized disk.
    let num_gts = num_gts.max(1);

    let gd_bytes = num_gts * 8; // 8 bytes per GD entry (u64)
    const MAX_SESP_GD: u64 = 16 * 1024 * 1024;
    if gd_bytes > MAX_SESP_GD {
        return Err(VmdkError::FieldOutOfRange {
            field: "grain_directory",
            value: gd_bytes,
            reason: "exceeds the 16 MiB cap",
        });
    }

    let gd_offset_bytes = hdr.gd_offset * SECTOR_SIZE;
    reader.seek(SeekFrom::Start(gd_offset_bytes))?;
    let mut buf = vec![0u8; gd_bytes as usize];
    reader.read_exact(&mut buf)?;

    let grain_dir = buf
        .chunks_exact(8)
        .map(|c| u64::from_le_bytes(c.try_into().expect("8 bytes")))
        .collect();

    Ok((grain_dir, grain_size_bytes, hdr.grains_offset))
}

// seSparse GTE lookups are handled inline in lib.rs `grain_location` /
// `se_read_gte`: the GD entry's allocated nibble (0x1) is checked, the GT table
// index is its low 32 bits (GT sector = gt_offset + idx*SE_GT_SECTORS), and the
// allocated (0x3) GTE's grain index is bit-rotated via `se_gte_grain_index`.

#[cfg(test)]
mod tests {
    use super::*;

    fn make_sesparse_header(capacity: u64) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[0..8].copy_from_slice(&SE_CONST_MAGIC.to_le_bytes());
        h[8..16].copy_from_slice(&SE_VERSION.to_le_bytes());
        h[16..24].copy_from_slice(&capacity.to_le_bytes());
        h[24..32].copy_from_slice(&SE_GRAIN_SECTORS.to_le_bytes()); // grain_size = 8
        h[32..40].copy_from_slice(&SE_GT_SECTORS.to_le_bytes()); // grain_table_size = 64
                                                                 // volatile header offset (80): just put 2
        h[80..88].copy_from_slice(&2u64.to_le_bytes());
        // gd_offset at 128
        h[128..136].copy_from_slice(&10u64.to_le_bytes()); // GD at sector 10
                                                           // gd_size at 136
        h[136..144].copy_from_slice(&1u64.to_le_bytes());
        // gt_offset at 144
        h[144..152].copy_from_slice(&11u64.to_le_bytes());
        // grains_offset at 192
        h[192..200].copy_from_slice(&75u64.to_le_bytes());
        h
    }

    #[test]
    fn sesparse_header_parse_ok() {
        let h = make_sesparse_header(4096);
        let hdr = SeConstHeader::parse(&h).expect("parse");
        assert_eq!(hdr.capacity, 4096);
        assert_eq!(hdr.grain_size, 8);
        assert_eq!(hdr.gd_offset, 10);
        assert_eq!(hdr.gt_offset, 11);
    }

    #[test]
    fn sesparse_wrong_magic_rejected() {
        let h = vec![0u8; 512];
        assert!(matches!(SeConstHeader::parse(&h), Err(VmdkError::BadMagic)));
    }

    #[test]
    fn sesparse_wrong_version_rejected() {
        let mut h = make_sesparse_header(8);
        h[8..16].copy_from_slice(&0u64.to_le_bytes()); // wrong version
        assert!(matches!(
            SeConstHeader::parse(&h),
            Err(VmdkError::UnsupportedVersion(_))
        ));
    }

    #[test]
    fn sesparse_wrong_grain_size_rejected() {
        let mut h = make_sesparse_header(8);
        h[24..32].copy_from_slice(&16u64.to_le_bytes()); // grain_size=16, not 8
        assert!(matches!(
            SeConstHeader::parse(&h),
            Err(VmdkError::FieldOutOfRange {
                field: "grain_size",
                ..
            })
        ));
    }

    #[test]
    fn sesparse_short_buffer_rejected() {
        assert!(matches!(
            SeConstHeader::parse(&[0u8; 100]),
            Err(VmdkError::FileTooSmall)
        ));
    }

    #[test]
    fn sesparse_wrong_grain_table_size_rejected() {
        let mut h = make_sesparse_header(8);
        h[32..40].copy_from_slice(&128u64.to_le_bytes()); // grain_table_size=128, not 64
        assert!(matches!(
            SeConstHeader::parse(&h),
            Err(VmdkError::FieldOutOfRange {
                field: "grain_table_size",
                ..
            })
        ));
    }

    #[test]
    fn sesparse_grain_directory_too_large_rejected() {
        // A capacity large enough that the GD would exceed the 16 MiB cap.
        let h = make_sesparse_header(100_000_000_000);
        assert!(matches!(
            open_sesparse(std::io::Cursor::new(h)),
            Err(VmdkError::FieldOutOfRange {
                field: "grain_directory",
                ..
            })
        ));
    }
}
