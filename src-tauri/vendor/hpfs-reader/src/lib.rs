//! Read-only High Performance File System volumes through bounded block sources.
#![forbid(unsafe_code)]

mod allocation;
mod directory;
mod header;
mod names;
#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use allocation::Extent;
pub use header::{HEADER_LENGTH, HEADER_OFFSET, Header};
use lamfold::{
    BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, LocalFileTimes, Metadata, NodeId,
    Result, SubstrateCtx,
};

const SECTOR_SIZE: usize = 512;
const ROOT: NodeId = 0;
const DIRECTORY_SIZE: usize = 4 * SECTOR_SIZE;
const FNODE_MAGIC: u32 = 0xf7e40aae;

pub struct Hpfs<S> {
    source: S,
    header: Header,
    hotfixes: BTreeMap<u32, u32>,
    code_pages: BTreeMap<u16, u16>,
    last_map: Option<(u32, Vec<Extent>)>,
}

impl<S: BlockSource> Hpfs<S> {
    fn read_bytes(&mut self, offset: u64, mut bytes: &mut [u8]) -> Result<()> {
        let sector_size = SECTOR_SIZE as u64;
        let total = u64::from(self.header.sectors) * sector_size;
        require(
            offset
                .checked_add(bytes.len() as u64)
                .is_some_and(|end| end <= total),
            "HPFS read exceeds volume",
        )?;
        if self.hotfixes.is_empty() {
            return self.source.read_at(offset, bytes);
        }
        let mut position = offset;
        while !bytes.is_empty() {
            let sector = (position / sector_size) as u32;
            let within = position % sector_size;
            let (physical, count) = if let Some(&replacement) = self.hotfixes.get(&sector) {
                (
                    u64::from(replacement) * sector_size + within,
                    sector_size - within,
                )
            } else {
                let next = self
                    .hotfixes
                    .range(sector..)
                    .next()
                    .map_or(total, |(&next, _)| u64::from(next) * sector_size);
                (position, next - position)
            };
            let count = count.min(bytes.len() as u64) as usize;
            let (head, tail) = bytes.split_at_mut(count);
            self.source.read_at(physical, head)?;
            position += count as u64;
            bytes = tail;
        }
        Ok(())
    }

    fn sector<const N: usize>(&mut self, sector: u32) -> Result<[u8; N]> {
        require(sector >= 20, "HPFS file structure in reserved sectors")?;
        let mut bytes = [0; N];
        self.read_bytes(u64::from(sector) * SECTOR_SIZE as u64, &mut bytes)?;
        Ok(bytes)
    }

    fn read_fnode(&mut self, sector: u32) -> Result<[u8; SECTOR_SIZE]> {
        let bytes = self.sector(sector)?;
        require(le32(&bytes, 0) == FNODE_MAGIC, "HPFS fnode signature")?;
        Ok(bytes)
    }
}

impl<S: BlockSource> FoldFrontend<S> for Hpfs<S> {
    const TAG: &'static str = "hpfs";

    fn probe(source: &mut S) -> Result<bool> {
        Ok(Header::read(source)?.is_some())
    }

    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header =
            Header::read(&mut source)?.ok_or(FoldError::Corrupt("HPFS signature absent"))?;
        if !matches!(header.functional_version, 2 | 3) {
            return Err(FoldError::Unsupported("HPFS functional version"));
        }
        let hotfixes = header.read_hotfixes(&mut source)?;
        let mut volume = Self {
            source,
            header,
            hotfixes,
            code_pages: BTreeMap::new(),
            last_map: None,
        };
        volume.read_code_pages()?;
        volume.record(ROOT)?;
        Ok(volume)
    }

    fn root(&self) -> NodeId {
        ROOT
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        cx: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        let entries = self.read_dir(dir, cx)?;
        let mut folded = None;
        let mut ambiguous = false;
        let folded_name = name.to_lowercase();
        for entry in entries {
            if entry.name == name {
                return Ok(Some(entry.node));
            }
            if entry.name.to_lowercase() == folded_name {
                ambiguous |= folded.is_some();
                folded = Some(entry.node);
            }
        }
        require(!ambiguous, "HPFS ambiguous case-insensitive filename")?;
        Ok(folded)
    }

    fn read_dir(&mut self, dir: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        self.directory(dir)
    }

    fn metadata(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let entry = self.record(node)?;
        let kind = entry.kind();
        Ok(Metadata {
            kind,
            size: if kind == FileKind::Directory {
                0
            } else {
                u64::from(entry.size)
            },
            mode: 0,
        })
    }

    fn inode_identity(&mut self, node: NodeId, _: &mut SubstrateCtx<'_>) -> Result<NodeId> {
        Ok(u64::from(self.record(node)?.fnode))
    }

    fn local_timestamps(
        &mut self,
        node: NodeId,
        _: &mut SubstrateCtx<'_>,
    ) -> Result<LocalFileTimes> {
        let entry = self.record(node)?;
        let time = |value: u32| (value != 0).then_some((i64::from(value), 0));
        Ok(LocalFileTimes {
            modified: time(entry.modified),
            accessed: time(entry.accessed),
            changed: None,
            created: time(entry.created),
        })
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        self.read_file(node, offset, bytes)
    }
}

fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

fn le32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn require(condition: bool, reason: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(reason))
    }
}
