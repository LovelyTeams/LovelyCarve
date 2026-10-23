//! TP-Link legacy update envelope with an explicit MINIFS segment descriptor.
//! Only the observed v1 envelope geometry is accepted; no arbitrary magic scan.
use crate::core::evidence::partitions::{Partition, PartitionKind};
use anyhow::{ensure, Context, Result};
use tokio_util::sync::CancellationToken;

fn word(bytes: &[u8], offset: usize) -> Option<usize> {
    Some(u32::from_be_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?) as usize)
}

pub(super) fn has_magic(bytes: &[u8]) -> bool {
    if bytes.len() < 148 || word(bytes, 0x20) != Some(1) {
        return false;
    }
    let Some(base) = word(bytes, 0x54).and_then(|offset| offset.checked_add(20)) else {
        return false;
    };
    bytes.get(base..).is_some_and(minifs::has_magic)
}

pub(super) fn open(bytes: &[u8], cancel: &CancellationToken) -> Result<super::FirmwareImage> {
    ensure!(!cancel.is_cancelled(), "MINIFS firmware open cancelled");
    ensure!(
        word(bytes, 0x18).and_then(|length| length.checked_add(20)) == Some(bytes.len()),
        "TP-Link MINIFS firmware length mismatch"
    );
    let base = word(bytes, 0x54)
        .and_then(|offset| offset.checked_add(20))
        .context("MINIFS segment offset overflow")?;
    let length = word(bytes, 0x58).context("MINIFS segment length missing")?;
    let end = base
        .checked_add(length)
        .context("MINIFS segment end overflow")?;
    ensure!(base >= 148, "MINIFS segment overlaps firmware header");
    let source = bytes
        .get(base..end)
        .context("MINIFS segment exceeds firmware")?;
    let index = minifs::Index::open(&source, minifs::Limits::default())?;
    ensure!(!cancel.is_cancelled(), "MINIFS firmware open cancelled");
    Ok(super::FirmwareImage {
        partitions: vec![Partition {
            id: 0,
            kind: PartitionKind::Minifs,
            start_lba: base as u64,
            length_lba: index.image_length(),
            label: Some("MINIFS".into()),
            unique_guid: None,
            amiga_geometry: None,
        }],
        plaintext: None,
        native_views: Vec::new(),
    })
}
