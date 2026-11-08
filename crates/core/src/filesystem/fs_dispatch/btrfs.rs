//! Btrfs volume adapter.
use super::*;
use crate::filesystem::evidence_session::EvidenceSession;

// ---- btrfs --------------------------------------------------------------

pub(super) struct BtrfsAdapter;
pub(super) struct BtrfsVol<'a> {
    pub(super) volume: crate::filesystem::volumes::btrfs::BtrfsVolume<'a>,
}

impl FsAdapter for BtrfsAdapter {
    fn name(&self) -> &'static str {
        "btrfs"
    }
    fn open<'a>(
        &self,
        image: &'a Arc<EvidenceSession>,
        id: &str,
    ) -> Result<Option<Box<dyn Volume + 'a>>> {
        let Some((source, _subvol, _inode)) = parse_btrfs_id(id) else {
            return Ok(None);
        };
        Ok(Some(Box::new(BtrfsVol {
            volume: open_btrfs_source(image, source)?,
        })))
    }
}

impl Volume for BtrfsVol<'_> {
    fn children(
        &self,
        _image: &Arc<EvidenceSession>,
        id: &str,
        _opts: ChildrenOptions,
    ) -> Result<Vec<FsNode>> {
        let (_, subvol, inode) = reparse_btrfs(id)?;
        self.volume.children(subvol, inode)
    }
    fn read_file(&self, id: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let (_, subvol, inode_number) = reparse_btrfs(id)?;
        let inode = self.volume.read_inode(subvol, inode_number)?;
        if inode.kind() != NodeKind::File && inode.kind() != NodeKind::Symlink {
            anyhow::bail!("node is not a regular file");
        }
        self.volume.read_inode_data(subvol, &inode, max_bytes)
    }
    fn file_stat(&self, id: &str, node_id: &str) -> Result<FsStat> {
        let (_, subvol, inode_number) = reparse_btrfs(id)?;
        let inode = self.volume.read_inode(subvol, inode_number)?;
        let data_runs = if inode.kind() == NodeKind::File {
            self.volume.data_runs(subvol, &inode)?
        } else {
            Vec::new()
        };
        let sample = if inode.kind() == NodeKind::File || inode.kind() == NodeKind::Symlink {
            self.volume
                .read_inode_data(subvol, &inode, 4096)
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok(build_stat(
            node_id,
            inode.size,
            Some(inode.objectid),
            Some((inode.mode & 0xFFFF) as u16),
            &sample,
            data_runs,
        ))
    }
    fn stream_file(
        &self,
        id: &str,
        chunk_size: usize,
        visitor: &mut dyn FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        let (_, subvol, inode_number) = reparse_btrfs(id)?;
        let inode = self.volume.read_inode(subvol, inode_number)?;
        if inode.kind() != NodeKind::File && inode.kind() != NodeKind::Symlink {
            anyhow::bail!("node is not a regular file");
        }
        self.volume
            .stream_inode_data(subvol, &inode, chunk_size, visitor)
    }
}

pub(super) fn reparse_btrfs(
    id: &str,
) -> Result<(crate::filesystem::volumes::btrfs::BtrfsSource, u64, u64)> {
    parse_btrfs_id(id).ok_or_else(|| anyhow::anyhow!("btrfs id reparse failed: {id}"))
}
