//! Read-only Macintosh File System over raw or Disk Copy 4.2 evidence.
#![forbid(unsafe_code)]

mod directory;
mod geometry;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use lamfold::{
    BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, LocalFileTimes, Metadata, NodeId,
    Result, SubstrateCtx,
};

pub use directory::names_equal;
use directory::{File, ForkInfo};
pub use geometry::{Header, PROBE_BYTES};

pub use classic_hfs::ForkKind;
const ROOT: NodeId = 0;

pub struct Mfs<S> {
    source: S,
    header: Header,
    block_map: Vec<u16>,
    files: BTreeMap<NodeId, File>,
}

impl<S: BlockSource> Mfs<S> {
    fn header(source: &mut S) -> Result<Option<Header>> {
        let size = source.len();
        let mut bytes = [0; PROBE_BYTES];
        let len = size.min(PROBE_BYTES as u64) as usize;
        source.read_at(0, &mut bytes[..len])?;
        Header::locate(&bytes[..len], size, |offset, bytes| {
            source.read_at(offset, bytes)
        })
    }

    fn file(&self, node: NodeId) -> Result<&File> {
        if node == ROOT {
            return Err(FoldError::IsDirectory);
        }
        self.files.get(&node).ok_or(FoldError::NotFound)
    }

    fn fork(&self, node: NodeId, fork: ForkKind) -> Result<ForkInfo> {
        let selection = match fork {
            ForkKind::Data => 0,
            ForkKind::Resource => 1,
        };
        Ok(self.file(node)?.forks[selection])
    }

    pub fn fork_size(&self, node: NodeId, fork: ForkKind) -> Result<u64> {
        Ok(u64::from(self.fork(node, fork)?.length))
    }

    pub fn read_fork_at(
        &mut self,
        node: NodeId,
        fork: ForkKind,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize> {
        let fork = self.fork(node, fork)?;
        let count = u64::from(fork.length)
            .saturating_sub(offset)
            .min(bytes.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        let block_size = self.header.block_size;
        let blocks = self.fork_blocks(fork)?;
        let mut consumed = 0;
        while consumed < count {
            let logical = offset + consumed as u64;
            let block = u64::from(blocks[(logical / block_size) as usize] - 2);
            let within = logical % block_size;
            let length = (count - consumed).min((block_size - within) as usize);
            let source = self.header.allocation.start + block * block_size + within;
            self.source
                .read_at(source, &mut bytes[consumed..consumed + length])?;
            consumed += length;
        }
        Ok(count)
    }

    fn fork_blocks(&self, fork: ForkInfo) -> Result<Vec<u16>> {
        let size = self.header.block_size;
        let allocated = u64::from(fork.allocated);
        let count = allocated / size;
        require(
            fork.length <= fork.allocated
                && allocated.is_multiple_of(size)
                && count <= self.block_map.len() as u64,
            "MFS fork length disagrees with physical allocation",
        )?;
        let mut blocks = Vec::with_capacity(count as usize);
        let mut block = fork.first;
        for _ in 0..count {
            let index = block
                .checked_sub(2)
                .ok_or(FoldError::Corrupt("short MFS fork allocation chain"))?;
            let &next = self
                .block_map
                .get(usize::from(index))
                .filter(|&&next| next != 0 && next != 0xfff)
                .ok_or(FoldError::Corrupt(
                    "MFS fork references a free, reserved or invalid block",
                ))?;
            blocks.push(block);
            block = next;
        }
        require(
            block == 1,
            "MFS fork allocation chain is circular or longer than declared",
        )?;
        Ok(blocks)
    }
}

impl<S: BlockSource> FoldFrontend<S> for Mfs<S> {
    const TAG: &'static str = "mfs";

    fn probe(source: &mut S) -> Result<bool> {
        Ok(Self::header(source)?.is_some())
    }

    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header =
            Self::header(&mut source)?.ok_or(FoldError::Corrupt("MFS signature absent"))?;
        let block_map = header.load_map(&mut source)?;
        let files = directory::load(&mut source, &header)?;
        Ok(Self {
            source,
            header,
            block_map,
            files,
        })
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
        if dir != ROOT {
            return Err(FoldError::NotDirectory);
        }
        Ok(self
            .files
            .iter()
            .find(|(_, file)| names_equal(&file.name, name))
            .map(|(&id, _)| id))
    }

    fn read_dir(&mut self, dir: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        if dir != ROOT {
            return Err(FoldError::NotDirectory);
        }
        Ok(self
            .files
            .iter()
            .map(|(&node, file)| DirEntry {
                name: file.name.clone(),
                node,
                kind: FileKind::Regular,
            })
            .collect())
    }

    fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        if node == ROOT {
            return Ok(Metadata {
                size: 0,
                kind: FileKind::Directory,
                mode: 0,
            });
        }
        Ok(Metadata {
            size: self.fork_size(node, ForkKind::Data)?,
            kind: FileKind::Regular,
            mode: 0,
        })
    }

    fn local_timestamps(
        &mut self,
        node: NodeId,
        _: &mut SubstrateCtx<'_>,
    ) -> Result<LocalFileTimes> {
        if node == ROOT {
            return Ok(LocalFileTimes::default());
        }
        Ok(self.file(node)?.times)
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        self.read_fork_at(node, ForkKind::Data, offset, bytes)
    }
}

fn require(condition: bool, reason: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(reason))
    }
}

fn be16(bytes: &[u8], at: usize) -> u16 {
    u16::from_be_bytes(bytes[at..at + 2].try_into().unwrap())
}
fn be32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap())
}
