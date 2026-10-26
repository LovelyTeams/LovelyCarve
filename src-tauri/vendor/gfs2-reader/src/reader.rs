use crate::{
    allocation::Allocation,
    directory::Directory,
    header::{Header, SUPERBLOCK_OFFSET},
    inode::Inode,
    journal, require,
    stream::Stream,
};
use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};
use std::collections::VecDeque;

/// Offline immutable GFS2 volume. Opening validates resource-group allocation
/// and every journal; dirty cluster members are never silently replayed.
pub struct Gfs2<S: BlockSource> {
    source: S,
    header: Header,
    allocation: Option<Allocation>,
    inodes: VecDeque<Inode>,
    stream: Option<Stream>,
}
impl<S: BlockSource> Gfs2<S> {
    pub fn header(&self) -> &Header {
        &self.header
    }
    /// Raw native extended attributes, bounded to 64 MiB per inode.
    pub fn attributes(&mut self, node: NodeId) -> Result<Vec<crate::Attribute>> {
        let inode = self.inode(node)?;
        self.map(&inode)?;
        let stream = self
            .stream
            .as_ref()
            .ok_or(FoldError::Corrupt("gfs2: missing attribute owner map"))?;
        let mut used = stream.metadata.clone();
        used.extend(stream.blocks.iter().flatten().copied());
        crate::attributes::read(
            &mut self.source,
            &self.header,
            &inode,
            self.allocation
                .as_mut()
                .ok_or(FoldError::Corrupt("gfs2: missing allocation state"))?,
            used,
        )
    }
    fn inode(&mut self, node: NodeId) -> Result<Inode> {
        if let Some(inode) = self
            .inodes
            .iter()
            .find(|inode| inode.number.address == node)
        {
            return Ok(inode.clone());
        }
        if let Some(allocation) = &mut self.allocation {
            allocation.check(&mut self.source, &self.header, node, true)?;
        }
        let inode = Inode::read(&mut self.source, &self.header, node)?;
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
            .is_some_and(|stream| stream.owner == inode.number.address)
        {
            return Ok(());
        }
        let stream = Stream::parse(&mut self.source, &self.header, inode)?;
        if let Some(allocation) = &mut self.allocation {
            for block in &stream.metadata {
                allocation.check(
                    &mut self.source,
                    &self.header,
                    *block,
                    *block == inode.number.address,
                )?;
            }
            for block in stream.blocks.iter().flatten() {
                allocation.check(&mut self.source, &self.header, *block, false)?;
            }
        }
        self.stream = Some(stream);
        Ok(())
    }
    fn directory(&mut self, inode: &Inode) -> Result<Directory> {
        self.map(inode)?;
        let stream = self
            .stream
            .as_ref()
            .ok_or(FoldError::Corrupt("gfs2: missing directory stream"))?;
        let directory = Directory::read(&mut self.source, &self.header, inode, stream)?;
        for block in &directory.leaves {
            require(
                !stream.metadata.contains(block) && !stream.blocks.contains(&Some(*block)),
                "gfs2: directory leaf/map overlap",
            )?;
            if let Some(allocation) = &mut self.allocation {
                allocation.check(&mut self.source, &self.header, *block, false)?;
            }
        }
        Ok(directory)
    }
    fn child(&mut self, parent: &Inode, name: &str) -> Result<Inode> {
        let entry = self
            .directory(parent)?
            .entries
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or(FoldError::Corrupt("gfs2: missing required system entry"))?;
        let inode = self.inode(entry.number.address)?;
        require(
            inode.number == entry.number && inode.kind() == entry.kind,
            "gfs2: system entry identity/kind",
        )?;
        Ok(inode)
    }
    fn regular_bytes(&mut self, inode: &Inode, limit: u64) -> Result<Vec<u8>> {
        require(
            inode.kind() == FileKind::Regular && inode.size <= limit,
            "gfs2: system file kind/size",
        )?;
        self.map(inode)?;
        let mut raw = vec![0; inode.size as usize];
        require(
            self.stream
                .as_ref()
                .ok_or(FoldError::Corrupt("gfs2: missing system map"))?
                .read(&mut self.source, &self.header, 0, &mut raw)?
                == raw.len(),
            "gfs2: short system file",
        )?;
        Ok(raw)
    }
}
impl<S: BlockSource> FoldFrontend<S> for Gfs2<S> {
    const TAG: &'static str = "gfs2";
    fn probe(source: &mut S) -> Result<bool> {
        if source.len() < SUPERBLOCK_OFFSET + 272 {
            return Ok(false);
        }
        let mut raw = [0; 272];
        crate::exact(source, SUPERBLOCK_OFFSET, &mut raw)?;
        Ok(Header::inspect_prefix(&raw, source.len()).is_ok())
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
        let master = reader.inode(reader.header.master.address)?;
        let root = reader.inode(reader.header.root.address)?;
        require(
            master.number == reader.header.master
                && root.number == reader.header.root
                && master.kind() == FileKind::Directory
                && root.kind() == FileKind::Directory,
            "gfs2: superblock directory identity",
        )?;
        let index = reader.child(&master, "rindex")?;
        let index_raw = reader.regular_bytes(&index, 96 * 131072)?;
        reader.allocation = Some(Allocation::parse(
            &mut reader.source,
            &reader.header,
            &index_raw,
        )?);
        reader.inodes.clear();
        reader.stream = None;
        let master = reader.inode(reader.header.master.address)?;
        reader.inode(reader.header.root.address)?;
        let index = reader.child(&master, "rindex")?;
        require(
            reader.regular_bytes(&index, 96 * 131072)? == index_raw,
            "gfs2: bootstrap rindex changed",
        )?;
        let journals = reader.child(&master, "jindex")?;
        let entries = reader.directory(&journals)?.entries;
        let mut journals_found = Vec::new();
        let mut journal_blocks = std::collections::HashSet::new();
        let mut journal_bytes = 0u64;
        for entry in entries {
            if matches!(entry.name.as_str(), "." | "..") {
                continue;
            }
            let number = entry
                .name
                .strip_prefix("journal")
                .and_then(|number| number.parse::<usize>().ok())
                .ok_or(FoldError::Corrupt("gfs2: unexpected journal index name"))?;
            require(
                entry.name == format!("journal{number}")
                    && number < 256
                    && entry.kind == FileKind::Regular,
                "gfs2: journal index identity",
            )?;
            let inode = reader.inode(entry.number.address)?;
            require(
                inode.number == entry.number && inode.kind() == FileKind::Regular,
                "gfs2: journal inode identity",
            )?;
            journal_bytes = journal_bytes
                .checked_add(inode.size)
                .ok_or(FoldError::Corrupt("gfs2: total journal size overflow"))?;
            require(
                journal_bytes <= 4 * 1024 * 1024 * 1024,
                "gfs2: total journal scan work limit",
            )?;
            reader.map(&inode)?;
            let stream = reader
                .stream
                .as_ref()
                .ok_or(FoldError::Corrupt("gfs2: missing journal stream"))?;
            for block in stream
                .metadata
                .iter()
                .copied()
                .chain(stream.blocks.iter().flatten().copied())
            {
                require(
                    journal_blocks.insert(block),
                    "gfs2: cross-journal allocation alias",
                )?;
            }
            journal::check(&mut reader.source, &reader.header, &inode, stream)?;
            journals_found.push(number);
        }
        journals_found.sort_unstable();
        require(
            !journals_found.is_empty()
                && journals_found.iter().copied().eq(0..journals_found.len()),
            "gfs2: missing/duplicate cluster journal",
        )?;
        let root = reader.inode(reader.header.root.address)?;
        let directory = reader.directory(&root)?;
        require(
            directory
                .entries
                .iter()
                .any(|entry| entry.name == ".." && entry.number == root.number),
            "gfs2: root parent identity",
        )?;
        Ok(reader)
    }
    fn root(&self) -> NodeId {
        self.header.root.address
    }
    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self
            .read_dir(dir, context)?
            .into_iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.node))
    }
    fn read_dir(&mut self, dir: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        let inode = self.inode(dir)?;
        let mut output = Vec::new();
        for entry in self.directory(&inode)?.entries {
            let target = self.inode(entry.number.address)?;
            require(
                target.number == entry.number && target.kind() == entry.kind,
                "gfs2: stale/mismatched directory entry",
            )?;
            if !matches!(entry.name.as_str(), "." | "..") {
                if target.kind() == FileKind::Directory {
                    let child = self.directory(&target)?;
                    require(
                        child
                            .entries
                            .iter()
                            .any(|parent| parent.name == ".." && parent.number == inode.number),
                        "gfs2: child parent identity",
                    )?;
                }
                output.push(DirEntry {
                    name: entry.name,
                    node: entry.number.address,
                    kind: entry.kind,
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
        bytes: &mut [u8],
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let inode = self.inode(node)?;
        require(
            matches!(inode.kind(), FileKind::Regular | FileKind::Symlink),
            "gfs2: content on non-file",
        )?;
        self.map(&inode)?;
        self.stream
            .as_ref()
            .ok_or(FoldError::Corrupt("gfs2: missing file stream"))?
            .read(&mut self.source, &self.header, offset, bytes)
    }
    fn read_link(
        &mut self,
        node: NodeId,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<Vec<u8>>> {
        let inode = self.inode(node)?;
        if inode.kind() != FileKind::Symlink {
            return Ok(None);
        }
        require(inode.size <= 65536, "gfs2: link target work limit")?;
        self.map(&inode)?;
        let mut raw = vec![0; inode.size as usize];
        require(
            self.stream
                .as_ref()
                .ok_or(FoldError::Corrupt("gfs2: missing link stream"))?
                .read(&mut self.source, &self.header, 0, &mut raw)?
                == raw.len(),
            "gfs2: short link target",
        )?;
        Ok(Some(raw))
    }
}
