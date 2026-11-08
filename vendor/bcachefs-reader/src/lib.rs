//! Snapshot-aware, read-only evidence access over rust-fs-bcachefs.
//! The backing source cannot write. Failed source reads remain errors even
//! when an upstream metadata fallback would otherwise conceal the failure.

#![forbid(unsafe_code)]

pub use fs_bcachefs::superblock::{Layout, Superblock, BCACHEFS_MAGIC, SB_HEADER_BYTES};
use fs_bcachefs::{fs::Node, inode::Inode, Error, Filesystem};
use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};
use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, Mutex},
};

const MAX_NODES: usize = 1_000_000;
type Fault = Arc<Mutex<Option<String>>>;

struct Device<S> {
    source: Mutex<S>,
    size: u64,
    fault: Fault,
}

impl<S: BlockSource + Send> fs_core::BlockRead for Device<S> {
    fn size_bytes(&self) -> u64 {
        self.size
    }

    fn read_at(&self, offset: u64, buffer: &mut [u8]) -> fs_core::Result<()> {
        if offset
            .checked_add(buffer.len() as u64)
            .is_none_or(|end| end > self.size)
        {
            return Err(fs_core::Error::OutOfBounds {
                offset,
                len: buffer.len() as u64,
                size: self.size,
            });
        }
        let mut source = self.source.lock().map_err(|_| {
            fs_core::Error::Io(std::io::Error::other("bcachefs source lock poisoned"))
        })?;
        match source.read_at(offset, buffer) {
            Ok(()) => Ok(()),
            Err(error) => {
                let message = format!("bcachefs source at byte {offset}: {error}");
                let mut fault = self.fault.lock().map_err(|_| {
                    fs_core::Error::Io(std::io::Error::other("bcachefs fault lock poisoned"))
                })?;
                if fault.is_none() {
                    *fault = Some(message.clone());
                }
                Err(fs_core::Error::Io(std::io::Error::other(message)))
            }
        }
    }
}

fn result<T>(fault: &Fault, outcome: fs_bcachefs::Result<T>) -> Result<T> {
    if let Some(message) = fault
        .lock()
        .map_err(|_| FoldError::Io("bcachefs fault lock poisoned"))?
        .take()
    {
        return Err(FoldError::Source(message));
    }
    outcome.map_err(|error| match error {
        Error::NotFound(_) => FoldError::NotFound,
        error => FoldError::Source(error.to_string()),
    })
}

fn device<S: BlockSource>(source: S) -> (Device<S>, Fault) {
    let fault = Arc::new(Mutex::new(None));
    (
        Device {
            size: source.len(),
            source: Mutex::new(source),
            fault: Arc::clone(&fault),
        },
        fault,
    )
}

pub struct Bcachefs<S: BlockSource + Send> {
    reader: Filesystem<Device<S>>,
    fault: Fault,
    nodes: Vec<Node>,
    identities: BTreeMap<(u32, u64), NodeId>,
}

impl<S: BlockSource + Send> Bcachefs<S> {
    pub fn header(&self) -> &Superblock {
        self.reader.superblock()
    }

    fn node(&self, id: NodeId) -> Result<Node> {
        self.nodes
            .get(usize::try_from(id).map_err(|_| FoldError::NotFound)?)
            .copied()
            .ok_or(FoldError::NotFound)
    }

    fn remember(&mut self, node: Node) -> Result<NodeId> {
        if let Some(&id) = self.identities.get(&(node.snapshot, node.ino)) {
            return Ok(id);
        }
        if self.nodes.len() >= MAX_NODES {
            return Err(FoldError::Unsupported("bcachefs node identity limit"));
        }
        let id = self.nodes.len() as u64;
        self.nodes.push(node);
        self.identities.insert((node.snapshot, node.ino), id);
        Ok(id)
    }

    fn inode(&self, node: NodeId) -> Result<Inode> {
        result(&self.fault, self.reader.inode_at(self.node(node)?))
    }
}

fn kind(inode: &Inode) -> FileKind {
    match inode.mode & 0xf000 {
        0x4000 => FileKind::Directory,
        0x8000 => FileKind::Regular,
        0xa000 => FileKind::Symlink,
        _ => FileKind::Other,
    }
}

impl<S: BlockSource + Send> FoldFrontend<S> for Bcachefs<S> {
    const TAG: &'static str = "bcachefs";

    fn probe(source: &mut S) -> Result<bool> {
        if source.len() < 4096 + SB_HEADER_BYTES as u64 {
            return Ok(false);
        }
        // The shared reference adapter below only borrows S for this probe.
        let (source, fault) = device(Borrowed(source));
        let outcome = Superblock::read(&source);
        match outcome {
            Ok(_) => result(&fault, Ok(true)),
            Err(
                Error::BadMagic { .. }
                | Error::BadChecksum { .. }
                | Error::Corrupt(_)
                | Error::Unsupported(_),
            ) => result(&fault, Ok(false)),
            Err(error) => result(&fault, Err(error)),
        }
    }

    fn open(source: S, _context: &mut SubstrateCtx<'_>) -> Result<Self> {
        let (source, fault) = device(source);
        let header = result(&fault, Superblock::read(&source))?;
        if header.version.major() != 1 || header.version.minor() > 39 {
            return Err(FoldError::Unsupported(
                "bcachefs metadata version newer than 1.39",
            ));
        }
        if header.nr_devices != 1 || header.dev_idx != 0 {
            return Err(FoldError::Unsupported("multi-device bcachefs"));
        }
        let members = result(&fault, header.members())?;
        let member = members
            .first()
            .ok_or(FoldError::Corrupt("bcachefs member missing"))?;
        if header.block_size == 0
            || !header.block_size.is_power_of_two()
            || header.block_size > 128
            || member.bucket_size == 0
            || member.first_bucket as u64 >= member.nbuckets
            || member
                .nbuckets
                .checked_mul(u64::from(member.bucket_size))
                .and_then(|sectors| sectors.checked_mul(512))
                .is_none_or(|bytes| bytes > source.size)
        {
            return Err(FoldError::Corrupt(
                "bcachefs member geometry exceeds source",
            ));
        }
        let reader = result(&fault, Filesystem::open(source))?;
        let root = reader.root();
        if !result(&fault, reader.inode_at(root))?.is_dir() {
            return Err(FoldError::Corrupt("bcachefs root is not a directory"));
        }
        Ok(Self {
            reader,
            fault,
            nodes: vec![root],
            identities: BTreeMap::from([((root.snapshot, root.ino), 0)]),
        })
    }

    fn root(&self) -> NodeId {
        0
    }

    fn lookup(
        &mut self,
        parent: NodeId,
        name: &str,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        validate_name(name)?;
        result(
            &self.fault,
            self.reader.lookup_at(self.node(parent)?, name.as_bytes()),
        )?
        .map(|node| self.remember(node))
        .transpose()
    }

    fn read_dir(
        &mut self,
        parent: NodeId,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Vec<DirEntry>> {
        let parent = self.node(parent)?;
        let entries = result(&self.fault, self.reader.readdir_at(parent))?;
        if entries.len() > MAX_NODES {
            return Err(FoldError::Unsupported("bcachefs directory entry limit"));
        }
        let mut names = HashSet::new();
        let mut output = Vec::with_capacity(entries.len());
        for entry in entries {
            let name = std::str::from_utf8(&entry.name)
                .map_err(|_| FoldError::Unsupported("non-UTF-8 bcachefs name"))?;
            validate_name(name)?;
            if !names.insert(name.to_owned()) {
                return Err(FoldError::Corrupt("duplicate bcachefs directory name"));
            }
            let node = result(&self.fault, self.reader.dirent_node(parent, &entry))?;
            let inode = result(&self.fault, self.reader.inode_at(node))?;
            let file_kind = kind(&inode);
            if (entry.d_type == 16 && file_kind != FileKind::Directory)
                || (entry.d_type != 16 && entry.d_type != (inode.mode >> 12) as u8)
            {
                return Err(FoldError::Corrupt(
                    "bcachefs directory type disagrees with inode",
                ));
            }
            output.push(DirEntry {
                name: name.to_owned(),
                node: self.remember(node)?,
                kind: file_kind,
            });
        }
        Ok(output)
    }

    fn metadata(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.inode(node)?;
        Ok(Metadata {
            kind: kind(&inode),
            size: inode.size,
            mode: inode.mode,
        })
    }

    fn inode_number(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<u64> {
        Ok(self.node(node)?.ino)
    }

    fn timestamps(&mut self, node: NodeId, _context: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        let raw = result(&self.fault, self.reader.inode_raw_at(self.node(node)?))?;
        let header = self.header();
        let time = |index: usize| -> Result<Option<(i64, u32)>> {
            let Some(&value) = raw.varints.get(index * 2) else {
                return Ok(None);
            };
            if header.time_base_hi != 0
                || header.time_precision == 0
                || raw.varints.get(index * 2 + 1) != Some(&0)
            {
                return Err(FoldError::Unsupported(
                    "bcachefs extended timestamp encoding",
                ));
            }
            let nanos = i128::from(header.time_base_lo)
                + i128::from(value as i64) * i128::from(header.time_precision);
            let seconds = i64::try_from(nanos.div_euclid(1_000_000_000))
                .map_err(|_| FoldError::Corrupt("bcachefs timestamp overflow"))?;
            Ok(Some((seconds, nanos.rem_euclid(1_000_000_000) as u32)))
        };
        Ok(FileTimes {
            accessed: time(0)?,
            changed: time(1)?,
            modified: time(2)?,
            created: time(3)?,
        })
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        buffer: &mut [u8],
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        match kind(&self.inode(node)?) {
            FileKind::Regular => {}
            FileKind::Directory => return Err(FoldError::IsDirectory),
            FileKind::Symlink | FileKind::Other => {
                return Err(FoldError::Unsupported("bcachefs non-regular content"))
            }
        }
        let data = result(
            &self.fault,
            self.reader
                .read_range_at(self.node(node)?, offset, buffer.len()),
        )?;
        buffer[..data.len()].copy_from_slice(&data);
        Ok(data.len())
    }

    fn read_link(
        &mut self,
        node: NodeId,
        _context: &mut SubstrateCtx<'_>,
    ) -> Result<Option<Vec<u8>>> {
        let inode = self.inode(node)?;
        if !inode.is_symlink() {
            return Ok(None);
        }
        if inode.size > 4096 {
            return Err(FoldError::Corrupt("bcachefs symlink over 4096 bytes"));
        }
        let data = result(&self.fault, self.reader.read_at(self.node(node)?))?;
        if data.len() as u64 != inode.size || data.contains(&0) {
            return Err(FoldError::Corrupt("invalid bcachefs symlink"));
        }
        Ok(Some(data))
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']) {
        return Err(FoldError::Corrupt("invalid bcachefs name"));
    }
    Ok(())
}

struct Borrowed<'a, S>(&'a mut S);
impl<S: BlockSource> BlockSource for Borrowed<'_, S> {
    fn len(&self) -> u64 {
        self.0.len()
    }
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<()> {
        self.0.read_at(offset, buffer)
    }
}

#[cfg(test)]
mod large_tests;
#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod tests;
