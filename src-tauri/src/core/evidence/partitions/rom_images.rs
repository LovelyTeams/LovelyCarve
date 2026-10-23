//! Signature recognition for CramFS and Linux ROMFS. Full bounds and integrity
//! checks run when opening the native filesystem reader.
use super::PartitionKind;

pub fn detect_kind(bytes: &[u8]) -> Option<PartitionKind> {
    if bytes.starts_with(&[0x45, 0x3d, 0xcd, 0x28]) || bytes.starts_with(&[0x28, 0xcd, 0x3d, 0x45])
    {
        Some(PartitionKind::Cramfs)
    } else if bytes.starts_with(b"-rom1fs-") {
        Some(PartitionKind::Romfs)
    } else {
        None
    }
}
