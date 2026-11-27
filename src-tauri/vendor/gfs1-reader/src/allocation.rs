use std::collections::VecDeque;

use lamfold::{BlockSource, FoldError, Result};

use crate::format::{Header, META_BYTES, RESOURCE_GROUP_BYTES, meta, require, u32be, u64be};

pub(crate) const INDEX_RECORD: usize = 96;
pub(crate) const INDEX_LIMIT: usize = 131072;

struct Group {
    first: u64,
    limit: u64,
    bitmap: u64,
}

pub(crate) struct Allocation {
    groups: Vec<Group>,
    pages: VecDeque<(u64, Vec<u8>)>,
}

impl Allocation {
    pub fn parse(source: &mut impl BlockSource, header: &Header, index: &[u8]) -> Result<Self> {
        require(
            !index.is_empty()
                && index.len().is_multiple_of(INDEX_RECORD)
                && index.len() / INDEX_RECORD <= INDEX_LIMIT,
            "invalid GFS1 resource index length",
        )?;
        let first_capacity = u64::from(header.block_size) - RESOURCE_GROUP_BYTES as u64;
        let continuation_capacity = u64::from(header.block_size) - META_BYTES as u64;
        let mut groups = Vec::with_capacity(index.len() / INDEX_RECORD);
        for record in index.as_chunks::<INDEX_RECORD>().0 {
            let bitmap = u64be(record, 0)?;
            let pages = u64::from(u32be(record, 8)?);
            let first = u64be(record, 16)?;
            let count = u64::from(u32be(record, 24)?);
            let bytes = u64::from(u32be(record, 28)?);
            let limit = first
                .checked_add(count)
                .ok_or(FoldError::Corrupt("GFS1 resource-group overflow"))?;
            require(
                count != 0
                    && bytes == count.div_ceil(4)
                    && pages
                        == 1 + bytes
                            .saturating_sub(first_capacity)
                            .div_ceil(continuation_capacity)
                    && bitmap.checked_add(pages) == Some(first)
                    && limit <= header.source_blocks,
                "invalid GFS1 resource-group geometry",
            )?;
            let raw = header.read_block(source, bitmap)?;
            meta(&raw, 2, 200)?;
            let accounted = [28, 32, 56, 60].iter().try_fold(0_u64, |sum, &at| {
                Ok::<_, FoldError>(sum + u64::from(u32be(&raw, at)?))
            })?;
            require(
                accounted <= count,
                "GFS1 resource-group counters exceed capacity",
            )?;
            groups.push(Group {
                first,
                limit,
                bitmap,
            });
        }
        groups.sort_unstable_by_key(|group| group.bitmap);
        require(
            groups
                .windows(2)
                .all(|pair| pair[0].limit <= pair[1].bitmap),
            "overlapping GFS1 resource groups",
        )?;
        Ok(Self {
            groups,
            pages: VecDeque::new(),
        })
    }

    pub fn disjoint(&self, start: u64, end: u64) -> bool {
        let next = self.groups.partition_point(|group| group.limit <= start);
        self.groups
            .get(next)
            .is_none_or(|group| end <= group.bitmap)
    }

    pub fn check(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        block: u64,
        metadata: bool,
    ) -> Result<()> {
        let before = self.groups.partition_point(|group| group.first <= block);
        let group = before
            .checked_sub(1)
            .and_then(|index| self.groups.get(index))
            .filter(|group| block < group.limit)
            .ok_or(FoldError::Corrupt(
                "GFS1 allocated block is outside resource groups",
            ))?;
        let within_group = block - group.first;
        let offset = within_group / 4;
        let first = u64::from(header.block_size) - RESOURCE_GROUP_BYTES as u64;
        let continuation = u64::from(header.block_size) - META_BYTES as u64;
        let (page, index) = if offset < first {
            (group.bitmap, RESOURCE_GROUP_BYTES + offset as usize)
        } else {
            let rest = offset - first;
            (
                group.bitmap + 1 + rest / continuation,
                META_BYTES + (rest % continuation) as usize,
            )
        };
        let page_index =
            if let Some(cached) = self.pages.iter().position(|(cached, _)| page == *cached) {
                cached
            } else {
                let raw = header.read_block(source, page)?;
                let (kind, format) = if page == group.bitmap {
                    (2, 200)
                } else {
                    (3, 300)
                };
                meta(&raw, kind, format)?;
                if self.pages.len() == 8 {
                    self.pages.pop_front();
                }
                self.pages.push_back((page, raw));
                self.pages.len() - 1
            };
        let raw = &self.pages[page_index].1;
        let status = (raw[index] >> ((within_group % 4) * 2)) & 3;
        require(
            status == if metadata { 3 } else { 1 },
            "unallocated or wrong-kind GFS1 block",
        )
    }
}
