//! Native XAD trees. Internal descriptor lengths describe the child page's
//! physical allocation, not the logical span of the subtree it indexes.
use crate::{
    exact, le16, le32, le64,
    metadata::{Header, Physical},
    require,
};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::{BTreeMap, HashSet};

const MAX_PAGES: usize = 65_536;
const MAX_RUNS: usize = 262_144;
#[derive(Clone, Debug)]
pub(crate) struct Run {
    pub logical: u64,
    pub physical: Physical,
    pub unwritten: bool,
}
#[derive(Clone, Default)]
pub(crate) struct Map {
    pub runs: Vec<Run>,
    pub pages: Vec<Physical>,
}
impl Map {
    pub fn parse(source: &mut impl BlockSource, header: &Header, root: &[u8]) -> Result<Self> {
        let mut walker = Walker {
            source,
            header,
            seen: HashSet::new(),
            levels: BTreeMap::new(),
            map: Self::default(),
        };
        walker.visit(root, None, 0)?;
        require(
            walker.levels.values().all(|(_, next, _)| *next == 0),
            "jfs: unterminated xtree siblings",
        )?;
        for pair in walker.map.runs.windows(2) {
            require(
                pair[0].logical + u64::from(pair[0].physical.count) <= pair[1].logical,
                "jfs: overlapping/unordered xtree leaves",
            )?;
        }
        Ok(walker.map)
    }
    /// Read ordinary file content: only format-defined holes and NOTRECORDED
    /// extents synthesize zero bytes. Physical runs always require exact reads.
    pub fn read(
        &self,
        source: &mut impl BlockSource,
        header: &Header,
        length: u64,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize> {
        require(length <= i64::MAX as u64, "jfs: signed file length")?;
        let count = length.saturating_sub(offset).min(bytes.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        let end = offset + count as u64;
        bytes[..count].fill(0);
        let block_size = u64::from(header.block_size);
        let first = self.runs.partition_point(|run| {
            (run.logical + u64::from(run.physical.count)) * block_size <= offset
        });
        for run in &self.runs[first..] {
            let start = run.logical * block_size;
            if start >= end {
                break;
            }
            let after = start + run.physical.length(header);
            let from = offset.max(start);
            let to = end.min(after);
            if !run.unwritten && from < to {
                exact(
                    source,
                    run.physical.byte(header) + from - start,
                    &mut bytes[(from - offset) as usize..(to - offset) as usize],
                )?;
            }
        }
        Ok(count)
    }
    /// Metadata maps must have complete initialized coverage. A missing inode
    /// map/dmap page cannot be interpreted as an empty, all-free metadata page.
    pub fn exact(
        &self,
        source: &mut impl BlockSource,
        header: &Header,
        length: u64,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<()> {
        let end = offset
            .checked_add(bytes.len() as u64)
            .filter(|end| *end <= length)
            .ok_or(FoldError::Corrupt("jfs: metadata map length"))?;
        let block_size = u64::from(header.block_size);
        let mut cursor = offset;
        let first = self.runs.partition_point(|run| {
            (run.logical + u64::from(run.physical.count)) * block_size <= offset
        });
        for run in &self.runs[first..] {
            if cursor == end {
                break;
            }
            let start = run.logical * block_size;
            let after = start + run.physical.length(header);
            require(
                start <= cursor && cursor < after && !run.unwritten,
                "jfs: hole/unwritten metadata map",
            )?;
            let next = end.min(after);
            exact(
                source,
                run.physical.byte(header) + cursor - start,
                &mut bytes[(cursor - offset) as usize..(next - offset) as usize],
            )?;
            cursor = next;
        }
        require(cursor == end, "jfs: incomplete metadata map")
    }
}

struct Walker<'a, S> {
    source: &'a mut S,
    header: &'a Header,
    seen: HashSet<u64>,
    levels: BTreeMap<usize, (u64, u64, u64)>,
    map: Map,
}
impl<S: BlockSource> Walker<'_, S> {
    fn visit(
        &mut self,
        bytes: &[u8],
        address: Option<Physical>,
        depth: usize,
    ) -> Result<Option<u64>> {
        require(
            depth <= 8 && bytes.len() >= 32,
            "jfs: xtree depth/header size",
        )?;
        let flag = bytes[16];
        let leaf = flag & 2 != 0;
        let next = usize::from(le16(bytes, 18)?);
        let maximum = usize::from(le16(bytes, 20)?);
        require(
            flag & !0xb7 == 0
                && flag & 6 == if leaf { 2 } else { 4 }
                && flag & 1 == u8::from(address.is_none())
                && next >= 2
                && next <= maximum
                && maximum * 16 <= bytes.len()
                && if address.is_some() {
                    maximum == 256
                } else {
                    matches!(maximum, 6 | 10 | 18)
                },
            "jfs: xtree node flags/slots",
        )?;
        // Inline roots do not use sibling/self fields. In particular the
        // directory-index root retains bytes of the former inline table;
        // Linux xtInitRoot intentionally initializes only flags/counts.
        if let Some(expected) = address {
            require(
                Physical::parse(bytes, 24, self.header)? == expected,
                "jfs: xtree self descriptor",
            )?;
        }
        let mut first = None;
        let mut previous = None;
        let mut previous_end = 0;
        for index in 2..next {
            let at = index * 16;
            let logical = (u64::from(bytes[at + 3]) << 32) | u64::from(le32(bytes, at + 4)?);
            let physical = Physical::parse(bytes, at + 8, self.header)?;
            require(
                physical.count != 0
                    && previous.is_none_or(|previous| previous < logical)
                    && (!leaf || logical >= previous_end),
                "jfs: xtree key order/extent count",
            )?;
            if bytes[at] & !8 != 0 {
                return Err(FoldError::Unsupported(
                    "jfs: transient/compressed/COW extent flags",
                ));
            }
            first.get_or_insert(logical);
            previous = Some(logical);
            if leaf {
                require(self.map.runs.len() < MAX_RUNS, "jfs: extent count limit")?;
                previous_end = logical + u64::from(physical.count);
                require(
                    previous_end <= 1u64 << 40,
                    "jfs: logical extent exceeds 40-bit address space",
                )?;
                self.map.runs.push(Run {
                    logical,
                    physical,
                    unwritten: bytes[at] & 8 != 0,
                });
            } else {
                require(
                    bytes[at] == 0
                        && physical.length(self.header) == 4096
                        && self.seen.len() < MAX_PAGES
                        && self.seen.insert(physical.start),
                    "jfs: xtree child size/cycle/page limit",
                )?;
                self.map.pages.push(physical);
                let mut child = [0; 4096];
                exact(self.source, physical.byte(self.header), &mut child)?;
                require(
                    self.visit(&child, Some(physical), depth + 1)? == Some(logical),
                    "jfs: xtree index lower key",
                )?;
            }
        }
        require(leaf || next > 2, "jfs: empty internal xtree")?;
        if let Some(address) = address {
            let first = first.ok_or(FoldError::Corrupt("jfs: empty external xtree"))?;
            let last = previous.ok_or(FoldError::Corrupt("jfs: empty external xtree"))?;
            if let Some((previous, next, previous_key)) = self.levels.get(&depth) {
                require(
                    *next == address.start && le64(bytes, 8)? == *previous && *previous_key < first,
                    "jfs: xtree sibling links/key order",
                )?;
            } else {
                require(le64(bytes, 8)? == 0, "jfs: xtree first sibling")?;
            }
            self.levels
                .insert(depth, (address.start, le64(bytes, 0)?, last));
        }
        Ok(first)
    }
}
