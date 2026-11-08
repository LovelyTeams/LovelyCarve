use crate::{exact, require, Header};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::VecDeque;

#[derive(Default)]
pub(crate) struct Allocation {
    pages: VecDeque<(u64, Vec<u8>)>,
}
impl Allocation {
    pub fn check(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        first: u64,
        count: u64,
    ) -> Result<()> {
        require(
            count > 0
                && first
                    .checked_add(count)
                    .is_some_and(|end| end <= header.blocks),
            "befs: allocation range bounds",
        )?;
        let bits_per_page = u64::from(header.block_size) * 8;
        let mut block = first;
        let end = first + count;
        while block < end {
            let page = 1 + block / bits_per_page;
            require(page < header.bitmap_end, "befs: bitmap page outside bitmap")?;
            if !self.pages.iter().any(|(number, _)| *number == page) {
                let mut bytes = vec![0; header.block_size as usize];
                exact(source, page * u64::from(header.block_size), &mut bytes)?;
                if self.pages.len() == 8 {
                    self.pages.pop_front();
                }
                self.pages.push_back((page, bytes));
            }
            let bytes = &self
                .pages
                .iter()
                .find(|(number, _)| *number == page)
                .ok_or(FoldError::Corrupt("befs: missing bitmap cache"))?
                .1;
            let bit = block % bits_per_page;
            let shift = (bit % 32) as u32;
            let take = (end - block).min(32 - u64::from(shift));
            let mask = if take == 32 {
                u32::MAX
            } else {
                ((1u32 << take) - 1) << shift
            };
            require(
                header.endian.u32(bytes, (bit / 32) as usize * 4)? & mask == mask,
                "befs: referenced unallocated block",
            )?;
            block += take;
        }
        Ok(())
    }
}
