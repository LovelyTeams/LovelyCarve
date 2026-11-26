//! Immutable VxFS fileset access through the shared bounded block-source API.
#![forbid(unsafe_code)]

mod allocation;
mod directory;
mod header;
#[cfg(test)]
mod tests;
mod volume;

use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};

pub use header::{Endian, HEADER_SIZE, Header, SUPERBLOCKS};
pub use volume::Vxfs;

const ROOT: u32 = 2;
const INODE_SIZE: usize = 256;
const FILESET_HEADER: u32 = 0x10000000;
const INODE_LIST: u32 = 0x20000000;
const INODE_TYPE_MASK: u32 = !0xfff;
const ORG_NONE: u8 = 0;
const ORG_DIRECT: u8 = 1;
const ORG_IMMEDIATE: u8 = 2;
const ORG_TYPED: u8 = 3;

#[derive(Clone, Debug)]
struct Inode {
    mode: u32,
    links: u32,
    size: u64,
    times: FileTimes,
    organization: u8,
    data: [u8; 96],
}

impl Inode {
    fn parse(raw: &[u8; INODE_SIZE], endian: Endian) -> Result<Self> {
        let time = |offset| {
            let seconds = endian.u32(raw, offset);
            let micros = endian.u32(raw, offset + 4);
            require(micros < 1_000_000, "VxFS inode timestamp precision")?;
            Ok(Some((i64::from(seconds), micros * 1000)))
        };
        let organization = raw[49];
        let size = endian.u64(raw, 16);
        require(
            organization != ORG_IMMEDIATE || size <= 96,
            "VxFS immediate inode exceeds inline data",
        )?;
        Ok(Self {
            mode: endian.u32(raw, 0),
            links: endian.u32(raw, 4),
            size,
            times: FileTimes {
                modified: time(32)?,
                accessed: time(24)?,
                changed: time(40)?,
                created: None,
            },
            organization,
            data: raw[80..176].try_into().unwrap(),
        })
    }

    fn kind(&self) -> FileKind {
        match self.mode & INODE_TYPE_MASK {
            0o040000 => FileKind::Directory,
            0o100000 => FileKind::Regular,
            0o120000 => FileKind::Symlink,
            _ => FileKind::Other,
        }
    }
}

impl<S: BlockSource> FoldFrontend<S> for Vxfs<S> {
    const TAG: &'static str = "vxfs";

    fn probe(source: &mut S) -> Result<bool> {
        for (offset, endian) in SUPERBLOCKS {
            if offset + HEADER_SIZE as u64 > source.len() {
                continue;
            }
            let mut bytes = [0; HEADER_SIZE];
            source.read_at(offset, &mut bytes)?;
            if Header::inspect(&bytes, offset, endian, source.len())?.is_some() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn open(source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        Self::open_volume(source)
    }

    fn root(&self) -> NodeId {
        u64::from(ROOT)
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        _: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self
            .directory(dir)?
            .into_iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, inode)| u64::from(inode)))
    }

    fn read_dir(&mut self, dir: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.directory(dir)?
            .into_iter()
            .map(|(name, inode)| {
                let node = u64::from(inode);
                let kind = self.user_inode(node)?.kind();
                Ok(DirEntry { name, node, kind })
            })
            .collect()
    }

    fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.user_inode(node)?;
        Ok(Metadata {
            size: inode.size,
            kind: inode.kind(),
            mode: inode.mode & 0xffff,
        })
    }

    fn timestamps(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        Ok(self.user_inode(node)?.times)
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let inode = self.user_inode(node)?;
        if !matches!(inode.kind(), FileKind::Regular | FileKind::Symlink) {
            return Err(FoldError::Unsupported("VxFS node has no file content"));
        }
        let len = inode.size.saturating_sub(offset).min(bytes.len() as u64) as usize;
        if len != 0 {
            self.read_user_bytes(node, &inode, offset, &mut bytes[..len])?;
        }
        Ok(len)
    }

    fn read_link(&mut self, node: NodeId, cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        let inode = self.user_inode(node)?;
        if inode.kind() != FileKind::Symlink {
            return Ok(None);
        }
        require(inode.size <= 65536, "VxFS symlink target size")?;
        let mut bytes = vec![0; inode.size as usize];
        self.read_at(node, 0, &mut bytes, cx)?;
        Ok(Some(bytes))
    }
}

fn require(condition: bool, reason: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(reason))
    }
}
