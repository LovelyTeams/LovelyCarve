//! ext4 volume adapter and extent-backed ranged reads.
use super::*;
use crate::filesystem::evidence_session::EvidenceSession;

// ---- ext4 ---------------------------------------------------------------

pub(super) struct Ext4Adapter;
pub(super) struct Ext4Vol {
    pub(super) volume: crate::filesystem::volumes::ext4::Ext4Volume,
}

impl FsAdapter for Ext4Adapter {
    fn name(&self) -> &'static str {
        "ext4"
    }
    fn open<'a>(
        &self,
        image: &'a Arc<EvidenceSession>,
        id: &str,
    ) -> Result<Option<Box<dyn Volume + 'a>>> {
        let Some((source, _inode)) = parse_ext4_id(&canonical_ext4_id(id)) else {
            return Ok(None);
        };
        Ok(Some(Box::new(Ext4Vol {
            volume: open_ext4_source(image, source)?,
        })))
    }
}

impl Volume for Ext4Vol {
    fn children(
        &self,
        _image: &Arc<EvidenceSession>,
        id: &str,
        _opts: ChildrenOptions,
    ) -> Result<Vec<FsNode>> {
        // Children listing uses the RAW id (not canonical): a recovered
        // deleted-directory id (`recover:ext4:…`) historically failed
        // `parse_ext4_id` here and fell through to an empty listing, so it
        // never expanded. read/stat/stream do canonicalise (to recover file
        // *content*); preserving that asymmetry keeps behaviour byte-identical.
        let Some((_, inode)) = parse_ext4_id(id) else {
            return Ok(Vec::new());
        };
        self.volume.children(inode, true)
    }
    fn read_file(&self, id: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let (_, inode_number) = parse_ext4_id(&canonical_ext4_id(id))
            .ok_or_else(|| anyhow::anyhow!("ext4 id reparse failed: {id}"))?;
        let inode = self.volume.read_inode(inode_number)?;
        if inode.kind() != NodeKind::File {
            anyhow::bail!("node is not a regular file");
        }
        self.volume.read_inode_data(&inode, max_bytes)
    }
    fn read_range(&self, id: &str, offset: u64, length: usize) -> Result<Option<Vec<u8>>> {
        let (_, inode_number) = parse_ext4_id(&canonical_ext4_id(id))
            .ok_or_else(|| anyhow::anyhow!("ext4 id reparse failed: {id}"))?;
        let inode = self.volume.read_inode(inode_number)?;
        if inode.kind() != NodeKind::File {
            anyhow::bail!("node is not a regular file");
        }
        self.volume
            .read_inode_range(&inode, offset, length)
            .map(Some)
    }
    fn file_stat(&self, id: &str, node_id: &str) -> Result<FsStat> {
        let (_, inode_number) = parse_ext4_id(&canonical_ext4_id(id))
            .ok_or_else(|| anyhow::anyhow!("ext4 id reparse failed: {id}"))?;
        let inode = self.volume.read_inode(inode_number)?;
        let data_runs = if inode.kind() == NodeKind::File {
            self.volume.physical_data_runs(&inode)?
        } else {
            Vec::new()
        };
        let sample = if inode.kind() == NodeKind::File {
            self.volume.read_inode_data(&inode, 4096)?
        } else {
            Vec::new()
        };
        Ok(build_stat(
            node_id,
            inode.size,
            Some(inode.number),
            Some(inode.mode),
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
        let (_, inode_number) = parse_ext4_id(&canonical_ext4_id(id))
            .ok_or_else(|| anyhow::anyhow!("ext4 id reparse failed: {id}"))?;
        let inode = self.volume.read_inode(inode_number)?;
        if inode.kind() != NodeKind::File {
            anyhow::bail!("node is not a regular file");
        }
        self.volume.stream_inode_data(&inode, chunk_size, visitor)
    }
}
