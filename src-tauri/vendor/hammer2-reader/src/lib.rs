//! Read-only HAMMER2 over an immutable volume source.
#![forbid(unsafe_code)]

mod format;
mod media;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};

use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};

pub use format::{BLOCK_BYTES, Header, Order, ZONE_BYTES};
use format::{BlockRef, DIRECT_DATA, DIRENT_VISIBLE, Inode, Kind, PFS_ROOT, display_name};
use media::Volume;

const ROOT: NodeId = 0;

pub struct Hammer2<S> {
    volume: Volume<S>,
    nodes: Vec<Node>,
    pfs: Vec<NodeId>,
    identities: HashMap<(usize, u64), NodeId>,
    directories: HashMap<NodeId, BTreeMap<String, NodeId>>,
}

struct Node {
    scope: usize,
    inode: Inode,
}

impl<S: BlockSource> Hammer2<S> {
    fn header(source: &mut S) -> Result<Option<Header>> {
        let length = source.len();
        let mut head = [0; 8];
        let available = length.min(head.len() as u64) as usize;
        source.read_at(0, &mut head[..available])?;
        Header::locate(&head[..available], length, |at, bytes| {
            source.read_at(at, bytes)
        })
    }

    fn node(&self, id: NodeId) -> Result<&Node> {
        let index = id.checked_sub(1).and_then(|n| usize::try_from(n).ok());
        index
            .and_then(|n| self.nodes.get(n))
            .ok_or(FoldError::NotFound)
    }

    fn add_node(&mut self, scope: usize, reference: &BlockRef, inode: Inode) -> NodeId {
        let identity = (scope, reference.data_offset);
        *self.identities.entry(identity).or_insert_with(|| {
            self.nodes.push(Node { scope, inode });
            self.nodes.len() as NodeId
        })
    }

    fn mount(source: S, header: Header) -> Result<Self> {
        if header.volume_count != 1 || header.volume_id != 0 {
            return Err(FoldError::Unsupported("multi-volume HAMMER2 filesystem"));
        }
        let mut volume = Volume::new(source, header.order, header.volume_size);
        let [superroot] = &header.roots[..] else {
            return Err(FoldError::Corrupt("invalid HAMMER2 super-root blockset"));
        };
        let inode = volume.inode(superroot)?;
        require(
            inode.kind == FileKind::Directory && inode.number == 0,
            "invalid HAMMER2 super-root inode",
        )?;

        let mut fs = Self {
            volume,
            nodes: Vec::new(),
            pfs: Vec::new(),
            identities: HashMap::new(),
            directories: HashMap::new(),
        };
        let mut roots = BTreeMap::new();
        for reference in
            fs.volume
                .leaves(&inode.blockset, DIRENT_VISIBLE..=u64::MAX, Kind::Inode)?
        {
            let inode = fs.volume.inode(&reference)?;
            require(
                inode.kind == FileKind::Directory
                    && inode.flags & PFS_ROOT != 0
                    && inode.number == 1
                    && !inode.name.is_empty(),
                "invalid HAMMER2 PFS root",
            )?;
            let name = inode.name.clone();
            let root = fs.add_node(fs.pfs.len(), &reference, inode);
            fs.pfs.push(root);
            require(
                roots.insert(name, root).is_none(),
                "duplicate HAMMER2 PFS name",
            )?;
        }
        require(!roots.is_empty(), "no readable HAMMER2 PFS roots")?;
        fs.directories.insert(ROOT, roots);
        Ok(fs)
    }

    fn load_directory(&mut self, id: NodeId) -> Result<BTreeMap<String, NodeId>> {
        let dir = self.node(id)?;
        if dir.inode.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        let scope = dir.scope;
        let roots = dir.inode.blockset.clone();
        let mut names = BTreeMap::new();
        for entry in self.volume.directory_leaves(&roots)? {
            let (name, reference, inode) = if entry.kind == Kind::Dirent {
                let (name, number, kind) = self.volume.dirent(&entry)?;
                let root = self.node(self.pfs[scope])?.inode.blockset.clone();
                let candidates = self.volume.leaves(&root, number..=number, Kind::Inode)?;
                let [reference] = &candidates[..] else {
                    return Err(FoldError::Corrupt(
                        "missing or ambiguous HAMMER2 directory target inode",
                    ));
                };
                let inode = self.volume.inode(reference)?;
                require(
                    inode.number == number && inode.raw_kind == kind,
                    "HAMMER2 directory target does not match inode",
                )?;
                (name, reference.clone(), inode)
            } else {
                let inode = self.volume.inode(&entry)?;
                (inode.name.clone(), entry, inode)
            };
            require(!name.is_empty(), "empty HAMMER2 entry name")?;
            let child = self.add_node(scope, &reference, inode);
            require(
                child != id && names.insert(name, child).is_none(),
                "cyclic or duplicate HAMMER2 directory entry",
            )?;
        }
        Ok(names)
    }

    fn children(&mut self, id: NodeId) -> Result<&BTreeMap<String, NodeId>> {
        if !self.directories.contains_key(&id) {
            let entries = self.load_directory(id)?;
            self.directories.insert(id, entries);
        }
        Ok(&self.directories[&id])
    }

    fn read_content(&mut self, id: NodeId, offset: u64, out: &mut [u8]) -> Result<usize> {
        if id == ROOT {
            return Err(FoldError::IsDirectory);
        }
        let inode = &self.node(id)?.inode;
        match inode.kind {
            FileKind::Directory => return Err(FoldError::IsDirectory),
            FileKind::Other => return Err(FoldError::Unsupported("HAMMER2 special file content")),
            _ => {}
        }
        let count = inode.size.saturating_sub(offset).min(out.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        if inode.flags & DIRECT_DATA != 0 {
            out[..count].copy_from_slice(&inode.inline[offset as usize..offset as usize + count]);
        } else {
            let eof = inode.size;
            let roots = inode.blockset.clone();
            self.volume
                .read_file(&roots, eof, offset, &mut out[..count])?;
        }
        Ok(count)
    }
}

impl<S: BlockSource> FoldFrontend<S> for Hammer2<S> {
    const TAG: &'static str = "hammer2";

    fn probe(src: &mut S) -> Result<bool> {
        Ok(Self::header(src)?.is_some())
    }

    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header =
            Self::header(&mut source)?.ok_or(FoldError::Corrupt("HAMMER2 volume header absent"))?;
        Self::mount(source, header)
    }

    fn root(&self) -> NodeId {
        ROOT
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        _: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self.children(dir)?.get(name).copied())
    }

    fn read_dir(&mut self, dir: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.children(dir)?;
        self.directories[&dir]
            .iter()
            .map(|(name, &node)| {
                Ok(DirEntry {
                    name: name.clone(),
                    node,
                    kind: self.node(node)?.inode.kind,
                })
            })
            .collect()
    }

    fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        if node == ROOT {
            return Ok(Metadata {
                kind: FileKind::Directory,
                size: 0,
                mode: 0,
            });
        }
        let inode = &self.node(node)?.inode;
        Ok(Metadata {
            kind: inode.kind,
            size: inode.size,
            mode: inode.mode,
        })
    }

    fn inode_number(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<u64> {
        if node == ROOT {
            return Ok(0);
        }
        Ok(self.node(node)?.inode.number)
    }

    fn timestamps(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        if node == ROOT {
            return Ok(FileTimes::default());
        }
        Ok(self.node(node)?.inode.times)
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        self.read_content(node, offset, bytes)
    }

    fn read_link(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        if node == ROOT || self.node(node)?.inode.kind != FileKind::Symlink {
            return Ok(None);
        }
        let size = self.node(node)?.inode.size;
        if size > 65_536 {
            return Err(FoldError::FileTooLarge { size, max: 65_536 });
        }
        let mut bytes = vec![0; size as usize];
        let count = self.read_content(node, 0, &mut bytes)?;
        require(count == bytes.len(), "incomplete HAMMER2 symbolic link")?;
        Ok(Some(bytes))
    }
}

fn require(condition: bool, message: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(message))
    }
}
