use crate::{
    allocation::Allocation,
    inode::Inode,
    require,
    streams::Map,
    tree::{Directory, Role},
    Header,
};
use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};
use std::collections::{HashSet, VecDeque};

/// Exact recorded timestamp words. The lower bits may be BeOS uniqueness
/// counters or Haiku subsecond fields; no origin/version bit distinguishes
/// them. Preserve all words while the common frontend reports UTC seconds.
#[derive(Clone, Copy, Debug)]
pub struct RecordedTimes {
    pub created: i64,
    pub modified: i64,
    pub status_or_reserved: i64,
}
/// The superblock has no timestamp-version flag. Unknown evidence must not
/// turn BeOS uniqueness bits or reserved words into subsecond/ctime claims.
#[derive(Clone, Copy, Debug, Default)]
pub enum TimestampEncoding {
    #[default]
    Unknown,
    BeOs,
    Haiku,
}
impl RecordedTimes {
    pub fn interpret(self, encoding: TimestampEncoding) -> Result<FileTimes> {
        let time = |raw: i64| -> Result<(i64, u32)> {
            let nanos = if matches!(encoding, TimestampEncoding::Haiku) && raw & 0xf000 != 0xf000 {
                ((raw as u64 & 0xfff0) << 14) as u32
            } else {
                0
            };
            require(nanos < 1_000_000_000, "befs: invalid Haiku subsecond field")?;
            Ok((raw >> 16, nanos))
        };
        Ok(FileTimes {
            created: Some(time(self.created)?),
            modified: Some(time(self.modified)?),
            changed: if matches!(encoding, TimestampEncoding::Haiku) && self.status_or_reserved != 0
            {
                Some(time(self.status_or_reserved)?)
            } else {
                None
            },
            ..FileTimes::default()
        })
    }
}
#[derive(Clone, Debug)]
pub struct Attribute {
    pub name: String,
    pub type_code: u32,
    pub size: u64,
    pub resident: bool,
    source_inode: u64,
}
/// Bounded read-only BFS; each cache has a fixed size or retains one object.
pub struct BeFs<S: BlockSource> {
    source: S,
    header: Header,
    allocation: Allocation,
    inodes: VecDeque<Inode>,
    directory: Option<(NodeId, Vec<DirEntry>)>,
    map: Option<(NodeId, Map)>,
    attributes: Option<(NodeId, Vec<Attribute>)>,
    timestamp_encoding: TimestampEncoding,
}
impl<S: BlockSource> BeFs<S> {
    pub fn header(&self) -> &Header {
        &self.header
    }
    /// Apply only an externally established provenance interpretation. This
    /// changes presentation of recorded words, never the source or its caches.
    pub fn set_timestamp_encoding(&mut self, encoding: TimestampEncoding) {
        self.timestamp_encoding = encoding;
    }
    fn inode(&mut self, node: NodeId) -> Result<Inode> {
        if let Some(inode) = self.inodes.iter().find(|inode| inode.number == node) {
            return Ok(inode.clone());
        }
        let inode = Inode::read(&mut self.source, &self.header, &mut self.allocation, node)?;
        if self.inodes.len() == 128 {
            self.inodes.pop_front();
        }
        self.inodes.push_back(inode.clone());
        Ok(inode)
    }
    fn map(&mut self, inode: &Inode) -> Result<()> {
        if !self
            .map
            .as_ref()
            .is_some_and(|(node, _)| *node == inode.number)
        {
            let map = Map::parse(&mut self.source, &self.header, &mut self.allocation, inode)?;
            self.map = Some((inode.number, map));
        }
        Ok(())
    }
    fn tree(&mut self, inode: &Inode, role: Role) -> Result<Directory> {
        require(
            inode.kind() == FileKind::Directory,
            "befs: B+tree on non-directory",
        )?;
        self.map(inode)?;
        let map = &self
            .map
            .as_ref()
            .ok_or(FoldError::Corrupt("befs: missing directory map"))?
            .1;
        Directory::read(
            &mut self.source,
            &self.header,
            map,
            inode.number,
            inode.parent,
            role,
        )
    }
    fn ensure_directory(&mut self, node: NodeId) -> Result<()> {
        if self
            .directory
            .as_ref()
            .is_some_and(|(number, _)| *number == node)
        {
            return Ok(());
        }
        let inode = self.inode(node)?;
        let entries = self.tree(&inode, Role::Names)?;
        let mut output = Vec::with_capacity(entries.entries.len());
        for (name, number) in entries.entries {
            let child = self.inode(number)?;
            let kind = child.kind();
            require(
                kind != FileKind::Directory || child.parent == node,
                "befs: child directory parent identity",
            )?;
            output.push(DirEntry {
                name,
                node: number,
                kind,
            });
        }
        self.directory = Some((node, output));
        Ok(())
    }
    pub fn recorded_times(&mut self, node: NodeId) -> Result<RecordedTimes> {
        let inode = self.inode(node)?;
        Ok(RecordedTimes {
            created: inode.created,
            modified: inode.modified,
            status_or_reserved: inode.status_or_reserved,
        })
    }
    fn symlink(&mut self, inode: &Inode) -> Result<Vec<u8>> {
        if let Some(raw) = inode.inline_link()? {
            return Ok(raw);
        }
        self.map(inode)?;
        let map = &self
            .map
            .as_ref()
            .ok_or(FoldError::Corrupt("befs: missing symlink map"))?
            .1;
        require(
            map.length > 0 && map.length <= 65536,
            "befs: symlink length limit",
        )?;
        let mut raw = vec![0; map.length as usize];
        map.exact(&mut self.source, &self.header, 0, &mut raw)?;
        // Long symlinks use the stream length; an optional final NUL is a
        // terminator, never part of the target's logical byte count.
        if raw.last() == Some(&0) {
            raw.pop();
        }
        require(
            !raw.is_empty() && !raw.contains(&0),
            "befs: invalid long symlink target",
        )?;
        Ok(raw)
    }
    pub fn attributes(&mut self, node: NodeId) -> Result<Vec<Attribute>> {
        if let Some((_, attributes)) = self
            .attributes
            .as_ref()
            .filter(|(number, _)| *number == node)
        {
            return Ok(attributes.clone());
        }
        let inode = self.inode(node)?;
        let mut names = HashSet::new();
        let mut output = Vec::new();
        for attribute in &inode.small {
            names.insert(attribute.name.clone());
            output.push(Attribute {
                name: attribute.name.clone(),
                type_code: attribute.type_code,
                size: attribute.bytes.len() as u64,
                resident: true,
                source_inode: node,
            });
        }
        if let Some(number) = inode.attributes {
            require(number != node, "befs: attribute directory aliases owner")?;
            let directory = self.inode(number)?;
            require(
                directory.parent == node && directory.mode & 0x08000000 != 0,
                "befs: attribute directory parent identity",
            )?;
            for (name, number) in self.tree(&directory, Role::Attributes)?.entries {
                require(
                    names.insert(name.clone()) && output.len() < 65536,
                    "befs: attribute name duplicate/count limit",
                )?;
                let attribute = self.inode(number)?;
                if attribute.parent != directory.number
                    || attribute.kind() != FileKind::Regular
                    // Modern Haiku sets S_ATTR in mode; the older on-disk
                    // INODE_ATTR_INODE flag is not required by its creator.
                    || (attribute.flags & 4 == 0 && attribute.mode & 0x10000000 == 0)
                {
                    return Err(FoldError::Source(format!("befs: external attribute {name:?}, inode {number}, parent {}, expected parent {}, mode {:#x}, flags {:#x}",attribute.parent,directory.number,attribute.mode,attribute.flags)));
                }
                let size = self.header.endian.nonnegative(&attribute.stream, 136)?;
                output.push(Attribute {
                    name,
                    type_code: attribute.type_code,
                    size,
                    resident: false,
                    source_inode: number,
                });
            }
        }
        output.sort_by(|left, right| left.name.cmp(&right.name));
        self.attributes = Some((node, output.clone()));
        Ok(output)
    }
    pub fn read_attribute_at(
        &mut self,
        node: NodeId,
        name: &str,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize> {
        let attribute = self
            .attributes(node)?
            .into_iter()
            .find(|attribute| attribute.name == name)
            .ok_or(FoldError::Unsupported("befs: no such attribute"))?;
        let inode = self.inode(attribute.source_inode)?;
        if attribute.resident {
            let small = inode
                .small
                .iter()
                .find(|entry| entry.name == name)
                .ok_or(FoldError::Corrupt("befs: missing inline attribute"))?;
            let start = offset.min(small.bytes.len() as u64) as usize;
            let count = (small.bytes.len() - start).min(bytes.len());
            bytes[..count].copy_from_slice(&small.bytes[start..start + count]);
            Ok(count)
        } else {
            self.map(&inode)?;
            self.map
                .as_ref()
                .ok_or(FoldError::Corrupt("befs: missing attribute map"))?
                .1
                .read(&mut self.source, &self.header, offset, bytes)
        }
    }
}
impl<S: BlockSource> FoldFrontend<S> for BeFs<S> {
    const TAG: &'static str = "befs";
    fn probe(source: &mut S) -> Result<bool> {
        // Source failures are not a negative filesystem signature. Decode
        // validation only after each complete positioned read succeeds.
        for offset in [512, 0] {
            if offset + 132 > source.len() {
                continue;
            }
            let mut raw = [0; 132];
            crate::exact(source, offset, &mut raw)?;
            if Header::inspect_at(&raw, offset, source.len()).is_ok() {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn open(mut source: S, _context: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header = Header::inspect(&mut source)?;
        if !header.clean {
            return Err(FoldError::Unsupported(
                "befs: dirty volume/nonempty journal requires replay; source left unchanged",
            ));
        }
        let mut allocation = Allocation::default();
        allocation.check(&mut source, &header, 0, header.bitmap_end)?;
        allocation.check(
            &mut source,
            &header,
            header.log.start,
            header.log.end - header.log.start,
        )?;
        let root = header.root;
        let mut reader = Self {
            source,
            header,
            allocation,
            inodes: VecDeque::new(),
            directory: None,
            map: None,
            attributes: None,
            timestamp_encoding: TimestampEncoding::Unknown,
        };
        let inode = reader.inode(root)?;
        require(inode.parent == root, "befs: root parent identity")?;
        if let Some(indices) = reader.header.indices {
            let inode = reader.inode(indices)?;
            require(
                inode.kind() == FileKind::Directory && inode.parent == indices,
                "befs: index root identity/type",
            )?;
        }
        reader.ensure_directory(root)?;
        Ok(reader)
    }
    fn root(&self) -> NodeId {
        self.header.root
    }
    fn lookup(
        &mut self,
        node: NodeId,
        name: &str,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        self.ensure_directory(node)?;
        let entries = &self
            .directory
            .as_ref()
            .ok_or(FoldError::Corrupt("befs: missing directory cache"))?
            .1;
        Ok(entries
            .binary_search_by(|entry| entry.name.as_str().cmp(name))
            .ok()
            .map(|index| entries[index].node))
    }
    fn read_dir(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.ensure_directory(node)?;
        Ok(self
            .directory
            .as_ref()
            .ok_or(FoldError::Corrupt("befs: missing directory cache"))?
            .1
            .clone())
    }
    fn metadata(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.inode(node)?;
        let kind = inode.kind();
        let size = match kind {
            FileKind::Directory | FileKind::Other => 0,
            FileKind::Regular => self.header.endian.nonnegative(&inode.stream, 136)?,
            FileKind::Symlink => self.symlink(&inode)?.len() as u64,
        };
        Ok(Metadata {
            kind,
            size,
            mode: inode.mode,
        })
    }
    fn timestamps(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        let times = self.recorded_times(node)?;
        times.interpret(self.timestamp_encoding)
    }
    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let inode = self.inode(node)?;
        match inode.kind() {
            FileKind::Directory => Err(FoldError::IsDirectory),
            FileKind::Other => Err(FoldError::Unsupported(
                "befs: special inode has no ordinary file content",
            )),
            FileKind::Symlink => {
                let raw = self.symlink(&inode)?;
                let start = offset.min(raw.len() as u64) as usize;
                let count = (raw.len() - start).min(bytes.len());
                bytes[..count].copy_from_slice(&raw[start..start + count]);
                Ok(count)
            }
            FileKind::Regular => {
                self.map(&inode)?;
                self.map
                    .as_ref()
                    .ok_or(FoldError::Corrupt("befs: missing file map"))?
                    .1
                    .read(&mut self.source, &self.header, offset, bytes)
            }
        }
    }
    fn read_link(
        &mut self,
        node: NodeId,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<Vec<u8>>> {
        let inode = self.inode(node)?;
        if inode.kind() == FileKind::Symlink {
            self.symlink(&inode).map(Some)
        } else {
            Ok(None)
        }
    }
}
