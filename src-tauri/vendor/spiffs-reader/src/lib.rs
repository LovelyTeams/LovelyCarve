//! Read-only SPIFFS over a bounded logical flash image.
#![forbid(unsafe_code)]

mod catalog;
mod geometry;
#[cfg(test)]
mod tests;

use lamfold::{
    BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, Metadata, NodeId, Result,
    SubstrateCtx,
};

use catalog::{Catalog, File};
use geometry::PAGE_HEADER;
pub use geometry::{ByteOrder, Geometry, PROBE_BYTES, Signature};

const ROOT: NodeId = 0;

pub struct Spiffs<S> {
    source: S,
    geometry: Geometry,
    catalog: Catalog,
}

impl<S: BlockSource> Spiffs<S> {
    fn detect(source: &mut S) -> Result<Option<Geometry>> {
        let length = source.len();
        let mut head = [0; PROBE_BYTES];
        let head = &mut head[..length.min(PROBE_BYTES as u64) as usize];
        source.read_at(0, head)?;
        Geometry::locate(head, length, |offset, bytes| source.read_at(offset, bytes))
    }

    fn file(&self, node: NodeId) -> Result<&File> {
        if node == ROOT {
            return Err(FoldError::IsDirectory);
        }
        self.catalog.files.get(&node).ok_or(FoldError::NotFound)
    }
}

impl<S: BlockSource> FoldFrontend<S> for Spiffs<S> {
    const TAG: &'static str = "spiffs";

    fn probe(source: &mut S) -> Result<bool> {
        Ok(Self::detect(source)?.is_some())
    }

    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let geometry = Self::detect(&mut source)?
            .ok_or(FoldError::Corrupt("SPIFFS geometry not recognized"))?;
        let catalog = Catalog::load(&mut source, geometry)?;
        Ok(Self {
            source,
            geometry,
            catalog,
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
        Ok(self.catalog.names.get(name).copied())
    }

    fn read_dir(&mut self, dir: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        if dir != ROOT {
            return Err(FoldError::NotDirectory);
        }
        Ok(self
            .catalog
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
            size: self.file(node)?.size,
            kind: FileKind::Regular,
            mode: 0,
        })
    }

    fn read_at(
        &mut self,
        node: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        if node == ROOT {
            return Err(FoldError::IsDirectory);
        }
        let file = self.catalog.files.get(&node).ok_or(FoldError::NotFound)?;
        let pages = file
            .pages
            .as_ref()
            .map_err(|&error| FoldError::Corrupt(error))?;
        let count = file.size.saturating_sub(offset).min(bytes.len() as u64) as usize;
        let data_per_page = self.geometry.data_bytes();
        let mut done = 0;
        while done < count {
            let logical = offset as usize + done;
            let page = usize::from(pages[logical / data_per_page]);
            let within = logical % data_per_page;
            let length = (data_per_page - within).min(count - done);
            let absolute = (page * self.geometry.page_bytes + PAGE_HEADER + within) as u64;
            self.source
                .read_at(absolute, &mut bytes[done..done + length])?;
            done += length;
        }
        Ok(count)
    }
}
