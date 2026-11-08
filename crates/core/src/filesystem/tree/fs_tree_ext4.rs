use std::sync::Arc;

use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::volumes::ext4::{
    canonical_ext4_id, open_ext4_source, parse_ext4_id, Ext4Source,
};
use crate::filesystem::volumes::ext4_journal::Ext4Journal;

/// Mines the ext4 jbd2 journal for the volume named by `node_id` (an `ext4:`
/// node id, or a `part:N` / `lvm:N:M` volume key on an ext4 volume). Returns
/// the parsed journal — the appended list of historical block revisions and
/// revoked blocks — or an error if the volume has no internal journal.
/// `max_bytes` caps the journal read.
pub fn ext4_journal(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    max_bytes: usize,
) -> anyhow::Result<Ext4Journal> {
    let image = image.filesystem_session(node_id);
    let source = if let Some((source, _inode)) = parse_ext4_id(&canonical_ext4_id(node_id)) {
        source
    } else if let Some(rest) = node_id.strip_prefix("lvm:") {
        let mut parts = rest.split(':');
        let part_id: u8 = parts
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("invalid lvm volume key: {node_id}"))?;
        let lv_index: usize = parts
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("invalid lvm volume key: {node_id}"))?;
        Ext4Source::Lvm { part_id, lv_index }
    } else {
        let part_id: u8 = node_id
            .strip_prefix("part:")
            .unwrap_or(node_id)
            .parse()
            .map_err(|_| anyhow::anyhow!("not an ext4 volume key: {node_id}"))?;
        Ext4Source::Partition(part_id)
    };
    let volume = open_ext4_source(image, source)?;
    let inum = volume
        .journal_inode()?
        .ok_or_else(|| anyhow::anyhow!("ext4 volume has no internal journal"))?;
    Ext4Journal::parse(&volume, inum, max_bytes)
}
