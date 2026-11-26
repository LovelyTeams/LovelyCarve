use std::collections::HashSet;

use lamfold::{BlockSource, FileKind, FoldError, NodeId, Result};

use crate::{Hpfs, SECTOR_SIZE, le16, le32, require};

const ANODE_MAGIC: u32 = 0x37e40aae;
const MAX_ALLOC_DEPTH: usize = 16;

#[derive(Debug, Clone, Copy)]
pub(crate) struct Extent {
    logical: u32,
    physical: u32,
    sectors: u32,
}

impl<S: BlockSource> Hpfs<S> {
    pub(crate) fn read_file(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize> {
        let record = self.record(node)?;
        if record.kind() != FileKind::Regular {
            return Err(FoldError::IsDirectory);
        }
        let length = u64::from(record.size)
            .saturating_sub(offset)
            .min(bytes.len() as u64) as usize;
        if length == 0 {
            return Ok(0);
        }
        if self
            .last_map
            .as_ref()
            .is_none_or(|(fnode, _)| *fnode != record.fnode)
        {
            let fnode = self.read_fnode(record.fnode)?;
            require(
                le16(&fnode, 54) & 0x100 == 0 && le32(&fnode, 160) == record.size,
                "HPFS directory and fnode disagree on file size or type",
            )?;
            let mut extents = Vec::new();
            self.walk_allocation(
                record.fnode,
                &fnode[56..160],
                0,
                &mut HashSet::new(),
                &mut extents,
            )?;
            let allocated = extents.last().map_or(0, |end| end.logical + end.sectors);
            require(
                u64::from(allocated) * SECTOR_SIZE as u64 >= u64::from(record.size),
                "HPFS allocation ends before logical EOF",
            )?;
            let mut physical = extents.clone();
            physical.sort_by_key(|extent| extent.physical);
            require(
                physical
                    .windows(2)
                    .all(|pair| pair[0].physical + pair[0].sectors <= pair[1].physical),
                "HPFS overlapping physical file extents",
            )?;
            self.last_map = Some((record.fnode, extents));
        }
        let mut consumed = 0;
        let sector_size = SECTOR_SIZE as u64;
        while consumed < length {
            let file_position = offset + consumed as u64;
            let file_sector = (file_position / sector_size) as u32;
            let extents = &self.last_map.as_ref().unwrap().1;
            let index =
                extents.partition_point(|extent| extent.logical + extent.sectors <= file_sector);
            let extent = extents
                .get(index)
                .ok_or(FoldError::Corrupt("HPFS missing allocated file bytes"))?;
            let within = file_position - u64::from(extent.logical) * sector_size;
            let count = (u64::from(extent.sectors) * sector_size - within)
                .min((length - consumed) as u64) as usize;
            self.read_bytes(
                u64::from(extent.physical) * sector_size + within,
                &mut bytes[consumed..consumed + count],
            )?;
            consumed += count;
        }
        Ok(length)
    }

    fn walk_allocation(
        &mut self,
        parent: u32,
        tree: &[u8],
        depth: usize,
        reached: &mut HashSet<u32>,
        extents: &mut Vec<Extent>,
    ) -> Result<()> {
        require(depth < MAX_ALLOC_DEPTH, "HPFS allocation tree depth")?;
        let internal = tree[0] & 0x80 != 0;
        let width = if internal { 8 } else { 12 };
        let count = usize::from(tree[5]);
        let capacity = (tree.len() - 8) / width;
        let end = 8 + count * width;
        require(
            count <= capacity
                && count + usize::from(tree[4]) == capacity
                && usize::from(le16(tree, 6)) == end,
            "HPFS allocation tree entry bounds",
        )?;
        let mut previous_limit = 0;
        for item in tree[8..end].chunks_exact(width) {
            let logical = le32(item, 0);
            if internal {
                require(
                    logical > previous_limit,
                    "HPFS unordered allocation tree boundaries",
                )?;
                let sector = le32(item, 4);
                require(
                    reached.insert(sector),
                    "HPFS cyclic or repeated allocation subtree",
                )?;
                let child = self.sector::<SECTOR_SIZE>(sector)?;
                require(
                    le32(&child, 0) == ANODE_MAGIC
                        && le32(&child, 4) == sector
                        && le32(&child, 8) == parent,
                    "HPFS allocation node signature or pointer",
                )?;
                self.walk_allocation(sector, &child[12..500], depth + 1, reached, extents)?;
                require(
                    extents
                        .last()
                        .is_some_and(|extent| extent.logical + extent.sectors <= logical),
                    "HPFS extent exceeds its allocation subtree boundary",
                )?;
                previous_limit = logical;
            } else {
                let sectors = le32(item, 4);
                let physical = le32(item, 8);
                let expected = extents
                    .last()
                    .map_or(0, |extent| extent.logical + extent.sectors);
                require(
                    logical == expected
                        && sectors != 0
                        && logical.checked_add(sectors).is_some()
                        && physical >= 20
                        && physical
                            .checked_add(sectors)
                            .is_some_and(|end| end <= self.header.sectors),
                    "HPFS noncontiguous or out-of-range extent",
                )?;
                extents.push(Extent {
                    logical,
                    physical,
                    sectors,
                });
            }
        }
        Ok(())
    }
}
