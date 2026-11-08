//! Cacheless, ordered bulk reads. Each worker decodes into its own output span;
//! no full-image allocation and no changes to checksum or length validation.
use rayon::prelude::*;

use super::{EwfError, EwfReader, Result};

impl EwfReader {
    pub(super) fn read_bulk_at(&self, buffer: &mut [u8], offset: u64) -> Result<usize> {
        let count = self
            .total_size
            .saturating_sub(offset)
            .min(buffer.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        let chunk_size = usize::try_from(self.chunk_size)
            .map_err(|_| EwfError::Parse("EWF chunk size exceeds address space".into()))?;
        let first_id = usize::try_from(offset / self.chunk_size)
            .map_err(|_| EwfError::Parse("EWF chunk index exceeds address space".into()))?;
        let within_chunk = (offset % self.chunk_size) as usize;
        let first_len = count.min(chunk_size - within_chunk);
        let (first, remaining) = buffer[..count].split_at_mut(first_len);
        let page = self.decompress_chunk(first_id)?;
        first.copy_from_slice(&page[within_chunk..within_chunk + first_len]);
        // Four coarse jobs keep I/O/decode overlapping without scheduling one
        // Rayon job per 32 KiB chunk or occupying every core in the desktop.
        let workers = rayon::current_num_threads().min(4);
        let job_size = remaining
            .len()
            .div_ceil(workers)
            .div_ceil(chunk_size)
            .max(1)
            * chunk_size;
        remaining
            .par_chunks_mut(job_size)
            .enumerate()
            .try_for_each(|(index, destination)| -> Result<()> {
                let first_job_chunk = first_id + 1 + index * (job_size / chunk_size);
                for (local, destination) in destination.chunks_mut(chunk_size).enumerate() {
                    let page = self.decompress_chunk(first_job_chunk + local)?;
                    destination.copy_from_slice(&page[..destination.len()]);
                }
                Ok(())
            })?;
        Ok(count)
    }
}
