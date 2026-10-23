//! Read-only Buffalo, TRX, SafeLoader, legacy uImage, Netgear CHK and Sercomm views. Source hashes and
//! hex reads retain the original container; only filesystem readers use the
//! verified plaintext span. Format references and scope: docs/formats/FIRMWARE.md.

use std::sync::Arc;

use anyhow::{ensure, Context, Result};
use tokio_util::sync::CancellationToken;

use crate::core::evidence::partitions::{Partition, PartitionKind};

#[path = "firmware_chk.rs"]
mod chk;
#[path = "firmware_minifs.rs"]
mod minifs_envelope;
#[path = "firmware_safeloader.rs"]
mod safeloader;
#[path = "firmware_segments.rs"]
mod segments;
#[path = "firmware_sercomm.rs"]
mod sercomm;
#[path = "firmware_uimage.rs"]
mod uimage;

const MAX_ENCRYPTED_BYTES: usize = 512 * 1024 * 1024;
const TRX_HEADER: usize = 28;

pub struct FirmwareImage {
    pub partitions: Vec<Partition>,
    plaintext: Option<(u64, Arc<[u8]>)>,
    native_views: Vec<NativeFirmwareView>,
}

/// An immutable logical byte source, addressed by the source evidence's partition
/// id. Sessions construct filesystem views; source extents continue to describe ZIP bytes.
pub(crate) struct NativeFirmwareView {
    pub partition_id: u8,
    pub label: String,
    pub kind: PartitionKind,
    pub source: Arc<dyn crate::core::evidence::partitions::SectorSource + Send + Sync>,
}

impl std::fmt::Debug for FirmwareImage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FirmwareImage")
            .field("partitions", &self.partitions)
            .field("native_views", &self.native_views.len())
            .field(
                "plaintext_span",
                &self
                    .plaintext
                    .as_ref()
                    .map(|(base, bytes)| (*base, bytes.len())),
            )
            .finish()
    }
}

pub fn has_firmware_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(b"bgn\0")
        || bytes.starts_with(b"start\0")
        || bytes.starts_with(b"asar1\0")
        || bytes.starts_with(b"HDR0")
        || bytes.starts_with(b"buffalo.enc\n")
        || bytes.starts_with(b"sErCoMm")
        || minifs_envelope::has_magic(bytes)
        || segments::has_wrapper_magic(bytes)
}

impl FirmwareImage {
    pub fn open(bytes: &[u8], cancel: &CancellationToken) -> Result<Self> {
        ensure!(!cancel.is_cancelled(), "firmware open cancelled");
        if minifs_envelope::has_magic(bytes) {
            return minifs_envelope::open(bytes, cancel);
        }
        if bytes.starts_with(b"sErCoMm") {
            return sercomm::open(bytes, cancel);
        }
        let encrypted_offset = if bytes.starts_with(b"bgn\0") {
            ensure!(
                be32(bytes, 12)? as usize == bytes.len(),
                "Buffalo bgn source length mismatch"
            );
            let tag = decode_enc(bytes, 20, cancel)?;
            ensure!(tag.data.len() == 152, "unsupported Buffalo bgn tag layout");
            Some(
                tag.end
                    .checked_add(3)
                    .context("Buffalo alignment overflow")?
                    & !3,
            )
        } else if bytes.starts_with(b"start\0") || bytes.starts_with(b"asar1\0") {
            Some(0)
        } else {
            None
        };

        let (payload, base, plaintext) = if let Some(offset) = encrypted_offset {
            let decoded = decode_enc(bytes, offset, cancel)?;
            let data: Arc<[u8]> = decoded.data.into();
            (
                Arc::clone(&data),
                decoded.start as u64,
                Some((decoded.start as u64, data)),
            )
        } else {
            // Unencrypted firmware is parsed in place; never duplicate a large source.
            let partitions = segments::parse_image(bytes, 0, cancel)?;
            return Ok(Self {
                partitions,
                plaintext: None,
                native_views: Vec::new(),
            });
        };
        let partitions = segments::parse_image(&payload, base, cancel)?;
        Ok(Self {
            partitions,
            plaintext,
            native_views: Vec::new(),
        })
    }

    pub(crate) fn native_views(&self) -> &[NativeFirmwareView] {
        &self.native_views
    }

    /// Read within the verified plaintext's original ciphertext span. This
    /// mapping preserves source offsets, but these are transformed bytes.
    pub fn read_plaintext(&self, offset: u64, length: usize) -> Result<Option<Vec<u8>>> {
        let Some((base, bytes)) = &self.plaintext else {
            return Ok(None);
        };
        let relative = offset
            .checked_sub(*base)
            .context("firmware read before plaintext span")?;
        let start = usize::try_from(relative).context("firmware read offset overflow")?;
        let end = start
            .checked_add(length)
            .context("firmware read length overflow")?;
        Ok(Some(
            bytes
                .get(start..end)
                .context("firmware read outside plaintext span")?
                .to_vec(),
        ))
    }
}

fn parse_trx(bytes: &[u8], base: u64, cancel: &CancellationToken) -> Result<Vec<Partition>> {
    let prefix = if bytes.starts_with(b"buffalo.enc\n") {
        let header_end = bytes
            .iter()
            .take(128)
            .enumerate()
            .filter(|(_, byte)| **byte == b'\n')
            .nth(1)
            .map(|(index, _)| index + 1)
            .context("truncated Buffalo plaintext header")?;
        let value = std::str::from_utf8(&bytes[12..header_end - 1])?
            .strip_prefix("filelen=")
            .context("invalid Buffalo filelen header")?;
        let declared: usize = value.parse().context("invalid Buffalo payload length")?;
        ensure!(
            declared == bytes.len() - header_end,
            "Buffalo plaintext length mismatch"
        );
        header_end
    } else {
        0
    };
    let bytes = bytes.get(prefix..).context("TRX prefix exceeds firmware")?;
    ensure!(
        bytes.starts_with(b"HDR0"),
        "unsupported firmware payload (expected Broadcom TRX)"
    );
    let length = le32(bytes, 4)? as usize;
    ensure!(
        (TRX_HEADER..=bytes.len()).contains(&length),
        "TRX length exceeds source or header"
    );
    ensure!(
        le32(bytes, 12)? >> 16 == 1,
        "unsupported TRX header version"
    );
    let expected = le32(bytes, 8)?;
    let mut crc = crc32fast::Hasher::new();
    for chunk in bytes[12..length].chunks(64 * 1024) {
        ensure!(!cancel.is_cancelled(), "firmware open cancelled");
        crc.update(chunk);
    }
    ensure!(!crc.finalize() == expected, "TRX checksum mismatch");
    let mut offsets = Vec::new();
    let mut empty_slot = false;
    for slot in 0..3 {
        let offset = le32(bytes, 16 + slot * 4)? as usize;
        if offset == 0 {
            empty_slot = true;
            continue;
        }
        ensure!(
            !empty_slot && offset >= TRX_HEADER && offset < length,
            "invalid TRX segment offset"
        );
        ensure!(
            offsets.last().is_none_or(|previous| offset > *previous),
            "TRX segments overlap or are unordered"
        );
        offsets.push(offset);
    }
    ensure!(!offsets.is_empty(), "TRX contains no segments");
    let mut partitions = Vec::new();
    for (index, &start) in offsets.iter().enumerate() {
        let end = offsets.get(index + 1).copied().unwrap_or(length);
        let payload = &bytes[start..end];
        let kind = if crate::core::evidence::partitions::squashfs::is_squashfs_magic(payload) {
            PartitionKind::Squashfs
        } else if crate::core::evidence::partitions::jffs2::is_jffs2_magic(payload) {
            PartitionKind::Jffs2
        } else if let Some(geometry) = yaffs2_reader::detect_bytes(payload) {
            match geometry.format {
                yaffs2_reader::Format::Yaffs1 => PartitionKind::Yaffs1,
                yaffs2_reader::Format::Yaffs2 => PartitionKind::Yaffs2,
            }
        } else if crate::core::evidence::partitions::ubifs::is_ubifs_magic(payload) {
            PartitionKind::Ubifs
        } else if let Some(kind) =
            crate::core::evidence::partitions::rom_images::detect_kind(payload)
        {
            kind
        } else {
            continue;
        };
        let absolute = base
            .checked_add(prefix as u64)
            .and_then(|value| value.checked_add(start as u64))
            .context("TRX source offset overflow")?;
        partitions.push(Partition {
            id: u8::try_from(partitions.len()).context("too many firmware volumes")?,
            kind,
            start_lba: absolute,
            length_lba: (end - start) as u64,
            label: Some(format!("TRX rootfs (segment {index})")),
            unique_guid: None,
        });
    }
    Ok(partitions)
}

struct EncPayload {
    data: Vec<u8>,
    start: usize,
    end: usize,
}

fn decode_enc(bytes: &[u8], offset: usize, cancel: &CancellationToken) -> Result<EncPayload> {
    let bytes = bytes
        .get(offset..)
        .context("Buffalo ENC offset exceeds source")?;
    ensure!(
        bytes.starts_with(b"start\0") || bytes.starts_with(b"asar1\0"),
        "invalid Buffalo ENC magic"
    );
    let seed = *bytes.get(6).context("truncated Buffalo ENC seed")?;
    let product_length = be32(bytes, 7)? as usize;
    ensure!(
        (1..=32).contains(&product_length),
        "invalid Buffalo product length"
    );
    let product = bytes
        .get(11..11 + product_length)
        .context("truncated Buffalo product")?;
    let version_offset = 15 + product_length;
    let version_length = be32(bytes, 11 + product_length)? as usize;
    ensure!(
        (1..=8).contains(&version_length),
        "invalid Buffalo version length"
    );
    let version = bytes
        .get(version_offset..version_offset + version_length)
        .context("truncated Buffalo version")?;
    let length_offset = version_offset + version_length;
    let length = be32(bytes, length_offset)? as usize;
    ensure!(
        length <= MAX_ENCRYPTED_BYTES,
        "Buffalo encrypted payload exceeds 512 MiB limit"
    );
    let start = length_offset + 4;
    let end = start
        .checked_add(length)
        .context("Buffalo payload length overflow")?;
    let data = bytes
        .get(start..end)
        .context("truncated Buffalo encrypted payload")?;
    let expected = be32(bytes, end)?;
    let mut product_plain = product.to_vec();
    Rc4::new(seed).apply(&mut product_plain);
    let mut version_plain = version.to_vec();
    Rc4::new(product[0]).apply(&mut version_plain);
    ensure!(
        product_plain.last() == Some(&0) && version_plain.last() == Some(&0),
        "unsupported Buffalo encryption key or mode"
    );
    let mut plaintext = data.to_vec();
    let mut cipher = Rc4::new(version[0]);
    let mut checksum = length as u32;
    for chunk in plaintext.chunks_mut(64 * 1024) {
        ensure!(!cancel.is_cancelled(), "firmware open cancelled");
        cipher.apply(chunk);
        for &byte in chunk.iter() {
            checksum ^= (byte as i8 as i32) as u32;
            for _ in 0..8 {
                checksum = (checksum >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(checksum & 1));
            }
        }
    }
    ensure!(
        checksum == expected,
        "Buffalo ENC checksum mismatch (wrong key/mode or corrupt source)"
    );
    Ok(EncPayload {
        data: plaintext,
        start: offset + start,
        end: offset + end + 4,
    })
}

// Standard RC4 with the format's one-byte seed prepended to the public default key.
struct Rc4 {
    state: [u8; 256],
    first: u8,
    second: u8,
}
impl Rc4 {
    fn new(seed: u8) -> Self {
        let key = [seed, b'B', b'u', b'f', b'f', b'a', b'l', b'o'];
        let mut state = std::array::from_fn(|index| index as u8);
        let mut second = 0u8;
        for first in 0..256 {
            second = second
                .wrapping_add(state[first])
                .wrapping_add(key[first % key.len()]);
            state.swap(first, second as usize);
        }
        Self {
            state,
            first: 0,
            second: 0,
        }
    }
    fn apply(&mut self, bytes: &mut [u8]) {
        for byte in bytes {
            self.first = self.first.wrapping_add(1);
            self.second = self.second.wrapping_add(self.state[self.first as usize]);
            self.state.swap(self.first as usize, self.second as usize);
            let index =
                self.state[self.first as usize].wrapping_add(self.state[self.second as usize]);
            *byte ^= self.state[index as usize];
        }
    }
}

fn le32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .context("truncated firmware field")?
            .try_into()?,
    ))
}
fn be32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .context("truncated firmware field")?
            .try_into()?,
    ))
}

#[cfg(test)]
#[path = "firmware_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "firmware_wrapper_tests.rs"]
mod wrapper_tests;

#[cfg(test)]
#[path = "firmware_sercomm_tests.rs"]
mod sercomm_tests;
