use crate::{exact, require, Geometry, Source};
use amiga_ffs::{FileChain, Volume};
use lamfold::{BlockSource, FoldError, Result};

/// FFS data is unchecked raw payload; adjacent validated pointers can be read
/// together. Large streams then use DiskImage's bulk path instead of evicting
/// its metadata cache through thousands of tiny file-data reads.
pub(crate) fn read_ffs<S: BlockSource>(
    volume: &mut Volume<Source<S>>,
    geometry: &Geometry,
    chain: &FileChain,
    offset: u64,
    bytes: &mut [u8],
) -> Result<usize> {
    let expected = u64::from(chain.byte_size)
        .saturating_sub(offset)
        .min(bytes.len() as u64) as usize;
    let block_size = u64::from(geometry.block_size);
    let mut done = 0;
    while done < expected {
        let position = offset + done as u64;
        let index = (position / block_size) as usize;
        let skip = position % block_size;
        let first = *chain
            .blocks
            .get(index)
            .ok_or(FoldError::Corrupt("amiga: short FFS map"))?;
        let mut run = 1usize;
        while let Some(next) = chain.blocks.get(index + run) {
            if u64::from(*next) != u64::from(first) + run as u64
                || run as u64 * block_size - skip >= (expected - done) as u64
            {
                break;
            }
            run += 1;
        }
        let count = (run as u64 * block_size - skip).min((expected - done) as u64) as usize;
        let physical = u64::from(first) * block_size + skip;
        require(
            physical
                .checked_add(count as u64)
                .is_some_and(|end| end <= geometry.blocks * block_size),
            "amiga: FFS run beyond filesystem volume",
        )?;
        exact(
            &mut volume.source_mut().source,
            physical,
            &mut bytes[done..done + count],
        )?;
        done += count;
    }
    Ok(done)
}
