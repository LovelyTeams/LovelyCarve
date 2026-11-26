//! Native endianness and fragment-aware reads of AIX JFS1 filesystems.
#![forbid(unsafe_code)]

mod directory;
mod header;
mod pages;
#[cfg(test)]
mod tests;

use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};

pub use header::{HEADER_SIZE, Header, Layout, SUPERBLOCK_OFFSETS};

const PAGE: u64 = 4096;
const FIRST_INODES: u64 = 32 * PAGE;
const INODE_SIZE: u64 = 128;
const ROOT: u64 = 2;

#[derive(Clone)]
struct Inode {
    mode: u32,
    size: u64,
    times: FileTimes,
    addresses: [u8; 48],
}

impl Inode {
    fn kind(&self) -> FileKind {
        match self.mode & 0o170000 {
            0o040000 => FileKind::Directory,
            0o100000 => FileKind::Regular,
            0o120000 => FileKind::Symlink,
            _ => FileKind::Other,
        }
    }
}

pub struct AixJfs<S> {
    source: S,
    header: Header,
}

impl<S: BlockSource> AixJfs<S> {
    pub fn header(&self) -> &Header {
        &self.header
    }

    fn inode(&mut self, inode: u64) -> Result<Inode> {
        require(
            (ROOT..=u64::from(u32::MAX)).contains(&inode),
            "AIX JFS invalid inode number",
        )?;
        let ag = inode / self.header.inodes_per_group;
        let index = inode % self.header.inodes_per_group;
        let group_bytes = self.header.fragments_per_group * self.header.fragment_size;
        let base = if ag == 0 {
            FIRST_INODES
        } else {
            ag.checked_mul(group_bytes).ok_or(FoldError::Corrupt(
                "AIX JFS allocation group offset overflow",
            ))?
        };
        let offset = base
            .checked_add(index * INODE_SIZE)
            .ok_or(FoldError::Corrupt("AIX JFS inode offset overflow"))?;
        let mut bytes = [0; INODE_SIZE as usize];
        self.read_volume(offset, &mut bytes)?;
        let links = be16(&bytes, 8);
        let mode = be32(&bytes, 4) & 0xffff;
        require(
            mode != 0 && links != 0,
            "AIX JFS directory references a free inode",
        )?;
        let size = u64::from(be32(&bytes, 20));
        require(
            size <= 1 << 31,
            "AIX JFS large inode exceeds standard block geometry",
        )?;
        let time = |offset| {
            let seconds = i64::from(be32(&bytes, offset) as i32);
            let nanos = be32(&bytes, offset + 4);
            require(nanos < 1_000_000_000, "AIX JFS timestamp nanoseconds")?;
            Ok(Some((seconds, nanos)))
        };
        Ok(Inode {
            mode,
            size,
            times: FileTimes {
                modified: time(28)?,
                accessed: time(36)?,
                changed: time(44)?,
                created: None,
            },
            addresses: bytes[80..].try_into().unwrap(),
        })
    }

    fn read_volume(&mut self, offset: u64, bytes: &mut [u8]) -> Result<()> {
        require(
            offset
                .checked_add(bytes.len() as u64)
                .is_some_and(|end| end <= self.header.volume_size),
            "AIX JFS allocated bytes exceed volume",
        )?;
        self.source.read_at(offset, bytes)
    }
}

impl<S: BlockSource> FoldFrontend<S> for AixJfs<S> {
    const TAG: &'static str = "aixjfs";

    fn probe(source: &mut S) -> Result<bool> {
        Ok(Header::read(source)?.is_some())
    }

    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header = Header::read(&mut source)?
            .ok_or(FoldError::Corrupt("AIX JFS superblock magic absent"))?;
        if header.filesystem_type != 0 {
            return Err(FoldError::Unsupported("AIX JFS nonstandard block geometry"));
        }
        if header.compression > 0 {
            return Err(FoldError::Unsupported("AIX JFS compressed volume"));
        }
        let mut volume = Self { source, header };
        require(
            volume.inode(ROOT)?.kind() == FileKind::Directory,
            "AIX JFS root inode is not a directory",
        )?;
        Ok(volume)
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
        Ok(self
            .entries(dir)?
            .into_iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, id)| id))
    }

    fn read_dir(&mut self, dir: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.entries(dir)?
            .into_iter()
            .map(|(name, node)| {
                Ok(DirEntry {
                    name,
                    node,
                    kind: self.inode(node)?.kind(),
                })
            })
            .collect()
    }

    fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.inode(node)?;
        Ok(Metadata {
            kind: inode.kind(),
            size: inode.size,
            mode: inode.mode,
        })
    }

    fn timestamps(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
        Ok(self.inode(node)?.times)
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let inode = self.inode(node)?;
        if !matches!(inode.kind(), FileKind::Regular | FileKind::Symlink) {
            return Err(FoldError::Unsupported("AIX JFS node has no file contents"));
        }
        let count = inode.size.saturating_sub(offset).min(bytes.len() as u64) as usize;
        if count != 0 {
            self.read_inode(&inode, offset, &mut bytes[..count])?;
        }
        Ok(count)
    }

    fn read_link(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        let inode = self.inode(node)?;
        if inode.kind() != FileKind::Symlink {
            return Ok(None);
        }
        require(inode.size <= 65536, "AIX JFS link size limit")?;
        let mut bytes = vec![0; inode.size as usize];
        self.read_inode(&inode, 0, &mut bytes)?;
        Ok(Some(bytes))
    }
}

fn require(condition: bool, detail: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(detail))
    }
}

fn be16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn be32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
