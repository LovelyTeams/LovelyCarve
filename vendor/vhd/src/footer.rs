//! VHD footer parsing (MS-VHD §2.1).
//!
//! The footer is a 512-byte structure at the end of every VHD file.
//! Fixed disks also have a copy at byte 0 of the file.
//! Dynamic disks have a copy at byte 0 and the real footer at the very end.

use crate::error::{Result, VhdError};
use crate::read::{be_u32, be_u64};

pub const FOOTER_SIZE: usize = 512;
pub const COOKIE: &[u8; 8] = b"conectix";
pub const CURRENT_VERSION: u32 = 0x0001_0000;

/// VHD disk type codes (§2.1, DiskType field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskType {
    Fixed = 2,
    Dynamic = 3,
    // Differencing = 4 — rejected at open time
}

/// Parsed VHD footer fields relevant to container reading.
#[derive(Debug, Clone)]
pub struct VhdFooter {
    pub disk_type: DiskType,
    pub current_size: u64, // CurrentSize (offset 48) — current/readable virtual size
    pub original_size: u64, // OriginalSize (offset 40) — size at creation
    pub data_offset: u64,  // offset to dynamic header (0xFFFF... for fixed)
}

impl VhdFooter {
    /// Parse the last 512 bytes of `data` as a VHD footer.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < FOOTER_SIZE {
            return Err(VhdError::FileTooSmall);
        }
        let footer = &data[data.len() - FOOTER_SIZE..];

        // Cookie: bytes 0–7
        if &footer[0..8] != COOKIE {
            return Err(VhdError::BadCookie);
        }

        // Version: bytes 12–15
        let version = be_u32(footer, 12);
        if version != CURRENT_VERSION {
            return Err(VhdError::UnsupportedVersion(version));
        }

        // DataOffset: bytes 16–23
        let data_offset = be_u64(footer, 16);

        // CurrentSize (virtual disk size): bytes 48–55 (MS-VHD §2.1).
        // OriginalSize is bytes 40–47; the two differ on a resized disk.
        let current_size = be_u64(footer, 48);
        // OriginalSize (offset 40) — creation size; libvhdi reports this as media size.
        let original_size = be_u64(footer, 40);

        // DiskType: bytes 60–63
        let disk_type_raw = be_u32(footer, 60);
        let disk_type = match disk_type_raw {
            2 => DiskType::Fixed,
            3 => DiskType::Dynamic,
            4 => return Err(VhdError::DifferencingNotSupported),
            other => return Err(VhdError::UnknownDiskType(other)),
        };

        // Checksum: bytes 64–67
        let stored_checksum = be_u32(footer, 64);
        let computed = checksum(footer);
        if stored_checksum != computed {
            return Err(VhdError::ChecksumMismatch {
                expected: stored_checksum,
                actual: computed,
            });
        }

        Ok(VhdFooter {
            disk_type,
            current_size,
            original_size,
            data_offset,
        })
    }
}

/// One's-complement checksum over the footer with the checksum field zeroed.
fn checksum(footer: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    for (i, &byte) in footer.iter().enumerate() {
        // Skip the checksum field itself (bytes 64–67)
        if (64..68).contains(&i) {
            continue;
        }
        sum = sum.wrapping_add(u32::from(byte));
    }
    !sum
}

// ── Test helpers ─────────────────────────────────────────────────────────────

/// Build a minimal valid Fixed VHD footer for testing.
///
/// Sets the cookie, version, current_size, disk_type=Fixed, data_offset=0xFFFF,
/// and a valid checksum. All other fields are zeroed.
#[cfg(any(test, feature = "test-helpers"))]
pub fn test_fixed_footer(virtual_size: u64) -> Vec<u8> {
    let mut footer = vec![0u8; FOOTER_SIZE];

    // Cookie
    footer[0..8].copy_from_slice(COOKIE);
    // Features: reserved (0x0000_0002)
    footer[8..12].copy_from_slice(&0x0000_0002u32.to_be_bytes());
    // FileFormatVersion
    footer[12..16].copy_from_slice(&CURRENT_VERSION.to_be_bytes());
    // DataOffset: 0xFFFF_FFFF_FFFF_FFFF for Fixed
    footer[16..24].copy_from_slice(&0xFFFF_FFFF_FFFF_FFFFu64.to_be_bytes());
    // OriginalSize (offset 40)
    footer[40..48].copy_from_slice(&virtual_size.to_be_bytes());
    // CurrentSize (offset 48)
    footer[48..56].copy_from_slice(&virtual_size.to_be_bytes());
    // DiskType: Fixed = 2
    footer[60..64].copy_from_slice(&2u32.to_be_bytes());
    // Checksum (computed with checksum field zeroed)
    let cs = checksum(&footer);
    footer[64..68].copy_from_slice(&cs.to_be_bytes());

    footer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Vec<u8> {
        test_fixed_footer(1024)
    }

    #[test]
    fn too_small_is_file_too_small() {
        assert!(matches!(
            VhdFooter::parse(&[0u8; 100]),
            Err(VhdError::FileTooSmall)
        ));
    }

    #[test]
    fn bad_cookie_rejected() {
        let mut f = base();
        f[0] = b'X';
        assert!(matches!(VhdFooter::parse(&f), Err(VhdError::BadCookie)));
    }

    #[test]
    fn unsupported_version_rejected() {
        let mut f = base();
        f[12..16].copy_from_slice(&0x0002_0000u32.to_be_bytes());
        assert!(matches!(
            VhdFooter::parse(&f),
            Err(VhdError::UnsupportedVersion(0x0002_0000))
        ));
    }

    #[test]
    fn differencing_rejected() {
        let mut f = base();
        f[60..64].copy_from_slice(&4u32.to_be_bytes());
        assert!(matches!(
            VhdFooter::parse(&f),
            Err(VhdError::DifferencingNotSupported)
        ));
    }

    #[test]
    fn unknown_disk_type_rejected() {
        let mut f = base();
        f[60..64].copy_from_slice(&99u32.to_be_bytes());
        assert!(matches!(
            VhdFooter::parse(&f),
            Err(VhdError::UnknownDiskType(99))
        ));
    }

    #[test]
    fn checksum_mismatch_rejected() {
        let mut f = base();
        f[100] ^= 0xFF; // reserved byte — cookie/version/type still valid
        assert!(matches!(
            VhdFooter::parse(&f),
            Err(VhdError::ChecksumMismatch { .. })
        ));
    }
}
