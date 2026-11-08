//! Pure-Rust reader for `monolithicSparse` VMDK extents (the format LDPlayer's
//! `.ldbk` virtual disks use).
//!
//! Resolves a media offset to file bytes through the sparse grain directory /
//! grain tables over a [`Backing`] that
//! can be either an mmap slice (the VMDK still embedded inside the `.ldbk` 7z,
//! read zero-copy) or an owned buffer (a decompressed member held in RAM). That
//! lets `core::evidence::readers::ldplayer_image` skip filesystem extraction
//! it required: a stored backup never copies a byte, and a compressed one
//! decodes straight to memory.
//!
//! Only plain (uncompressed-grain, header grain-directory) `monolithicSparse`
//! extents are handled — LDPlayer never writes anything else. Compressed-grain
//! / `streamOptimized` (footer grain-directory) extents are rejected with a
//! clear error rather than mis-read.

use std::{io::Cursor, sync::Arc};

use anyhow::{bail, Context};
use binrw::BinRead;
use memmap2::Mmap;

use crate::evidence::partitions::SectorSource;

/// `SparseExtentHeader.flags` bit 16 — grains are compressed. We can only serve
/// raw grains, so a set bit means "not ours".
const FLAG_COMPRESSED_GRAINS: u32 = 1 << 16;
/// Sentinel `gdOffset` meaning the grain directory lives in the footer
/// (`streamOptimized`), which we do not support.
const GD_AT_FOOTER: u64 = u64::MAX;
const SECTOR: u64 = 512;

#[derive(Debug, BinRead)]
#[br(little, magic = b"KDMV")]
struct SparseExtentHeader {
    #[br(seek_before = std::io::SeekFrom::Start(8))]
    flags: u32,
    capacity_sectors: u64,
    grain_sectors: u64,
    #[br(seek_before = std::io::SeekFrom::Start(44))]
    num_gtes_per_gt: u32,
    redundant_gd_sector: u64,
    gd_sector: u64,
}

/// Raw byte source under a [`SparseVmdk`]: either a window into a shared mmap
/// (the VMDK embedded in the `.ldbk`) or an owned, decompressed buffer.
pub enum Backing {
    /// `mmap[base..base + len]` is the VMDK extent. Reads page in lazily, so a
    /// sparse 256 GiB disk only ever touches the grains actually visited.
    Mapped {
        mmap: Arc<Mmap>,
        base: usize,
        len: usize,
    },
    /// A decompressed extent held entirely in memory.
    Owned(Arc<Vec<u8>>),
}

impl Backing {
    fn bytes(&self) -> &[u8] {
        match self {
            Backing::Mapped { mmap, base, len } => &mmap[*base..*base + *len],
            Backing::Owned(buf) => buf.as_slice(),
        }
    }
}

/// A `monolithicSparse` VMDK exposed as a flat [`SectorSource`] of its virtual
/// media. Unallocated grains read back as zeros.
pub struct SparseVmdk {
    backing: Backing,
    /// Virtual media size in bytes (`capacity` sectors × 512).
    capacity_bytes: u64,
    grain_bytes: u64,
    num_gtes_per_gt: u64,
    /// Byte offset of the grain directory within the extent.
    gd_offset_bytes: u64,
}

impl std::fmt::Debug for SparseVmdk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SparseVmdk")
            .field("capacity_bytes", &self.capacity_bytes)
            .field("grain_bytes", &self.grain_bytes)
            .finish()
    }
}

impl SparseVmdk {
    /// Parse the `SparseExtentHeader` and return a reader, or an error if the
    /// extent is not a plain header-grain-directory `monolithicSparse` VMDK.
    pub fn open(backing: Backing) -> anyhow::Result<Self> {
        let data = backing.bytes();
        if data.len() < 512 {
            bail!(
                "VMDK extent is too small to hold a header ({} bytes)",
                data.len()
            );
        }
        let header = SparseExtentHeader::read(&mut Cursor::new(data))
            .context("reading VMDK sparse extent header")?;
        if header.flags & FLAG_COMPRESSED_GRAINS != 0 {
            bail!("compressed-grain VMDK is not supported by the sparse reader");
        }
        if header.grain_sectors == 0 || header.num_gtes_per_gt == 0 {
            bail!(
                "invalid VMDK geometry (grainSize={}, numGTEsPerGT={})",
                header.grain_sectors,
                header.num_gtes_per_gt
            );
        }
        // Prefer the primary grain directory; fall back to the redundant one.
        // A footer-resident GD (streamOptimized) is out of scope.
        let gd_sector = match (header.gd_sector, header.redundant_gd_sector) {
            (g, _) if g != 0 && g != GD_AT_FOOTER => g,
            (_, r) if r != 0 && r != GD_AT_FOOTER => r,
            _ => bail!("VMDK grain directory is not in the header (streamOptimized?)"),
        };
        Ok(Self {
            backing,
            capacity_bytes: header.capacity_sectors.saturating_mul(SECTOR),
            grain_bytes: header.grain_sectors.saturating_mul(SECTOR),
            num_gtes_per_gt: header.num_gtes_per_gt as u64,
            gd_offset_bytes: gd_sector.saturating_mul(SECTOR),
        })
    }

    /// Resolve a media byte offset to the file offset of that byte within the
    /// extent, or `None` when the grain is unallocated (reads as zero).
    fn grain_file_offset(&self, media_off: u64) -> Option<u64> {
        let data = self.backing.bytes();
        let grain_index = media_off / self.grain_bytes;
        let gt_number = grain_index / self.num_gtes_per_gt;
        let gte_index = grain_index % self.num_gtes_per_gt;

        let gd_entry_off = self
            .gd_offset_bytes
            .checked_add(gt_number.checked_mul(4)?)?;
        let gt_sector = read_u32(data, usize::try_from(gd_entry_off).ok()?)? as u64;
        if gt_sector == 0 {
            return None; // whole grain table unallocated
        }
        let gt_entry_off = gt_sector
            .checked_mul(SECTOR)?
            .checked_add(gte_index.checked_mul(4)?)?;
        let grain_sector = read_u32(data, usize::try_from(gt_entry_off).ok()?)? as u64;
        if grain_sector == 0 {
            return None; // unallocated grain
        }
        let within = media_off % self.grain_bytes;
        grain_sector.checked_mul(SECTOR)?.checked_add(within)
    }
}

impl SectorSource for SparseVmdk {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        let data = self.backing.bytes();
        let start = offset.min(self.capacity_bytes);
        let available = (self.capacity_bytes - start).min(len as u64) as usize;
        if available == 0 {
            return Ok(Vec::new());
        }
        let mut out = vec![0u8; available];
        let mut filled = 0usize;
        while filled < available {
            let pos = start + filled as u64;
            let within = pos % self.grain_bytes;
            let take = ((self.grain_bytes - within) as usize).min(available - filled);
            if let Some(file_off) = self.grain_file_offset(pos) {
                let lo = file_off as usize;
                // An out-of-range grain pointer (corrupt extent) leaves the gap
                // zero-filled rather than erroring the whole read.
                if let Some(slice) = lo.checked_add(take).and_then(|hi| data.get(lo..hi)) {
                    out[filled..filled + take].copy_from_slice(slice);
                }
            }
            filled += take;
        }
        Ok(out)
    }

    fn size(&self) -> u64 {
        self.capacity_bytes
    }
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    let bytes: [u8; 4] = data.get(offset..offset + 4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal monolithicSparse VMDK in memory: one grain table, two
    /// grains (grain 0 allocated, grain 1 sparse), and return the bytes.
    fn synth_vmdk() -> Vec<u8> {
        let grain_sectors = 1u64; // 512-byte grains keep the fixture tiny
        let num_gtes = 4u64;
        let capacity = 2u64; // two grains of media
                             // Layout (sectors): [0]=header, [1]=grain dir, [2]=grain table,
                             // [3]=grain for media-grain 0. Media-grain 1 stays unallocated.
        let gd_sector = 1u64;
        let gt_sector = 2u64;
        let grain0_sector = 3u64;
        let mut data = vec![0u8; (4 * SECTOR) as usize];

        data[0..4].copy_from_slice(b"KDMV");
        data[8..12].copy_from_slice(&3u32.to_le_bytes()); // flags: newline + redundant GT
        data[12..20].copy_from_slice(&capacity.to_le_bytes());
        data[20..28].copy_from_slice(&grain_sectors.to_le_bytes());
        data[44..48].copy_from_slice(&(num_gtes as u32).to_le_bytes());
        data[56..64].copy_from_slice(&gd_sector.to_le_bytes());

        // Grain directory entry 0 → grain table sector.
        let gd = (gd_sector * SECTOR) as usize;
        data[gd..gd + 4].copy_from_slice(&(gt_sector as u32).to_le_bytes());
        // Grain table entry 0 → grain0 sector; entry 1 stays 0 (sparse).
        let gt = (gt_sector * SECTOR) as usize;
        data[gt..gt + 4].copy_from_slice(&(grain0_sector as u32).to_le_bytes());
        // Fill grain 0 with a marker byte.
        let g0 = (grain0_sector * SECTOR) as usize;
        for b in &mut data[g0..g0 + SECTOR as usize] {
            *b = 0xAB;
        }
        data
    }

    #[test]
    fn reads_allocated_and_sparse_grains() {
        let vmdk = SparseVmdk::open(Backing::Owned(Arc::new(synth_vmdk()))).expect("parse vmdk");
        assert_eq!(vmdk.size(), 2 * SECTOR);

        // Grain 0 is allocated with 0xAB.
        let g0 = vmdk.read(0, SECTOR as usize).expect("read grain 0");
        assert_eq!(g0.len(), SECTOR as usize);
        assert!(g0.iter().all(|&b| b == 0xAB));

        // Grain 1 is sparse → zeros.
        let g1 = vmdk.read(SECTOR, SECTOR as usize).expect("read grain 1");
        assert!(g1.iter().all(|&b| b == 0));

        // A read spanning the grain boundary stitches 0xAB then 0x00.
        let span = vmdk.read(SECTOR - 4, 8).expect("read across boundary");
        assert_eq!(span, [0xAB, 0xAB, 0xAB, 0xAB, 0, 0, 0, 0]);

        // Reads past the media end clamp.
        let tail = vmdk.read(2 * SECTOR - 2, 16).expect("read past end");
        assert_eq!(tail.len(), 2);
    }

    #[test]
    fn rejects_non_vmdk_and_compressed() {
        assert!(SparseVmdk::open(Backing::Owned(Arc::new(vec![0u8; 1024]))).is_err());

        let mut compressed = synth_vmdk();
        compressed[8..12].copy_from_slice(&(3u32 | FLAG_COMPRESSED_GRAINS).to_le_bytes());
        assert!(SparseVmdk::open(Backing::Owned(Arc::new(compressed))).is_err());
    }
}
