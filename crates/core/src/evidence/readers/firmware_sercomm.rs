//! Attach verified Sercomm logical UBI data without changing source geometry.
use super::{uimage, FirmwareImage, NativeFirmwareView};
use crate::evidence::partitions::{Partition, PartitionKind, SectorSource};
use anyhow::Result;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

struct NativeSource(sercomm_firmware::NandImage);
impl SectorSource for NativeSource {
    fn size(&self) -> u64 {
        self.0.logical_length()
    }
    fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        self.0.read_range(offset, length)
    }
}

#[inline(never)]
pub(super) fn open(bytes: &[u8], cancel: &CancellationToken) -> Result<FirmwareImage> {
    let decoded = sercomm_firmware::open(bytes, cancel)?;
    uimage::parse(decoded.kernel_bytes(), 0, cancel, 0)?;
    let extent = decoded.source_extent();
    let label = format!("Sercomm {} rootfs (decoded UBI)", decoded.member_name());
    Ok(FirmwareImage {
        partitions: vec![Partition {
            id: 0,
            kind: PartitionKind::Ubi,
            start_lba: extent.start,
            length_lba: extent.end - extent.start,
            label: Some(label.clone()),
            unique_guid: None,
            amiga_geometry: None,
        }],
        plaintext: None,
        native_views: vec![NativeFirmwareView {
            partition_id: 0,
            label,
            kind: PartitionKind::Ubi,
            source: Arc::new(NativeSource(decoded)),
        }],
    })
}
