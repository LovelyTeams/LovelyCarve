use crate::{
    allocation::Allocation,
    dtree::Directory,
    exact,
    inodes::{Inode, Inodes},
    le32,
    metadata::{Header, Physical},
    require,
    xtree::Map,
};
use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};
use std::collections::{HashSet, VecDeque};

/// Immutable Linux JFS volume. IAG, dmap, inode and recent-content caches have
/// fixed entry bounds; no cache grows with a whole-volume file traversal.
pub struct Jfs<S> {
    source: S,
    header: Header,
    allocation: Allocation,
    inodes: Inodes,
    inode_cache: VecDeque<Inode>,
    directory: Option<(u32, Directory)>,
    file_map: Option<(u32, Map)>,
}
impl<S: BlockSource> Jfs<S> {
    pub fn header(&self) -> &Header {
        &self.header
    }
    fn inode(&mut self, node: NodeId) -> Result<Inode> {
        let number =
            u32::try_from(node).map_err(|_| FoldError::Corrupt("jfs: inode number overflow"))?;
        if let Some(inode) = self.inode_cache.iter().find(|inode| inode.number == number) {
            return Ok(inode.clone());
        }
        let inode =
            self.inodes
                .read(&mut self.source, &self.header, &mut self.allocation, number)?;
        if self.inode_cache.len() == 128 {
            self.inode_cache.pop_front();
        }
        self.inode_cache.push_back(inode.clone());
        Ok(inode)
    }
    fn ensure_directory(&mut self, node: NodeId) -> Result<()> {
        let inode = self.inode(node)?;
        require(
            inode.kind() == FileKind::Directory,
            "jfs: listing/lookup on non-directory",
        )?;
        if self
            .directory
            .as_ref()
            .is_some_and(|(cached, _)| *cached == inode.number)
        {
            return Ok(());
        }
        let directory = Directory::read(&mut self.source, &self.header, &inode.raw[224..512])?;
        let indexed = self.header.flags & 0x00200000 != 0;
        let next_index = le32(&inode.raw, 120)?;
        let index_map = if indexed && next_index > 14 {
            Some(Map::parse(
                &mut self.source,
                &self.header,
                &inode.raw[128..224],
            )?)
        } else {
            None
        };
        if indexed {
            require(
                next_index >= 2,
                "jfs: invalid persistent directory next-index",
            )?;
            let mut seen = HashSet::new();
            for entry in &directory.entries {
                let index = entry
                    .index
                    .ok_or(FoldError::Corrupt("jfs: missing persistent index"))?;
                require(
                    index >= 2 && index < next_index && seen.insert(index),
                    "jfs: directory index bounds/duplicate",
                )?;
                let offset = u64::from(index - 2) * 8;
                let mut slot = [0; 8];
                match &index_map {
                    Some(map) => map.exact(
                        &mut self.source,
                        &self.header,
                        inode.length,
                        offset,
                        &mut slot,
                    )?,
                    None => {
                        require(offset + 8 <= 96, "jfs: inline directory index bounds")?;
                        slot.copy_from_slice(
                            &inode.raw[128 + offset as usize..136 + offset as usize],
                        );
                    }
                }
                let page = (u64::from(slot[3]) << 32) | u64::from(le32(&slot, 4)?);
                require(
                    slot[1] == 1 && slot[2] == entry.slot && page == entry.page,
                    "jfs: directory index/leaf position mismatch",
                )?;
            }
        }
        let index_allocations = index_map.iter().flat_map(|map| {
            map.pages
                .iter()
                .copied()
                .chain(map.runs.iter().map(|run| run.physical))
        });
        inode.verify_allocation(
            &mut self.source,
            &self.header,
            &mut self.allocation,
            directory.pages.iter().copied().chain(index_allocations),
        )?;
        self.directory = Some((inode.number, directory));
        Ok(())
    }
    fn directory(&self) -> Result<&Directory> {
        self.directory
            .as_ref()
            .map(|(_, directory)| directory)
            .ok_or(FoldError::Corrupt("jfs: missing directory cache"))
    }
}
impl<S: BlockSource> FoldFrontend<S> for Jfs<S> {
    const TAG: &'static str = "jfs";
    fn probe(source: &mut S) -> Result<bool> {
        Ok(Header::read(source).is_ok())
    }
    fn open(mut source: S, _context: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header = Header::read(&mut source)?;
        let mut bmap_raw = [0; 512];
        exact(&mut source, 45056 + 2 * 512, &mut bmap_raw)?;
        let mut fileset_raw = [0; 512];
        exact(&mut source, 45056 + 16 * 512, &mut fileset_raw)?;
        let stamp = le32(&fileset_raw, 0)?;
        let bmap_inode = Inode::decode(bmap_raw, 2, 1, stamp, &header)?;
        let fileset_inode = Inode::decode(fileset_raw, 16, 1, stamp, &header)?;
        let bmap = bmap_inode.data_map(&mut source, &header)?;
        let mut allocation =
            Allocation::open(&mut source, &header, bmap.clone(), bmap_inode.length)?;
        bmap_inode.verify_allocation(
            &mut source,
            &header,
            &mut allocation,
            bmap.pages
                .iter()
                .copied()
                .chain(bmap.runs.iter().map(|run| run.physical)),
        )?;
        for inode in [&bmap_inode, &fileset_inode] {
            let extent = Physical::parse(&inode.raw, 16, &header)?;
            require(
                extent.byte(&header) == 45056 && extent.length(&header) == 16384,
                "jfs: primary aggregate inode extent identity",
            )?;
            allocation.check(&mut source, &header, extent)?;
        }
        let fileset_map = fileset_inode.data_map(&mut source, &header)?;
        let inodes = Inodes::open(
            &mut source,
            &header,
            fileset_inode,
            fileset_map,
            &mut allocation,
        )?;
        let mut reader = Self {
            source,
            header,
            allocation,
            inodes,
            inode_cache: VecDeque::new(),
            directory: None,
            file_map: None,
        };
        let root = reader.inode(2)?;
        require(
            root.kind() == FileKind::Directory && le32(&root.raw, 244)? == 2,
            "jfs: root directory/parent identity",
        )?;
        reader.ensure_directory(2)?;
        Ok(reader)
    }
    fn root(&self) -> NodeId {
        2
    }
    fn lookup(
        &mut self,
        directory: NodeId,
        name: &str,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        self.ensure_directory(directory)?;
        let units: Vec<u16> = name.encode_utf16().collect();
        let entries = &self.directory()?.entries;
        Ok(entries
            .binary_search_by(|entry| entry.units.cmp(&units))
            .ok()
            .map(|index| u64::from(entries[index].inode)))
    }
    fn read_dir(
        &mut self,
        directory: NodeId,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Vec<DirEntry>> {
        self.ensure_directory(directory)?;
        let entries: Vec<_> = self
            .directory()?
            .entries
            .iter()
            .map(|entry| (entry.inode, entry.name.clone()))
            .collect();
        entries
            .into_iter()
            .map(|(number, name)| {
                let inode = self.inode(u64::from(number))?;
                if inode.kind() == FileKind::Directory {
                    require(
                        u64::from(le32(&inode.raw, 244)?) == directory
                            && u64::from(number) != directory,
                        "jfs: directory parent/cycle mismatch",
                    )?;
                }
                Ok(DirEntry {
                    name,
                    node: u64::from(number),
                    kind: inode.kind(),
                })
            })
            .collect()
    }
    fn metadata(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.inode(node)?;
        Ok(Metadata {
            kind: inode.kind(),
            size: inode.length,
            mode: inode.mode & 0xffff,
        })
    }
    fn timestamps(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        self.inode(node)?.times()
    }
    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let inode = self.inode(node)?;
        require(
            matches!(inode.kind(), FileKind::Regular | FileKind::Symlink),
            "jfs: inode has no regular content",
        )?;
        if let Some(link) = inode.inline_link()? {
            inode.verify_allocation(
                &mut self.source,
                &self.header,
                &mut self.allocation,
                std::iter::empty(),
            )?;
            let count = inode.length.saturating_sub(offset).min(bytes.len() as u64) as usize;
            if count != 0 {
                bytes[..count].copy_from_slice(&link[offset as usize..offset as usize + count]);
            }
            return Ok(count);
        }
        if self
            .file_map
            .as_ref()
            .is_none_or(|(cached, _)| *cached != inode.number)
        {
            let map = inode.data_map(&mut self.source, &self.header)?;
            inode.verify_allocation(
                &mut self.source,
                &self.header,
                &mut self.allocation,
                map.pages
                    .iter()
                    .copied()
                    .chain(map.runs.iter().map(|run| run.physical)),
            )?;
            self.file_map = Some((inode.number, map));
        }
        let (_, map) = self
            .file_map
            .as_ref()
            .ok_or(FoldError::Corrupt("jfs: missing file map"))?;
        map.read(&mut self.source, &self.header, inode.length, offset, bytes)
    }
    fn read_link(
        &mut self,
        node: NodeId,
        context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<Vec<u8>>> {
        let inode = self.inode(node)?;
        if inode.kind() != FileKind::Symlink {
            return Ok(None);
        }
        require(inode.length <= 1024 * 1024, "jfs: symlink length limit")?;
        let mut bytes = vec![0; inode.length as usize];
        require(
            self.read_at(node, 0, &mut bytes, context)? == bytes.len() && !bytes.contains(&0),
            "jfs: short/NUL symlink content",
        )?;
        Ok(Some(bytes))
    }
}
