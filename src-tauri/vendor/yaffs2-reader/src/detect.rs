use crate::{ByteOrder, ReadAt, exact, tags::Tags, wire::parse_header};
use anyhow::{Result, ensure};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub page_size: usize,
    pub spare_size: usize,
    pub tags_offset: usize,
    pub order: ByteOrder,
}
impl Geometry {
    pub fn validate(self) -> Result<()> {
        ensure!(
            [512, 1024, 2048, 4096, 8192, 16384].contains(&self.page_size)
                && [16, 32, 64, 128, 256, 512].contains(&self.spare_size)
                && [0, 2].contains(&self.tags_offset)
                && self.tags_offset + 16 <= self.spare_size,
            "unsupported YAFFS2 geometry"
        );
        Ok(())
    }
    pub(crate) fn stride(self) -> usize {
        self.page_size + self.spare_size
    }
}
/// Examine a bounded prefix; ambiguous candidates are never guessed. Callers
/// with known NAND geometry may bypass this probe with `open_with_geometry`.
pub fn detect(source: &dyn ReadAt) -> Result<Option<Geometry>> {
    let length = source.size().min(2 * 1024 * 1024) as usize;
    if length < 528 {
        return Ok(None);
    }
    let bytes = exact(source, 0, length)?;
    Ok(prefix(&bytes, source.size()))
}
/// In-memory recognition, including partial prefixes used by partition probes.
pub fn detect_bytes(bytes: &[u8]) -> Option<Geometry> {
    prefix(
        &bytes[..bytes.len().min(2 * 1024 * 1024)],
        bytes.len() as u64,
    )
}
fn prefix(bytes: &[u8], source_size: u64) -> Option<Geometry> {
    let mut candidates = Vec::new();
    for page_size in [512, 1024, 2048, 4096, 8192, 16384] {
        for spare_size in [16, 32, 64, 128, 256, 512] {
            for tags_offset in [0, 2] {
                for order in [ByteOrder::Little, ByteOrder::Big] {
                    let geometry = Geometry {
                        page_size,
                        spare_size,
                        tags_offset,
                        order,
                    };
                    if geometry.validate().is_ok() && plausible(bytes, source_size, geometry) {
                        candidates.push(geometry);
                    }
                }
            }
        }
    }
    if candidates.len() == 1 {
        candidates.pop()
    } else {
        None
    }
}
fn plausible(bytes: &[u8], source_size: u64, geometry: Geometry) -> bool {
    let mut used = 0;
    let mut headers = 0;
    for page in bytes.chunks_exact(geometry.stride()).take(2048) {
        let tags = match Tags::parse(&page[geometry.page_size..], geometry) {
            Ok(Some(tags)) => tags,
            Ok(None) => continue,
            Err(_) => return false,
        };
        if tags.sequence == 0x21 || tags.object == 0x10 {
            continue;
        }
        used += 1;
        if tags.chunk == 0 {
            let header = match parse_header(page, geometry.order, tags.object) {
                Ok(header) => header,
                Err(_) => return false,
            };
            if tags.validate_header(&header).is_err() {
                return false;
            }
            headers += 1;
        }
        if used >= 8 && headers >= 1 {
            return true;
        }
    }
    headers >= 1 && (used >= 2 || source_size == geometry.stride() as u64)
}
