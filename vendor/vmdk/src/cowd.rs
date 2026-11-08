//! COWD (Copy-On-Write Disk) sparse extent reader — used by `VMware` `ESXi` vmfsSparse/vmfsThin.
//!
//! Magic: `"COWD"` = `0x43_4F_57_44` (big-endian) at byte 0.
//! All header fields are little-endian `u32` (vs `u64` in VMDK4).
//! The grain directory is always at sector 4 (byte offset 2048).
//! Fixed 4096 grain table entries per grain table.
//!
//! Reference: QEMU `vmdk.c` `vmdk_open_vmfs_sparse()`;
//! libvmdk `cowd_sparse_file_header.h`.

use std::io::{Read, Seek, SeekFrom};

use crate::error::VmdkError;

/// COWD magic bytes (big-endian field at offset 0): `"COWD"`.
pub(crate) const COWD_MAGIC: u32 = 0x434F_5744; // 'C' 'O' 'W' 'D'

/// Grain directory always starts at sector 4 in all COWD files.
const COWD_GD_SECTOR: u32 = 4;

/// Fixed number of GTEs per grain table in COWD format.
pub(crate) const COWD_GTES_PER_GT: usize = 4096;

/// Sector size (shared with VMDK4).
const SECTOR_SIZE: u64 = 512;

/// Parsed COWD sparse extent header.
///
/// The raw header is 1060 bytes (root file) but we only read the fields needed
/// for grain-table navigation. `gd_entries` (offset 24) and `next_free`
/// (offset 28) are present in the wire format but unused for read-only access.
pub(crate) struct CowdHeader {
    pub capacity: u32,   // virtual disk size in sectors (32-bit limit)
    pub grain_size: u32, // grain size in sectors
}

impl CowdHeader {
    /// Parse the first 512 bytes of a COWD extent file.
    ///
    /// Returns `Err(BadMagic)` if the magic does not match `"COWD"`.
    pub fn parse(data: &[u8]) -> Result<Self, VmdkError> {
        if data.len() < 32 {
            return Err(VmdkError::FileTooSmall);
        }
        // Magic is stored big-endian at offset 0 per the COWD spec.
        let magic = u32::from_be_bytes(data[0..4].try_into().expect("4 bytes"));
        if magic != COWD_MAGIC {
            return Err(VmdkError::BadMagic);
        }

        let version = u32::from_le_bytes(data[4..8].try_into().expect("4 bytes"));
        if version != 1 {
            return Err(VmdkError::UnsupportedVersion(version));
        }

        let capacity = u32::from_le_bytes(data[12..16].try_into().expect("4 bytes"));
        let grain_size = u32::from_le_bytes(data[16..20].try_into().expect("4 bytes"));
        if grain_size == 0 {
            return Err(VmdkError::FieldOutOfRange {
                field: "grain_size",
                value: u64::from(grain_size),
                reason: "must be > 0",
            });
        }

        Ok(CowdHeader {
            capacity,
            grain_size,
        })
    }
}

/// Open a COWD sparse extent, loading the grain directory into memory.
///
/// Returns `(grain_dir, grain_size_bytes)` where `grain_dir[i]` is the sector
/// offset of the grain table for the i-th group of `COWD_GTES_PER_GT` grains.
pub(crate) fn open_cowd<R: Read + Seek>(mut reader: R) -> Result<(Vec<u32>, u64), VmdkError> {
    let mut hdr_bytes = [0u8; 512];
    reader.read_exact(&mut hdr_bytes)?;
    let hdr = CowdHeader::parse(&hdr_bytes)?;

    let grain_size_bytes =
        u64::from(hdr.grain_size)
            .checked_mul(SECTOR_SIZE)
            .ok_or(VmdkError::GeometryOverflow {
                field: "grain_size",
            })?;

    let num_grains = u64::from(hdr.capacity).div_ceil(u64::from(hdr.grain_size));
    let num_gts = num_grains.div_ceil(COWD_GTES_PER_GT as u64);

    // `capacity` is a u32 and `grain_size >= 1`, so num_gts <= u32::MAX / 4096 and the
    // grain directory is at most ~4 MiB — no explicit cap is needed (it cannot exceed
    // the structural u32 bound), and the multiply cannot overflow u64.
    let gd_bytes = (num_gts * 4) as usize;

    let gd_offset = u64::from(COWD_GD_SECTOR) * SECTOR_SIZE;
    reader.seek(SeekFrom::Start(gd_offset))?;
    let mut buf = vec![0u8; gd_bytes];
    reader.read_exact(&mut buf)?;

    let grain_dir = buf
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().expect("4")))
        .collect();

    Ok((grain_dir, grain_size_bytes))
}

// COWD grain lookups reuse the generic `grain_location` path in lib.rs once the
// grain directory is loaded (COWD GTEs are 4-byte sector offsets, identical in
// layout to VMDK4), so no COWD-specific GTE lookup helper is needed here.

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn make_cowd_header(capacity: u32, grain_size: u32, gd_entries: u32) -> Vec<u8> {
        let mut h = vec![0u8; 512];
        // Magic big-endian "COWD"
        h[0..4].copy_from_slice(&COWD_MAGIC.to_be_bytes());
        // Version 1 (little-endian)
        h[4..8].copy_from_slice(&1u32.to_le_bytes());
        // Flags at offset 8 (ignored)
        h[12..16].copy_from_slice(&capacity.to_le_bytes());
        h[16..20].copy_from_slice(&grain_size.to_le_bytes());
        // GD sector at offset 20 (always 4 in real files, stored in header)
        h[20..24].copy_from_slice(&4u32.to_le_bytes());
        h[24..28].copy_from_slice(&gd_entries.to_le_bytes());
        // next_free at 28
        h[28..32].copy_from_slice(&100u32.to_le_bytes());
        h
    }

    #[test]
    fn cowd_header_parse_ok() {
        let h = make_cowd_header(1024, 8, 1);
        let hdr = CowdHeader::parse(&h).expect("parse");
        assert_eq!(hdr.capacity, 1024);
        assert_eq!(hdr.grain_size, 8);
    }

    #[test]
    fn cowd_header_bad_magic_rejected() {
        let h = vec![0u8; 512];
        assert!(matches!(CowdHeader::parse(&h), Err(VmdkError::BadMagic)));
    }

    #[test]
    fn cowd_header_short_buffer_rejected() {
        assert!(matches!(
            CowdHeader::parse(&[0u8; 16]),
            Err(VmdkError::FileTooSmall)
        ));
    }

    #[test]
    fn cowd_header_wrong_version_rejected() {
        let mut h = make_cowd_header(1024, 8, 1);
        h[4..8].copy_from_slice(&2u32.to_le_bytes()); // version=2
        assert!(matches!(
            CowdHeader::parse(&h),
            Err(VmdkError::UnsupportedVersion(2))
        ));
    }

    #[test]
    fn cowd_header_zero_grain_rejected() {
        let h = make_cowd_header(1024, 0, 1);
        assert!(matches!(
            CowdHeader::parse(&h),
            Err(VmdkError::FieldOutOfRange {
                field: "grain_size",
                ..
            })
        ));
    }

    #[test]
    fn open_cowd_all_sparse_returns_empty_gd() {
        // Build a minimal COWD: header + 3 empty sectors + GD (at sector 4) with one zero entry.
        let capacity = 8u32; // 8 sectors = 1 grain
        let grain_size = 8u32;
        let h = make_cowd_header(capacity, grain_size, 1);
        let mut bytes = h;
        // sectors 1–3 padding
        bytes.extend_from_slice(&vec![0u8; 512 * 3]);
        // sector 4: GD — one entry pointing to GT at sector 5
        let mut gd = vec![0u8; 512];
        gd[0..4].copy_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(&gd);
        // sector 5: GT (4096 entries × 4 bytes, all zero = sparse)
        bytes.extend_from_slice(&vec![0u8; COWD_GTES_PER_GT * 4]);

        let (grain_dir, gsz) = open_cowd(Cursor::new(bytes)).expect("open_cowd");
        assert_eq!(grain_dir.len(), 1);
        assert_eq!(grain_dir[0], 5); // GT is at sector 5
        assert_eq!(gsz, 8 * 512);
    }
}
