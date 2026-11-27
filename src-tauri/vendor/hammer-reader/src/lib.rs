//! Read-only HAMMER (the original DragonFly BSD filesystem).
#![forbid(unsafe_code)]

mod format;
#[cfg(test)]
mod tests;
mod tree;
mod undo;

use std::{
    collections::{BTreeMap, HashMap},
    ops::RangeInclusive,
};

use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};

use format::{
    DATA, DATA_MAX, DIRENT, FIX, INODE, Inode, Key, LOCAL_INODE, LOCAL_MISC, SNAPSHOT, display_name,
};
pub use format::{Header, Order, PROBE_BYTES};
use tree::Media;

const ROOT: NodeId = 0;
const SNAPSHOTS: NodeId = NodeId::MAX;
const CURRENT: u64 = u64::MAX;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct View {
    pfs: u32,
    object: i64,
    as_of: u64,
}

impl View {
    fn key_range(
        self,
        locality: u32,
        kind: u16,
        position: RangeInclusive<i64>,
    ) -> RangeInclusive<Key> {
        let base = Key {
            localization: self.pfs | locality,
            object: self.object,
            kind,
            position: *position.start(),
        };
        base..=Key {
            position: *position.end(),
            ..base
        }
    }
}

struct Node {
    view: View,
    inode: Inode,
}

pub struct Hammer<S> {
    media: Media<S>,
    nodes: Vec<Node>,
    node_ids: HashMap<View, NodeId>,
    directories: HashMap<NodeId, BTreeMap<String, NodeId>>,
}

impl<S: BlockSource> Hammer<S> {
    fn header(source: &mut S) -> Result<Option<Header>> {
        let length = source.len();
        let mut bytes = [0; PROBE_BYTES];
        let count = length.min(PROBE_BYTES as u64) as usize;
        source.read_at(0, &mut bytes[..count])?;
        Header::parse(&bytes[..count], length)
    }

    fn node(&self, id: NodeId) -> Result<&Node> {
        usize::try_from(id)
            .ok()
            .and_then(|index| self.nodes.get(index))
            .ok_or(FoldError::NotFound)
    }

    fn resolve_inode(&mut self, view: View) -> Result<NodeId> {
        if let Some(&id) = self.node_ids.get(&view) {
            return Ok(id);
        }
        let found = self
            .media
            .records(view.key_range(LOCAL_INODE, INODE, 0..=0), view.as_of)?;
        let [record] = &found[..] else {
            return Err(FoldError::Corrupt("missing HAMMER inode record"));
        };
        let inode = Inode::decode(&self.media.payload(*record)?, self.media.header.order)?;
        require(
            inode.raw_kind == record.object_kind,
            "HAMMER inode type conflicts with its record",
        )?;
        let id = self.nodes.len() as NodeId;
        self.nodes.push(Node { view, inode });
        self.node_ids.insert(view, id);
        Ok(id)
    }

    fn read_snapshot_list(&mut self) -> Result<BTreeMap<String, NodeId>> {
        let view = self.node(ROOT)?.view;
        let records = self.media.records(
            view.key_range(LOCAL_MISC, SNAPSHOT, i64::MIN..=i64::MAX),
            CURRENT,
        )?;
        let order = self.media.header.order;
        let mut snapshots = BTreeMap::new();
        for record in records {
            let payload = self.media.payload(record)?;
            require(payload.len() == 128, "invalid HAMMER snapshot record size")?;
            let tid = order.u64(&payload);
            require(
                tid != 0 && tid != CURRENT && record.key.position as u64 == tid,
                "invalid HAMMER snapshot transaction",
            )?;
            let root = self.resolve_inode(View { as_of: tid, ..view })?;
            require(
                self.node(root)?.inode.kind == FileKind::Directory,
                "invalid HAMMER snapshot root inode",
            )?;
            snapshots.insert(format!("{tid:016x}"), root);
        }
        Ok(snapshots)
    }

    fn read_directory(&mut self, id: NodeId) -> Result<BTreeMap<String, NodeId>> {
        let dir = self.node(id)?;
        if dir.inode.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        let view = dir.view;
        let locality = if dir.inode.local_dirents {
            LOCAL_INODE
        } else {
            LOCAL_MISC
        };
        let records = self.media.records(
            view.key_range(locality, DIRENT, i64::MIN..=i64::MAX),
            view.as_of,
        )?;
        let order = self.media.header.order;
        let mut names = BTreeMap::new();
        for record in records {
            let bytes = self.media.payload(record)?;
            require(
                (17..=271).contains(&bytes.len()) && !bytes[16..].contains(&0),
                "invalid HAMMER directory record name",
            )?;
            let object = order.u64(&bytes) as i64;
            let pfs = order.u32(&bytes[8..]);
            require(
                object > 0 && pfs & 65535 == 0,
                "invalid HAMMER directory record target",
            )?;
            let node = self.resolve_inode(View {
                pfs,
                object,
                as_of: view.as_of,
            })?;
            require(
                node != id && self.node(node)?.inode.raw_kind == record.object_kind,
                "cyclic or mismatched HAMMER directory entry",
            )?;
            require(
                names.insert(display_name(&bytes[16..]), node).is_none(),
                "duplicate HAMMER directory name",
            )?;
        }
        if id == ROOT && self.media.header.version >= 3 {
            let snapshots = self.read_snapshot_list()?;
            if !snapshots.is_empty() {
                self.directories.insert(SNAPSHOTS, snapshots);
                names.insert("@snapshots".into(), SNAPSHOTS);
            }
        }
        Ok(names)
    }

    fn children(&mut self, id: NodeId) -> Result<&BTreeMap<String, NodeId>> {
        if !self.directories.contains_key(&id) {
            let entries = self.read_directory(id)?;
            self.directories.insert(id, entries);
        }
        Ok(&self.directories[&id])
    }

    fn link_bytes(&mut self, id: NodeId) -> Result<Option<Vec<u8>>> {
        if id == SNAPSHOTS {
            return Ok(None);
        }
        let node = self.node(id)?;
        if node.inode.kind != FileKind::Symlink {
            return Ok(None);
        }
        if let Some(bytes) = &node.inode.link {
            return Ok(Some(bytes.clone()));
        }
        let size = node.inode.size;
        let records = self
            .media
            .records(node.view.key_range(LOCAL_MISC, FIX, 1..=1), node.view.as_of)?;
        let [record] = &records[..] else {
            return Err(FoldError::Corrupt("HAMMER symlink has no target record"));
        };
        require(
            size == record.length as u64,
            "HAMMER symlink target length mismatch",
        )?;
        Ok(Some(self.media.payload(*record)?))
    }

    fn read_data(&mut self, view: View, size: u64, offset: u64, bytes: &mut [u8]) -> Result<()> {
        let end = offset + bytes.len() as u64;
        let min = offset as i64 + 1;
        let max = (end.saturating_add(DATA_MAX as u64 - 1).min(i64::MAX as u64)) as i64;
        let records = self
            .media
            .records(view.key_range(LOCAL_MISC, DATA, min..=max), view.as_of)?;
        bytes.fill(0);
        let mut end_of_previous = 0;
        for record in records {
            let extent_end = record.key.position as u64;
            let start = extent_end
                .checked_sub(record.length as u64)
                .ok_or(FoldError::Corrupt("invalid HAMMER data extent"))?;
            // The kernel may leave the final block partially allocated past EOF.
            require(
                start >= end_of_previous && record.length > 0 && start < size,
                "overlapping or out-of-bounds HAMMER data extents",
            )?;
            end_of_previous = extent_end;
            if start >= end {
                break;
            }
            let block = self.media.payload(record)?;
            let from = start.max(offset);
            let to = extent_end.min(end);
            bytes[(from - offset) as usize..(to - offset) as usize]
                .copy_from_slice(&block[(from - start) as usize..(to - start) as usize]);
        }
        Ok(())
    }
}

impl<S: BlockSource> FoldFrontend<S> for Hammer<S> {
    const TAG: &'static str = "hammer";

    fn probe(source: &mut S) -> Result<bool> {
        Ok(Self::header(source)?.is_some())
    }

    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header =
            Self::header(&mut source)?.ok_or(FoldError::Corrupt("HAMMER volume header absent"))?;
        header.validate_mount(&mut source)?;
        let mut fs = Self {
            media: Media::new(source, header),
            nodes: Vec::new(),
            node_ids: HashMap::new(),
            directories: HashMap::new(),
        };
        let root = fs.resolve_inode(View {
            pfs: 0,
            object: 1,
            as_of: CURRENT,
        })?;
        require(
            root == ROOT && fs.node(root)?.inode.kind == FileKind::Directory,
            "invalid HAMMER root inode",
        )?;
        Ok(fs)
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
                    kind: if node == SNAPSHOTS {
                        FileKind::Directory
                    } else {
                        self.node(node)?.inode.kind
                    },
                })
            })
            .collect()
    }

    fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        if node == SNAPSHOTS {
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
        if node == SNAPSHOTS {
            return Ok(0);
        }
        Ok(self.node(node)?.view.object as u64)
    }

    fn timestamps(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        if node == SNAPSHOTS {
            return Ok(FileTimes::default());
        }
        Ok(self.node(node)?.inode.times)
    }

    fn read_link(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        self.link_bytes(node)
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        if node == SNAPSHOTS {
            return Err(FoldError::IsDirectory);
        }
        let object = self.node(node)?;
        if object.inode.kind == FileKind::Directory {
            return Err(FoldError::IsDirectory);
        }
        if !matches!(object.inode.raw_kind, 2 | 7) {
            return Err(FoldError::Unsupported("HAMMER special file content"));
        }
        let count = object
            .inode
            .size
            .saturating_sub(offset)
            .min(bytes.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        if object.inode.kind == FileKind::Symlink {
            let link = self
                .link_bytes(node)?
                .ok_or(FoldError::Corrupt("HAMMER symbolic link target unreadable"))?;
            bytes[..count].copy_from_slice(&link[offset as usize..offset as usize + count]);
        } else {
            self.read_data(object.view, object.inode.size, offset, &mut bytes[..count])?;
        }
        Ok(count)
    }
}

fn require(condition: bool, message: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(message))
    }
}
