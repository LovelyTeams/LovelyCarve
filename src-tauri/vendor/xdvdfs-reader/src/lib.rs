//! Seekable XDVDFS reads using the upstream Xbox DVD disk structures.
#![forbid(unsafe_code)]

mod directory;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use lamfold::{
    BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};
use xdvdfs::layout::{DiskRegion, VolumeDescriptor};
use xdvdfs::util::cmp_ignore_case_utf8;

pub use xdvdfs::layout::VOLUME_HEADER_MAGIC;
pub const SECTOR_SIZE: usize = xdvdfs::layout::SECTOR_SIZE as usize;
pub const DESCRIPTOR_OFFSET: u64 = 32 * SECTOR_SIZE as u64;
const DATA_START: u64 = DESCRIPTOR_OFFSET + SECTOR_SIZE as u64;
const ROOT: NodeId = 1;

pub fn inspect_descriptor(
    bytes: &[u8; SECTOR_SIZE],
    size: u64,
) -> Result<Option<VolumeDescriptor>> {
    if !bytes.starts_with(&VOLUME_HEADER_MAGIC) {
        return Ok(None);
    }
    let volume = VolumeDescriptor::deserialize::<()>(bytes)
        .map_err(|_| FoldError::Corrupt("XDVDFS volume descriptor encoding"))?;
    require(volume.is_valid(), "XDVDFS closing volume magic is absent")?;
    region_offset(volume.root_table.region, size)?;
    Ok(Some(volume))
}

fn read_descriptor(source: &mut impl BlockSource) -> Result<Option<VolumeDescriptor>> {
    if source.len() < DATA_START {
        return Ok(None);
    }
    let mut bytes = [0; SECTOR_SIZE];
    source.read_at(DESCRIPTOR_OFFSET, &mut bytes)?;
    inspect_descriptor(&bytes, source.len())
}

fn region_offset(region: DiskRegion, size: u64) -> Result<u64> {
    let start = u64::from(region.sector) * SECTOR_SIZE as u64;
    if region.size != 0 {
        require(
            start >= DATA_START && start + u64::from(region.size) <= size,
            "XDVDFS extent outside the game partition",
        )?;
    }
    Ok(start)
}

#[derive(Clone, Copy)]
struct Entry {
    region: DiskRegion,
    kind: FileKind,
}

pub struct Xdvdfs<S> {
    source: S,
    nodes: HashMap<NodeId, Entry>,
    directories: HashMap<NodeId, Vec<DirEntry>>,
    directory_sectors: HashSet<u32>,
}

impl<S: BlockSource> Xdvdfs<S> {
    fn node(&self, id: NodeId) -> Result<Entry> {
        self.nodes.get(&id).copied().ok_or(FoldError::NotFound)
    }
}

impl<S: BlockSource> FoldFrontend<S> for Xdvdfs<S> {
    const TAG: &'static str = "xdvdfs";

    fn probe(source: &mut S) -> Result<bool> {
        Ok(read_descriptor(source)?.is_some())
    }

    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let volume = read_descriptor(&mut source)?
            .ok_or(FoldError::Corrupt("XDVDFS volume magic is absent"))?;
        let region = volume.root_table.region;
        let root = Entry {
            region,
            kind: FileKind::Directory,
        };
        let mut directory_sectors = HashSet::new();
        if region.size != 0 {
            directory_sectors.insert(region.sector);
        }
        Ok(Self {
            source,
            nodes: HashMap::from([(ROOT, root)]),
            directories: HashMap::new(),
            directory_sectors,
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
        Ok(self
            .entries(dir)?
            .iter()
            .find(|entry| cmp_ignore_case_utf8(&entry.name, name).is_eq())
            .map(|entry| entry.node))
    }

    fn read_dir(&mut self, dir: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        Ok(self.entries(dir)?.to_vec())
    }

    fn metadata(&mut self, id: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let entry = self.node(id)?;
        let mode = if entry.kind == FileKind::Directory {
            0o040555
        } else {
            0o100444
        };
        Ok(Metadata {
            size: u64::from(entry.region.size),
            kind: entry.kind,
            mode,
        })
    }

    fn read_at(
        &mut self,
        id: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let entry = self.node(id)?;
        if entry.kind == FileKind::Directory {
            return Err(FoldError::IsDirectory);
        }
        let count = u64::from(entry.region.size)
            .saturating_sub(offset)
            .min(bytes.len() as u64) as usize;
        if count != 0 {
            let start = region_offset(entry.region, self.source.len())?;
            self.source.read_at(start + offset, &mut bytes[..count])?;
        }
        Ok(count)
    }
}

fn require(condition: bool, detail: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(detail))
    }
}
