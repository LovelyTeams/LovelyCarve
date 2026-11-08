//! Native decrypted filesystem views attached to stable source partition ids.

use std::sync::Arc;

use anyhow::ensure;
use tokio_util::sync::CancellationToken;

use super::EvidenceSession;
use crate::evidence::image::{DiskImage, ImageInfo};

#[derive(Debug)]
pub(super) struct AttachedEncryptedVolume {
    pub(super) session: Arc<EvidenceSession>,
    pub(super) cancellation: CancellationToken,
}

impl EvidenceSession {
    /// Publish a native filesystem view once; its ids retain the source partition id.
    pub fn attach_decrypted_partition(
        &self,
        partition_id: u8,
        mut plaintext: DiskImage,
        cancellation: CancellationToken,
    ) -> anyhow::Result<()> {
        ensure!(
            plaintext.partitions.len() == 1 && plaintext.partitions[0].start_lba == 0,
            "原分区内解锁目前要求解密内容为单个完整卷"
        );
        plaintext.partitions[0].id = partition_id;
        plaintext.id.clone_from(&self.image.id);
        let attached = AttachedEncryptedVolume {
            session: Arc::new(Self::from(plaintext)),
            cancellation,
        };
        self.decrypted_partitions[usize::from(partition_id)]
            .set(attached)
            .map_err(|_| anyhow::anyhow!("分区已解锁"))
    }

    /// Borrow the stable native view for any partition- or LVM-addressed node.
    pub fn decrypted_session_for_node(&self, node_id: &str) -> Option<&Arc<Self>> {
        let mut fields = node_id.split(':');
        while let Some(field) = fields.next() {
            if matches!(field, "part" | "lvm") {
                let partition_id = fields.next()?.parse::<u8>().ok()?;
                let attached = self.decrypted_partitions[usize::from(partition_id)].get()?;
                return (!attached.cancellation.is_cancelled()).then_some(&attached.session);
            }
        }
        None
    }

    /// Metadata describes unlocked filesystems while retaining source geometry and identity.
    pub fn info(&self) -> ImageInfo {
        let mut info = self.image.info();
        for (partition_id, slot) in self.decrypted_partitions.iter().enumerate() {
            let Some(attached) = slot.get().filter(|view| !view.cancellation.is_cancelled()) else {
                continue;
            };
            let Some(plain) = attached.session.partitions.first() else {
                continue;
            };
            let partition_id = partition_id as u8;
            info.unlocked_partitions.push(partition_id);
            if let Some(partition) = info
                .partitions
                .iter_mut()
                .find(|part| part.id == partition_id)
            {
                partition.kind = plain.kind;
                if partition.label.is_none() {
                    partition.label.clone_from(&plain.label);
                }
            } else {
                let mut partition = plain.clone();
                partition.length_lba = self.len() / self.sector_size as u64;
                info.partitions.push(partition);
            }
        }
        info
    }

    pub fn revoke_decrypted_partitions(&self) {
        for attached in self
            .decrypted_partitions
            .iter()
            .filter_map(|slot| slot.get())
        {
            attached.cancellation.cancel();
        }
    }
}
