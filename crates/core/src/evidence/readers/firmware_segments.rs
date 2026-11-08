//! Bounded dispatch of validated firmware envelopes and filesystem-bearing spans.
use super::{chk, safeloader, uimage};
use crate::evidence::partitions::Partition;
use crate::evidence::partitions::{volume_probe::probe_volume, SectorSource};
use anyhow::{ensure, Context, Result};
use tokio_util::sync::CancellationToken;

pub(super) const UIMAGE_MAGIC: &[u8; 4] = &[0x27, 0x05, 0x19, 0x56];
pub(super) const CHK_MAGIC: &[u8; 4] = &[0x2a, 0x23, 0x24, 0x5e];

struct FirmwareSlice<'a>(&'a [u8]);
impl SectorSource for FirmwareSlice<'_> {
    fn size(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(self.0.len());
        let end = start.saturating_add(length).min(self.0.len());
        Ok(self.0[start..end].to_vec())
    }
}

pub(super) fn has_wrapper_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(UIMAGE_MAGIC) || bytes.starts_with(CHK_MAGIC) || safeloader::has_magic(bytes)
}

pub(super) fn parse_image(
    bytes: &[u8],
    base: u64,
    cancel: &CancellationToken,
) -> Result<Vec<Partition>> {
    let mut partitions = parse_payload(bytes, base, cancel, 0)?;
    ensure!(
        !partitions.is_empty(),
        "firmware contains no supported filesystem segments"
    );
    for (index, partition) in partitions.iter_mut().enumerate() {
        partition.id = u8::try_from(index).context("too many firmware filesystems")?;
    }
    Ok(partitions)
}

pub(super) fn append_partitions(
    target: &mut Vec<Partition>,
    incoming: Vec<Partition>,
) -> Result<()> {
    ensure!(
        target
            .len()
            .checked_add(incoming.len())
            .is_some_and(|count| count <= 256),
        "firmware filesystem count limit"
    );
    target.extend(incoming);
    Ok(())
}

pub(super) fn parse_payload(
    bytes: &[u8],
    base: u64,
    cancel: &CancellationToken,
    depth: usize,
) -> Result<Vec<Partition>> {
    ensure!(!cancel.is_cancelled(), "firmware open cancelled");
    ensure!(depth <= 4, "firmware envelope nesting limit");
    if bytes.starts_with(b"HDR0") || bytes.starts_with(b"buffalo.enc\n") {
        return super::parse_trx(bytes, base, cancel);
    }
    if bytes.starts_with(UIMAGE_MAGIC) {
        return uimage::parse(bytes, base, cancel, depth);
    }
    if bytes.starts_with(CHK_MAGIC) {
        return chk::parse(bytes, base, cancel, depth);
    }
    if safeloader::has_magic(bytes) {
        return safeloader::parse(bytes, base, cancel, depth);
    }
    let Some(hit) = probe_volume(&FirmwareSlice(bytes), 1)? else {
        return Ok(Vec::new());
    };
    Ok(vec![Partition {
        id: 0,
        kind: hit.kind,
        start_lba: base,
        length_lba: bytes.len() as u64,
        label: Some("Firmware filesystem".into()),
        unique_guid: None,
        amiga_geometry: None,
    }])
}

pub(super) fn crc32(bytes: &[u8], cancel: &CancellationToken) -> Result<u32> {
    let mut hasher = crc32fast::Hasher::new();
    for chunk in bytes.chunks(64 * 1024) {
        ensure!(!cancel.is_cancelled(), "firmware checksum cancelled");
        hasher.update(chunk);
    }
    Ok(hasher.finalize())
}

pub(super) fn source_offset(base: u64, offset: usize) -> Result<u64> {
    base.checked_add(offset as u64)
        .context("firmware source offset overflow")
}
