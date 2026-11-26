use crate::{
    extents,
    format::{Allocation, DIRECTORY_BYTES},
    le64,
    object::{DirectoryNode, Entry, Extent},
    require, Header, SECTOR_BYTES,
};
use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

const SECTOR: u64 = SECTOR_BYTES as u64;
const MAX_DIRECTORY_NODES: usize = 4096;

pub struct Ocfs1<S> {
    source: S,
    header: Header,
    bitmap: Allocation,
    label: String,
    id: [u8; 16],
    entries: VecDeque<(NodeId, Arc<Entry>)>,
    directory: Option<(u64, Arc<Vec<DirEntry>>)>,
    file: Option<(u64, Vec<Extent>)>,
}

impl<S: BlockSource> Ocfs1<S> {
    pub fn header(&self) -> &Header {
        &self.header
    }
    pub fn label(&self) -> &str {
        &self.label
    }
    pub fn volume_id(&self) -> [u8; 16] {
        self.id
    }

    fn read_sector(&mut self, address: u64) -> Result<[u8; SECTOR_BYTES]> {
        require(
            address.is_multiple_of(SECTOR),
            "ocfs1: unaligned metadata sector",
        )?;
        self.bitmap
            .check(&mut self.source, &self.header, address, SECTOR)?;
        let mut sector = [0; SECTOR_BYTES];
        self.source.read_at(address, &mut sector)?;
        Ok(sector)
    }

    fn remember(&mut self, id: NodeId, entry: Entry) -> Arc<Entry> {
        let entry = Arc::new(entry);
        if self.entries.len() == 256 {
            self.entries.pop_front();
        }
        self.entries.push_back((id, Arc::clone(&entry)));
        entry
    }

    fn entry(&mut self, id: NodeId) -> Result<Arc<Entry>> {
        if let Some((_, entry)) = self.entries.iter().find(|(current, _)| *current == id) {
            return Ok(Arc::clone(entry));
        }
        let raw = self.read_sector(id)?;
        let parent = le64(&raw, 432)?;
        let directory = DirectoryNode::parse(&self.read_sector(parent)?, parent)?;
        require(
            directory.offsets.contains(&id),
            "ocfs1: file entry not indexed in its parent node",
        )?;
        let entry = Entry::parse(&raw, id)?.ok_or(FoldError::NotFound)?;
        Ok(self.remember(id, entry))
    }

    fn head(&mut self, node: NodeId) -> Result<u64> {
        if node == self.header.root {
            return Ok(node);
        }
        self.entry(node)?.directory_head()
    }

    fn directory(&mut self, node: NodeId) -> Result<Arc<Vec<DirEntry>>> {
        let head = self.head(node)?;
        if let Some((cached, entries)) = &self.directory {
            if *cached == head {
                return Ok(Arc::clone(entries));
            }
        }
        let mut names = HashSet::new();
        let mut seen = HashSet::new();
        let mut children = HashSet::new();
        let mut entries = Vec::new();
        let mut address = Some(head);
        while let Some(offset) = address {
            require(
                seen.len() < MAX_DIRECTORY_NODES && seen.insert(offset),
                "ocfs1: directory chain cycle or limit",
            )?;
            self.bitmap
                .check(&mut self.source, &self.header, offset, DIRECTORY_BYTES)?;
            let directory = DirectoryNode::parse(&self.read_sector(offset)?, offset)?;
            require(
                directory.first == (offset == head),
                "ocfs1: directory chain head flag",
            )?;
            for id in directory.offsets {
                let raw = self.read_sector(id)?;
                let Some(entry) = Entry::parse(&raw, id)? else {
                    continue;
                };
                require(
                    entry.parent_node == offset,
                    "ocfs1: directory record belongs to another node",
                )?;
                require(
                    names.insert(entry.name.clone()),
                    "ocfs1: duplicate directory name",
                )?;
                if entry.metadata.kind == FileKind::Directory {
                    let child = entry.directory_head()?;
                    require(
                        child != self.header.root && children.insert(child),
                        "ocfs1: root loop or repeated child directory",
                    )?;
                    self.bitmap
                        .check(&mut self.source, &self.header, child, DIRECTORY_BYTES)?;
                    require(
                        DirectoryNode::parse(&self.read_sector(child)?, child)?.first,
                        "ocfs1: child directory is not a head node",
                    )?;
                }
                entries.push(DirEntry {
                    name: entry.name.clone(),
                    node: id,
                    kind: entry.metadata.kind,
                });
                self.remember(id, entry);
            }
            address = directory.next;
        }
        require(
            seen.is_disjoint(&children),
            "ocfs1: child loops into enclosing directory chain",
        )?;
        let entries = Arc::new(entries);
        self.directory = Some((head, Arc::clone(&entries)));
        Ok(entries)
    }

    fn ensure_file(&mut self, node: NodeId, entry: &Entry) -> Result<()> {
        if self
            .file
            .as_ref()
            .is_some_and(|(cached, _)| *cached == node)
        {
            return Ok(());
        }
        if !matches!(entry.metadata.kind, FileKind::Regular | FileKind::Symlink) {
            return Err(FoldError::Unsupported("ocfs1: object has no file data"));
        }
        self.file = Some((
            node,
            extents::resolve(
                &mut self.source,
                &self.header,
                &mut self.bitmap,
                node,
                entry,
            )?,
        ));
        Ok(())
    }
}

impl<S: BlockSource> FoldFrontend<S> for Ocfs1<S> {
    const TAG: &'static str = "ocfs1";

    fn probe(source: &mut S) -> Result<bool> {
        Ok(Header::read(source).is_ok())
    }

    fn open(mut source: S, _context: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header = Header::read(&mut source)?;
        let (label, id) = header.label(&mut source)?;
        header.check_clean(&mut source)?;
        let mut reader = Self {
            source,
            header,
            label,
            id,
            bitmap: Allocation::default(),
            entries: VecDeque::new(),
            directory: None,
            file: None,
        };
        reader.directory(reader.root())?;
        Ok(reader)
    }

    fn root(&self) -> NodeId {
        self.header.root
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self
            .directory(dir)?
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| entry.node))
    }

    fn read_dir(&mut self, dir: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        Ok(self.directory(dir)?.as_ref().clone())
    }

    fn metadata(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        if node == self.root() {
            return Ok(Metadata {
                kind: FileKind::Directory,
                size: 0,
                mode: self.header.root_mode,
            });
        }
        Ok(self.entry(node)?.metadata.clone())
    }

    fn timestamps(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        if node == self.root() {
            return Ok(FileTimes::default());
        }
        Ok(self.entry(node)?.times)
    }

    fn inode_identity(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<NodeId> {
        if node == self.root() {
            return Ok(node);
        }
        let entry = self.entry(node)?;
        if entry.metadata.kind == FileKind::Directory {
            entry.directory_head()
        } else {
            Ok(node)
        }
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        output: &mut [u8],
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        if node == self.root() {
            return Err(FoldError::IsDirectory);
        }
        let entry = self.entry(node)?;
        self.ensure_file(node, &entry)?;
        let len = entry
            .metadata
            .size
            .saturating_sub(offset)
            .min(output.len() as u64) as usize;
        if len == 0 {
            return Ok(0);
        }
        let end = offset + len as u64;
        let (_, spans) = self
            .file
            .as_ref()
            .ok_or(FoldError::Corrupt("ocfs1: missing file extent map"))?;
        let first = spans.partition_point(|span| span.logical + span.bytes <= offset);
        let mut copied = 0;
        for span in &spans[first..] {
            if span.logical >= end {
                break;
            }
            let start = span.logical.max(offset);
            let stop = span.end()?.min(end);
            let requested = &mut output[(start - offset) as usize..(stop - offset) as usize];
            self.source
                .read_at(span.physical + (start - span.logical), requested)?;
            copied += requested.len();
        }
        require(copied == len, "ocfs1: short logical read")?;
        Ok(copied)
    }

    fn read_link(
        &mut self,
        node: NodeId,
        context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<Vec<u8>>> {
        if node == self.root() {
            return Ok(None);
        }
        let stat = self.entry(node)?;
        if stat.metadata.kind != FileKind::Symlink {
            return Ok(None);
        }
        require(
            (1..=4096).contains(&stat.metadata.size),
            "ocfs1: invalid symlink size",
        )?;
        let mut target = vec![0; stat.metadata.size as usize];
        let len = self.read_at(node, 0, &mut target, context)?;
        require(
            len == target.len() && !target.contains(&0),
            "ocfs1: invalid symlink target",
        )?;
        Ok(Some(target))
    }
}
