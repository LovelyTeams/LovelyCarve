//! Multi-file sparse extent reader (twoGbMaxExtentSparse).
//!
//! Each SPARSE extent is an independent binary VMDK file with its own
//! `SparseExtentHeader`, grain directory, and grain tables.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::cowd::{self, COWD_GTES_PER_GT};
use crate::descriptor::SparseEntry;
use crate::header::{SparseExtentHeader, SECTOR_SIZE};

struct SparseChunk {
    byte_start: u64,
    byte_end: u64,
    grain_dir: Vec<u32>,
    grain_size_bytes: u64,
    num_gtes_per_gt: u64,
    file: BufReader<File>,
}

pub(crate) struct MultiSparseReader {
    chunks: Vec<SparseChunk>,
    pos: u64,
    total_bytes: u64,
}

impl MultiSparseReader {
    pub(crate) fn open(dir: &Path, entries: &[SparseEntry]) -> io::Result<Self> {
        let mut chunks = Vec::with_capacity(entries.len());
        let mut byte_offset = 0u64;

        for entry in entries {
            let path = crate::descriptor::resolve_extent_path(dir, entry.filename.as_ref())?;
            let mut file = BufReader::new(File::open(&path)?);

            let mut hdr_bytes = [0u8; 512];
            file.read_exact(&mut hdr_bytes)?;

            // Detect COWD extent (vmfsSparse/vmfsThin) vs standard VMDK4.
            let magic_be = u32::from_be_bytes(hdr_bytes[0..4].try_into().expect("4 bytes"));
            if magic_be == cowd::COWD_MAGIC {
                file.seek(SeekFrom::Start(0))?;
                let (grain_dir, grain_size_bytes) = cowd::open_cowd(&mut file)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
                let capacity = u64::from(
                    cowd::CowdHeader::parse(&hdr_bytes)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
                        .capacity,
                );
                if capacity != entry.size_sectors {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "COWD extent capacity mismatch",
                    ));
                }
                let byte_end =
                    byte_offset
                        .checked_add(capacity * SECTOR_SIZE)
                        .ok_or_else(|| {
                            io::Error::new(io::ErrorKind::InvalidData, "extent capacity overflow")
                        })?;
                chunks.push(SparseChunk {
                    byte_start: byte_offset,
                    byte_end,
                    grain_dir,
                    grain_size_bytes,
                    num_gtes_per_gt: COWD_GTES_PER_GT as u64,
                    file,
                });
                byte_offset = byte_end;
                continue;
            }

            let hdr = SparseExtentHeader::parse(&hdr_bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

            let grain_size_bytes = hdr
                .grain_size
                .checked_mul(SECTOR_SIZE)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "grain_size overflow"))?;
            let num_gtes_per_gt = u64::from(hdr.num_gtes_per_gt);

            let num_grains = hdr
                .capacity
                .checked_add(hdr.grain_size - 1)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "capacity overflow"))?
                / hdr.grain_size;
            let num_gts = num_grains
                .checked_add(num_gtes_per_gt - 1)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "num_gts overflow"))?
                / num_gtes_per_gt;
            // grain_size >= 8 (header-enforced) bounds num_gts <= u64::MAX/8, so the
            // 4-byte-per-entry multiply cannot overflow u64; the MAX_GD cap below is the
            // real protection against an unbounded allocation.
            let gd_byte_len = num_gts * 4;

            const MAX_GD: u64 = 16 * 1024 * 1024;
            if gd_byte_len > MAX_GD {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "GD too large"));
            }

            let gd_byte_offset = hdr
                .gd_offset
                .checked_mul(SECTOR_SIZE)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "gd_offset overflow"))?;
            file.seek(SeekFrom::Start(gd_byte_offset))?;
            let mut gd_bytes = vec![0u8; gd_byte_len as usize];
            file.read_exact(&mut gd_bytes)?;

            let grain_dir = gd_bytes
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes(c.try_into().expect("4-byte chunk")))
                .collect();

            if hdr.capacity != entry.size_sectors {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "sparse extent capacity mismatch",
                ));
            }
            if hdr.compressed {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "compressed split extent is not supported",
                ));
            }
            let size_bytes = entry.size_sectors.checked_mul(SECTOR_SIZE).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "extent size overflow")
            })?;
            let end = byte_offset.checked_add(size_bytes).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "extent capacity overflow")
            })?;

            chunks.push(SparseChunk {
                byte_start: byte_offset,
                byte_end: end,
                grain_dir,
                grain_size_bytes,
                num_gtes_per_gt,
                file,
            });
            byte_offset = end;
        }

        Ok(MultiSparseReader {
            chunks,
            pos: 0,
            total_bytes: byte_offset,
        })
    }

    fn chunk_for(&self, pos: u64) -> Option<usize> {
        self.chunks
            .iter()
            .position(|c| pos >= c.byte_start && pos < c.byte_end)
    }
}

impl Read for MultiSparseReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.total_bytes || buf.is_empty() {
            return Ok(0);
        }

        let chunk_idx = self.chunk_for(self.pos).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "position out of virtual range")
        })?;

        let (byte_start, byte_end, grain_size, num_gtes_per_gt) = {
            let c = &self.chunks[chunk_idx];
            (
                c.byte_start,
                c.byte_end,
                c.grain_size_bytes,
                c.num_gtes_per_gt,
            )
        };

        let local_pos = self.pos - byte_start;
        let remaining_virtual = (self.total_bytes - self.pos) as usize;
        let remaining_in_grain = (grain_size - (local_pos % grain_size)) as usize;
        let remaining_in_chunk = (byte_end - self.pos) as usize;
        let to_read = buf
            .len()
            .min(remaining_virtual)
            .min(remaining_in_grain)
            .min(remaining_in_chunk);

        let grain_idx = local_pos / grain_size;
        let offset_in_grain = local_pos % grain_size;
        let gd_idx = (grain_idx / num_gtes_per_gt) as usize;
        let gte_local_idx = grain_idx % num_gtes_per_gt;

        let gt_sector = self.chunks[chunk_idx]
            .grain_dir
            .get(gd_idx)
            .copied()
            .unwrap_or(0);

        if gt_sector == 0 {
            buf[..to_read].fill(0);
            self.pos += to_read as u64;
            return Ok(to_read);
        }

        let gte_file_pos = u64::from(gt_sector) * SECTOR_SIZE + gte_local_idx * 4;
        let chunk = &mut self.chunks[chunk_idx];
        chunk.file.seek(SeekFrom::Start(gte_file_pos))?;
        let mut gte_bytes = [0u8; 4];
        chunk.file.read_exact(&mut gte_bytes)?;
        let gte = u32::from_le_bytes(gte_bytes);

        let n = if gte <= 1 {
            buf[..to_read].fill(0);
            to_read
        } else {
            let file_offset = u64::from(gte) * SECTOR_SIZE + offset_in_grain;
            chunk.file.seek(SeekFrom::Start(file_offset))?;
            chunk.file.read(&mut buf[..to_read])?
        };

        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for MultiSparseReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(n) => self.pos as i64 + n,
            SeekFrom::End(n) => self.total_bytes as i64 + n,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::SparseEntry;
    use std::io::Write as _;

    /// A minimal all-sparse VMDK4 extent: header + a zeroed grain directory.
    fn all_sparse_extent() -> Vec<u8> {
        let mut v = vec![0u8; 1024];
        v[0..4].copy_from_slice(&0x564D_444Bu32.to_le_bytes()); // KDMV
        v[4..8].copy_from_slice(&1u32.to_le_bytes()); // version 1
        v[12..20].copy_from_slice(&8u64.to_le_bytes()); // capacity = 8 sectors
        v[20..28].copy_from_slice(&8u64.to_le_bytes()); // grain_size = 8
        v[44..48].copy_from_slice(&512u32.to_le_bytes()); // num_gtes_per_gt
        v[56..64].copy_from_slice(&1u64.to_le_bytes()); // gd_offset = sector 1
                                                        // GD at sector 1 stays all-zero → grain directory entry 0 (sparse).
        v
    }

    fn open_one(dir: &std::path::Path, bytes: &[u8], sectors: u64) -> MultiSparseReader {
        std::fs::File::create(dir.join("s001.vmdk"))
            .unwrap()
            .write_all(bytes)
            .unwrap();
        let e = SparseEntry {
            size_sectors: sectors,
            filename: Box::from("s001.vmdk"),
        };
        MultiSparseReader::open(dir, &[e]).unwrap()
    }

    #[test]
    fn all_sparse_reads_zeros_and_seeks() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = open_one(dir.path(), &all_sparse_extent(), 8);
        // Read through the sparse grain → zeros (covers gt_sector==0 fill path).
        let mut b = [0xFFu8; 512];
        r.read_exact(&mut b).unwrap();
        assert_eq!(b, [0u8; 512]);
        // Seek variants.
        assert_eq!(r.seek(SeekFrom::Start(1024)).unwrap(), 1024);
        assert_eq!(r.seek(SeekFrom::Current(-512)).unwrap(), 512);
        assert_eq!(r.seek(SeekFrom::End(-256)).unwrap(), 8 * 512 - 256);
        // Read at end → 0; empty buffer → 0.
        r.seek(SeekFrom::Start(8 * 512)).unwrap();
        assert_eq!(r.read(&mut [0u8; 4]).unwrap(), 0);
        r.seek(SeekFrom::Start(0)).unwrap();
        assert_eq!(r.read(&mut []).unwrap(), 0);
    }

    #[test]
    fn seek_before_start_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut r = open_one(dir.path(), &all_sparse_extent(), 8);
        assert!(r.seek(SeekFrom::End(-99999)).is_err());
    }

    #[test]
    fn grain_directory_too_large_rejected() {
        // capacity huge + grain_size 8 → GD exceeds the 16 MiB cap.
        let mut v = all_sparse_extent();
        v[12..20].copy_from_slice(&100_000_000_000u64.to_le_bytes());
        let dir = tempfile::tempdir().unwrap();
        std::fs::File::create(dir.path().join("s001.vmdk"))
            .unwrap()
            .write_all(&v)
            .unwrap();
        let e = SparseEntry {
            size_sectors: 8,
            filename: Box::from("s001.vmdk"),
        };
        assert!(MultiSparseReader::open(dir.path(), &[e]).is_err());
    }

    #[test]
    fn missing_extent_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let e = SparseEntry {
            size_sectors: 8,
            filename: Box::from("absent.vmdk"),
        };
        assert!(MultiSparseReader::open(dir.path(), &[e]).is_err());
    }
}
