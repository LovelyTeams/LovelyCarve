//! The read path — resolving a virtual offset to physical bytes and implementing
//! `Read`/`Seek` over the decoded virtual sector stream. Sparse grains read as zeros,
//! streamOptimized grains are zlib-decompressed, and (in recovery mode) damaged
//! pointers resolve through the redundant grain directory (see `recovery.rs`).

use std::io::{self, Read, Seek, SeekFrom};

use crate::header::SECTOR_SIZE;
use crate::{bytes, diag, sesparse, FormatState, VmdkReader};

/// Where the bytes for a virtual offset live.
pub(crate) enum GrainLookup {
    /// Grain is not allocated — fill output with zeros.
    Sparse,
    /// Grain is uncompressed; data begins at this file byte offset.
    FileOffset(u64),
    /// Grain is zlib-compressed (streamOptimized); `data_offset` is the first
    /// byte of compressed payload (after the 12-byte `GrainMarker` header),
    /// `data_size` is the compressed length, and `offset_in_grain` is where
    /// to start reading within the decompressed grain.
    Compressed {
        data_offset: u64,
        data_size: u32,
        offset_in_grain: u64,
    },
}

impl<R: Read + Seek> VmdkReader<R> {
    /// Grain size in bytes for the sparse/seSparse read path (0 for flat, which is
    /// handled before this is reached on the read path).
    pub(crate) fn sparse_grain_size_bytes(&self) -> u64 {
        match &self.fmt {
            FormatState::Sparse {
                grain_size_bytes, ..
            }
            | FormatState::SeSparse {
                grain_size_bytes, ..
            } => *grain_size_bytes,
            FormatState::Flat => 0,
        }
    }

    /// Resolve `virtual_offset` to a [`GrainLookup`] describing where to find the data.
    pub(crate) fn grain_location(&mut self, virtual_offset: u64) -> io::Result<GrainLookup> {
        // seSparse uses nibble-typed, bit-rotated 8-byte grain entries — resolved separately.
        if matches!(self.fmt, FormatState::SeSparse { .. }) {
            return self.grain_location_sesparse(virtual_offset);
        }
        self.grain_location_sparse(virtual_offset)
    }

    /// Resolve a virtual offset for a seSparse (VMFS6) extent.
    fn grain_location_sesparse(&mut self, virtual_offset: u64) -> io::Result<GrainLookup> {
        let FormatState::SeSparse {
            grain_dir,
            grain_size_bytes,
            gt_offset_sectors,
            grains_offset_sectors,
        } = &self.fmt
        else {
            return Ok(GrainLookup::Sparse); // dispatched only for seSparse
        };
        {
            let grain_size_bytes = *grain_size_bytes;
            let grain_sectors = grain_size_bytes / SECTOR_SIZE;
            let grains_offset = *grains_offset_sectors;
            let gt_off = *gt_offset_sectors;
            let grain_idx = virtual_offset / grain_size_bytes;
            let offset_in_grain = virtual_offset % grain_size_bytes;
            let gd_idx = (grain_idx / sesparse::SE_GTES_PER_GT) as usize;
            let gte_idx = grain_idx % sesparse::SE_GTES_PER_GT;
            let gd_entry = grain_dir.get(gd_idx).copied().unwrap_or(0);

            let Some(gte) = self.se_read_gte(gd_entry, gt_off, gte_idx)? else {
                return Ok(GrainLookup::Sparse);
            };
            match gte & sesparse::SE_GTE_TYPE_MASK {
                // Unallocated: the whole entry must be zero (already handled by se_read_gte
                // for the GD level; a zero GTE here means a sparse grain within an allocated GT).
                0 if gte == 0 => Ok(GrainLookup::Sparse),
                sesparse::SE_GTE_TYPE_UNMAPPED | sesparse::SE_GTE_TYPE_ZERO => {
                    Ok(GrainLookup::Sparse)
                }
                sesparse::SE_GTE_TYPE_ALLOCATED => {
                    let grain_index = sesparse::se_gte_grain_index(gte);
                    let cluster_sector = grains_offset + grain_index * grain_sectors;
                    Ok(GrainLookup::FileOffset(
                        cluster_sector * SECTOR_SIZE + offset_in_grain,
                    ))
                }
                _ => Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "seSparse grain entry has unsupported type nibble",
                )),
            }
        }
    }

    /// Resolve a virtual offset for a VMDK4 sparse / streamOptimized extent.
    fn grain_location_sparse(&mut self, virtual_offset: u64) -> io::Result<GrainLookup> {
        let (
            gd_idx,
            gt_sector,
            gte_idx,
            offset_in_grain,
            compressed,
            grain_size_bytes,
            num_gtes_per_gt,
        ) = {
            let FormatState::Sparse {
                grain_dir,
                grain_size_bytes,
                num_gtes_per_gt,
                compressed,
            } = &self.fmt
            else {
                return Ok(GrainLookup::Sparse); // Flat — not reached from Read::read
            };
            let grain_idx = virtual_offset / grain_size_bytes;
            let offset_in_grain = virtual_offset % grain_size_bytes;
            let gd_idx = (grain_idx / num_gtes_per_gt) as usize;
            let gte_idx = grain_idx % num_gtes_per_gt;
            let gt_sector = grain_dir.get(gd_idx).copied().unwrap_or(0);
            (
                gd_idx,
                gt_sector,
                gte_idx,
                offset_in_grain,
                *compressed,
                *grain_size_bytes,
                *num_gtes_per_gt,
            )
        };
        // Recovery mode: if the primary grain-table pointer is unusable, resolve it
        // through the redundant grain directory instead.
        let primary_gt_sector = gt_sector;
        let gt_sector = if self.rgd_fallback {
            self.resilient_gt_sector(gd_idx, gt_sector, num_gtes_per_gt)?
        } else {
            gt_sector
        };
        // The grain table was recovered when fallback swapped in a different (RGD) pointer.
        let mut from_rgd = self.rgd_fallback && gt_sector != primary_gt_sector && gt_sector != 0;
        if gt_sector == 0 {
            return Ok(GrainLookup::Sparse);
        }
        // Use cached GT if available; otherwise read from file and cache it.
        let gte = if let Some(gt) = self.gt_cache.get(&gt_sector) {
            gt.get(gte_idx as usize).copied().unwrap_or(0)
        } else {
            // Read the full GT (num_gtes_per_gt entries × 4 bytes) into the cache.
            let gt_byte_offset = u64::from(gt_sector) * SECTOR_SIZE;
            self.inner.seek(SeekFrom::Start(gt_byte_offset))?;
            let gt_size = num_gtes_per_gt as usize * 4;
            let mut gt_bytes = vec![0u8; gt_size];
            self.inner.read_exact(&mut gt_bytes)?;
            let gt: Vec<u32> = bytes::le_u32_table(&gt_bytes);
            let gte = gt.get(gte_idx as usize).copied().unwrap_or(0);
            self.gt_cache.insert(gt_sector, gt);
            gte
        };
        // Content-level recovery: the primary grain-table pointer was usable but this
        // entry is sparse — if the redundant grain table still holds the grain pointer,
        // the primary entry was lost to corruption, so recover it.
        let gte = if self.rgd_fallback && gte <= 1 {
            let rgd_gte = self.rgd_gte(gd_idx, gte_idx, num_gtes_per_gt)?;
            if rgd_gte > 1 {
                diag::entry_recovered(gd_idx, gte_idx);
                from_rgd = true;
                rgd_gte
            } else {
                gte
            }
        } else {
            gte
        };
        if gte <= 1 {
            diag::grain_resolved(virtual_offset, "sparse");
            return Ok(GrainLookup::Sparse); // sparse or explicitly-zeroed grain
        }
        if from_rgd {
            self.rgd_recovery_count += 1;
        }
        if compressed {
            // GrainMarker layout: u64 LBA (8 bytes) + u32 dataSize (4 bytes) + data.
            let marker_offset = u64::from(gte) * SECTOR_SIZE;
            let mut marker_hdr = [0u8; 12];
            self.read_exact_at(marker_offset, &mut marker_hdr)?;
            let data_size = u32::from_le_bytes(marker_hdr[8..12].try_into().expect("4 bytes"));
            // Cap data_size to prevent allocation amplification from crafted markers.
            // A legitimate compressed grain cannot expand to more than 64 KiB past the
            // raw grain size; 65536 bytes of headroom absorbs any real compressor overhead.
            let max_data = grain_size_bytes.saturating_add(65536);
            if u64::from(data_size) > max_data {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!(
                        "compressed grain data_size {data_size} exceeds limit {max_data}: \
                         likely a crafted or corrupt VMDK"
                    ),
                ));
            }
            diag::grain_resolved(virtual_offset, "compressed");
            return Ok(GrainLookup::Compressed {
                data_offset: marker_offset + 12,
                data_size,
                offset_in_grain,
            });
        }
        diag::grain_resolved(virtual_offset, "file");
        Ok(GrainLookup::FileOffset(
            u64::from(gte) * SECTOR_SIZE + offset_in_grain,
        ))
    }

    /// Decompress a zlib-wrapped grain and copy the requested slice into `buf`.
    fn read_compressed_grain(
        &mut self,
        buf: &mut [u8],
        data_offset: u64,
        data_size: u32,
        offset_in_grain: u64,
    ) -> io::Result<usize> {
        use flate2::read::ZlibDecoder;

        let grain_size_bytes = self.sparse_grain_size_bytes();
        let mut compressed = vec![0u8; data_size as usize];
        self.read_exact_at(data_offset, &mut compressed)?;

        // Bound the decode to one grain (+1 sentinel byte). A legitimate grain
        // decompresses to exactly grain_size_bytes; anything larger is a crafted
        // decompression bomb and is refused before the expansion is materialised.
        let mut decoder = ZlibDecoder::new(compressed.as_slice()).take(grain_size_bytes + 1);
        let mut grain_data = Vec::new();
        decoder.read_to_end(&mut grain_data)?;
        if grain_data.len() as u64 > grain_size_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "compressed grain decompresses beyond its grain size (possible decompression bomb)",
            ));
        }

        let start = offset_in_grain as usize;
        let end = (start + buf.len()).min(grain_data.len());
        let n = end.saturating_sub(start);
        if n > 0 {
            buf[..n].copy_from_slice(&grain_data[start..end]);
        }
        Ok(n)
    }
}

impl<R: Read + Seek> Read for VmdkReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.virtual_disk_size || buf.is_empty() {
            return Ok(0);
        }
        let remaining_virtual = (self.virtual_disk_size - self.pos) as usize;

        // Flat: direct pass-through to the inner reader at the current position.
        if matches!(self.fmt, FormatState::Flat) {
            let to_read = buf.len().min(remaining_virtual);
            self.inner.seek(SeekFrom::Start(self.pos))?;
            let n = self.inner.read(&mut buf[..to_read])?;
            self.pos += n as u64;
            return Ok(n);
        }

        // Sparse / StreamOptimized / SeSparse: clamp at grain boundary then do GTE lookup.
        let grain_size_bytes = self.sparse_grain_size_bytes();
        let remaining_in_grain = (grain_size_bytes - (self.pos % grain_size_bytes)) as usize;
        let to_read = buf.len().min(remaining_virtual).min(remaining_in_grain);

        let location = self.grain_location(self.pos)?;
        let n = match location {
            GrainLookup::Sparse => {
                buf[..to_read].fill(0);
                to_read
            }
            GrainLookup::FileOffset(file_off) => {
                self.inner.seek(SeekFrom::Start(file_off))?;
                self.inner.read(&mut buf[..to_read])?
            }
            GrainLookup::Compressed {
                data_offset,
                data_size,
                offset_in_grain,
            } => self.read_compressed_grain(
                &mut buf[..to_read],
                data_offset,
                data_size,
                offset_in_grain,
            )?,
        };

        self.pos += n as u64;
        Ok(n)
    }
}

impl<R: Read + Seek> Seek for VmdkReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(n) => self.pos as i64 + n,
            SeekFrom::End(n) => self.virtual_disk_size as i64 + n,
        };
        if new_pos < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = new_pos as u64;
        Ok(self.pos)
    }
}
