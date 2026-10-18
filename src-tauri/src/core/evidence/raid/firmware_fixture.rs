//! Shared helpers for the firmware RAID detection tests: in-memory member
//! images and read-back of assembled devices.
use super::*;
use crate::core::evidence::readers::ewf_image::EwfAcquiryInfo;

/// Strip size of every synthetic volume.
pub(super) const CHUNK: usize = 4096;

struct MemSource(Vec<u8>);

impl SectorSource for MemSource {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        let start = (offset as usize).min(self.0.len());
        let end = (start + len).min(self.0.len());
        Ok(self.0[start..end].to_vec())
    }
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
}

/// A loaded member image; `serial` is the drive serial number its
/// acquisition recorded.
pub(super) fn disk(bytes: Vec<u8>, serial: Option<&str>) -> Arc<DiskImage> {
    disk_with(bytes, serial, Vec::new())
}

pub(super) fn disk_with(
    bytes: Vec<u8>,
    serial: Option<&str>,
    partitions: Vec<Partition>,
) -> Arc<DiskImage> {
    let mut image =
        DiskImage::from_assembled("member".into(), Arc::new(MemSource(bytes)), partitions, 512)
            .expect("member image");
    image.ewf_acquiry = serial.map(|serial| EwfAcquiryInfo {
        serial_number: Some(serial.to_string()),
        ..EwfAcquiryInfo::default()
    });
    Arc::new(image)
}

pub(super) fn read_all(device: &AssembledArray) -> Vec<u8> {
    device
        .source
        .read(0, device.size_bytes as usize)
        .expect("read assembled device")
}

/// The assembled device of `array`.
pub(super) fn device_of<'a>(
    assembled: &'a [AssembledArray],
    array: &RaidArray,
) -> &'a AssembledArray {
    assembled
        .iter()
        .find(|device| device.set_uuid == array.set_uuid)
        .expect("assembled device")
}

/// `(slot, state)` of every listed member, in listing order.
pub(super) fn slots(array: &RaidArray) -> Vec<(Option<u32>, RaidMemberState)> {
    array
        .members
        .iter()
        .map(|member| (member.slot, member.state))
        .collect()
}

pub(super) fn mentions(diagnostics: &[String], text: &str) -> bool {
    diagnostics.iter().any(|message| message.contains(text))
}
