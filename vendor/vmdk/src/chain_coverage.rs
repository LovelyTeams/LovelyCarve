//! Local chain integration: distinguish inherited holes from explicit zeroes.
use crate::{sesparse, FormatState, VmdkReader};
use std::io::{self, Read, Seek};

impl<R: Read + Seek> VmdkReader<R> {
    /// Whether a sector is owned by this layer, including explicit zero markers.
    /// This differs from `is_allocated`, which enumerates physical data grains.
    pub fn has_layer_data(&mut self, lba: u64) -> io::Result<bool> {
        if lba >= self.virtual_disk_size / 512 {
            return Ok(false);
        }
        let offset = lba * 512;
        match &self.fmt {
            FormatState::Flat => Ok(true),
            FormatState::Sparse {
                grain_dir,
                grain_size_bytes,
                num_gtes_per_gt,
                ..
            } => {
                let grain = offset / grain_size_bytes;
                let table = grain_dir
                    .get((grain / num_gtes_per_gt) as usize)
                    .copied()
                    .unwrap_or(0);
                if table == 0 {
                    return Ok(false);
                }
                let position = u64::from(table) * 512 + (grain % num_gtes_per_gt) * 4;
                let mut bytes = [0; 4];
                self.read_exact_at(position, &mut bytes)?;
                Ok(u32::from_le_bytes(bytes) != 0)
            }
            FormatState::SeSparse {
                grain_dir,
                grain_size_bytes,
                gt_offset_sectors,
                ..
            } => {
                let grain = offset / grain_size_bytes;
                let directory = grain_dir
                    .get((grain / sesparse::SE_GTES_PER_GT) as usize)
                    .copied()
                    .unwrap_or(0);
                let table_offset = *gt_offset_sectors;
                match self.se_read_gte(directory, table_offset, grain % sesparse::SE_GTES_PER_GT)? {
                    None | Some(0) => Ok(false),
                    Some(entry) => match entry & sesparse::SE_GTE_TYPE_MASK {
                        sesparse::SE_GTE_TYPE_ALLOCATED
                        | sesparse::SE_GTE_TYPE_UNMAPPED
                        | sesparse::SE_GTE_TYPE_ZERO => Ok(true),
                        _ => Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "invalid seSparse grain type",
                        )),
                    },
                }
            }
        }
    }
}
