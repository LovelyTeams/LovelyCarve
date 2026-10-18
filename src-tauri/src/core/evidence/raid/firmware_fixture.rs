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

/// Physical member identity from the same descriptor parser used by Windows.
pub(super) fn physical_disk(bytes: Vec<u8>, serial: &[u8], bus: u32) -> Arc<DiskImage> {
    let mut descriptor = vec![0; 36];
    descriptor[0..4].copy_from_slice(&40u32.to_le_bytes());
    descriptor[24..28].copy_from_slice(&36u32.to_le_bytes());
    descriptor[28..32].copy_from_slice(&bus.to_le_bytes());
    descriptor.extend_from_slice(serial);
    descriptor.push(0);
    let size = descriptor.len() as u32;
    descriptor[4..8].copy_from_slice(&size.to_le_bytes());
    let mut image = disk(bytes, None);
    Arc::get_mut(&mut image)
        .expect("unique fixture image")
        .physical_drive_serial =
        crate::core::evidence::physical_disk::parse_descriptor(&descriptor)
            .expect("device descriptor")
            .serial;
    image
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
