//! Filesystem-only routing for decoded firmware and unlocked native volumes.
//! Source reads, source geometry and export projections remain on the owner.
use super::EvidenceSession;
use std::sync::Arc;

impl EvidenceSession {
    pub fn filesystem_session<'a>(self: &'a Arc<Self>, node_id: &str) -> &'a Arc<Self> {
        self.decrypted_session_for_node(node_id)
            .or_else(|| self.firmware_session_for_node(node_id))
            .unwrap_or(self)
    }

    fn firmware_session_for_node(&self, node_id: &str) -> Option<&Arc<Self>> {
        let remainder = [
            "part:",
            "ubi:part:",
            "squashfs:ubi:part:",
            "ubifs:ubi:part:",
        ]
        .iter()
        .find_map(|prefix| node_id.strip_prefix(prefix))?;
        let partition_id = remainder.split(':').next()?.parse::<u8>().ok()?;
        self.firmware_views.get(&partition_id)
    }

    pub(crate) fn native_volume_size(&self, volume_key: &str) -> Option<u64> {
        let session = self
            .decrypted_session_for_node(volume_key)
            .or_else(|| self.firmware_session_for_node(volume_key))?;
        let partition = session.partitions.first()?;
        partition.length_lba.checked_mul(session.sector_size as u64)
    }
}
