//! ReFS default data and snapshot streams share the same cached volume.
use super::*;
use crate::filesystem::volumes::refs::{open_refs_source, RefsSource, RefsVolume};

enum RefsPath {
    File(String),
    Stream { path: String, name: String },
}

fn parse(id: &str) -> Option<(RefsSource, RefsPath)> {
    if let Some((source, path)) = parse_refs_id(id) {
        return Some((source, RefsPath::File(path)));
    }
    parse_refs_snap_id(id).map(|(source, path, name)| (source, RefsPath::Stream { path, name }))
}

pub(super) struct RefsAdapter;
struct RefsVol {
    volume: RefsVolume,
}

impl FsAdapter for RefsAdapter {
    fn name(&self) -> &'static str {
        "refs"
    }
    fn open<'a>(
        &self,
        image: &'a Arc<EvidenceSession>,
        id: &str,
    ) -> Result<Option<Box<dyn Volume + 'a>>> {
        let Some((source, _)) = parse(id) else {
            return Ok(None);
        };
        Ok(Some(Box::new(RefsVol {
            volume: open_refs_source(image, source)?,
        })))
    }
}

impl RefsVol {
    fn path_of(&self, id: &str) -> Result<RefsPath> {
        parse(id)
            .map(|(_, path)| path)
            .ok_or_else(|| anyhow::anyhow!("invalid ReFS node id: {id}"))
    }
}

impl Volume for RefsVol {
    fn children(
        &self,
        _image: &Arc<EvidenceSession>,
        id: &str,
        _opts: ChildrenOptions,
    ) -> Result<Vec<FsNode>> {
        match self.path_of(id)? {
            RefsPath::File(path) => self.volume.children(&path),
            RefsPath::Stream { .. } => Ok(Vec::new()),
        }
    }
    fn read_file(&self, id: &str, max_bytes: usize) -> Result<Vec<u8>> {
        match self.path_of(id)? {
            RefsPath::File(path) => self.volume.read_file(&path, max_bytes),
            RefsPath::Stream { path, name } => {
                self.volume.read_named_stream(&path, &name, max_bytes)
            }
        }
    }
    fn read_range(&self, id: &str, offset: u64, length: usize) -> Result<Option<Vec<u8>>> {
        match self.path_of(id)? {
            RefsPath::File(path) => self.volume.read_range(&path, offset, length).map(Some),
            RefsPath::Stream { path, name } => self
                .volume
                .read_named_stream_range(&path, &name, offset, length)
                .map(Some),
        }
    }
    fn file_stat(&self, id: &str, node_id: &str) -> Result<FsStat> {
        match self.path_of(id)? {
            RefsPath::File(path) => self.volume.file_stat(&path, node_id),
            RefsPath::Stream { path, name } => self.volume.named_stream_stat(&path, &name, node_id),
        }
    }
    fn stream_file(
        &self,
        id: &str,
        chunk_size: usize,
        visitor: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        match self.path_of(id)? {
            RefsPath::File(path) => self.volume.stream_file(&path, chunk_size, visitor),
            RefsPath::Stream { path, name } => self
                .volume
                .stream_named_stream(&path, &name, chunk_size, visitor),
        }
    }
}
