//! Immutable current or historical checkpoint view, with native inode identity.
use crate::{header::Header, inode::Inode, le16, le32, le64, require, storage::Storage};
use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub number: u64,
    pub snapshot: bool,
    pub created: i64,
    pub inodes: u64,
    /// Absent when the native block-count-per-checkpoint feature is disabled.
    pub blocks: Option<u64>,
}
enum Selection {
    Latest,
    Number(u64),
}
pub struct Nilfs2<S: BlockSource> {
    storage: Storage<S>,
    checkpoints: Inode,
    ifile: Inode,
    checkpoint: Checkpoint,
    inodes: VecDeque<(u64, Inode)>,
    parents: HashMap<u64, u64>,
}
struct Entry {
    node: u64,
    name: String,
    kind: Option<FileKind>,
}
impl<S: BlockSource> Nilfs2<S> {
    pub fn header(&self) -> &Header {
        &self.storage.header
    }
    pub fn checkpoint(&self) -> &Checkpoint {
        &self.checkpoint
    }
    pub fn open_checkpoint(source: S, number: u64) -> Result<Self> {
        Self::open_selected(source, Selection::Number(number))
    }
    fn checkpoint_record(
        storage: &mut Storage<S>,
        checkpoints: &Inode,
        number: u64,
    ) -> Result<Vec<u8>> {
        require(
            number != 0 && number <= storage.header.last_checkpoint,
            "nilfs2: checkpoint number outside committed history",
        )?;
        let size = storage.header.checkpoint_size;
        let index = number
            .checked_add(32usize.div_ceil(size) as u64 - 1)
            .ok_or(FoldError::Corrupt("nilfs2: checkpoint slot overflow"))?;
        let per_block = storage.header.block_size as u64 / size as u64;
        let raw = storage.required_block(
            checkpoints,
            4,
            index / per_block,
            storage.header.last_checkpoint,
        )?;
        let offset = (index % per_block) as usize * size;
        let record = raw[offset..offset + size].to_vec();
        let flags = le32(&record, 0)?;
        require(
            flags & !0xf == 0 && flags & 2 == 0 && le64(&record, 24)? == number,
            "nilfs2: invalid checkpoint record identity/flags",
        )?;
        if flags & 4 != 0 {
            return Err(FoldError::Unsupported("nilfs2: legacy sketch checkpoint"));
        }
        Ok(record)
    }
    fn info(header: &Header, raw: &[u8]) -> Result<Checkpoint> {
        Ok(Checkpoint {
            number: le64(raw, 24)?,
            snapshot: le32(raw, 0)? & 1 != 0,
            created: le64(raw, 32)? as i64,
            inodes: le64(raw, 48)?,
            blocks: if header.block_count_feature {
                Some(le64(raw, 56)?)
            } else {
                None
            },
        })
    }
    pub fn snapshots(&mut self) -> Result<Vec<Checkpoint>> {
        let latest = self.storage.header.last_checkpoint;
        let raw = self
            .storage
            .required_block(&self.checkpoints, 4, 0, latest)?;
        let count = le64(&raw, 8)?;
        require(count <= 65536, "nilfs2: snapshot enumeration work limit")?;
        let mut next = le64(&raw, 16)?;
        let last = le64(&raw, 24)?;
        let mut output = Vec::new();
        let mut seen = HashSet::new();
        let mut previous = 0;
        while next != 0 {
            require(
                seen.insert(next) && output.len() < 65536 && next > previous,
                "nilfs2: snapshot list cycle/order/work limit",
            )?;
            let raw = Self::checkpoint_record(&mut self.storage, &self.checkpoints, next)?;
            require(
                le32(&raw, 0)? & 1 != 0 && le64(&raw, 16)? == previous,
                "nilfs2: snapshot flag/previous identity",
            )?;
            output.push(Self::info(&self.storage.header, &raw)?);
            previous = next;
            next = le64(&raw, 8)?;
        }
        require(
            previous == last && output.len() as u64 == count,
            "nilfs2: snapshot list count/tail",
        )?;
        Ok(output)
    }
    fn open_selected(mut source: S, selection: Selection) -> Result<Self> {
        let header = Header::inspect(&mut source)?;
        let (mut storage, checkpoints, usage) = Storage::open(source, header)?;
        let latest = storage.header.last_checkpoint;
        let raw = storage.required_block(&checkpoints, 4, 0, latest)?;
        require(
            le64(&raw, 0)? != 0 && le64(&raw, 0)? <= latest && le64(&raw, 8)? <= le64(&raw, 0)?,
            "nilfs2: checkpoint header counts",
        )?;
        let raw = storage.required_block(&usage, 5, 0, latest)?;
        require(
            le64(&raw, 0)?
                .checked_add(le64(&raw, 8)?)
                .is_some_and(|count| count == storage.header.segments)
                && le64(&raw, 16)? < storage.header.segments,
            "nilfs2: segment usage header counts",
        )?;
        let selected = match selection {
            Selection::Latest => latest,
            Selection::Number(number) => number,
        };
        let raw = Self::checkpoint_record(&mut storage, &checkpoints, selected)?;
        let checkpoint = Self::info(&storage.header, &raw)?;
        require(
            checkpoint.inodes != 0,
            "nilfs2: checkpoint without root inode",
        )?;
        let ifile = Inode::parse(&raw[64..64 + storage.header.inode_size])?;
        require(
            ifile.kind() == FileKind::Regular,
            "nilfs2: ifile inode mode",
        )?;
        let mut reader = Self {
            storage,
            checkpoints,
            ifile,
            checkpoint,
            inodes: VecDeque::new(),
            parents: HashMap::from([(2, 2)]),
        };
        let root = reader.inode(2)?;
        let entries = reader.directory(2, &root)?;
        require(
            entries
                .iter()
                .any(|entry| entry.name == ".." && entry.node == 2),
            "nilfs2: root parent identity",
        )?;
        Ok(reader)
    }
    fn inode(&mut self, number: u64) -> Result<Inode> {
        if let Some((_, inode)) = self.inodes.iter().find(|(node, _)| *node == number) {
            return Ok(inode.clone());
        }
        require(
            number == 2 || number >= self.storage.header.first_inode as u64,
            "nilfs2: reserved inode reference",
        )?;
        let raw = self.storage.entry(
            &self.ifile,
            6,
            number,
            self.storage.header.inode_size,
            self.checkpoint.number,
        )?;
        let inode = Inode::parse(&raw)?;
        // Formatter-created inodes can retain non-sector-aligned i_blocks
        // values. Allocation comes from DAT/segment proofs, not this counter.
        require(
            inode.blocks
                <= self.storage.header.segments
                    * self.storage.header.blocks_per_segment as u64
                    * (self.storage.header.block_size as u64 / 512),
            "nilfs2: inode allocation counter exceeds source span",
        )?;
        if self.inodes.len() == 128 {
            self.inodes.pop_front();
        }
        self.inodes.push_back((number, inode.clone()));
        Ok(inode)
    }
    fn directory(&mut self, owner: u64, inode: &Inode) -> Result<Vec<Entry>> {
        require(
            inode.kind() == FileKind::Directory
                && inode
                    .size
                    .is_multiple_of(self.storage.header.block_size as u64)
                && inode.size / self.storage.header.block_size as u64 <= 131072,
            "nilfs2: directory kind/size/work limit",
        )?;
        let mut entries = Vec::new();
        let mut names = HashSet::new();
        for logical in 0..inode.size / self.storage.header.block_size as u64 {
            let raw = self
                .storage
                .required_block(inode, owner, logical, self.checkpoint.number)?;
            let mut offset = 0;
            while offset < raw.len() {
                let record = &raw[offset..];
                let number = le64(record, 0)?;
                let stored_length = le16(record, 8)? as usize;
                let length = if stored_length == 65535 && raw.len() == 65536 {
                    65536
                } else {
                    stored_length
                };
                require(
                    length >= 16 && length.is_multiple_of(8) && length <= record.len(),
                    "nilfs2: directory record bounds/alignment",
                )?;
                let name_length = record[10] as usize;
                require(12 + name_length <= length, "nilfs2: directory name bounds")?;
                if number != 0 {
                    require(
                        name_length != 0 && entries.len() < 1_000_000,
                        "nilfs2: empty/excessive directory names",
                    )?;
                    let name = &record[12..12 + name_length];
                    require(
                        !name.iter().any(|byte| matches!(byte, 0 | b'/')),
                        "nilfs2: invalid name component",
                    )?;
                    let name = String::from_utf8(name.to_vec())
                        .map_err(|_| FoldError::Unsupported("nilfs2: non-UTF-8 name"))?;
                    require(
                        names.insert(name.clone()),
                        "nilfs2: duplicate directory name",
                    )?;
                    let kind = match record[11] {
                        0 => None,
                        1 => Some(FileKind::Regular),
                        2 => Some(FileKind::Directory),
                        7 => Some(FileKind::Symlink),
                        3..=6 => Some(FileKind::Other),
                        _ => return Err(FoldError::Corrupt("nilfs2: directory entry kind")),
                    };
                    entries.push(Entry {
                        node: number,
                        name,
                        kind,
                    });
                }
                offset += length;
            }
        }
        require(
            entries.iter().any(|entry| {
                entry.name == "."
                    && entry.node == owner
                    && entry.kind.is_none_or(|kind| kind == FileKind::Directory)
            }) && entries.iter().any(|entry| {
                entry.name == ".." && entry.kind.is_none_or(|kind| kind == FileKind::Directory)
            }),
            "nilfs2: dot/parent directory identity",
        )?;
        if let Some(parent) = self.parents.get(&owner) {
            require(
                entries
                    .iter()
                    .any(|entry| entry.name == ".." && entry.node == *parent),
                "nilfs2: directory parent identity",
            )?;
        }
        Ok(entries)
    }
}
impl<S: BlockSource> FoldFrontend<S> for Nilfs2<S> {
    const TAG: &'static str = "nilfs2";
    fn probe(source: &mut S) -> Result<bool> {
        match Header::inspect(source) {
            Ok(_) => Ok(true),
            Err(FoldError::Corrupt(_) | FoldError::Unsupported(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }
    fn open(source: S, _context: &mut SubstrateCtx<'_>) -> Result<Self> {
        Self::open_selected(source, Selection::Latest)
    }
    fn root(&self) -> NodeId {
        2
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
        let entries = self.directory(node, &inode)?;
        let mut output = Vec::new();
        let mut directories = HashSet::new();
        for entry in entries {
            let inode = self.inode(entry.node)?;
            require(
                entry.kind.is_none_or(|kind| kind == inode.kind()),
                "nilfs2: directory entry/inode mode mismatch",
            )?;
            if matches!(entry.name.as_str(), "." | "..") {
                require(
                    inode.kind() == FileKind::Directory,
                    "nilfs2: dot/parent inode mode",
                )?;
                continue;
            }
            if inode.kind() == FileKind::Directory {
                require(
                    entry.node != node && directories.insert(entry.node),
                    "nilfs2: directory alias/self cycle",
                )?;
                let parent_count = self.parents.len();
                match self.parents.entry(entry.node) {
                    std::collections::hash_map::Entry::Occupied(parent) => require(
                        *parent.get() == node,
                        "nilfs2: multiple directory parents/cycle",
                    )?,
                    std::collections::hash_map::Entry::Vacant(parent) => {
                        require(
                            parent_count < 1_000_000,
                            "nilfs2: directory identity work limit",
                        )?;
                        parent.insert(node);
                    }
                }
                self.directory(entry.node, &inode)?;
            }
            output.push(DirEntry {
                node: entry.node,
                name: entry.name,
                kind: inode.kind(),
            });
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
            "nilfs2: content read on non-file",
        )?;
        self.storage
            .read(&inode, node, self.checkpoint.number, offset, bytes)
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
        require(inode.size <= 65536, "nilfs2: symlink target work limit")?;
        let mut bytes = vec![0; inode.size as usize];
        require(
            self.read_at(node, 0, &mut bytes, context)? == bytes.len(),
            "nilfs2: incomplete symlink target",
        )?;
        Ok(Some(bytes))
    }
}
