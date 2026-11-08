//! QNX4 512-byte blocks, inline 64-byte inodes and one-based extent chains.
use super::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
const LE: Endian = Endian::Little;
#[derive(Clone)]
struct Entry {
    info: Info,
    extents: Vec<(u64, u64)>,
    logical_ends: Vec<u64>,
}
impl Entry {
    fn physical(&self, offset: u64) -> Result<(u64, u64)> {
        let index = self.logical_ends.partition_point(|end| *end <= offset);
        let (physical, length) = self
            .extents
            .get(index)
            .ok_or(FoldError::Corrupt("QNX4 logical extent bounds"))?;
        let logical = self.logical_ends[index] - length;
        Ok((
            physical + offset - logical,
            self.logical_ends[index] - offset,
        ))
    }
}
pub struct Qnx4<S> {
    source: S,
    entries: BTreeMap<u64, Arc<Entry>>,
    extent_count: usize,
}
impl<S: BlockSource> Qnx4<S> {
    fn raw(&mut self, node: u64) -> Result<[u8; 64]> {
        let offset = node
            .checked_mul(64)
            .ok_or(FoldError::Corrupt("QNX4 inode offset"))?;
        let mut bytes = [0; 64];
        exact(&mut self.source, offset, &mut bytes)?;
        require(
            bytes[63] & 1 != 0 && bytes[63] & 8 == 0,
            "QNX4 unallocated/link inode target",
        )?;
        Ok(bytes)
    }
    fn extent(&self, block: u32, count: u32) -> Result<(u64, u64)> {
        require(block > 0 && count > 0, "QNX4 empty allocated extent")?;
        let offset = (block as u64 - 1) * 512;
        let length = count as u64 * 512;
        require(
            offset
                .checked_add(length)
                .is_some_and(|end| end <= self.source.len()),
            "QNX4 extent outside volume",
        )?;
        Ok((offset, length))
    }
    fn entry(&mut self, node: u64) -> Result<Arc<Entry>> {
        if let Some(entry) = self.entries.get(&node) {
            return Ok(Arc::clone(entry));
        }
        require(self.entries.len() < 100_000, "QNX4 inode cache limit")?;
        let bytes = self.raw(node)?;
        let size = LE.u32(&bytes, 16)? as u64;
        let mode = LE.u16(&bytes, 50)?;
        let count = LE.u16(&bytes, 48)? as usize;
        let mut extents = Vec::new();
        let mut chain = LE.u32(&bytes, 28)?;
        let first = (LE.u32(&bytes, 20)?, LE.u32(&bytes, 24)?);
        if count > 0 && first != (0, 0) {
            extents.push(self.extent(first.0, first.1)?);
        }
        require(
            !extents.is_empty() || (size == 0 && count <= 1 && chain == 0),
            "QNX4 missing first extent",
        )?;
        let mut seen = BTreeSet::new();
        let mut previous = 0;
        while extents.len() < count && (size > 0 || first != (0, 0)) {
            require(
                chain != 0 && seen.insert(chain),
                "QNX4 extent chain missing/cyclic",
            )?;
            let (offset, _) = self.extent(chain, 1)?;
            let mut block = [0; 512];
            exact(&mut self.source, offset, &mut block)?;
            require(
                &block[496..503] == b"IamXblk" && LE.u32(&block, 4)? == previous,
                "QNX4 extent block signature/backlink",
            )?;
            let entries = block[8] as usize;
            require(
                (1..=60).contains(&entries) && entries <= count - extents.len(),
                "QNX4 extent block count",
            )?;
            for index in 0..entries {
                extents.push(self.extent(
                    LE.u32(&block, 16 + index * 8)?,
                    LE.u32(&block, 20 + index * 8)?,
                )?);
            }
            previous = chain;
            chain = LE.u32(&block, 0)?;
        }
        require(
            extents.iter().map(|(_, length)| *length).sum::<u64>() >= size,
            "QNX4 extents shorter than file",
        )?;
        let mut ordered = extents.clone();
        ordered.sort_unstable();
        require(
            ordered
                .windows(2)
                .all(|pair| pair[0].0 + pair[0].1 <= pair[1].0),
            "QNX4 overlapping file extents",
        )?;
        require(
            self.extent_count + extents.len() <= 1_000_000,
            "QNX4 extent cache limit",
        )?;
        self.extent_count += extents.len();
        let mut logical_end = 0;
        let logical_ends = extents
            .iter()
            .map(|(_, length)| {
                logical_end += length;
                logical_end
            })
            .collect();
        let entry = Entry {
            info: Info {
                metadata: Metadata {
                    kind: kind(mode),
                    mode: mode as u32,
                    size,
                },
                times: times(LE, &bytes, 32)?,
            },
            extents,
            logical_ends,
        };
        let entry = Arc::new(entry);
        self.entries.insert(node, Arc::clone(&entry));
        Ok(entry)
    }
    pub(super) fn probe_source(source: &mut S) -> Result<bool> {
        if source.len() < 576 {
            return Ok(false);
        }
        let mut bytes = [0; 64];
        exact(source, 512, &mut bytes)?;
        Ok(bytes[..2] == *b"/\0"
            && kind(LE.u16(&bytes, 50)?) == FileKind::Directory
            && bytes[63] & 1 != 0)
    }
    pub(super) fn open_source(source: S) -> Result<Self> {
        let mut reader = Self {
            source,
            entries: BTreeMap::new(),
            extent_count: 0,
        };
        require(
            Self::probe_source(&mut reader.source)?,
            "QNX4 root signature",
        )?;
        let entries = reader.directory(8)?;
        let bitmap = entries
            .iter()
            .find(|entry| entry.name == ".bitmap")
            .ok_or(FoldError::Corrupt("QNX4 root missing .bitmap"))?;
        let metadata = reader.info(bitmap.node)?.metadata;
        require(
            metadata.kind == FileKind::Regular && metadata.size > 0,
            "QNX4 bitmap inode",
        )?;
        Ok(reader)
    }
    pub(super) fn root_node(&self) -> u64 {
        8
    }
    pub(super) fn info(&mut self, node: u64) -> Result<Info> {
        Ok(self.entry(node)?.info.clone())
    }
    pub(super) fn content(&mut self, node: u64, offset: u64, output: &mut [u8]) -> Result<usize> {
        let entry = self.entry(node)?;
        let count = entry
            .info
            .metadata
            .size
            .saturating_sub(offset)
            .min(output.len() as u64) as usize;
        let mut done = 0;
        while done < count {
            let (physical, remaining) = entry.physical(offset + done as u64)?;
            let take = remaining.min((count - done) as u64) as usize;
            exact(&mut self.source, physical, &mut output[done..done + take])?;
            done += take;
        }
        require(done == count, "QNX4 short logical read")?;
        Ok(done)
    }
    pub(super) fn directory(&mut self, node: u64) -> Result<Vec<DirEntry>> {
        let entry = self.entry(node)?;
        require(
            entry.info.metadata.kind == FileKind::Directory,
            "QNX4 not a directory",
        )?;
        let length = entry.info.metadata.size;
        require(
            length % 64 == 0 && length <= 32 * 1024 * 1024,
            "QNX4 directory size/limit",
        )?;
        let mut result = Vec::new();
        let mut names = BTreeSet::new();
        for offset in (0..length).step_by(64) {
            let mut bytes = [0; 64];
            require(
                self.content(node, offset, &mut bytes)? == 64,
                "QNX4 short directory entry",
            )?;
            if bytes[0] == 0 || bytes[63] & 9 == 0 {
                continue;
            }
            let limit = if bytes[63] & 8 != 0 { 48 } else { 16 };
            let length = bytes[..limit]
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(limit);
            let filename = name(&bytes[..length])?;
            if filename == "." || filename == ".." {
                continue;
            }
            require(
                names.insert(filename.clone()),
                "QNX4 duplicate directory name",
            )?;
            let child = if bytes[63] & 8 != 0 {
                let block = LE.u32(&bytes, 48)?;
                require(block > 0 && bytes[52] < 8, "QNX4 inode link bounds")?;
                (block as u64 - 1) * 8 + bytes[52] as u64
            } else {
                entry.physical(offset)?.0 / 64
            };
            let metadata = self.info(child)?.metadata;
            result.push(DirEntry {
                name: filename,
                node: child,
                kind: metadata.kind,
            });
        }
        Ok(result)
    }
}
