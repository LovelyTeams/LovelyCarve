//! ZIP scanner: Digler's local-header walk, closed by a central-directory walk
//! instead of a bounded end-record search, with ZIP64 records, APK signing
//! blocks and data descriptors validated against the bytes they describe.

use anyhow::bail;

use crate::carving::carver::CarveSource;
use crate::carving::carver_formats::FormatScanResult;
use crate::carving::carver_reader::{seek_at, CarveReader};

const LOCAL_HEADER: u32 = 0x0403_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const DATA_DESCRIPTOR: u32 = 0x0807_4b50;
const DIGITAL_SIGNATURE: u32 = 0x0505_4b50;
const ZIP64_END_RECORD: u32 = 0x0606_4b50;
const ZIP64_END_LOCATOR: u32 = 0x0706_4b50;
const END_RECORD: u32 = 0x0605_4b50;
/// WinZip writes this before the first local header of a spanned archive
/// that fitted on one segment.
const SPANNING_MARKER: &[u8; 4] = b"PK00";
const APK_SIGNING_BLOCK_MAGIC: &[u8; 16] = b"APK Sig Block 42";
const ZIP64_EXTRA: u16 = 0x0001;
/// Largest stored `mimetype` member read to name ODF / EPUB files.
const MAX_MIMETYPE_BYTES: u64 = 128;

pub(crate) fn scan_zip<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    if &reader.read_array::<4>()? != SPANNING_MARKER {
        reader.unread(4)?;
    }
    let mut members = MemberKinds::default();
    let mut entries = 0usize;
    loop {
        match u32::from_le_bytes(reader.read_array()?) {
            LOCAL_HEADER => {
                parse_local_entry(reader, &mut members)?;
                entries += 1;
            }
            CENTRAL_HEADER if entries > 0 => {
                let (size, central_entries) = walk_central_directory(reader)?;
                // Started on a member inside a larger archive: its central
                // directory also lists the members before this point.
                if central_entries > entries {
                    bail!("ZIP local header belongs to a larger archive");
                }
                return Ok(Some(FormatScanResult::with_extension(
                    size,
                    members.extension(),
                )));
            }
            _ if entries > 0 && skip_apk_signing_block(reader)? => {}
            _ => bail!("invalid ZIP record sequence"),
        }
    }
}

fn parse_local_entry<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
    members: &mut MemberKinds,
) -> anyhow::Result<()> {
    let entry = reader.read_array::<26>()?;
    let flags = le_u16(&entry[2..4]);
    let compression = le_u16(&entry[4..6]);
    let name_length = usize::from(le_u16(&entry[22..24]));
    let variable = reader.read_vec(name_length + usize::from(le_u16(&entry[24..26])))?;
    let (name, extra) = variable.split_at(name_length);
    let zip64_size = zip64_compressed_size(extra);
    let mut compressed_size = u64::from(le_u32(&entry[14..18]));
    if compressed_size == u64::from(u32::MAX) {
        compressed_size = zip64_size.unwrap_or(compressed_size);
    }
    let has_descriptor = flags & 0x0008 != 0;
    let name = String::from_utf8_lossy(name);
    members.observe(&name);
    let data_start = reader.bytes_read();
    if has_descriptor && compressed_size == 0 {
        return seek_past_descriptor(reader, data_start, zip64_size.is_some());
    }
    if name == "mimetype" && compression == 0 && compressed_size <= MAX_MIMETYPE_BYTES {
        let payload = reader.read_vec(compressed_size as usize)?;
        members.observe_mimetype(&payload);
    } else {
        reader.discard_exact(compressed_size)?;
    }
    if has_descriptor {
        let position = reader.bytes_read();
        let length = match peek_u32(reader, position)? {
            Some(DATA_DESCRIPTOR) => descriptor_fields(
                reader,
                position + 4,
                position - data_start,
                zip64_size.is_some(),
            )?
            .map(|fields| fields + 4),
            _ => descriptor_fields(
                reader,
                position,
                position - data_start,
                zip64_size.is_some(),
            )?,
        };
        reader.discard_exact(length.unwrap_or(0))?;
    }
    Ok(())
}

/// Compressed size from the local ZIP64 extended-information field, which
/// carries the uncompressed size first and then the compressed size.
fn zip64_compressed_size(extra: &[u8]) -> Option<u64> {
    let mut rest = extra;
    while rest.len() >= 4 {
        let (id, length) = (le_u16(&rest[..2]), usize::from(le_u16(&rest[2..4])));
        let body = rest.get(4..4 + length)?;
        if id == ZIP64_EXTRA {
            return body.get(8..16).map(le_u64);
        }
        rest = &rest[4 + length..];
    }
    None
}

/// Moves past the data and descriptor of a streamed member whose sizes live
/// only in its descriptor. Stops at the first record after the data: a signed
/// descriptor whose compressed size matches the data (or that is followed by
/// another record), or the next local / central header, in which case the
/// descriptor carried no signature. The search therefore never runs past the
/// member into unrelated evidence.
fn seek_past_descriptor<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
    data_start: u64,
    zip64: bool,
) -> anyhow::Result<()> {
    loop {
        let remaining = reader.remaining();
        if !seek_at(reader, b"PK", remaining)? {
            bail!("ZIP data descriptor not found");
        }
        let position = reader.bytes_read();
        match peek_u32(reader, position)? {
            Some(DATA_DESCRIPTOR) => {
                let data_length = position - data_start;
                if let Some(fields) = descriptor_fields(reader, position + 4, data_length, zip64)? {
                    return reader.discard_exact(fields + 4);
                }
                for length in [16, 24] {
                    if matches!(
                        peek_u32(reader, position + length)?,
                        Some(LOCAL_HEADER | CENTRAL_HEADER | DATA_DESCRIPTOR)
                    ) {
                        return reader.discard_exact(length);
                    }
                }
            }
            Some(LOCAL_HEADER | CENTRAL_HEADER) => return Ok(()),
            _ => {}
        }
        reader.discard_exact(1)?;
    }
}

/// Length of the crc + sizes block at `fields` (12 bytes, or 20 with ZIP64
/// sizes) when it records `data_length` as the compressed size. ZIP64
/// members use 8-byte sizes; the other layout is tried as a fallback.
fn descriptor_fields<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
    fields: u64,
    data_length: u64,
    zip64: bool,
) -> anyhow::Result<Option<u64>> {
    let layouts = if zip64 {
        [(20, 8), (12, 4)]
    } else {
        [(12, 4), (20, 8)]
    };
    for (length, width) in layouts {
        if recorded_size(reader, fields, width)? == Some(data_length) {
            return Ok(Some(length));
        }
    }
    Ok(None)
}

/// Compressed size stored after the crc of a descriptor block at `fields`.
fn recorded_size<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
    fields: u64,
    width: usize,
) -> anyhow::Result<Option<u64>> {
    if reader.remaining_from(fields + 4) < width as u64 {
        return Ok(None);
    }
    let mut bytes = [0u8; 8];
    reader.peek_at(fields + 4, &mut bytes[..width])?;
    Ok(Some(if width == 4 {
        u64::from(le_u32(&bytes))
    } else {
        le_u64(&bytes)
    }))
}

/// Skips an APK signing block, which sits between the last member and the
/// central directory: a 64-bit length, the pairs, the length again and a
/// 16-byte magic. The caller already consumed four bytes of the length.
fn skip_apk_signing_block<S: CarveSource>(reader: &mut CarveReader<'_, S>) -> anyhow::Result<bool> {
    let start = reader.bytes_read() - 4;
    if reader.remaining_from(start) < 8 {
        return Ok(false);
    }
    let mut length = [0u8; 8];
    reader.peek_at(start, &mut length)?;
    let length = u64::from_le_bytes(length);
    // The trailing length and magic alone take 24 bytes.
    let Some(end) = start
        .checked_add(8)
        .and_then(|value| value.checked_add(length))
        .filter(|_| length >= 24)
    else {
        return Ok(false);
    };
    if reader.remaining_from(end - 16) < 16 {
        return Ok(false);
    }
    let mut magic = [0u8; 16];
    reader.peek_at(end - 16, &mut magic)?;
    if &magic != APK_SIGNING_BLOCK_MAGIC {
        return Ok(false);
    }
    reader.discard_exact(end - reader.bytes_read())?;
    Ok(true)
}

/// Walks central directory records up to and including the end record;
/// returns the archive size and the number of central directory entries.
/// The first record's signature is already read.
fn walk_central_directory<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<(u64, usize)> {
    let mut signature = CENTRAL_HEADER;
    let mut entries = 0usize;
    loop {
        match signature {
            CENTRAL_HEADER => {
                entries += 1;
                let header = reader.read_array::<42>()?;
                let variable = u64::from(le_u16(&header[24..26]))
                    + u64::from(le_u16(&header[26..28]))
                    + u64::from(le_u16(&header[28..30]));
                reader.discard_exact(variable)?;
            }
            DIGITAL_SIGNATURE => {
                let length = u64::from(u16::from_le_bytes(reader.read_array()?));
                reader.discard_exact(length)?;
            }
            ZIP64_END_RECORD => {
                let length = u64::from_le_bytes(reader.read_array()?);
                reader.discard_exact(length)?;
            }
            ZIP64_END_LOCATOR => reader.discard_exact(16)?,
            END_RECORD => {
                let record = reader.read_array::<18>()?;
                reader.discard_up_to(u64::from(le_u16(&record[16..18])))?;
                return Ok((reader.bytes_read(), entries));
            }
            _ => bail!("invalid ZIP central directory record"),
        }
        signature = u32::from_le_bytes(reader.read_array()?);
    }
}

fn peek_u32<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
    relative: u64,
) -> anyhow::Result<Option<u32>> {
    if reader.remaining_from(relative) < 4 {
        return Ok(None);
    }
    let mut bytes = [0u8; 4];
    reader.peek_at(relative, &mut bytes)?;
    Ok(Some(u32::from_le_bytes(bytes)))
}

/// Member names and the ODF / EPUB `mimetype` payload that identify ZIP-based
/// document and package formats.
#[derive(Default)]
struct MemberKinds {
    content_types: bool,
    relationships: bool,
    word: bool,
    presentation: bool,
    workbook: bool,
    android_manifest: bool,
    java_manifest: bool,
    wheel: bool,
    mimetype: Option<&'static str>,
}

impl MemberKinds {
    fn observe(&mut self, name: &str) {
        match name {
            "[Content_Types].xml" => self.content_types = true,
            "_rels/.rels" => self.relationships = true,
            "word/document.xml" => self.word = true,
            "ppt/presentation.xml" => self.presentation = true,
            "xl/workbook.xml" => self.workbook = true,
            "AndroidManifest.xml" => self.android_manifest = true,
            "META-INF/MANIFEST.MF" => self.java_manifest = true,
            _ if name.ends_with(".dist-info/WHEEL") => self.wheel = true,
            _ => {}
        }
    }

    fn observe_mimetype(&mut self, payload: &[u8]) {
        self.mimetype = match payload {
            b"application/vnd.oasis.opendocument.text" => Some("odt"),
            b"application/vnd.oasis.opendocument.spreadsheet" => Some("ods"),
            b"application/vnd.oasis.opendocument.presentation" => Some("odp"),
            b"application/vnd.oasis.opendocument.graphics" => Some("odg"),
            b"application/epub+zip" => Some("epub"),
            _ => None,
        };
    }

    fn extension(&self) -> &'static str {
        let office = self.content_types && self.relationships;
        if office && self.word {
            "docx"
        } else if office && self.presentation {
            "pptx"
        } else if office && self.workbook {
            "xlsx"
        } else if let Some(extension) = self.mimetype {
            extension
        } else if self.android_manifest {
            "apk"
        } else if self.java_manifest {
            "jar"
        } else if self.wheel {
            "whl"
        } else {
            "zip"
        }
    }
}

fn le_u16(bytes: &[u8]) -> u16 {
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn le_u32(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn le_u64(bytes: &[u8]) -> u64 {
    let mut value = [0u8; 8];
    value.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(value)
}

#[cfg(test)]
#[path = "carver_format_zip_tests.rs"]
mod tests;
