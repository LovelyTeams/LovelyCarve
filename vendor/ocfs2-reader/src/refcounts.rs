//! Reference-count records are indexed by physical cluster position. Index
//! leaves point to metadata blocks, not cluster-aligned file-data extents.
use crate::{
    allocation::Allocation,
    extents::Tree,
    header::{Header, Record},
    inode::Inode,
    le16, le32, le64, require,
};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::HashSet;

struct Count {
    first: u64,
    end: u64,
    references: u32,
}
pub(crate) struct Refcounts {
    ranges: Vec<Count>,
    pub metadata: HashSet<u64>,
}
struct Walker<'a, S: BlockSource> {
    source: &'a mut S,
    header: &'a Header,
    allocation: Option<&'a Allocation>,
    root: u64,
    generation: u32,
    result: Refcounts,
    pointers: Vec<(u64, u64)>,
    leaves: Vec<(u64, u64)>,
}
impl Refcounts {
    pub fn load(
        source: &mut impl BlockSource,
        header: &Header,
        inode: &Inode,
        allocation: Option<&Allocation>,
    ) -> Result<Option<Self>> {
        if inode.dynamic & 16 == 0 {
            require(
                inode.refcount == 0,
                "ocfs2: refcount root without inode feature",
            )?;
            return Ok(None);
        }
        require(inode.refcount != 0, "ocfs2: missing refcount root")?;
        let raw = header.record(source, inode.refcount, Record::Refcount)?;
        require(
            le64(&raw, 24)? == 0 && le32(&raw, 48)? != 0,
            "ocfs2: refcount root parent/inode count",
        )?;
        let mut walker = Walker {
            source,
            header,
            allocation,
            root: inode.refcount,
            generation: le32(&raw, 64)?,
            result: Self {
                ranges: Vec::new(),
                metadata: HashSet::new(),
            },
            pointers: Vec::new(),
            leaves: Vec::new(),
        };
        walker.record(inode.refcount, Record::Refcount, &raw)?;
        match le32(&raw, 52)? {
            0 => {
                require(le32(&raw, 56)? == 0, "ocfs2: inline refcount allocation")?;
                walker.records(&raw, 0, 1u64 << 32)?;
            }
            2 => {
                let depth = le16(&raw, 128)?;
                require(depth <= 5, "ocfs2: refcount index depth")?;
                walker.index(&raw[128..], depth, 0, 1u64 << 32)?;
                require(
                    walker.pointers.len() as u64 == le32(&raw, 56)? as u64,
                    "ocfs2: refcount leaf count",
                )?;
                for pair in walker.leaves.windows(2) {
                    require(pair[0].1 == pair[1].0, "ocfs2: refcount index leaf chain")?;
                }
                require(
                    walker.leaves.last().map_or(0, |leaf| leaf.0) == le64(&raw, 40)?
                        && walker.leaves.last().is_none_or(|leaf| leaf.1 == 0),
                    "ocfs2: refcount last extent leaf",
                )?;
                let pointers = std::mem::take(&mut walker.pointers);
                for (index, (lower, block)) in pointers.iter().enumerate() {
                    let upper = pointers.get(index + 1).map_or(1u64 << 32, |next| next.0);
                    let raw = header.record(walker.source, *block, Record::Refcount)?;
                    walker.record(*block, Record::Refcount, &raw)?;
                    require(
                        le32(&raw, 52)? == 1
                            && le64(&raw, 24)? == walker.root
                            && le32(&raw, 60)? as u64 == *lower
                            && le32(&raw, 64)? == walker.generation,
                        "ocfs2: refcount leaf parent/key/generation",
                    )?;
                    walker.records(&raw, *lower, upper)?;
                }
            }
            _ => return Err(FoldError::Corrupt("ocfs2: refcount root flags")),
        }
        walker.result.ranges.sort_by_key(|range| range.first);
        for pair in walker.result.ranges.windows(2) {
            require(
                pair[0].end <= pair[1].first,
                "ocfs2: overlapping refcount records",
            )?;
        }
        Ok(Some(walker.result))
    }
    pub fn check(references: Option<&Self>, header: &Header, tree: &Tree) -> Result<()> {
        let shared: Vec<_> = tree
            .extents
            .iter()
            .filter(|extent| extent.flags & 2 != 0)
            .collect();
        if let Some(references) = references {
            require(
                tree.metadata.is_disjoint(&references.metadata),
                "ocfs2: data/refcount metadata alias",
            )?;
        }
        if shared.is_empty() {
            return Ok(());
        }
        let references = references.ok_or(FoldError::Corrupt(
            "ocfs2: shared data without reference counts",
        ))?;
        let mut events = Vec::with_capacity(shared.len() * 2);
        for extent in shared {
            let first = extent.physical / header.cluster_size as u64;
            let end = first + extent.length / header.cluster_size as u64;
            events.push((first, 1i64));
            events.push((end, -1i64));
        }
        events.sort_unstable();
        let mut position = 0;
        let mut previous = events[0].0;
        let mut count = 0i64;
        while position < events.len() {
            let at = events[position].0;
            if previous < at && count > 0 {
                let mut cluster = previous;
                while cluster < at {
                    let index = references
                        .ranges
                        .partition_point(|range| range.first <= cluster);
                    let range = index
                        .checked_sub(1)
                        .and_then(|index| references.ranges.get(index))
                        .ok_or(FoldError::Corrupt(
                            "ocfs2: missing physical refcount coverage",
                        ))?;
                    require(
                        range.end > cluster && range.references as i64 >= count,
                        "ocfs2: insufficient refcount/physical coverage",
                    )?;
                    cluster = range.end.min(at);
                }
            }
            while position < events.len() && events[position].0 == at {
                count += events[position].1;
                position += 1;
            }
            require(count >= 0, "ocfs2: invalid shared-extent accounting")?;
            previous = at;
        }
        require(count == 0, "ocfs2: incomplete shared-extent accounting")
    }
}
impl<S: BlockSource> Walker<'_, S> {
    fn record(&mut self, block: u64, kind: Record, raw: &[u8]) -> Result<()> {
        require(
            self.result.metadata.len() < 131072 && self.result.metadata.insert(block),
            "ocfs2: refcount metadata cycle/alias/work limit",
        )?;
        if let Some(allocation) = self.allocation {
            allocation.check_raw(self.header, block, kind, raw)?;
        }
        Ok(())
    }
    fn records(&mut self, raw: &[u8], lower: u64, upper: u64) -> Result<()> {
        let capacity = le16(raw, 128)? as usize;
        let used = le16(raw, 130)? as usize;
        require(
            capacity == (raw.len() - 144) / 16 && used <= capacity,
            "ocfs2: refcount array capacity",
        )?;
        let mut previous = lower;
        for record in raw[144..144 + used * 16].as_chunks::<16>().0 {
            let first = le64(record, 0)?;
            let length = le32(record, 8)? as u64;
            let references = le32(record, 12)?;
            let end = first
                .checked_add(length)
                .ok_or(FoldError::Corrupt("ocfs2: refcount range overflow"))?;
            require(
                length != 0
                    && references != 0
                    && first >= previous
                    && end <= upper
                    && end <= self.header.clusters as u64,
                "ocfs2: refcount physical range/order",
            )?;
            require(
                self.result.ranges.len() < 1_000_000,
                "ocfs2: refcount record work limit",
            )?;
            if let Some(allocation) = self.allocation {
                allocation.check_data(
                    self.header,
                    first * self.header.cluster_size as u64,
                    length * self.header.cluster_size as u64,
                )?;
            }
            self.result.ranges.push(Count {
                first,
                end,
                references,
            });
            previous = end;
        }
        Ok(())
    }
    fn index(&mut self, raw: &[u8], depth: u16, lower: u64, upper: u64) -> Result<()> {
        require(le16(raw, 0)? == depth, "ocfs2: refcount child tree depth")?;
        let capacity = le16(raw, 2)? as usize;
        let used = le16(raw, 4)? as usize;
        require(
            used <= capacity && 16 + capacity * 16 <= raw.len(),
            "ocfs2: refcount index capacity",
        )?;
        let mut previous = lower;
        for record in raw[16..16 + used * 16].as_chunks::<16>().0 {
            let key = le32(record, 0)? as u64;
            let block = le64(record, 8)?;
            let length = if depth == 0 {
                le16(record, 4)? as u64
            } else {
                le32(record, 4)? as u64
            };
            require(
                length != 0 && key >= previous && key + length <= upper,
                "ocfs2: refcount index key bounds/order",
            )?;
            self.header.block_offset(block)?;
            if depth == 0 {
                require(
                    length == 1 && record[7] == 0 && self.pointers.len() < 131072,
                    "ocfs2: refcount leaf pointer layout/work limit",
                )?;
                self.pointers.push((key, block));
            } else {
                let child = self.header.record(self.source, block, Record::Extent)?;
                self.record(block, Record::Extent, &child)?;
                if depth == 1 {
                    self.leaves.push((block, le64(&child, 40)?));
                }
                self.index(&child[48..], depth - 1, key, key + length)?;
            }
            previous = key + length;
        }
        Ok(())
    }
}
