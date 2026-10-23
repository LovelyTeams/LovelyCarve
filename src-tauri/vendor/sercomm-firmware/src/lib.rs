//! Bounded Sercomm ZIP/NAND decoder for the OpenWrt Netgear NAND profile.
use anyhow::{Context, Result, ensure};
use std::{io::Cursor, ops::Range, sync::Arc};
use tokio_util::sync::CancellationToken;

const SEAL: usize = 512;
const KERNEL: usize = 2 * 1024 * 1024;
const ROOTFS: usize = 6 * 1024 * 1024;
const FOOTER: usize = 128;
const MAX_BYTES: usize = 512 * 1024 * 1024;

/// Verified container data. Physical extents refer to the compressed ZIP member;
/// the rootfs range belongs exclusively to the decoded NAND byte space.
pub struct NandImage {
    source_offset: u64,
    source_length: u64,
    name: String,
    bytes: Arc<[u8]>,
    kernel: Range<usize>,
    rootfs: Range<usize>,
}
impl NandImage {
    pub fn source_extent(&self) -> Range<u64> {
        self.source_offset..self.source_offset + self.source_length
    }
    pub fn member_name(&self) -> &str {
        &self.name
    }
    pub fn kernel_bytes(&self) -> &[u8] {
        &self.bytes[self.kernel.clone()]
    }
    pub fn logical_length(&self) -> u64 {
        self.rootfs.len() as u64
    }
    pub fn read_range(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        if offset >= self.logical_length() {
            return Ok(Vec::new());
        }
        let start = offset as usize;
        let count = length.min(self.rootfs.len() - start);
        Ok(self.bytes[self.rootfs.start + start..self.rootfs.start + start + count].to_vec())
    }
}
struct OpeningReader<'a> {
    image: &'a NandImage,
    cancel: &'a CancellationToken,
}
impl ubidump::ReadAt for OpeningReader<'_> {
    fn size(&self) -> u64 {
        self.image.logical_length()
    }
    fn read_at(&self, offset: u64, length: usize) -> Result<Vec<u8>> {
        ensure!(
            !self.cancel.is_cancelled(),
            "Sercomm UBI validation cancelled"
        );
        ensure!(
            offset
                .checked_add(length as u64)
                .is_some_and(|end| end <= self.size()),
            "Sercomm decoded UBI read exceeds logical volume"
        );
        self.image.read_range(offset, length)
    }
}

/// Validate the seal, ZIP32 stream, NAND padding, matching footer and UBI layout.
/// The caller must also validate the returned legacy uImage's CRCs before use.
#[inline(never)]
pub fn open(bytes: &[u8], cancel: &CancellationToken) -> Result<NandImage> {
    ensure!(
        (SEAL + 22..=MAX_BYTES).contains(&bytes.len()),
        "Sercomm source length exceeds limits"
    );
    ensure!(
        bytes.starts_with(b"sErCoMm")
            && bytes.get(7..11) == Some(&[0, 1, 0, 0])
            && bytes.get(63..70) == Some(b"sErCoMm"),
        "unsupported Sercomm seal version or second magic"
    );
    ensure!(
        bytes[59..63]
            .iter()
            .chain(&bytes[70..511])
            .all(|byte| *byte == 0),
        "unsupported Sercomm seal reserved fields"
    );
    let mut checksum = 0u8;
    for chunk in bytes.chunks(64 * 1024) {
        ensure!(!cancel.is_cancelled(), "Sercomm checksum cancelled");
        checksum = chunk
            .iter()
            .fold(checksum, |value, byte| value.wrapping_add(*byte));
    }
    ensure!(checksum == 0, "Sercomm seal checksum mismatch");
    let payload = &bytes[SEAL..];
    let (data_start, data_length, method, expected, crc, name) = zip_member(payload)?;
    ensure!(
        (ROOTFS + FOOTER + 128 * 1024..=MAX_BYTES).contains(&expected),
        "Sercomm decoded NAND size exceeds limits"
    );
    let compressed = payload
        .get(data_start..data_start + data_length)
        .context("Sercomm ZIP member extent")?;
    let decoded = inflate(compressed, method, expected, crc, cancel)?;
    let root_end = decoded.len() - FOOTER;
    ensure!(
        decoded.get(root_end..root_end + 70) == bytes.get(..70)
            && decoded[root_end + 70..].iter().all(|byte| *byte == 0),
        "Sercomm footer identity or padding mismatch"
    );
    ensure!(
        decoded[..KERNEL].iter().all(|byte| *byte == 0),
        "unsupported Sercomm NAND prefix"
    );
    let kernel = &decoded[KERNEL..ROOTFS];
    ensure!(
        kernel.starts_with(&[0x27, 0x05, 0x19, 0x56]) && kernel.get(30) == Some(&2),
        "Sercomm NAND kernel is not a legacy uImage kernel"
    );
    let kernel_end = 64usize
        .checked_add(be32(kernel, 12)? as usize)
        .context("Sercomm kernel length overflow")?;
    ensure!(
        kernel_end <= kernel.len() && kernel[kernel_end..].iter().all(|byte| *byte == 0),
        "Sercomm kernel length or NAND padding mismatch"
    );
    ensure!(
        decoded.get(ROOTFS..ROOTFS + 4) == Some(b"UBI#")
            && (root_end - ROOTFS).is_multiple_of(128 * 1024),
        "unsupported Sercomm NAND UBI layout"
    );
    let image = NandImage {
        source_offset: (SEAL + data_start) as u64,
        source_length: data_length as u64,
        name,
        bytes: decoded.into(),
        kernel: KERNEL..KERNEL + kernel_end,
        rootfs: ROOTFS..root_end,
    };
    let layout = ubidump::UbiImage::open(&OpeningReader {
        image: &image,
        cancel,
    })
    .context("Sercomm decoded UBI layout")?;
    ensure!(
        layout.peb_size == 128 * 1024,
        "unsupported Sercomm NAND erase-block geometry"
    );
    ensure!(!cancel.is_cancelled(), "Sercomm open cancelled");
    Ok(image)
}

fn be32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_be_bytes(
        bytes
            .get(offset..offset + 4)
            .context("truncated Sercomm kernel field")?
            .try_into()?,
    ))
}

type Member = (usize, usize, zip::CompressionMethod, usize, u32, String);

fn zip_member(payload: &[u8]) -> Result<Member> {
    let end = payload
        .len()
        .checked_sub(22)
        .context("truncated Sercomm ZIP")?;
    let eocd = &payload[end..];
    ensure!(
        eocd.starts_with(b"PK\x05\x06")
            && eocd[4..8] == [0; 4]
            && le16(eocd, 8)? == 1
            && le16(eocd, 10)? == 1
            && le16(eocd, 20)? == 0,
        "unsupported Sercomm ZIP end record (one ZIP32 member required)"
    );
    let central_size = le32(eocd, 12)? as usize;
    let central_start = le32(eocd, 16)? as usize;
    ensure!(
        central_start.checked_add(central_size) == Some(end),
        "Sercomm ZIP central directory extent"
    );
    let central = payload
        .get(central_start..end)
        .context("Sercomm ZIP central directory")?;
    ensure!(
        central.starts_with(b"PK\x01\x02") && central.len() >= 46 && le16(central, 34)? == 0,
        "invalid Sercomm ZIP central header"
    );
    ensure!(
        46 + usize::from(le16(central, 28)?)
            + usize::from(le16(central, 30)?)
            + usize::from(le16(central, 32)?)
            == central_size,
        "Sercomm ZIP directory entry count/length"
    );
    let flags = le16(central, 8)?;
    ensure!(
        flags & !0x0806 == 0,
        "unsupported Sercomm ZIP encryption, descriptor or flags"
    );
    let mut archive = zip::ZipArchive::new(Cursor::new(payload)).context("Sercomm ZIP index")?;
    ensure!(
        archive.len() == 1
            && archive.offset() == 0
            && archive.central_directory_start() == central_start as u64,
        "Sercomm ZIP prefix or directory disagreement"
    );
    let file = archive.by_index_raw(0).context("Sercomm ZIP member")?;
    let name = std::str::from_utf8(file.name_raw())
        .context("Sercomm ZIP non-UTF8 member name")?
        .to_owned();
    ensure!(
        !file.is_dir()
            && !file.encrypted()
            && file
                .unix_mode()
                .is_none_or(|mode| matches!(mode & 0xf000, 0 | 0x8000))
            && name.ends_with(".bin")
            && !name.contains(['/', '\\', '\0'])
            && !matches!(name.as_str(), ".bin" | "..bin"),
        "unsupported Sercomm ZIP member name/type"
    );
    let start = usize::try_from(file.data_start().context("Sercomm ZIP data offset")?)?;
    let length = usize::try_from(file.compressed_size())?;
    let expected = usize::try_from(file.size())?;
    ensure!(
        file.header_start() == 0
            && file.central_header_start() == central_start as u64
            && start.checked_add(length) == Some(central_start),
        "Sercomm ZIP local/data extent"
    );
    let local = payload.get(..start).context("Sercomm ZIP local header")?;
    ensure!(
        local.starts_with(b"PK\x03\x04")
            && local.len() >= 30
            && 30 + usize::from(le16(local, 26)?) + usize::from(le16(local, 28)?) == start
            && le16(local, 6)? == flags
            && le16(local, 8)? == le16(central, 10)?
            && le32(local, 14)? == file.crc32()
            && le32(local, 18)? as usize == length
            && le32(local, 22)? as usize == expected
            && local.get(30..30 + usize::from(le16(local, 26)?)) == Some(file.name_raw()),
        "Sercomm ZIP local/central metadata disagreement"
    );
    Ok((
        start,
        length,
        file.compression(),
        expected,
        file.crc32(),
        name,
    ))
}

fn inflate(
    bytes: &[u8],
    method: zip::CompressionMethod,
    expected: usize,
    crc: u32,
    cancel: &CancellationToken,
) -> Result<Vec<u8>> {
    let mut decoded = Vec::new();
    decoded
        .try_reserve_exact(expected)
        .context("Sercomm decoded NAND allocation")?;
    let mut checksum = crc32fast::Hasher::new();
    match method {
        zip::CompressionMethod::Stored => {
            ensure!(
                bytes.len() == expected,
                "Sercomm stored ZIP length mismatch"
            );
            for chunk in bytes.chunks(64 * 1024) {
                ensure!(!cancel.is_cancelled(), "Sercomm decode cancelled");
                decoded.extend_from_slice(chunk);
                checksum.update(chunk);
            }
        }
        zip::CompressionMethod::Deflated => {
            let mut decoder = flate2::Decompress::new(false);
            let mut buffer = vec![0; 64 * 1024];
            loop {
                ensure!(!cancel.is_cancelled(), "Sercomm decode cancelled");
                let before_in = decoder.total_in();
                let before_out = decoder.total_out();
                let start = usize::try_from(before_in)?;
                let end = start.saturating_add(64 * 1024).min(bytes.len());
                let status = decoder.decompress(
                    &bytes[start..end],
                    &mut buffer,
                    flate2::FlushDecompress::None,
                )?;
                let produced = usize::try_from(decoder.total_out() - before_out)?;
                ensure!(
                    decoded
                        .len()
                        .checked_add(produced)
                        .is_some_and(|size| size <= expected),
                    "Sercomm ZIP output exceeds declared length"
                );
                decoded.extend_from_slice(&buffer[..produced]);
                checksum.update(&buffer[..produced]);
                if status == flate2::Status::StreamEnd {
                    ensure!(
                        decoder.total_in() == bytes.len() as u64,
                        "Sercomm ZIP trailing DEFLATE bytes"
                    );
                    break;
                }
                ensure!(
                    decoder.total_in() > before_in || produced > 0,
                    "Sercomm ZIP truncated DEFLATE stream"
                );
            }
        }
        _ => anyhow::bail!("unsupported Sercomm ZIP compression: {method:?}"),
    }
    ensure!(
        decoded.len() == expected && checksum.finalize() == crc,
        "Sercomm ZIP logical length or CRC mismatch"
    );
    Ok(decoded)
}

fn le16(bytes: &[u8], offset: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .context("truncated Sercomm ZIP field")?
            .try_into()?,
    ))
}
fn le32(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .context("truncated Sercomm ZIP field")?
            .try_into()?,
    ))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
