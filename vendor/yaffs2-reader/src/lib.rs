//! Native YAFFS2 NAND page/OOB reading. Source bytes remain immutable.
mod detect;
mod index;
mod legacy_scan;
mod legacy_tags;
mod read;
mod scan;
mod tags;
mod wire;

use anyhow::{Context, Result, ensure};
pub use detect::{Format, Geometry, detect, detect_bytes};
pub use legacy_scan::FileSizeSource;
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;
pub use wire::{ByteOrder, FileType, Node};

/// Random source access. Return exactly the requested bytes or an error;
/// implementers must never replace a source failure with unaudited zeroes.
pub trait ReadAt {
    fn size(&self) -> u64;
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>>;
}
pub(crate) fn exact(source: &dyn ReadAt, offset: u64, length: usize) -> Result<Vec<u8>> {
    ensure!(
        offset
            .checked_add(length as u64)
            .is_some_and(|end| end <= source.size()),
        "YAFFS2 read exceeds source"
    );
    let bytes = source.read_at(offset, length)?;
    ensure!(
        bytes.len() == length,
        "YAFFS2 source short read at {offset:#x}"
    );
    Ok(bytes)
}
pub(crate) fn cancelled(token: &CancellationToken) -> Result<()> {
    ensure!(!token.is_cancelled(), "YAFFS2 operation cancelled");
    Ok(())
}

#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    pub name: String,
    /// Directory identity; a hard link's content inode can differ.
    pub object_id: u32,
}
#[derive(Debug, Clone)]
pub(crate) struct Chunk {
    pub offset: u64,
    pub length: u32,
}
pub(crate) struct Object {
    pub node: Node,
    pub chunks: BTreeMap<u32, Chunk>,
}

/// An immutable current namespace and page map. Deleted paths, checkpoint
/// streams and summary objects are not projected as live user files.
pub struct YaffsImage {
    pub geometry: Geometry,
    pub orphan_data_objects: usize,
    pub(crate) length: u64,
    pub(crate) objects: BTreeMap<u32, Object>,
    pub(crate) directories: BTreeMap<u32, Vec<DirectoryEntry>>,
}
/// Immutable filesystem boundary for host adapters. Implementers preserve
/// directory identity, return logical content consistently across ranges and
/// propagate source faults; they must not perform host writes or follow links.
/// The concrete index and its drop implementation stay inside this crate.
pub trait ReadOnlyFilesystem: Send + Sync {
    fn format(&self) -> Format;
    fn children(&self, number: u32) -> Result<&[DirectoryEntry]>;
    fn resolve(&self, path: &str) -> Result<u32>;
    fn content_node(&self, number: u32) -> Result<&Node>;
    fn read_range(
        &self,
        source: &dyn ReadAt,
        number: u32,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>>;
}
#[inline(never)]
pub fn open_filesystem(source: &dyn ReadAt) -> Result<Box<dyn ReadOnlyFilesystem>> {
    Ok(Box::new(YaffsImage::open(source)?))
}
impl ReadOnlyFilesystem for YaffsImage {
    fn format(&self) -> Format {
        self.geometry.format
    }
    fn children(&self, number: u32) -> Result<&[DirectoryEntry]> {
        YaffsImage::children(self, number)
    }
    fn resolve(&self, path: &str) -> Result<u32> {
        YaffsImage::resolve(self, path)
    }
    fn content_node(&self, number: u32) -> Result<&Node> {
        YaffsImage::content_node(self, number)
    }
    fn read_range(
        &self,
        source: &dyn ReadAt,
        number: u32,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        YaffsImage::read_range(self, source, number, offset, length)
    }
}
impl YaffsImage {
    pub fn open(source: &dyn ReadAt) -> Result<Self> {
        Self::open_cancellable(source, &CancellationToken::new())
    }
    pub fn open_cancellable(source: &dyn ReadAt, token: &CancellationToken) -> Result<Self> {
        cancelled(token)?;
        let geometry = detect(source)?.context("YAFFS2 geometry not recognised or ambiguous")?;
        Self::open_with_geometry(source, geometry, token)
    }
    pub fn open_with_geometry(
        source: &dyn ReadAt,
        geometry: Geometry,
        token: &CancellationToken,
    ) -> Result<Self> {
        Self::open_configured(source, geometry, FileSizeSource::Header, token)
    }
    /// Read a known YAFFS1 512/16 layout using its independently known size policy.
    pub fn open_yaffs1(
        source: &dyn ReadAt,
        order: ByteOrder,
        policy: FileSizeSource,
        token: &CancellationToken,
    ) -> Result<Self> {
        Self::open_configured(
            source,
            Geometry {
                format: Format::Yaffs1,
                page_size: 512,
                spare_size: 16,
                tags_offset: 0,
                order,
            },
            policy,
            token,
        )
    }
    fn open_configured(
        source: &dyn ReadAt,
        geometry: Geometry,
        policy: FileSizeSource,
        token: &CancellationToken,
    ) -> Result<Self> {
        geometry.validate()?;
        let length = source.size();
        let (objects, orphan_data_objects) = match geometry.format {
            Format::Yaffs2 => scan::scan(source, geometry, token)?,
            Format::Yaffs1 => legacy_scan::scan(source, geometry, policy, token)?,
        };
        let directories = index::build(&objects, token)?;
        cancelled(token)?;
        ensure!(
            source.size() == length,
            "YAFFS2 source size changed while opening"
        );
        Ok(Self {
            geometry,
            orphan_data_objects,
            length,
            objects,
            directories,
        })
    }
    pub fn node(&self, object_id: u32) -> Result<&Node> {
        Ok(&self
            .objects
            .get(&object_id)
            .context("YAFFS2 object not found")?
            .node)
    }
    pub fn content_node(&self, mut object_id: u32) -> Result<&Node> {
        let mut visited = std::collections::BTreeSet::new();
        loop {
            ensure!(
                visited.len() < 256 && visited.insert(object_id),
                "YAFFS2 hard-link cycle/depth limit"
            );
            let node = self.node(object_id)?;
            if node.kind != FileType::HardLink {
                return Ok(node);
            }
            object_id = node.equivalent;
        }
    }
    pub fn children(&self, number: u32) -> Result<&[DirectoryEntry]> {
        ensure!(
            self.node(number)?.kind == FileType::Directory,
            "YAFFS2 entry is not a directory"
        );
        Ok(self
            .directories
            .get(&number)
            .map(Vec::as_slice)
            .unwrap_or_default())
    }
    pub fn resolve(&self, path: &str) -> Result<u32> {
        if matches!(path, "" | "/") {
            return Ok(1);
        }
        ensure!(
            path.starts_with('/') && !path.contains(['\\', '\0']),
            "YAFFS2 invalid path"
        );
        let mut object = 1;
        for (depth, name) in path[1..].split('/').enumerate() {
            ensure!(
                depth < 256 && !matches!(name, "" | "." | ".."),
                "YAFFS2 invalid/deep path"
            );
            object = self
                .children(object)?
                .iter()
                .find(|entry| entry.name == name)
                .context("YAFFS2 path not found")?
                .object_id;
        }
        Ok(object)
    }
}

#[cfg(test)]
mod legacy_tests;
#[cfg(test)]
mod tests;
