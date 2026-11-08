//! Direct byte-range addressing for deleted directory entries.
//!
//! FAT and exFAT can retain a deleted directory entry whose first cluster and
//! logical size are still known. Those entries use `deleted:run:` ids so their
//! content stays readable without the removed carving/orphan-recovery layer.

use std::sync::Arc;

use crate::filesystem::evidence_session::EvidenceSession;
use crate::filesystem::tree::fs_tree::{detect_mime_bytes, shannon_entropy, FsDataRun, FsStat};

pub const DELETED_RUN_PREFIX: &str = "deleted:run:";

pub fn parse_run_id(id: &str) -> Option<(u64, u64)> {
    if !id.starts_with(DELETED_RUN_PREFIX) {
        return None;
    }
    let mut segments: Vec<&str> = id.split(':').collect();
    let length = parse_hex(segments.pop()?)?;
    let offset = parse_hex(segments.pop()?)?;
    Some((offset, length))
}

fn parse_hex(value: &str) -> Option<u64> {
    let hex = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))?;
    u64::from_str_radix(hex, 16).ok()
}

pub fn read(
    image: &Arc<EvidenceSession>,
    id: &str,
    max_bytes: usize,
) -> Option<anyhow::Result<Vec<u8>>> {
    let (offset, length) = parse_run_id(id)?;
    let requested = usize::try_from(length).unwrap_or(usize::MAX).min(max_bytes);
    Some(image.read_range(offset, requested))
}

pub fn stat(image: &Arc<EvidenceSession>, id: &str) -> Option<FsStat> {
    let (offset, length) = parse_run_id(id)?;
    let sample = image
        .read_range(offset, usize::try_from(length.min(4096)).unwrap_or(4096))
        .unwrap_or_default();
    let magic_hex = (!sample.is_empty()).then(|| {
        sample
            .iter()
            .take(16)
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    });
    let entropy = (!sample.is_empty()).then(|| shannon_entropy(&sample));
    Some(FsStat {
        node_id: id.to_string(),
        size: length,
        inode: None,
        mode: None,
        mime: detect_mime_bytes(&sample).to_string(),
        magic_hex,
        entropy,
        data_runs: vec![FsDataRun {
            logical_offset: 0,
            physical_offset: offset,
            length,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::parse_run_id;

    #[test]
    fn parses_deleted_run_id() {
        assert_eq!(
            parse_run_id("deleted:run:part:2:0x1000:0x2000"),
            Some((0x1000, 0x2000))
        );
        assert_eq!(parse_run_id("deleted:run:part:2:1000:2000"), None);
    }
}
