//! Signature recognition for CramFS and Linux ROMFS. Full bounds and integrity
//! checks run when opening the native filesystem reader.
use super::PartitionKind;

pub fn detect_kind(bytes: &[u8]) -> Option<PartitionKind> {
    if bytes.starts_with(b"-rom1fs-") {
        Some(PartitionKind::Romfs)
    } else if lamfold_cramfs::superblock_offset(bytes).is_some() {
        Some(PartitionKind::Cramfs)
    } else {
        None
    }
}
