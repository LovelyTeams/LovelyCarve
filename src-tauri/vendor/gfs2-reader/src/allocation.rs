//! The rindex describes disjoint data regions; two-bit bitmaps distinguish
//! used data, live inodes, unlinked inodes and free storage.
use crate::{disk_hash, header::Header, meta, require, u32be, u64be};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::VecDeque;

#[derive(Clone)]
pub(crate) struct Group {
    address: u64,
    length: u32,
    first: u64,
    count: u32,
    bitmap_bytes: u32,
}
pub(crate) struct Allocation {
    groups: Vec<Group>,
    cache: VecDeque<(u64, Vec<u8>)>,
}
impl Allocation {
    pub fn parse(source: &mut impl BlockSource, header: &Header, index: &[u8]) -> Result<Self> {
        require(
            !index.is_empty() && index.len().is_multiple_of(96) && index.len() / 96 <= 131072,
            "gfs2: rindex length/work limit",
        )?;
        let mut groups = Vec::new();
        for raw in index.as_chunks::<96>().0 {
            let group = Group {
                address: u64be(raw, 0)?,
                length: u32be(raw, 8)?,
                first: u64be(raw, 16)?,
                count: u32be(raw, 24)?,
                bitmap_bytes: u32be(raw, 28)?,
            };
            header.block_offset(group.address)?;
            require(
                group.length != 0
                    && group.count != 0
                    && group.address.checked_add(group.length as u64) == Some(group.first),
                "gfs2: resource-group geometry",
            )?;
            require(
                group
                    .first
                    .checked_add(group.count as u64)
                    .is_some_and(|end| end <= header.source_blocks),
                "gfs2: resource group outside source",
            )?;
            require(
                group.bitmap_bytes as u64 == (group.count as u64).div_ceil(4),
                "gfs2: bitmap length",
            )?;
            let capacity = header.block_size as u64 - 128
                + (group.length as u64 - 1) * (header.block_size as u64 - 24);
            require(
                group.bitmap_bytes as u64 <= capacity
                    && (group.length == 1
                        || group.bitmap_bytes as u64 > capacity - (header.block_size as u64 - 24)),
                "gfs2: bitmap page coverage",
            )?;
            let mut raw = header.read_block(source, group.address)?;
            meta(&raw, 2, 200)?;
            let free = u32be(&raw, 28)?;
            let dinodes = u32be(&raw, 32)?;
            require(
                free <= group.count && dinodes <= group.count - free,
                "gfs2: resource-group counters",
            )?;
            let recorded_crc = u32be(&raw, 64)?;
            if recorded_crc != 0 {
                raw[64..68].fill(0);
                require(
                    disk_hash(&raw[..128]) == recorded_crc,
                    "gfs2: resource-group CRC",
                )?;
            }
            if header.format == 1802 || u64be(&raw, 48)? != 0 {
                require(
                    u64be(&raw, 48)? == group.first
                        && u32be(&raw, 56)? == group.count
                        && u32be(&raw, 60)? == group.bitmap_bytes,
                    "gfs2: rindex/header disagreement",
                )?;
            }
            groups.push(group);
        }
        groups.sort_by_key(|group| group.address);
        for pair in groups.windows(2) {
            require(
                pair[0].first + pair[0].count as u64 <= pair[1].address,
                "gfs2: overlapping resource groups",
            )?;
        }
        Ok(Self {
            groups,
            cache: VecDeque::new(),
        })
    }
    pub fn check(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        block: u64,
        inode: bool,
    ) -> Result<()> {
        let index = self.groups.partition_point(|group| group.first <= block);
        let group = self
            .groups
            .get(
                index
                    .checked_sub(1)
                    .ok_or(FoldError::Corrupt("gfs2: block outside resource groups"))?,
            )
            .ok_or(FoldError::Corrupt("gfs2: missing resource group"))?;
        require(
            block - group.first < group.count as u64,
            "gfs2: block outside resource-group data",
        )?;
        let relative = block - group.first;
        let byte = relative / 4;
        let first_capacity = header.block_size as u64 - 128;
        let (page, at) = if byte < first_capacity {
            (group.address, 128 + byte as usize)
        } else {
            let rest = byte - first_capacity;
            (
                group.address + 1 + rest / (header.block_size as u64 - 24),
                24 + (rest % (header.block_size as u64 - 24)) as usize,
            )
        };
        require(
            page < group.address + group.length as u64,
            "gfs2: bitmap page outside group",
        )?;
        if !self.cache.iter().any(|(cached, _)| *cached == page) {
            let raw = header.read_block(source, page)?;
            if page == group.address {
                meta(&raw, 2, 200)?;
            } else {
                meta(&raw, 3, 300)?;
            }
            if self.cache.len() == 8 {
                self.cache.pop_front();
            }
            self.cache.push_back((page, raw));
        }
        let raw = &self
            .cache
            .iter()
            .find(|(cached, _)| *cached == page)
            .ok_or(FoldError::Corrupt("gfs2: missing bitmap page"))?
            .1;
        let state = (raw[at] >> ((relative % 4) * 2)) & 3;
        require(
            state == if inode { 3 } else { 1 },
            "gfs2: free/unlinked/wrong-kind allocation",
        )
    }
}
