//! NTFS volume adapter and ranged reads.
use super::*;
use crate::filesystem::evidence_session::EvidenceSession;

// ---- NTFS ---------------------------------------------------------------

pub(super) struct NtfsAdapter;

pub(super) struct NtfsVol<'a> {
    pub(super) source: NtfsSource,
    pub(super) volume: NtfsVolume<'a>,
}

impl FsAdapter for NtfsAdapter {
    fn name(&self) -> &'static str {
        "ntfs"
    }
    fn open<'a>(
        &self,
        image: &'a Arc<EvidenceSession>,
        id: &str,
    ) -> Result<Option<Box<dyn Volume + 'a>>> {
        let Some((source, _record)) = parse_ntfs_id(id) else {
            return Ok(None);
        };
        let volume = open_ntfs_for_source(image, source)?;
        Ok(Some(Box::new(NtfsVol { source, volume })))
    }
}

impl Volume for NtfsVol<'_> {
    fn children(
        &self,
        image: &Arc<EvidenceSession>,
        id: &str,
        opts: ChildrenOptions,
    ) -> Result<Vec<FsNode>> {
        let record = reparse_ntfs(id)?;
        if !opts.include_deleted_ntfs {
            return self.volume.children(record);
        }
        let snapshot = ntfs_deleted_snapshot_for_listing(image, &self.volume, self.source);
        self.volume
            .children_with_deleted(record, snapshot.as_deref())
    }
    fn read_file(&self, id: &str, max_bytes: usize) -> Result<Vec<u8>> {
        self.volume.read_file(reparse_ntfs(id)?, max_bytes)
    }
    fn read_range(&self, id: &str, offset: u64, length: usize) -> Result<Option<Vec<u8>>> {
        self.volume
            .read_range(reparse_ntfs(id)?, offset, length)
            .map(Some)
    }
    fn file_stat(&self, id: &str, node_id: &str) -> Result<FsStat> {
        self.volume.file_stat(reparse_ntfs(id)?, node_id)
    }
    fn stream_file(
        &self,
        id: &str,
        chunk_size: usize,
        visitor: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        self.volume
            .stream_file(reparse_ntfs(id)?, chunk_size, visitor)
    }
}

pub(super) fn reparse_ntfs(id: &str) -> Result<u64> {
    parse_ntfs_id(id)
        .map(|(_, record)| record)
        .ok_or_else(|| anyhow::anyhow!("ntfs id reparse failed: {id}"))
}

pub(super) fn ntfs_root_key(source: NtfsSource) -> String {
    match source {
        NtfsSource::Partition(part_id) => format!("ntfs:part:{part_id}"),
        NtfsSource::Lvm { part_id, lv_index } => format!("ntfs:lvm:{part_id}:{lv_index}"),
    }
}
