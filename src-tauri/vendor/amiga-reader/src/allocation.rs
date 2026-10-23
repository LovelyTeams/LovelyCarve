use crate::{require, Geometry, Source};
use amiga_ffs::{BlockSource as _, Volume};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::{HashSet, VecDeque};

pub(crate) struct Allocation {
    pages: Vec<u32>,
    metadata: HashSet<u64>,
    cache: VecDeque<(usize, Vec<u8>)>,
}
impl Allocation {
    pub fn open<S: BlockSource>(
        volume: &mut Volume<Source<S>>,
        geometry: &Geometry,
    ) -> Result<Self> {
        let needed = (geometry.blocks - u64::from(geometry.reserved))
            .div_ceil(u64::from(geometry.block_size) * 8 - 32) as usize;
        require(
            needed > 0 && needed <= 2_000_000,
            "amiga: bitmap size limit",
        )?;
        let mut pages = Vec::with_capacity(needed);
        let mut metadata = HashSet::from([geometry.root]);
        let mut next = volume.root().bitmap_ext;
        let mut pointers = volume.root().bitmap_pages.to_vec();
        loop {
            for pointer in pointers {
                if pages.len() == needed {
                    require(pointer == 0, "amiga: excess bitmap pointer")?;
                    continue;
                }
                require(
                    u64::from(pointer) >= u64::from(geometry.reserved)
                        && u64::from(pointer) < geometry.blocks
                        && metadata.insert(u64::from(pointer)),
                    "amiga: bitmap pointer bounds/duplicate",
                )?;
                pages.push(pointer);
            }
            if next == 0 {
                break;
            }
            require(
                pages.len() < needed
                    && u64::from(next) >= u64::from(geometry.reserved)
                    && u64::from(next) < geometry.blocks
                    && metadata.insert(u64::from(next)),
                "amiga: bitmap extension bounds/cycle/excess",
            )?;
            let mut bytes = vec![0; geometry.block_size as usize];
            volume
                .source_mut()
                .read_block(u64::from(next), &mut bytes)?;
            pointers = bytes[..bytes.len() - 4]
                .as_chunks::<4>()
                .0
                .iter()
                .copied()
                .map(u32::from_be_bytes)
                .collect();
            next = amiga_ffs::be32(&bytes, bytes.len() - 4);
        }
        require(pages.len() == needed, "amiga: incomplete allocation bitmap")?;
        let mut allocation = Self {
            pages,
            metadata,
            cache: VecDeque::new(),
        };
        let metadata: Vec<_> = allocation.metadata.iter().copied().collect();
        for block in metadata {
            allocation.check(volume, geometry, block)?;
        }
        Ok(allocation)
    }
    pub fn check<S: BlockSource>(
        &mut self,
        volume: &mut Volume<Source<S>>,
        geometry: &Geometry,
        block: u64,
    ) -> Result<()> {
        require(
            block >= u64::from(geometry.reserved) && block < geometry.blocks,
            "amiga: allocated block outside volume/reserved area",
        )?;
        let bit = block - u64::from(geometry.reserved);
        let per_page = u64::from(geometry.block_size) * 8 - 32;
        let index = (bit / per_page) as usize;
        if !self.cache.iter().any(|(cached, _)| *cached == index) {
            let pointer = *self
                .pages
                .get(index)
                .ok_or(FoldError::Corrupt("amiga: missing bitmap page"))?;
            let mut bytes = vec![0; geometry.block_size as usize];
            volume
                .source_mut()
                .read_block(u64::from(pointer), &mut bytes)?;
            require(amiga_ffs::checksum_ok(&bytes), "amiga: bitmap checksum")?;
            if self.cache.len() == 8 {
                self.cache.pop_front();
            }
            self.cache.push_back((index, bytes));
        }
        let bytes = &self
            .cache
            .iter()
            .find(|(cached, _)| *cached == index)
            .ok_or(FoldError::Corrupt("amiga: missing bitmap cache"))?
            .1;
        let bit = bit % per_page;
        require(
            amiga_ffs::be32(bytes, 4 + (bit / 32) as usize * 4) & (1u32 << (bit % 32)) == 0,
            "amiga: referenced free block",
        )
    }
    pub fn data<S: BlockSource>(
        &mut self,
        volume: &mut Volume<Source<S>>,
        geometry: &Geometry,
        block: u64,
    ) -> Result<()> {
        require(
            !self.metadata.contains(&block),
            "amiga: file data overlaps volume metadata",
        )?;
        self.check(volume, geometry, block)
    }
}
