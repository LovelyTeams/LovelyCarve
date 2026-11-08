//! Opt-in RGD recovery — resolving reads through the redundant grain directory
//! when the primary grain directory is damaged. The read path (`grain_location`,
//! `iter_allocated_grains`) calls into these resolvers; they are no-ops unless
//! `enable_rgd_fallback()` was called.

use std::io::{self, Read, Seek, SeekFrom};

use crate::header::{GD_AT_END, SECTOR_SIZE};
use crate::VmdkReader;

impl<R: Read + Seek> VmdkReader<R> {
    /// Enable opt-in RGD fallback: a read whose primary grain-table pointer is out of
    /// bounds is resolved through the redundant grain directory instead, recovering
    /// data from a damaged primary GD that `qemu-img` would simply fail on.
    pub fn enable_rgd_fallback(&mut self) {
        self.rgd_fallback = true;
    }

    /// Number of grains resolved via the redundant grain directory so far (pointer- or
    /// entry-level recovery). Zero on a healthy image; non-zero quantifies how much of a
    /// damaged image was reconstructed from the RGD.
    #[must_use]
    pub fn rgd_recovery_count(&self) -> u64 {
        self.rgd_recovery_count
    }

    /// Resolve the grain-table sector for `gd_idx`, preferring the primary pointer and
    /// falling back to the redundant grain directory when the primary is unusable.
    /// Returns the primary pointer unchanged when no better candidate exists, so the
    /// non-fallback error/sparse behaviour is preserved.
    pub(crate) fn resilient_gt_sector(
        &mut self,
        gd_idx: usize,
        primary: u32,
        num_gtes_per_gt: u64,
    ) -> io::Result<u32> {
        let file_len = self.inner.seek(SeekFrom::End(0))?;
        let gt_byte_len = num_gtes_per_gt * 4;
        let usable = |sec: u32| {
            sec != 0
                && u64::from(sec)
                    .saturating_mul(SECTOR_SIZE)
                    .saturating_add(gt_byte_len)
                    <= file_len
        };
        if usable(primary) {
            return Ok(primary);
        }
        let rgd = self.rgd_dir_entry(gd_idx, file_len)?;
        if usable(rgd) {
            crate::diag::pointer_recovered(gd_idx, primary, rgd);
            return Ok(rgd);
        }
        Ok(primary)
    }

    /// Read entry `gd_idx` from the redundant grain directory, or 0 if the RGD is
    /// absent or the entry itself would fall outside the file.
    pub(crate) fn rgd_dir_entry(&mut self, gd_idx: usize, file_len: u64) -> io::Result<u32> {
        if self.rgd_offset == 0 || self.rgd_offset == GD_AT_END {
            return Ok(0);
        }
        if gd_idx >= self.gd_entry_count {
            return Ok(0);
        }
        let entry_byte = self
            .rgd_offset
            .saturating_mul(SECTOR_SIZE)
            .saturating_add(gd_idx as u64 * 4);
        if entry_byte.saturating_add(4) > file_len {
            return Ok(0);
        }
        let mut b = [0u8; 4];
        self.read_exact_at(entry_byte, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    /// Read the full redundant grain table referenced by RGD entry `gd_idx`, or `None`
    /// if the RGD entry is absent or the grain table would fall outside the file.
    pub(crate) fn read_redundant_gt(
        &mut self,
        gd_idx: usize,
        num_gtes_per_gt: u64,
    ) -> io::Result<Option<Vec<u8>>> {
        let file_len = self.inner.seek(SeekFrom::End(0))?;
        let sector = self.rgd_dir_entry(gd_idx, file_len)?;
        if sector == 0 {
            return Ok(None);
        }
        let gt_byte = u64::from(sector) * SECTOR_SIZE;
        let gt_byte_len = num_gtes_per_gt * 4;
        if gt_byte.saturating_add(gt_byte_len) > file_len {
            return Ok(None);
        }
        let mut b = vec![0u8; gt_byte_len as usize];
        self.read_exact_at(gt_byte, &mut b)?;
        Ok(Some(b))
    }

    /// Read grain-table entry `gte_idx` from the redundant grain table referenced by
    /// RGD entry `gd_idx`, or 0 if the RGD entry or the target entry is out of bounds.
    /// Used for content-level recovery when a primary GT entry has been lost.
    pub(crate) fn rgd_gte(
        &mut self,
        gd_idx: usize,
        gte_idx: u64,
        num_gtes_per_gt: u64,
    ) -> io::Result<u32> {
        let file_len = self.inner.seek(SeekFrom::End(0))?;
        let rgd_gt_sector = self.rgd_dir_entry(gd_idx, file_len)?;
        if rgd_gt_sector == 0 {
            return Ok(0);
        }
        let gt_byte = u64::from(rgd_gt_sector) * SECTOR_SIZE;
        let gt_byte_len = num_gtes_per_gt * 4;
        if gt_byte.saturating_add(gt_byte_len) > file_len {
            return Ok(0);
        }
        let entry_byte = gt_byte + gte_idx * 4;
        if entry_byte.saturating_add(4) > file_len {
            return Ok(0);
        }
        let mut b = [0u8; 4];
        self.read_exact_at(entry_byte, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }
}
