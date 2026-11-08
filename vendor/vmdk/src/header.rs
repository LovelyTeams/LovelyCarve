//! Sparse extent header (Virtual Disk Format 1.1, §4.1).

use crate::error::{Result, VmdkError};

pub const MAGIC: u32 = 0x564D_444B;
pub const VERSION: u32 = 1;
/// Version 2 enables the zeroed-grain feature (GTE == 1 → explicit zero grain).
pub const VERSION_ZEROED_GRAIN: u32 = 2;
pub const VERSION_STREAM_OPT: u32 = 3;
pub const SECTOR_SIZE: u64 = 512;

/// Maximum grain-table entries per grain table (VDF 1.1 §4.1: `numGTEsPerGT` = 512).
///
/// QEMU's `vmdk_open_vmdk4` rejects any larger value. The read path allocates
/// `num_gtes_per_gt * 4` bytes per grain table, so this bound caps that allocation
/// at 2 KiB and prevents a crafted header from forcing a huge allocation.
pub const MAX_NUM_GTES_PER_GT: u32 = 512;

/// Upper bound on `grainSize` (in sectors). Real VMDK grains are powers of two and
/// tiny — 128 sectors (64 KiB) is the VMware default and ESXi/VMFS tops out around
/// 2048 sectors (1 MiB). This cap (32 MiB) is orders of magnitude above any genuine
/// file, yet bounds the compressed-grain read allocation (`vec![0u8; data_size]`,
/// where `data_size <= grain_size_bytes + 64 KiB`) so a crafted header cannot force
/// a multi-gigabyte allocation. Mirrors the `MAX_NUM_GTES_PER_GT` allocation cap.
pub const MAX_GRAIN_SIZE_SECTORS: u64 = 0x10000;

/// Sentinel `gdOffset` in the *primary* header of a `streamOptimized` extent.
///
/// When `gdOffset == GD_AT_END` the real GD location is in the *footer* header
/// appended to the end of the file: `SparseExtentHeader` at `file_end − 1024`,
/// followed by an EOS marker at `file_end − 512` (VDF 1.1 §4.6).
pub const GD_AT_END: u64 = 0xffff_ffff_ffff_ffff;

/// Parsed fields from the 512-byte `SparseExtentHeader`.
pub struct SparseExtentHeader {
    pub version: u32,           // 1 = monolithicSparse, 3 = streamOptimized
    pub capacity: u64,          // virtual disk size in sectors
    pub grain_size: u64,        // grain size in sectors
    pub descriptor_offset: u64, // in sectors
    pub descriptor_size: u64,   // in sectors
    pub num_gtes_per_gt: u32,
    pub rgd_offset: u64, // redundant grain directory offset in sectors (0 if absent)
    pub gd_offset: u64,  // grain directory offset in sectors
    /// `true` when `compress_algorithm == 1` (stream-optimised / DEFLATE).
    pub compressed: bool,
}

impl SparseExtentHeader {
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 512 {
            return Err(VmdkError::FileTooSmall);
        }

        let magic = u32::from_le_bytes(data[0..4].try_into().expect("4 bytes"));
        if magic != MAGIC {
            return Err(VmdkError::BadMagic);
        }

        let version = u32::from_le_bytes(data[4..8].try_into().expect("4 bytes"));
        // Accept v1 (base), v2 (zeroed-grain feature) and v3 (streamOptimized).
        // QEMU accepts any VMDK4-magic version; we cap at the three defined values.
        if version != VERSION && version != VERSION_ZEROED_GRAIN && version != VERSION_STREAM_OPT {
            return Err(VmdkError::UnsupportedVersion(version));
        }

        let capacity = u64::from_le_bytes(data[12..20].try_into().expect("8 bytes"));
        let grain_size = u64::from_le_bytes(data[20..28].try_into().expect("8 bytes"));
        let descriptor_offset = u64::from_le_bytes(data[28..36].try_into().expect("8 bytes"));
        let descriptor_size = u64::from_le_bytes(data[36..44].try_into().expect("8 bytes"));
        let num_gtes_per_gt = u32::from_le_bytes(data[44..48].try_into().expect("4 bytes"));
        let rgd_offset = u64::from_le_bytes(data[48..56].try_into().expect("8 bytes"));
        let gd_offset = u64::from_le_bytes(data[56..64].try_into().expect("8 bytes"));
        let compress_algorithm = u16::from_le_bytes(data[77..79].try_into().expect("2 bytes"));

        // v1: compression must be absent; v3 (streamOptimized): deflate (1) is expected.
        // Spec note (VDF 1.1 §4.4): COMPRESSION_DEFLATE is described as RFC 1951 (raw
        // DEFLATE), but both VMware tooling and QEMU actually produce RFC 1950 payloads
        // (2-byte zlib header + DEFLATE stream + Adler-32 trailer).  Use ZlibDecoder,
        // not DeflateDecoder — the spec has a documentation error.
        match (version, compress_algorithm) {
            (VERSION | VERSION_ZEROED_GRAIN, 0) | (VERSION_STREAM_OPT, 1) => {}
            _ => return Err(VmdkError::CompressedNotSupported),
        }

        // Validate geometry before these values feed division arithmetic in the reader.
        // VDF 1.1 §4.1: minimum grain size is 8 sectors (4 KiB).
        if grain_size < 8 {
            return Err(VmdkError::FieldOutOfRange {
                field: "grain_size",
                value: grain_size,
                reason: "must be >= 8 sectors (VDF 1.1 §4.1)",
            });
        }
        // Upper bound: caps the compressed-grain read allocation so a crafted header
        // cannot drive a multi-gigabyte allocation (fuzz_recover oom-2763835523).
        if grain_size > MAX_GRAIN_SIZE_SECTORS {
            return Err(VmdkError::FieldOutOfRange {
                field: "grain_size",
                value: grain_size,
                reason: "exceeds the maximum supported grain size (32 MiB)",
            });
        }
        if num_gtes_per_gt == 0 {
            return Err(VmdkError::FieldOutOfRange {
                field: "num_gtes_per_gt",
                value: u64::from(num_gtes_per_gt),
                reason: "must be > 0",
            });
        }
        // VDF 1.1 defines numGTEsPerGT as 512; QEMU's vmdk_open_vmdk4 rejects any
        // larger value. Enforcing it here bounds the read path's grain-table
        // allocation (`vec![0u8; num_gtes_per_gt * 4]`) at parse time, so no caller
        // can be driven into a multi-gigabyte allocation by a crafted header.
        if num_gtes_per_gt > MAX_NUM_GTES_PER_GT {
            return Err(VmdkError::FieldOutOfRange {
                field: "num_gtes_per_gt",
                value: u64::from(num_gtes_per_gt),
                reason: "exceeds the spec maximum of 512",
            });
        }

        Ok(SparseExtentHeader {
            version,
            capacity,
            grain_size,
            descriptor_offset,
            descriptor_size,
            num_gtes_per_gt,
            rgd_offset,
            gd_offset,
            compressed: compress_algorithm != 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_header() -> Vec<u8> {
        let mut h = vec![0u8; 512];
        h[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        h[4..8].copy_from_slice(&VERSION.to_le_bytes());
        h[12..20].copy_from_slice(&8u64.to_le_bytes()); // capacity
        h[20..28].copy_from_slice(&8u64.to_le_bytes()); // grain_size
        h[44..48].copy_from_slice(&512u32.to_le_bytes()); // num_gtes_per_gt
        h
    }

    #[test]
    fn parse_rejects_short_buffer() {
        assert!(matches!(
            SparseExtentHeader::parse(&[0u8; 100]),
            Err(VmdkError::FileTooSmall)
        ));
    }

    #[test]
    fn parse_rejects_bad_magic() {
        let h = vec![0u8; 512];
        assert!(matches!(
            SparseExtentHeader::parse(&h),
            Err(VmdkError::BadMagic)
        ));
    }

    #[test]
    fn parse_rejects_unsupported_version() {
        let mut h = valid_header();
        h[4..8].copy_from_slice(&4u32.to_le_bytes()); // version 4 is undefined
        assert!(matches!(
            SparseExtentHeader::parse(&h),
            Err(VmdkError::UnsupportedVersion(4))
        ));
    }

    #[test]
    fn parse_accepts_version_2() {
        let mut h = valid_header();
        h[4..8].copy_from_slice(&VERSION_ZEROED_GRAIN.to_le_bytes());
        let hdr = SparseExtentHeader::parse(&h).expect("v2 parses");
        assert_eq!(hdr.version, 2);
    }

    #[test]
    fn parse_rejects_grain_size_below_minimum() {
        let mut h = valid_header();
        h[20..28].copy_from_slice(&4u64.to_le_bytes()); // < 8
        assert!(matches!(
            SparseExtentHeader::parse(&h),
            Err(VmdkError::FieldOutOfRange {
                field: "grain_size",
                value: 4,
                ..
            })
        ));
    }

    #[test]
    fn parse_rejects_grain_size_above_maximum() {
        // A huge grainSize would size the compressed-grain read allocation into the
        // gigabytes (fuzz_recover oom-2763835523); it must be rejected at parse time.
        let mut h = valid_header();
        let oversized = MAX_GRAIN_SIZE_SECTORS + 1;
        h[20..28].copy_from_slice(&oversized.to_le_bytes());
        assert!(matches!(
            SparseExtentHeader::parse(&h),
            Err(VmdkError::FieldOutOfRange {
                field: "grain_size",
                ..
            })
        ));
    }

    #[test]
    fn parse_rejects_zero_num_gtes() {
        let mut h = valid_header();
        h[44..48].copy_from_slice(&0u32.to_le_bytes());
        assert!(matches!(
            SparseExtentHeader::parse(&h),
            Err(VmdkError::FieldOutOfRange {
                field: "num_gtes_per_gt",
                value: 0,
                ..
            })
        ));
    }

    #[test]
    fn parse_rejects_num_gtes_above_spec_max() {
        // VDF 1.1 defines numGTEsPerGT as 512; QEMU rejects anything larger.
        // Without this bound a crafted header drives an unguarded
        // `vec![0u8; num_gtes_per_gt * 4]` in the read path — e.g. 0xFFFFFFFF
        // yields a ~17 GiB allocation (allocation-amplification DoS).
        let mut h = valid_header();
        h[44..48].copy_from_slice(&513u32.to_le_bytes());
        assert!(matches!(
            SparseExtentHeader::parse(&h),
            Err(VmdkError::FieldOutOfRange {
                field: "num_gtes_per_gt",
                value: 513,
                ..
            })
        ));

        // The extreme crafted value must also be rejected, not allocated.
        let mut h = valid_header();
        h[44..48].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert!(matches!(
            SparseExtentHeader::parse(&h),
            Err(VmdkError::FieldOutOfRange {
                field: "num_gtes_per_gt",
                value: 0xFFFF_FFFF,
                ..
            })
        ));
    }

    #[test]
    fn parse_accepts_num_gtes_at_spec_max() {
        // Exactly 512 is the canonical value and must remain valid.
        let mut h = valid_header();
        h[44..48].copy_from_slice(&512u32.to_le_bytes());
        assert!(SparseExtentHeader::parse(&h).is_ok());
    }

    #[test]
    fn parse_rejects_compressed_flag_on_v1() {
        let mut h = valid_header();
        h[77..79].copy_from_slice(&1u16.to_le_bytes()); // compress on v1 is invalid
        assert!(matches!(
            SparseExtentHeader::parse(&h),
            Err(VmdkError::CompressedNotSupported)
        ));
    }
}
