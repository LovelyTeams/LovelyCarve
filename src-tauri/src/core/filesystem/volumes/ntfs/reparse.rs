//! Bounded NTFS reparse metadata. Targets remain evidence paths, never host paths.
use super::*;
use anyhow::{ensure, Context};
use ntfs::structured_values::NtfsFileAttributeFlags;

pub(super) const WOF_TAG: u32 = 0x8000_0017;

#[derive(Debug, Clone)]
pub(super) struct ReparsePoint {
    pub tag: u32,
    pub data: Vec<u8>,
}

impl ReparsePoint {
    pub(super) fn parse(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() >= 8, "truncated NTFS reparse header");
        let tag = u32::from_le_bytes(bytes[..4].try_into()?);
        let size = usize::from(u16::from_le_bytes(bytes[4..6].try_into()?));
        ensure!(
            bytes.len() <= 16 * 1024 && 8 + size <= bytes.len(),
            "invalid NTFS reparse length: tag {tag:#010x}, declared {size}, attribute {}",
            bytes.len()
        );
        Ok(Self {
            tag,
            // Attribute values can retain trailing bytes (including a 16-byte
            // zero tail on AF_UNIX points). Only the declared payload belongs
            // to the reparse data; names and WOF must never read into that tail.
            data: bytes[8..8 + size].to_vec(),
        })
    }

    pub(super) fn link_target(&self) -> Result<Option<String>> {
        self.name_at(0)
    }

    pub(super) fn print_name(&self) -> Result<Option<String>> {
        self.name_at(4)
    }

    pub(super) fn symlink_flags(&self) -> Result<Option<u32>> {
        if self.tag != 0xa000_000c {
            return Ok(None);
        }
        Ok(Some(u32::from_le_bytes(
            self.data
                .get(8..12)
                .context("truncated symlink flags")?
                .try_into()?,
        )))
    }

    fn name_at(&self, field: usize) -> Result<Option<String>> {
        let start = match self.tag {
            0xa000_000c => 12,
            0xa000_0003 => 8,
            _ => return Ok(None),
        };
        ensure!(self.data.len() >= start, "truncated NTFS link metadata");
        let offset = usize::from(u16::from_le_bytes(self.data[field..field + 2].try_into()?));
        let length = usize::from(u16::from_le_bytes(
            self.data[field + 2..field + 4].try_into()?,
        ));
        ensure!(
            offset % 2 == 0 && length % 2 == 0,
            "unaligned NTFS link target"
        );
        let bytes = self
            .data
            .get(start + offset..start + offset + length)
            .context("NTFS link target outside reparse data")?;
        let words: Vec<_> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        Ok(Some(
            String::from_utf16(&words).context("invalid UTF-16 NTFS link target")?,
        ))
    }
}

pub(super) fn read_reparse<T: Read + Seek>(
    file: &ntfs::NtfsFile,
    fs: &mut T,
) -> Result<Option<ReparsePoint>> {
    // Deleted files can retain non-resident attribute lists whose clusters
    // have been freed or reused. Do not read those clusters just to establish
    // the absence of a reparse point. Missing SI or a physical reparse
    // attribute still takes the validated lookup path below.
    if !file.flags().contains(NtfsFileFlags::IN_USE)
        && file.info().is_ok_and(|info| {
            !info
                .file_attributes()
                .contains(NtfsFileAttributeFlags::REPARSE_POINT)
        })
    {
        let mut has_local_reparse = false;
        for attribute in file.attributes_raw() {
            if attribute?.raw_type() == NtfsAttributeType::ReparsePoint as u32 {
                has_local_reparse = true;
                break;
            }
        }
        if !has_local_reparse {
            return Ok(None);
        }
    }
    // Filter before resolving attribute-list references: deleted files may
    // retain stale references for unrelated attributes. Matching reparse
    // references still validate their identity and join connected extents.
    let mut attributes = file.attributes().of_type(NtfsAttributeType::ReparsePoint);
    if let Some(item) = attributes.next(fs) {
        let item = item?;
        let attribute = item.to_attribute()?;
        ensure!(
            attribute.value_length() <= 16 * 1024,
            "NTFS reparse point exceeds 16 KiB"
        );
        let mut value = attribute.value(fs)?;
        let mut bytes = vec![0; attribute.value_length() as usize];
        value.read_exact(fs, &mut bytes)?;
        return ReparsePoint::parse(&bytes).map(Some);
    }
    Ok(None)
}

pub(super) fn read_link<T: Read + Seek>(
    file: &ntfs::NtfsFile,
    fs: &mut T,
) -> Result<Option<String>> {
    read_reparse(file, fs)?.map_or(Ok(None), |point| point.link_target())
}

#[cfg(test)]
#[path = "reparse_tests.rs"]
mod tests;
