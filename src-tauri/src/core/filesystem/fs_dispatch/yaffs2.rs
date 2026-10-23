//! Direct adapter: the boxed volume owns the immutable YAFFS2 reader once.
use super::{ChildrenOptions, EvidenceSession, FsAdapter, FsNode, FsStat, Volume};
use crate::core::filesystem::volumes::yaffs2::{open_yaffs2_source, parse_yaffs2_id, Yaffs2Volume};
use anyhow::{Context, Result};
use std::sync::Arc;

pub(super) struct Yaffs2Adapter;
impl FsAdapter for Yaffs2Adapter {
    fn name(&self) -> &'static str {
        "yaffs2"
    }
    #[inline(never)]
    fn open<'a>(
        &self,
        image: &'a Arc<EvidenceSession>,
        id: &str,
    ) -> Result<Option<Box<dyn Volume + 'a>>> {
        let Some((partition, _)) = parse_yaffs2_id(id) else {
            return Ok(None);
        };
        Ok(Some(Box::new(open_yaffs2_source(image, partition)?)))
    }
}
fn path(id: &str) -> Result<String> {
    Ok(parse_yaffs2_id(id)
        .context("YAFFS2 invalid node identity")?
        .1)
}
impl Volume for Yaffs2Volume {
    fn children(
        &self,
        _image: &Arc<EvidenceSession>,
        id: &str,
        _options: ChildrenOptions,
    ) -> Result<Vec<FsNode>> {
        Yaffs2Volume::children(self, &path(id)?)
    }
    fn read_file(&self, id: &str, maximum: usize) -> Result<Vec<u8>> {
        Yaffs2Volume::read_file(self, &path(id)?, maximum)
    }
    fn read_range(&self, id: &str, offset: u64, length: usize) -> Result<Option<Vec<u8>>> {
        Ok(Some(self.read_file_range(&path(id)?, offset, length)?))
    }
    fn file_stat(&self, id: &str, node_id: &str) -> Result<FsStat> {
        Yaffs2Volume::file_stat(self, &path(id)?, node_id)
    }
    fn stream_file(
        &self,
        id: &str,
        chunk_size: usize,
        visitor: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        Yaffs2Volume::stream_file(self, &path(id)?, chunk_size, visitor)
    }
}
