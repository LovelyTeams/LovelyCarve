//! Cluster-based extent trees, including sparse and unwritten regions. File,
//! xattr and directory-index roots use the same checked record traversal.
use crate::{
    exact,
    header::{Header, Record},
    inode::Inode,
    le16, le32, le64, require,
};
use lamfold::{BlockSource, FileKind, FoldError, Result};
use std::collections::HashSet;

pub(crate) struct Root<'a> {
    pub raw: &'a [u8],
    pub offset: usize,
    pub end: usize,
    pub last_leaf: u64,
}
#[derive(Clone, Debug)]
pub(crate) struct Extent {
    pub logical: u64,
    pub physical: u64,
    pub length: u64,
    pub flags: u8,
}
pub(crate) struct Tree {
    pub extents: Vec<Extent>,
    pub metadata: HashSet<u64>,
    leaves: Vec<(u64, u64)>,
    pub clusters: u64,
}
impl Tree {
    pub fn parse(source: &mut impl BlockSource, header: &Header, root: Root<'_>) -> Result<Self> {
        let mut tree = Self {
            extents: Vec::new(),
            metadata: HashSet::new(),
            leaves: Vec::new(),
            clusters: 0,
        };
        let list = root
            .raw
            .get(root.offset..root.end)
            .ok_or(FoldError::Corrupt("ocfs2: extent-root bounds"))?;
        let depth = le16(list, 0)?;
        require(depth <= 5, "ocfs2: extent-tree depth limit")?;
        tree.walk(source, header, list, depth, 0, 1u64 << 32)?;
        for pair in tree.extents.windows(2) {
            require(
                pair[0].logical + pair[0].length <= pair[1].logical,
                "ocfs2: overlapping logical extents",
            )?;
        }
        for (index, (_, next)) in tree.leaves.iter().enumerate() {
            require(
                *next == tree.leaves.get(index + 1).map_or(0, |leaf| leaf.0),
                "ocfs2: leaf chain/order",
            )?;
        }
        require(
            root.last_leaf == tree.leaves.last().map_or(0, |leaf| leaf.0),
            "ocfs2: last extent leaf identity",
        )?;
        let mut physical: Vec<_> = tree.extents.iter().collect();
        physical.sort_by_key(|extent| extent.physical);
        let mut maximum_end = 0;
        let mut nonref_end = 0;
        let mut ranges = Vec::new();
        for extent in physical {
            require(
                extent.physical
                    >= if extent.flags & 2 != 0 {
                        nonref_end
                    } else {
                        maximum_end
                    },
                "ocfs2: non-refcounted physical overlap",
            )?;
            maximum_end = maximum_end.max(extent.physical + extent.length);
            if extent.flags & 2 == 0 {
                nonref_end = nonref_end.max(extent.physical + extent.length);
            }
            ranges.push((extent.physical, maximum_end));
        }
        for block in &tree.metadata {
            let at = *block * header.block_size as u64;
            let index = ranges.partition_point(|range| range.0 <= at);
            require(
                index
                    .checked_sub(1)
                    .is_none_or(|index| ranges[index].1 <= at),
                "ocfs2: extent metadata/data overlap",
            )?;
        }
        Ok(tree)
    }
    fn walk(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        list: &[u8],
        depth: u16,
        lower: u64,
        upper: u64,
    ) -> Result<()> {
        require(le16(list, 0)? == depth, "ocfs2: child extent depth")?;
        let capacity = le16(list, 2)? as usize;
        let used = le16(list, 4)? as usize;
        require(
            used <= capacity && 16 + capacity * 16 <= list.len(),
            "ocfs2: extent record capacity",
        )?;
        let mut previous_end = lower;
        for index in 0..used {
            let record = &list[16 + index * 16..32 + index * 16];
            let logical = le32(record, 0)? as u64;
            let count = if depth == 0 {
                le16(record, 4)? as u64
            } else {
                le32(record, 4)? as u64
            };
            let block = le64(record, 8)?;
            if count == 0 {
                require(
                    depth == 0 && index == 0 && block == 0,
                    "ocfs2: illegal empty extent record",
                )?;
                continue;
            }
            require(
                logical >= previous_end
                    && logical.checked_add(count).is_some_and(|end| end <= upper),
                "ocfs2: extent separator/range",
            )?;
            header.block_offset(block)?;
            if depth == 0 {
                let flags = record[7];
                if flags & !3 != 0 {
                    return Err(FoldError::Unsupported("ocfs2: unknown extent flags"));
                }
                require(
                    flags & 1 == 0 || header.readonly_compatible & 1 != 0,
                    "ocfs2: unwritten feature mismatch",
                )?;
                require(
                    flags & 2 == 0 || header.incompatible & 0x1000 != 0,
                    "ocfs2: refcounted feature mismatch",
                )?;
                let physical = block * header.block_size as u64;
                let length = count * header.cluster_size as u64;
                require(
                    physical.is_multiple_of(header.cluster_size as u64)
                        && physical
                            .checked_add(length)
                            .is_some_and(|end| end <= header.blocks * header.block_size as u64),
                    "ocfs2: physical extent bounds/alignment",
                )?;
                require(
                    self.extents.len() < 1_000_000,
                    "ocfs2: extent record work limit",
                )?;
                self.extents.push(Extent {
                    logical: logical * header.cluster_size as u64,
                    physical,
                    length,
                    flags,
                });
                self.clusters += count;
            } else {
                require(
                    self.metadata.len() < 131072 && self.metadata.insert(block),
                    "ocfs2: extent metadata cycle/alias/work limit",
                )?;
                let raw = header.record(source, block, Record::Extent)?;
                if depth == 1 {
                    self.leaves.push((block, le64(&raw, 40)?));
                }
                self.walk(
                    source,
                    header,
                    &raw[48..],
                    depth - 1,
                    logical,
                    logical + count,
                )?;
            }
            previous_end = logical + count;
        }
        Ok(())
    }
}
pub(crate) struct Stream {
    pub size: u64,
    pub tree: Tree,
    inline: Option<Vec<u8>>,
    dense: bool,
}
impl Stream {
    pub fn inode(source: &mut impl BlockSource, header: &Header, inode: &Inode) -> Result<Self> {
        if let Some(raw) = inode.inline()? {
            return Ok(Self {
                size: inode.size,
                tree: Tree {
                    extents: Vec::new(),
                    metadata: HashSet::new(),
                    leaves: Vec::new(),
                    clusters: 0,
                },
                inline: Some(raw.to_vec()),
                dense: true,
            });
        }
        require(
            inode.flags & 0x0cc0 == 0,
            "ocfs2: bitmap/chain/dealloc inode is not a regular stream",
        )?;
        let tree = Tree::parse(
            source,
            header,
            Root {
                raw: &inode.raw,
                offset: 192,
                end: inode.data_end(),
                last_leaf: inode.last_extent,
            },
        )?;
        require(
            tree.clusters == inode.clusters as u64,
            "ocfs2: extent/inode allocated cluster count",
        )?;
        for extent in &tree.extents {
            let at = inode.address * header.block_size as u64;
            require(
                !(extent.physical <= at && at < extent.physical + extent.length),
                "ocfs2: inode/data overlap",
            )?;
        }
        let dense = inode.kind() == FileKind::Directory
            || header.incompatible & 0x10 == 0
            || inode.flags & 0x100 != 0;
        let stream = Self {
            size: inode.size,
            tree,
            inline: None,
            dense,
        };
        if dense {
            stream.check_dense()?;
        }
        Ok(stream)
    }
    pub fn external(tree: Tree, size: u64) -> Result<Self> {
        let stream = Self {
            size,
            tree,
            inline: None,
            dense: true,
        };
        stream.check_dense()?;
        Ok(stream)
    }
    fn check_dense(&self) -> Result<()> {
        let mut end = 0;
        for extent in &self.tree.extents {
            require(
                extent.logical == end && extent.flags & 1 == 0,
                "ocfs2: gap/unwritten extent in dense metadata stream",
            )?;
            end += extent.length;
        }
        require(end >= self.size, "ocfs2: incomplete dense stream")
    }
    pub fn physical_at(&self, offset: u64) -> Result<Option<u64>> {
        let index = self
            .tree
            .extents
            .partition_point(|extent| extent.logical <= offset);
        let Some(extent) = index
            .checked_sub(1)
            .and_then(|index| self.tree.extents.get(index))
        else {
            return Ok(None);
        };
        if offset >= extent.logical + extent.length || extent.flags & 1 != 0 {
            return Ok(None);
        }
        Ok(Some(extent.physical + offset - extent.logical))
    }
    pub fn read(
        &self,
        source: &mut impl BlockSource,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize> {
        let count = self.size.saturating_sub(offset).min(bytes.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        if let Some(inline) = &self.inline {
            bytes[..count].copy_from_slice(&inline[offset as usize..offset as usize + count]);
            return Ok(count);
        }
        let mut copied = 0;
        while copied < count {
            let at = offset + copied as u64;
            let index = self
                .tree
                .extents
                .partition_point(|extent| extent.logical <= at);
            let current = index
                .checked_sub(1)
                .and_then(|index| self.tree.extents.get(index))
                .filter(|extent| at < extent.logical + extent.length);
            let end = current.map_or_else(
                || {
                    self.tree
                        .extents
                        .get(index)
                        .map_or(self.size, |extent| extent.logical)
                },
                |extent| extent.logical + extent.length,
            );
            let take = (end - at).min((count - copied) as u64) as usize;
            require(take != 0, "ocfs2: non-advancing logical map")?;
            if let Some(extent) = current.filter(|extent| extent.flags & 1 == 0) {
                exact(
                    source,
                    extent.physical + at - extent.logical,
                    &mut bytes[copied..copied + take],
                )?;
            } else {
                require(!self.dense, "ocfs2: hole in dense stream")?;
                bytes[copied..copied + take].fill(0);
            }
            copied += take;
        }
        Ok(count)
    }
}
