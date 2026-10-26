//! Positioned persistent allocation-map checks. Dmap locations account for
//! every preceding control-page level, including aggregates above 2^23 blocks.
use crate::{
    le32, le64,
    metadata::{Header, Physical},
    require,
    xtree::Map,
};
use lamfold::{BlockSource, Result};

pub(crate) struct Allocation {
    map: Map,
    length: u64,
    cache: Vec<(u64, Box<[u8; 4096]>)>,
}
impl Allocation {
    pub fn open(
        source: &mut impl BlockSource,
        header: &Header,
        map: Map,
        length: u64,
    ) -> Result<Self> {
        require(
            length >= 5 * 4096 && length.is_multiple_of(4096),
            "jfs: allocation-map length",
        )?;
        let mut control = [0; 4096];
        map.exact(source, header, length, 0, &mut control)?;
        require(
            le64(&control, 0)? == header.blocks
                && le64(&control, 8)? <= header.blocks
                && le32(&control, 16)? == (4096 / header.block_size).ilog2(),
            "jfs: allocation-map control geometry",
        )?;
        let mut allocation = Self {
            map,
            length,
            cache: Vec::new(),
        };
        // The map's own pages and runs must be represented as allocated too;
        // loading their dmap pages uses only exact reads, avoiding recursion.
        let extents: Vec<_> = allocation
            .map
            .pages
            .iter()
            .copied()
            .chain(allocation.map.runs.iter().map(|run| run.physical))
            .collect();
        for extent in extents {
            allocation.check(source, header, extent)?;
        }
        Ok(allocation)
    }
    fn page(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        number: u64,
    ) -> Result<&[u8; 4096]> {
        if let Some(index) = self.cache.iter().position(|(cached, _)| *cached == number) {
            return Ok(&self.cache[index].1);
        }
        let first = number * 8192;
        require(first < header.blocks, "jfs: dmap beyond aggregate")?;
        let logical_page = (first >> 13) + (first >> 23) + (first >> 33) + 4;
        let mut page = Box::new([0; 4096]);
        self.map.exact(
            source,
            header,
            self.length,
            logical_page * 4096,
            page.as_mut(),
        )?;
        let count = (header.blocks - first).min(8192);
        require(
            u64::from(le32(page.as_ref(), 0)?) == count
                && le64(page.as_ref(), 8)? == first
                && page[2048..3072] == page[3072..4096],
            "jfs: dmap geometry/working-persistent bitmap mismatch",
        )?;
        let mut allocated = 0;
        for word in 0..256 {
            let persistent = le32(page.as_ref(), 3072 + word * 4)?;
            for bit in 0..32 {
                let local = word as u64 * 32 + bit;
                let set = persistent & (0x80000000 >> bit) != 0;
                if local < count {
                    allocated += u64::from(set);
                } else {
                    require(set, "jfs: free block beyond final dmap range")?;
                }
            }
        }
        require(
            u64::from(le32(page.as_ref(), 4)?) == count - allocated,
            "jfs: dmap free block count",
        )?;
        if self.cache.len() == 64 {
            self.cache.clear();
        }
        self.cache.push((number, page));
        Ok(&self.cache[self.cache.len() - 1].1)
    }
    pub fn check(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        extent: Physical,
    ) -> Result<()> {
        let end = extent.start + u64::from(extent.count);
        require(
            end <= header.blocks && extent.count > 0,
            "jfs: allocated extent bounds",
        )?;
        let mut current = extent.start;
        while current < end {
            let page = self.page(source, header, current / 8192)?;
            let local = current % 8192;
            let bit = (local % 32) as u32;
            let take = (end - current).min(u64::from(32 - bit)) as u32;
            let mask = if take == 32 {
                u32::MAX
            } else {
                ((1u32 << take) - 1) << (32 - bit - take)
            };
            require(
                le32(page, 3072 + (local / 32) as usize * 4)? & mask == mask,
                "jfs: extent references unallocated block",
            )?;
            current += u64::from(take);
        }
        Ok(())
    }
}
