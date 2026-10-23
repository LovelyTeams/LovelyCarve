//! Read-only committed UBIFS trees, adapted from rust_ubidump's UBIFS readers.
//! Dirty/recovery states fail explicitly; no journal replay or guessed nodes.
use crate::ReadAt;
use anyhow::{Context, Result, ensure};
use std::collections::BTreeMap;
#[path = "filesystem_codec.rs"]
mod codec;
#[path = "filesystem_index.rs"]
mod index;
#[path = "filesystem_nodes.rs"]
mod nodes;
pub use nodes::MAGIC as UBIFS_MAGIC;
pub use nodes::{DirectoryEntry, FileType, Inode};
use nodes::{Geometry, Key, Location, word};

pub fn validate_superblock(bytes: &[u8]) -> Result<()> {
    ensure!(bytes.len() == 4096, "truncated UBIFS superblock");
    nodes::validate_node(bytes, 6)
}

pub struct UbifsImage {
    pub commit: u64,
    geometry: Geometry,
    inodes: BTreeMap<u32, Inode>,
    directories: BTreeMap<u32, Vec<DirectoryEntry>>,
    data: BTreeMap<(u32, u32), Location>,
}
impl UbifsImage {
    pub fn open(source: &dyn ReadAt) -> Result<Self> {
        let geometry = Geometry::open(source)?;
        let master = index::Master::open(source, &geometry)?;
        let mut builder = index::IndexBuilder::new(geometry, &master);
        builder.walk(source, master.root, None, 0)?;
        builder.finish(master.highest_inode)
    }
    pub fn leb_size(&self) -> u32 {
        self.geometry.leb_size
    }
    pub fn version(&self) -> u32 {
        self.geometry.version
    }
    pub fn time_granularity(&self) -> u32 {
        self.geometry.time_granularity
    }
    pub fn inode(&self, number: u32) -> Result<&Inode> {
        self.inodes.get(&number).context("UBIFS inode not found")
    }
    pub fn children(&self, number: u32) -> Result<&[DirectoryEntry]> {
        ensure!(
            self.inode(number)?.kind == FileType::Directory,
            "UBIFS entry is not a directory"
        );
        Ok(self
            .directories
            .get(&number)
            .map(Vec::as_slice)
            .unwrap_or_default())
    }
    pub fn resolve(&self, path: &str) -> Result<u32> {
        ensure!(
            path.is_empty() || path.starts_with('/'),
            "invalid UBIFS path"
        );
        ensure!(path.split('/').count() <= 257, "UBIFS path depth limit");
        let mut inode = 1;
        for name in path.split('/').skip(1) {
            ensure!(
                !name.is_empty() && name != "." && name != ".." && !name.contains(['\\', '\0']),
                "invalid UBIFS path component"
            );
            inode = self
                .children(inode)?
                .iter()
                .find(|entry| entry.name == name)
                .context("UBIFS path not found")?
                .inode;
        }
        Ok(inode)
    }
    pub fn read_range(
        &self,
        source: &dyn ReadAt,
        number: u32,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        let inode = self.inode(number)?;
        ensure!(
            matches!(inode.kind, FileType::File | FileType::Symlink),
            "UBIFS entry has no file content"
        );
        let count = inode.size.saturating_sub(offset).min(length as u64);
        ensure!(
            count <= 256 * 1024 * 1024,
            "UBIFS single read exceeds 256 MiB; use streaming"
        );
        let mut out = vec![0; count as usize];
        if count == 0 {
            return Ok(out);
        }
        if inode.kind == FileType::Symlink || inode.flags & 0x20 != 0 {
            ensure!(
                inode.inline.len() as u64 == inode.size,
                "UBIFS inline file size mismatch"
            );
            let start = offset as usize;
            out.copy_from_slice(&inode.inline[start..start + count as usize]);
            return Ok(out);
        }
        let mut done = 0usize;
        while done < out.len() {
            let position = offset
                .checked_add(done as u64)
                .context("UBIFS file offset overflow")?;
            let block = u32::try_from(position / 4096).context("UBIFS file block overflow")?;
            let within = (position % 4096) as usize;
            let take = (4096 - within).min(out.len() - done);
            if let Some(&location) = self.data.get(&(number, block)) {
                let bytes = self.geometry.read_node(source, location, 1)?;
                ensure!(
                    Key::parse(&bytes[24..32])?
                        == Key {
                            inode: number,
                            kind: 1,
                            value: block
                        },
                    "UBIFS data key changed"
                );
                let size = word(&bytes, 40)? as usize;
                let compression = u16::from_le_bytes(nodes::number(&bytes, 44)?);
                ensure!(bytes[46..48] == [0, 0], "unsupported encrypted UBIFS data");
                let decoded = codec::decode(&bytes[48..], compression, size)?;
                let available = decoded.len().saturating_sub(within).min(take);
                // Short data nodes and absent blocks represent zero-filled
                // holes up to the inode's logical length, as in Linux UBIFS.
                out[done..done + available].copy_from_slice(
                    &decoded[within.min(decoded.len())..within.min(decoded.len()) + available],
                );
            }
            done += take;
        }
        Ok(out)
    }
}

#[cfg(test)]
#[path = "filesystem_tests.rs"]
mod tests;
