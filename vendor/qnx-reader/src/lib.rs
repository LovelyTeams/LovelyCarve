//! Read-only QNX4 extent and QNX6 Power-Safe tree readers. Source reads are
//! exact and bounded; no mounting, journal replay or host path traversal.
#![forbid(unsafe_code)]
mod qnx4;
mod qnx6;
#[cfg(test)]
mod tests;

use lamfold::{
    BlockSource, DirEntry, FileKind, FileTimes, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};
pub use qnx4::Qnx4;
pub use qnx6::Qnx6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Endian {
    Little,
    Big,
}
impl Endian {
    fn u16(self, bytes: &[u8], offset: usize) -> Result<u16> {
        let value = bytes
            .get(offset..offset + 2)
            .ok_or(FoldError::Corrupt("QNX short u16"))?;
        Ok(match self {
            Self::Little => u16::from_le_bytes([value[0], value[1]]),
            Self::Big => u16::from_be_bytes([value[0], value[1]]),
        })
    }
    fn u32(self, bytes: &[u8], offset: usize) -> Result<u32> {
        let value: [u8; 4] = bytes
            .get(offset..offset + 4)
            .ok_or(FoldError::Corrupt("QNX short u32"))?
            .try_into()
            .map_err(|_| FoldError::Corrupt("QNX u32"))?;
        Ok(match self {
            Self::Little => u32::from_le_bytes(value),
            Self::Big => u32::from_be_bytes(value),
        })
    }
    fn u64(self, bytes: &[u8], offset: usize) -> Result<u64> {
        let value: [u8; 8] = bytes
            .get(offset..offset + 8)
            .ok_or(FoldError::Corrupt("QNX short u64"))?
            .try_into()
            .map_err(|_| FoldError::Corrupt("QNX u64"))?;
        Ok(match self {
            Self::Little => u64::from_le_bytes(value),
            Self::Big => u64::from_be_bytes(value),
        })
    }
}
fn require(condition: bool, reason: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(reason))
    }
}
fn exact(source: &mut impl BlockSource, offset: u64, bytes: &mut [u8]) -> Result<()> {
    require(
        offset
            .checked_add(bytes.len() as u64)
            .is_some_and(|end| end <= source.len()),
        "QNX source range/truncation",
    )?;
    source.read_at(offset, bytes)
}
fn kind(mode: u16) -> FileKind {
    match mode & 0xf000 {
        0x8000 => FileKind::Regular,
        0x4000 => FileKind::Directory,
        0xa000 => FileKind::Symlink,
        _ => FileKind::Other,
    }
}
fn times(order: Endian, bytes: &[u8], start: usize) -> Result<FileTimes> {
    let read = |offset| {
        order
            .u32(bytes, offset)
            .map(|value| Some((value as i64, 0)))
    };
    Ok(FileTimes {
        created: read(start)?,
        modified: read(start + 4)?,
        accessed: read(start + 8)?,
        changed: read(start + 12)?,
    })
}
fn name(bytes: &[u8]) -> Result<String> {
    require(
        !bytes.is_empty()
            && !bytes.contains(&0)
            && !bytes.contains(&b'/')
            && !bytes.contains(&b'\\'),
        "QNX invalid name",
    )?;
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_| FoldError::Unsupported("QNX non-UTF-8 filename"))
}
#[derive(Clone)]
struct Info {
    metadata: Metadata,
    times: FileTimes,
}

macro_rules! frontend {
    ($reader:ident, $tag:literal) => {
        impl<S: BlockSource> FoldFrontend<S> for $reader<S> {
            const TAG: &'static str = $tag;
            fn probe(source: &mut S) -> Result<bool> {
                Self::probe_source(source)
            }
            fn open(source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
                Self::open_source(source)
            }
            fn root(&self) -> NodeId {
                self.root_node()
            }
            fn lookup(
                &mut self,
                directory: NodeId,
                name: &str,
                _: &mut SubstrateCtx<'_>,
            ) -> Result<Option<NodeId>> {
                Ok(self
                    .directory(directory)?
                    .into_iter()
                    .find(|entry| entry.name == name)
                    .map(|entry| entry.node))
            }
            fn read_dir(
                &mut self,
                node: NodeId,
                _: &mut SubstrateCtx<'_>,
            ) -> Result<Vec<DirEntry>> {
                self.directory(node)
            }
            fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
                Ok(self.info(node)?.metadata)
            }
            fn timestamps(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<FileTimes> {
                Ok(self.info(node)?.times)
            }
            fn read_at(
                &mut self,
                node: NodeId,
                offset: u64,
                bytes: &mut [u8],
                _: &mut SubstrateCtx<'_>,
            ) -> Result<usize> {
                require(
                    matches!(
                        self.info(node)?.metadata.kind,
                        FileKind::Regular | FileKind::Symlink
                    ),
                    "QNX node has no file content",
                )?;
                self.content(node, offset, bytes)
            }
            fn read_link(
                &mut self,
                node: NodeId,
                _: &mut SubstrateCtx<'_>,
            ) -> Result<Option<Vec<u8>>> {
                let metadata = self.info(node)?.metadata;
                if metadata.kind != FileKind::Symlink {
                    return Ok(None);
                }
                require(metadata.size <= 65536, "QNX symlink size limit")?;
                let mut bytes = vec![0; metadata.size as usize];
                require(
                    self.content(node, 0, &mut bytes)? == bytes.len(),
                    "QNX short symlink",
                )?;
                Ok(Some(bytes))
            }
        }
    };
}
frontend!(Qnx4, "qnx4");
frontend!(Qnx6, "qnx6");
