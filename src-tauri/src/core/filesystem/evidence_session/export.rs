//! Export-only plaintext projection. Ordinary evidence reads retain source bytes.

use std::sync::Arc;

use anyhow::{ensure, Context};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::EvidenceSession;
use crate::core::common::error::Failure;

#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImageExportMode {
    #[default]
    Source,
    UnlockedPartitions,
}

/// The plaintext of one attached decrypted view.
struct PlaintextReader {
    session: Arc<EvidenceSession>,
    cancellation: CancellationToken,
}

impl PlaintextReader {
    fn len(&self) -> u64 {
        self.session.len()
    }

    fn read_into(&self, offset: u64, output: &mut [u8]) -> anyhow::Result<usize> {
        ensure!(
            !self.cancellation.is_cancelled(),
            Failure::cancelled("解密会话已关闭")
        );
        self.session.read_into(offset, output)
    }
}

struct PlaintextRegion {
    start: u64,
    end: u64,
    reader: PlaintextReader,
}

/// A fixed snapshot of unlocked volumes, positioned in the original disk layout.
/// Plaintext starts at the original partition start. Container overhead becomes
/// zero padding at the partition tail; other partitions and disk gaps stay raw.
pub struct RawExportReader {
    source: Arc<EvidenceSession>,
    regions: Vec<PlaintextRegion>,
}

impl EvidenceSession {
    pub fn raw_export_reader(
        self: &Arc<Self>,
        mode: ImageExportMode,
    ) -> anyhow::Result<RawExportReader> {
        let mut regions = Vec::new();
        if matches!(mode, ImageExportMode::UnlockedPartitions) {
            for (partition_id, slot) in self.decrypted_partitions.iter().enumerate() {
                let Some(attached) = slot.get() else { continue };
                ensure!(
                    !attached.cancellation.is_cancelled(),
                    Failure::cancelled("解密会话已关闭")
                );
                let partition = self
                    .partitions
                    .iter()
                    .find(|partition| usize::from(partition.id) == partition_id);
                let (start, length) = if let Some(partition) = partition {
                    (
                        partition
                            .start_lba
                            .checked_mul(self.sector_size as u64)
                            .context(Failure::corrupt("partition offset overflow"))?,
                        partition
                            .length_lba
                            .checked_mul(self.sector_size as u64)
                            .context(Failure::corrupt("partition size overflow"))?,
                    )
                } else {
                    ensure!(
                        self.partitions.is_empty() && partition_id == 0,
                        "unlocked partition is missing from source geometry"
                    );
                    (0, self.len())
                };
                regions.push(PlaintextRegion {
                    start,
                    end: start
                        .checked_add(length)
                        .context(Failure::corrupt("partition end overflow"))?,
                    reader: PlaintextReader {
                        session: Arc::clone(&attached.session),
                        cancellation: attached.cancellation.clone(),
                    },
                });
            }
            regions.sort_by_key(|region| region.start);
            let mut previous_end = 0;
            for region in &regions {
                ensure!(
                    region.start >= previous_end
                        && region.start < region.end
                        && region.end <= self.len(),
                    "unlocked partition overlaps or lies outside source image"
                );
                ensure!(
                    region.reader.len() > 0 && region.reader.len() <= region.end - region.start,
                    "plaintext size exceeds source partition or is empty"
                );
                previous_end = region.end;
            }
        }
        Ok(RawExportReader {
            source: Arc::clone(self),
            regions,
        })
    }
}

impl RawExportReader {
    /// Read a complete bounded chunk; a short source/plaintext read is an error.
    pub fn read_into(&self, offset: u64, output: &mut [u8]) -> anyhow::Result<usize> {
        let length = self
            .source
            .len()
            .saturating_sub(offset)
            .min(output.len() as u64) as usize;
        let mut filled = 0;
        while filled < length {
            let position = offset + filled as u64;
            let next_region = self.regions.iter().find(|region| region.end > position);
            let (count, read) =
                if let Some(region) = next_region.filter(|region| region.start <= position) {
                    let relative = position - region.start;
                    let count = (region.end - position).min((length - filled) as u64) as usize;
                    let plaintext_count = region
                        .reader
                        .len()
                        .saturating_sub(relative)
                        .min(count as u64) as usize;
                    let read = region
                        .reader
                        .read_into(relative, &mut output[filled..filled + plaintext_count])?;
                    ensure!(
                        read == plaintext_count,
                        "decrypted image short read at {position}: {read} / {plaintext_count}"
                    );
                    output[filled + plaintext_count..filled + count].fill(0);
                    (count, count)
                } else {
                    let end = next_region.map_or(self.source.len(), |region| region.start);
                    let count = (end - position).min((length - filled) as u64) as usize;
                    let read = self
                        .source
                        .read_into(position, &mut output[filled..filled + count])?;
                    (count, read)
                };
            ensure!(
                read == count,
                "source image short read at {position}: {read} / {count}"
            );
            filled += count;
        }
        Ok(length)
    }
}

#[cfg(test)]
mod tests;
