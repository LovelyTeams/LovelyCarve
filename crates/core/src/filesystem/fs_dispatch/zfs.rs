//! ZFS dataset and file-path volume adapter.
use super::*;
use crate::filesystem::evidence_session::EvidenceSession;

// ---- ZFS ----------------------------------------------------------------

pub(super) struct ZfsAdapter;
pub(super) struct ZfsVol {
    pub(super) volume: crate::filesystem::volumes::zfs::ZfsVolume,
}

impl FsAdapter for ZfsAdapter {
    fn name(&self) -> &'static str {
        "zfs"
    }
    fn open<'a>(
        &self,
        image: &'a Arc<EvidenceSession>,
        id: &str,
    ) -> Result<Option<Box<dyn Volume + 'a>>> {
        let Some((source, _path)) = parse_zfs_id(id) else {
            return Ok(None);
        };
        Ok(Some(Box::new(ZfsVol {
            volume: open_zfs_source(image, source)?,
        })))
    }
}

impl Volume for ZfsVol {
    fn children(
        &self,
        _image: &Arc<EvidenceSession>,
        id: &str,
        _opts: ChildrenOptions,
    ) -> Result<Vec<FsNode>> {
        match reparse_zfs(id)? {
            ZfsPath::Dsl { dataset_path } => self.volume.dsl_children(&dataset_path),
            ZfsPath::Zpl {
                dataset_path,
                file_path,
            } => self.volume.zpl_children(&dataset_path, &file_path),
        }
    }
    fn read_file(&self, id: &str, max_bytes: usize) -> Result<Vec<u8>> {
        match reparse_zfs(id)? {
            ZfsPath::Dsl { .. } => anyhow::bail!("ZFS DSL directory is not a regular file"),
            ZfsPath::Zpl {
                dataset_path,
                file_path,
            } => self.volume.read_file(&dataset_path, &file_path, max_bytes),
        }
    }
    fn file_stat(&self, id: &str, node_id: &str) -> Result<FsStat> {
        match reparse_zfs(id)? {
            ZfsPath::Dsl { .. } => anyhow::bail!("ZFS DSL directory has no file_stat"),
            ZfsPath::Zpl {
                dataset_path,
                file_path,
            } => self.volume.file_stat(&dataset_path, &file_path, node_id),
        }
    }
    fn stream_file(
        &self,
        id: &str,
        chunk_size: usize,
        visitor: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        match reparse_zfs(id)? {
            ZfsPath::Dsl { .. } => anyhow::bail!("ZFS DSL directory cannot be streamed"),
            ZfsPath::Zpl {
                dataset_path,
                file_path,
            } => self
                .volume
                .stream_file(&dataset_path, &file_path, chunk_size, visitor),
        }
    }
}

pub(super) fn reparse_zfs(id: &str) -> Result<ZfsPath> {
    parse_zfs_id(id)
        .map(|(_, path)| path)
        .ok_or_else(|| anyhow::anyhow!("zfs id reparse failed: {id}"))
}
