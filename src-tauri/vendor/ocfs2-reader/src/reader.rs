use crate::{
    allocation::{Allocation, Key},
    directory::Directory,
    extents::Stream,
    header::{Header, Record},
    inode::Inode,
    journal, require,
};
use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};
use std::collections::{HashMap, VecDeque};

/// Immutable OCFS2 volume. Allocation and slot journals are validated before
/// ordinary file reads. Caches are per reader and bounded to one stream/128 inodes.
pub struct Ocfs2<S: BlockSource> {
    source: S,
    header: Header,
    allocation: Option<Allocation>,
    inodes: VecDeque<Inode>,
    stream: Option<(u64, Stream)>,
}
impl<S: BlockSource> Ocfs2<S> {
    pub fn header(&self) -> &Header {
        &self.header
    }
    pub fn attributes(&mut self, node: NodeId) -> Result<Vec<crate::Attribute>> {
        let inode = self.inode(node)?;
        self.map(&inode)?;
        crate::attributes::read(
            &mut self.source,
            &self.header,
            &inode,
            self.allocation.as_ref().ok_or(FoldError::Corrupt(
                "ocfs2: missing attribute allocation state",
            ))?,
            &self
                .stream
                .as_ref()
                .ok_or(FoldError::Corrupt("ocfs2: missing owner stream"))?
                .1
                .tree,
        )
    }
    fn inode(&mut self, node: u64) -> Result<Inode> {
        if let Some(inode) = self.inodes.iter().find(|inode| inode.address == node) {
            return Ok(inode.clone());
        }
        let inode = Inode::read(&mut self.source, &self.header, node)?;
        if let Some(allocation) = &self.allocation {
            allocation.check_raw(&self.header, node, Record::Inode, &inode.raw)?;
        }
        if self.inodes.len() == 128 {
            self.inodes.pop_front();
        }
        self.inodes.push_back(inode.clone());
        Ok(inode)
    }
    fn map(&mut self, inode: &Inode) -> Result<()> {
        if self
            .stream
            .as_ref()
            .is_some_and(|(owner, _)| *owner == inode.address)
        {
            return Ok(());
        }
        let stream = Stream::inode(&mut self.source, &self.header, inode)?;
        if let Some(allocation) = &self.allocation {
            for block in &stream.tree.metadata {
                allocation.check_record(&mut self.source, &self.header, *block, Record::Extent)?;
            }
            for extent in &stream.tree.extents {
                allocation.check_data(&self.header, extent.physical, extent.length)?;
            }
        }
        require(
            !stream
                .tree
                .extents
                .iter()
                .any(|extent| extent.flags & 2 != 0)
                || inode.dynamic & 16 != 0 && inode.refcount != 0,
            "ocfs2: shared extent without refcount root",
        )?;
        let references = crate::refcounts::Refcounts::load(
            &mut self.source,
            &self.header,
            inode,
            self.allocation.as_ref(),
        )?;
        crate::refcounts::Refcounts::check(references.as_ref(), &self.header, &stream.tree)?;
        self.stream = Some((inode.address, stream));
        Ok(())
    }
    fn directory(&mut self, inode: &Inode) -> Result<Directory> {
        self.map(inode)?;
        let directory = Directory::read(
            &mut self.source,
            &self.header,
            inode,
            &self
                .stream
                .as_ref()
                .ok_or(FoldError::Corrupt("ocfs2: missing directory stream"))?
                .1,
        )?;
        crate::index::validate(
            &mut self.source,
            &self.header,
            inode,
            &directory,
            self.allocation.as_ref(),
        )?;
        Ok(directory)
    }
    fn bytes(&mut self, inode: &Inode, maximum: u64) -> Result<Vec<u8>> {
        require(inode.size <= maximum, "ocfs2: system-file read work limit")?;
        self.map(inode)?;
        let mut raw = vec![0; inode.size as usize];
        require(
            self.stream
                .as_ref()
                .ok_or(FoldError::Corrupt("ocfs2: missing system stream"))?
                .1
                .read(&mut self.source, 0, &mut raw)?
                == raw.len(),
            "ocfs2: incomplete system-file read",
        )?;
        Ok(raw)
    }
}
impl<S: BlockSource> FoldFrontend<S> for Ocfs2<S> {
    const TAG: &'static str = "ocfs2";
    fn probe(source: &mut S) -> Result<bool> {
        for block in [512, 1024, 2048, 4096] {
            if source.len() < block as u64 * 3 {
                continue;
            }
            let mut raw = vec![0; block as usize];
            crate::exact(source, block as u64 * 2, &mut raw)?;
            if Header::inspect_block(&raw, block, source.len()).is_ok() {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn open(mut source: S, _context: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header = Header::inspect(&mut source)?;
        let mut reader = Self {
            source,
            header,
            allocation: None,
            inodes: VecDeque::new(),
            stream: None,
        };
        let system = reader.inode(reader.header.system)?;
        let entries = reader.directory(&system)?.entries;
        let mut systems = HashMap::new();
        for entry in entries {
            if matches!(entry.name.as_str(), "." | "..") {
                continue;
            }
            systems.insert(entry.name, reader.inode(entry.node)?);
        }
        let global = systems
            .get("global_bitmap")
            .ok_or(FoldError::Corrupt("ocfs2: missing global bitmap"))?;
        let mut pools = vec![(
            Key::GlobalInode,
            systems
                .get("global_inode_alloc")
                .ok_or(FoldError::Corrupt("ocfs2: missing global inode allocator"))?
                .clone(),
        )];
        for slot in 0..reader.header.slots {
            for (key, name) in [
                (Key::Inode(slot), "inode_alloc"),
                (Key::Extent(slot), "extent_alloc"),
            ] {
                pools.push((
                    key,
                    systems
                        .get(&format!("{name}:{slot:04}"))
                        .ok_or(FoldError::Corrupt("ocfs2: missing slot allocator"))?
                        .clone(),
                ));
            }
        }
        reader.allocation = Some(Allocation::load(
            &mut reader.source,
            &reader.header,
            global,
            pools,
        )?);
        reader.inodes.clear();
        reader.stream = None;
        for inode in systems.values() {
            reader.inode(inode.address)?;
        }
        let system = reader.inode(reader.header.system)?;
        require(
            reader
                .directory(&system)?
                .entries
                .iter()
                .any(|entry| entry.name == ".." && entry.node == system.address),
            "ocfs2: system directory parent identity",
        )?;
        let slot_map = systems
            .get("slot_map")
            .ok_or(FoldError::Corrupt("ocfs2: missing slot map"))?;
        let slots = reader.bytes(slot_map, 1024 * 1024)?;
        let extended = reader.header.incompatible & 0x100 != 0;
        let width = if extended { 8 } else { 2 };
        require(
            slots.len() >= width * reader.header.slots as usize,
            "ocfs2: slot map size",
        )?;
        for slot in 0..reader.header.slots as usize {
            let empty = if extended {
                slots[slot * width] == 0
            } else {
                crate::le16(&slots, slot * width)? == u16::MAX
            };
            if !empty {
                return Err(FoldError::Unsupported(
                    "ocfs2: occupied cluster slot; offline clean volume required",
                ));
            }
            let inode = systems
                .get(&format!("journal:{slot:04}"))
                .ok_or(FoldError::Corrupt("ocfs2: missing slot journal"))?;
            reader.map(inode)?;
            journal::check(
                &mut reader.source,
                &reader.header,
                inode,
                &reader
                    .stream
                    .as_ref()
                    .ok_or(FoldError::Corrupt("ocfs2: missing journal stream"))?
                    .1,
            )?;
            let local = systems
                .get(&format!("local_alloc:{slot:04}"))
                .ok_or(FoldError::Corrupt("ocfs2: missing local allocator"))?;
            require(
                crate::le32(&local.raw, 184)? == 0 && crate::le32(&local.raw, 188)? == 0,
                "ocfs2: unrecovered local allocation state",
            )?;
            let truncate = systems
                .get(&format!("truncate_log:{slot:04}"))
                .ok_or(FoldError::Corrupt("ocfs2: missing truncate log"))?;
            require(
                crate::le16(&truncate.raw, 194)? == 0,
                "ocfs2: nonempty truncate log requires recovery",
            )?;
        }
        let root = reader.inode(reader.header.root)?;
        require(
            reader
                .directory(&root)?
                .entries
                .iter()
                .any(|entry| entry.name == ".." && entry.node == root.address),
            "ocfs2: root parent identity",
        )?;
        Ok(reader)
    }
    fn root(&self) -> NodeId {
        self.header.root
    }
    fn lookup(
        &mut self,
        node: NodeId,
        name: &str,
        context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self
            .read_dir(node, context)?
            .into_iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.node))
    }
    fn read_dir(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        let inode = self.inode(node)?;
        let directory = self.directory(&inode)?;
        let mut output = Vec::new();
        let mut child_directories = std::collections::HashSet::new();
        for entry in directory.entries {
            let target = self.inode(entry.node)?;
            require(
                entry.kind.is_none_or(|kind| kind == target.kind()),
                "ocfs2: directory entry/inode kind mismatch",
            )?;
            if matches!(entry.name.as_str(), "." | "..") {
                require(
                    target.kind() == FileKind::Directory,
                    "ocfs2: dot/parent inode kind",
                )?;
            } else {
                if target.kind() == FileKind::Directory {
                    require(
                        entry.node != node && child_directories.insert(entry.node),
                        "ocfs2: directory inode alias/cycle",
                    )?;
                    require(
                        self.directory(&target)?
                            .entries
                            .iter()
                            .any(|parent| parent.name == ".." && parent.node == node),
                        "ocfs2: child directory parent identity",
                    )?;
                }
                output.push(DirEntry {
                    name: entry.name,
                    node: entry.node,
                    kind: target.kind(),
                });
            }
        }
        Ok(output)
    }
    fn metadata(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.inode(node)?;
        Ok(Metadata {
            kind: inode.kind(),
            size: inode.size,
            mode: inode.mode,
        })
    }
    fn timestamps(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        Ok(self.inode(node)?.times)
    }
    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        raw: &mut [u8],
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let inode = self.inode(node)?;
        require(
            matches!(inode.kind(), FileKind::Regular | FileKind::Symlink),
            "ocfs2: content read on non-file",
        )?;
        self.map(&inode)?;
        self.stream
            .as_ref()
            .ok_or(FoldError::Corrupt("ocfs2: missing content stream"))?
            .1
            .read(&mut self.source, offset, raw)
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
        require(inode.size <= 65536, "ocfs2: symlink target work limit")?;
        let mut raw = vec![0; inode.size as usize];
        require(
            self.read_at(node, 0, &mut raw, context)? == raw.len(),
            "ocfs2: incomplete symlink target",
        )?;
        Ok(Some(raw))
    }
}
