//! TIFF scanner. The file extent is the furthest byte referenced by any IFD:
//! the IFDs themselves, out-of-line tag values, image strips / tiles and
//! embedded JPEG streams, following IFD chains and EXIF / GPS / sub-IFDs.
//! TIFF-based camera raw files (DNG, CR2, NEF, ARW...) carve the same way.
//! IFD tables must be well formed; references that point past the carve bound
//! or claim more than the image geometry allows (corrupt or malicious files)
//! are ignored rather than trusted.

use std::collections::HashSet;

use anyhow::bail;

use crate::carving::carver::CarveSource;
use crate::carving::carver_formats::FormatScanResult;
use crate::carving::carver_reader::CarveReader;

const IMAGE_WIDTH: u16 = 256;
const IMAGE_LENGTH: u16 = 257;
const BITS_PER_SAMPLE: u16 = 258;
const COMPRESSION: u16 = 259;
const STRIP_OFFSETS: u16 = 273;
const SAMPLES_PER_PIXEL: u16 = 277;
const ROWS_PER_STRIP: u16 = 278;
const TILE_WIDTH: u16 = 322;
const TILE_LENGTH: u16 = 323;
const STRIP_BYTE_COUNTS: u16 = 279;
const TILE_OFFSETS: u16 = 324;
const TILE_BYTE_COUNTS: u16 = 325;
const SUB_IFDS: u16 = 330;
const JPEG_OFFSET: u16 = 513;
const JPEG_LENGTH: u16 = 514;
const EXIF_IFD: u16 = 34665;
const GPS_IFD: u16 = 34853;
const INTEROPERABILITY_IFD: u16 = 40965;
const MAX_IFDS: usize = 256;
const MAX_ENTRIES: u16 = 4096;
/// Strip / tile tables longer than this are not followed.
const MAX_ARRAY_VALUES: u32 = 1 << 20;
/// Largest out-of-line tag value followed; ICC profiles, XMP packets and
/// maker notes stay far below it.
const MAX_TAG_VALUE_BYTES: u64 = 64 * 1024 * 1024;
/// Room a compressed strip or tile may take beyond twice its raw size.
const CHUNK_SLACK_BYTES: u64 = 64 * 1024;

#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    value: [u8; 4],
}

struct Tiff {
    little_endian: bool,
    bound: u64,
    end: u64,
}

pub(crate) fn scan_tiff<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let header = reader.read_array::<8>()?;
    let little_endian = match &header[..4] {
        b"II\x2a\x00" => true,
        b"MM\x00\x2a" => false,
        _ => bail!("invalid TIFF header"),
    };
    let mut tiff = Tiff {
        little_endian,
        bound: reader.remaining_from(0),
        end: 8,
    };
    let first_ifd = u64::from(tiff.u32(&header[4..8]));
    if first_ifd < 8 {
        bail!("invalid TIFF IFD offset");
    }
    let mut pending = vec![(first_ifd, true)];
    let mut visited = HashSet::new();
    while let Some((offset, primary)) = pending.pop() {
        if !visited.insert(offset) {
            continue;
        }
        if visited.len() > MAX_IFDS {
            bail!("too many TIFF IFDs");
        }
        let (entries, next) = tiff.read_ifd(reader, offset)?;
        if primary && visited.len() == 1 {
            let has = |tag| entries.iter().any(|entry| entry.tag == tag);
            if !has(IMAGE_WIDTH) || !has(IMAGE_LENGTH) {
                bail!("TIFF IFD0 has no image dimensions");
            }
        }
        tiff.cover_entries(reader, &entries, &mut pending)?;
        if next != 0 && primary {
            pending.push((next, true));
        }
    }
    Ok(Some(FormatScanResult::with_extension(
        tiff.end.min(tiff.bound),
        "tif",
    )))
}

impl Tiff {
    fn read_ifd<S: CarveSource>(
        &mut self,
        reader: &mut CarveReader<'_, S>,
        offset: u64,
    ) -> anyhow::Result<(Vec<Entry>, u64)> {
        let mut count = [0u8; 2];
        self.read_at(reader, offset, &mut count)?;
        let count = self.u16(&count);
        if count == 0 || count > MAX_ENTRIES {
            bail!("implausible TIFF IFD entry count");
        }
        let table_length = usize::from(count) * 12 + 4;
        let mut table = vec![0u8; table_length];
        self.read_at(reader, offset + 2, &mut table)?;
        self.end = self.end.max(offset + 2 + table_length as u64);
        let mut entries = Vec::with_capacity(usize::from(count));
        for raw in table[..table_length - 4].as_chunks::<12>().0 {
            let entry = Entry {
                tag: self.u16(&raw[..2]),
                kind: self.u16(&raw[2..4]),
                count: self.u32(&raw[4..8]),
                value: [raw[8], raw[9], raw[10], raw[11]],
            };
            // Tags should ascend, but real writers (and exploit samples) do not
            // always sort them; field types alone keep random bytes out.
            if type_size(entry.kind) == 0 {
                bail!("unknown TIFF field type {}", entry.kind);
            }
            entries.push(entry);
        }
        let next = u64::from(self.u32(&table[table_length - 4..]));
        Ok((entries, next))
    }

    fn cover_entries<S: CarveSource>(
        &mut self,
        reader: &mut CarveReader<'_, S>,
        entries: &[Entry],
        pending: &mut Vec<(u64, bool)>,
    ) -> anyhow::Result<()> {
        for entry in entries {
            let length = u64::from(entry.count) * type_size(entry.kind);
            if length > 4 && length <= MAX_TAG_VALUE_BYTES {
                self.cover(u64::from(self.u32(&entry.value)), length);
            }
            if matches!(
                entry.tag,
                SUB_IFDS | EXIF_IFD | GPS_IFD | INTEROPERABILITY_IFD
            ) {
                for child in self.values(reader, entry)? {
                    if child != 0 {
                        pending.push((child, false));
                    }
                }
            }
        }
        let geometry = Geometry::read(self, reader, entries)?;
        for (offsets, counts, raw_size) in [
            (STRIP_OFFSETS, STRIP_BYTE_COUNTS, geometry.strip_bytes()),
            (TILE_OFFSETS, TILE_BYTE_COUNTS, geometry.tile_bytes()),
            (JPEG_OFFSET, JPEG_LENGTH, None),
        ] {
            let Some(offsets) = find(entries, offsets) else {
                continue;
            };
            let offsets = self.values(reader, &offsets)?;
            let counts = match find(entries, counts) {
                Some(counts) => self.values(reader, &counts)?,
                // Some writers leave StripByteCounts out of a lone raw strip.
                None if offsets.len() == 1 && geometry.uncompressed => {
                    raw_size.into_iter().collect()
                }
                None => Vec::new(),
            };
            let limit = raw_size.map(|size| size.saturating_mul(2) + CHUNK_SLACK_BYTES);
            for (offset, count) in offsets.into_iter().zip(counts) {
                if limit.is_none_or(|limit| count <= limit) {
                    self.cover(offset, count);
                }
            }
        }
        Ok(())
    }

    /// Unsigned integer values of a SHORT / LONG / IFD field; empty when the
    /// table lies past the carve bound.
    fn values<S: CarveSource>(
        &self,
        reader: &mut CarveReader<'_, S>,
        entry: &Entry,
    ) -> anyhow::Result<Vec<u64>> {
        let width = match entry.kind {
            3 => 2,
            4 | 13 => 4,
            _ => return Ok(Vec::new()),
        };
        if entry.count > MAX_ARRAY_VALUES {
            return Ok(Vec::new());
        }
        let length = entry.count as usize * width;
        let bytes = if length <= 4 {
            entry.value[..length].to_vec()
        } else {
            let offset = u64::from(self.u32(&entry.value));
            if offset.saturating_add(length as u64) > self.bound {
                return Ok(Vec::new());
            }
            let mut bytes = vec![0u8; length];
            reader.peek_at(offset, &mut bytes)?;
            bytes
        };
        Ok(bytes
            .chunks_exact(width)
            .map(|value| {
                if width == 2 {
                    u64::from(self.u16(value))
                } else {
                    u64::from(self.u32(value))
                }
            })
            .collect())
    }

    /// Extends the file over `[offset, offset + length)` when that lies inside
    /// the carve bound.
    fn cover(&mut self, offset: u64, length: u64) {
        let end = offset.saturating_add(length);
        if end <= self.bound {
            self.end = self.end.max(end);
        }
    }

    fn read_at<S: CarveSource>(
        &self,
        reader: &mut CarveReader<'_, S>,
        offset: u64,
        destination: &mut [u8],
    ) -> anyhow::Result<()> {
        if offset.saturating_add(destination.len() as u64) > self.bound {
            bail!("TIFF structure runs past the carve bound");
        }
        reader.peek_at(offset, destination)
    }

    fn u16(&self, bytes: &[u8]) -> u16 {
        let bytes = [bytes[0], bytes[1]];
        if self.little_endian {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        }
    }

    fn u32(&self, bytes: &[u8]) -> u32 {
        let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
        if self.little_endian {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        }
    }
}

fn find(entries: &[Entry], tag: u16) -> Option<Entry> {
    entries.iter().find(|entry| entry.tag == tag).copied()
}

/// Raw (uncompressed) sizes an IFD's strips and tiles can hold.
struct Geometry {
    width: u64,
    length: u64,
    bits_per_pixel: u64,
    rows_per_strip: u64,
    tile: Option<(u64, u64)>,
    uncompressed: bool,
}

impl Geometry {
    fn read<S: CarveSource>(
        tiff: &Tiff,
        reader: &mut CarveReader<'_, S>,
        entries: &[Entry],
    ) -> anyhow::Result<Self> {
        let mut first = |tag, default: u64| -> anyhow::Result<Vec<u64>> {
            Ok(match find(entries, tag) {
                Some(entry) => tiff.values(reader, &entry)?,
                None => vec![default],
            })
        };
        let one = |values: Vec<u64>| values.first().copied().unwrap_or(0);
        let width = one(first(IMAGE_WIDTH, 0)?);
        let length = one(first(IMAGE_LENGTH, 0)?);
        let bits = first(BITS_PER_SAMPLE, 1)?;
        let samples = one(first(SAMPLES_PER_PIXEL, 1)?);
        let bits_per_pixel = if bits.len() > 1 {
            bits.iter().sum()
        } else {
            one(bits).saturating_mul(samples)
        };
        let rows_per_strip = one(first(ROWS_PER_STRIP, u64::from(u32::MAX))?);
        let tile = match (find(entries, TILE_WIDTH), find(entries, TILE_LENGTH)) {
            (Some(_), Some(_)) => Some((one(first(TILE_WIDTH, 0)?), one(first(TILE_LENGTH, 0)?))),
            _ => None,
        };
        let uncompressed = one(first(COMPRESSION, 1)?) == 1;
        Ok(Self {
            width,
            length,
            bits_per_pixel,
            rows_per_strip,
            tile,
            uncompressed,
        })
    }

    fn row_bytes(&self, pixels: u64) -> u64 {
        pixels.saturating_mul(self.bits_per_pixel).div_ceil(8)
    }

    fn strip_bytes(&self) -> Option<u64> {
        (self.width > 0 && self.length > 0 && self.bits_per_pixel > 0).then(|| {
            self.row_bytes(self.width)
                .saturating_mul(self.rows_per_strip.min(self.length))
        })
    }

    fn tile_bytes(&self) -> Option<u64> {
        let (width, length) = self.tile?;
        (width > 0 && length > 0 && self.bits_per_pixel > 0)
            .then(|| self.row_bytes(width).saturating_mul(length))
    }
}

/// Byte size of one value of a TIFF field type; 0 for unknown types.
fn type_size(kind: u16) -> u64 {
    match kind {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use tokio_util::sync::CancellationToken;

    use super::*;

    #[derive(Clone)]
    struct Bytes(Arc<Vec<u8>>);

    impl CarveSource for Bytes {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }

        fn read_into(&self, offset: u64, destination: &mut [u8]) -> anyhow::Result<usize> {
            let start = (offset as usize).min(self.0.len());
            let length = destination.len().min(self.0.len() - start);
            destination[..length].copy_from_slice(&self.0[start..start + length]);
            Ok(length)
        }

        fn worker_handle(&self) -> Self {
            self.clone()
        }
    }

    fn carve(bytes: &[u8]) -> Option<u64> {
        let source = Bytes(Arc::new(bytes.to_vec()));
        let cancellation = CancellationToken::new();
        let mut lookahead = Vec::new();
        let mut reader = CarveReader::new(
            &source,
            0,
            bytes.len() as u64,
            &bytes[..bytes.len().min(64)],
            &mut lookahead,
            &cancellation,
        );
        scan_tiff(&mut reader)
            .ok()
            .flatten()
            .map(|result| result.size)
    }

    fn entry(tag: u16, kind: u16, count: u32, value: u32) -> Vec<u8> {
        let mut bytes = tag.to_le_bytes().to_vec();
        bytes.extend_from_slice(&kind.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&value.to_le_bytes());
        bytes
    }

    /// Header, IFD0 right after it, then two strips of pixel data.
    fn strip_tiff() -> Vec<u8> {
        let entries = [
            entry(IMAGE_WIDTH, 3, 1, 16),
            entry(IMAGE_LENGTH, 3, 1, 2),
            entry(STRIP_OFFSETS, 4, 2, 200),
            entry(STRIP_BYTE_COUNTS, 3, 2, 16 | (16 << 16)),
        ];
        let mut bytes = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for entry in &entries {
            bytes.extend_from_slice(entry);
        }
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.resize(200, 0);
        // STRIP_OFFSETS has two LONG values, so they live out of line at 200.
        let mut strip_table = Vec::new();
        strip_table.extend_from_slice(&208u32.to_le_bytes());
        strip_table.extend_from_slice(&224u32.to_le_bytes());
        bytes.extend_from_slice(&strip_table);
        bytes.extend_from_slice(&[0x55; 32]);
        bytes
    }

    #[test]
    fn extent_covers_strips_after_the_ifd() {
        let tiff = strip_tiff();
        let mut evidence = tiff.clone();
        evidence.extend_from_slice(&[0xaa; 512]);
        assert_eq!(carve(&evidence), Some(tiff.len() as u64));
    }

    #[test]
    fn accepts_unsorted_tags_but_needs_image_dimensions() {
        let mut unsorted = strip_tiff();
        // Swap the first two entries so tags descend, as some writers do.
        let (first, second) = (10..22, 22..34);
        let saved = unsorted[first.clone()].to_vec();
        unsorted.copy_within(second.clone(), first.start);
        unsorted[second].copy_from_slice(&saved);
        assert_eq!(carve(&unsorted), Some(unsorted.len() as u64));

        let mut no_width = strip_tiff();
        no_width[10..12].copy_from_slice(&255u16.to_le_bytes());
        assert_eq!(carve(&no_width), None);

        let mut bad_type = strip_tiff();
        bad_type[12..14].copy_from_slice(&99u16.to_le_bytes());
        assert_eq!(carve(&bad_type), None);
    }

    #[test]
    fn single_strip_without_byte_counts_uses_the_geometry() {
        // 16 x 2 pixels, one 8-bit sample: one 32-byte strip at 64.
        let entries = [
            entry(IMAGE_WIDTH, 3, 1, 16),
            entry(IMAGE_LENGTH, 3, 1, 2),
            entry(BITS_PER_SAMPLE, 3, 1, 8),
            entry(STRIP_OFFSETS, 4, 1, 64),
        ];
        let mut bytes = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for entry in &entries {
            bytes.extend_from_slice(entry);
        }
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.resize(64 + 32, 0x11);
        let mut evidence = bytes.clone();
        evidence.extend_from_slice(&[0xee; 100]);
        assert_eq!(carve(&evidence), Some(bytes.len() as u64));
    }

    #[test]
    fn references_past_the_bound_or_the_geometry_are_ignored() {
        // The last strip ends one byte past the evidence: only the first counts.
        let mut truncated = strip_tiff();
        truncated.truncate(truncated.len() - 1);
        assert_eq!(carve(&truncated), Some(224));

        // A strip claiming far more than a 16 x 2 image holds (CVE-2013-3906
        // style) does not stretch the file, even inside larger evidence.
        let entries = [
            entry(IMAGE_WIDTH, 3, 1, 16),
            entry(IMAGE_LENGTH, 3, 1, 2),
            entry(STRIP_OFFSETS, 4, 1, 64),
            entry(STRIP_BYTE_COUNTS, 4, 1, 10_000_000),
        ];
        let mut inflated = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        inflated.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for entry in &entries {
            inflated.extend_from_slice(entry);
        }
        inflated.extend_from_slice(&0u32.to_le_bytes());
        let ifd_end = inflated.len() as u64;
        inflated.resize(12_000_000, 0x33);
        assert_eq!(carve(&inflated), Some(ifd_end));
    }
}
