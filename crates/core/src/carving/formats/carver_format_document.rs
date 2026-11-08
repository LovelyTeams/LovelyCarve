//! PDF and SQLite scanners (PDF end detection extends Digler's marker search).

use anyhow::bail;

use crate::carving::carver::CarveSource;
use crate::carving::carver_formats::FormatScanResult;
use crate::carving::carver_reader::{seek_at, CarveReader};

const PDF_EOF: &[u8] = b"%%EOF";
/// Digler's reach from one position to the next end-of-file marker.
const PDF_EOF_REACH: u64 = 16 * 1024 * 1024;
/// `startxref`, its offset and the line breaks sit well inside this span.
const STARTXREF_LOOKBACK: usize = 64;
/// Bare `%%EOF` strings (inside streams or comments) skipped before giving up.
const MAX_BARE_MARKERS: usize = 32;
/// Bytes inspected where an object header or `xref` keyword should begin.
const SECTION_PEEK: usize = 32;
/// Bytes after an end marker searched for the start of an update section.
const SECTION_LOOKAHEAD: usize = 4096;

/// A PDF ends at a `%%EOF` whose line is preceded by `startxref <offset>`.
/// Incremental updates and the main part of a linearized file follow such an
/// end directly with new objects or an xref table and close with their own
/// marker, whose `startxref` must point at an xref section of this file. The
/// carved size covers the last accepted marker and its line break.
pub(crate) fn scan_pdf<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    if &reader.read_array::<5>()? != b"%PDF-" {
        bail!("invalid PDF file");
    }
    let mut fallback_end = None;
    let mut end = None;
    for _ in 0..MAX_BARE_MARKERS {
        if !seek_at(reader, PDF_EOF, PDF_EOF_REACH)? {
            break;
        }
        let marker = reader.bytes_read();
        let has_startxref = startxref_before(reader, marker)?.is_some();
        let marker_end = consume_marker(reader)?;
        if has_startxref {
            end = Some(marker_end);
            break;
        }
        fallback_end.get_or_insert(marker_end);
    }
    let Some(mut end) = end.or(fallback_end) else {
        bail!("PDF end-of-file marker not found");
    };
    while opens_pdf_section(reader)? {
        if !seek_at(reader, PDF_EOF, PDF_EOF_REACH)? {
            break;
        }
        let marker = reader.bytes_read();
        match startxref_before(reader, marker)? {
            Some(xref) if xref < marker && xref_section_at(reader, xref)? => {}
            _ => break,
        }
        end = consume_marker(reader)?;
    }
    Ok(Some(FormatScanResult::sized(end)))
}

/// Consumes `%%EOF` plus one optional line break; returns the new offset.
fn consume_marker<S: CarveSource>(reader: &mut CarveReader<'_, S>) -> anyhow::Result<u64> {
    reader.discard_exact(PDF_EOF.len() as u64)?;
    let mut line_break = [0u8; 2];
    let available = reader.remaining().min(2) as usize;
    let line_break = &mut line_break[..available];
    let position = reader.bytes_read();
    reader.peek_at(position, line_break)?;
    let length = match line_break {
        [b'\r', b'\n', ..] => 2,
        [b'\r' | b'\n', ..] => 1,
        _ => 0,
    };
    reader.discard_exact(length)?;
    Ok(reader.bytes_read())
}

/// The `startxref` offset written on the lines just before `marker`.
fn startxref_before<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
    marker: u64,
) -> anyhow::Result<Option<u64>> {
    let from = marker.saturating_sub(STARTXREF_LOOKBACK as u64);
    let mut lines = [0u8; STARTXREF_LOOKBACK];
    let lines = &mut lines[..(marker - from) as usize];
    reader.peek_at(from, lines)?;
    let Some(keyword) = memchr::memmem::rfind(lines, b"startxref") else {
        return Ok(None);
    };
    let rest = skip_pdf_whitespace(&lines[keyword + b"startxref".len()..]);
    let digits = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    if digits == 0 || digits > 19 || !rest[digits..].iter().all(|byte| is_pdf_whitespace(*byte)) {
        return Ok(None);
    }
    Ok(std::str::from_utf8(&rest[..digits])
        .ok()
        .and_then(|text| text.parse().ok()))
}

/// Whether the bytes at the current position begin an update section, after
/// the padding and comment lines some writers put first (linearizers pad with
/// spaces; ExifTool opens its updates with `%BeginExifToolUpdate`).
fn opens_pdf_section<S: CarveSource>(reader: &mut CarveReader<'_, S>) -> anyhow::Result<bool> {
    let position = reader.bytes_read();
    let mut head = vec![0u8; reader.remaining().min(SECTION_LOOKAHEAD as u64) as usize];
    reader.peek_at(position, &mut head)?;
    let mut rest = head.as_slice();
    loop {
        // NUL is PDF whitespace, but after a marker it is slack, not a section.
        let start = rest
            .iter()
            .position(|byte| !matches!(byte, b'\t' | b'\n' | b'\x0c' | b'\r' | b' '))
            .unwrap_or(rest.len());
        rest = &rest[start..];
        if !rest.starts_with(b"%") || rest.starts_with(b"%PDF-") || rest.starts_with(PDF_EOF) {
            return Ok(is_section_start(rest));
        }
        let Some(line_end) = rest.iter().position(|byte| matches!(byte, b'\r' | b'\n')) else {
            return Ok(false);
        };
        rest = &rest[line_end..];
    }
}

/// Whether an xref table or xref stream object starts at `offset`.
fn xref_section_at<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
    offset: u64,
) -> anyhow::Result<bool> {
    let mut head = [0u8; SECTION_PEEK];
    let length = (reader.bytes_read() - offset).min(SECTION_PEEK as u64) as usize;
    let head = &mut head[..length];
    reader.peek_at(offset, head)?;
    Ok(is_section_start(skip_pdf_whitespace(head)))
}

/// `xref` or an indirect object header (`12 0 obj`).
fn is_section_start(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"xref") {
        return true;
    }
    let object = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    let rest = &bytes[object..];
    let gap = rest.iter().take_while(|byte| **byte == b' ').count();
    let rest = &rest[gap..];
    let generation = rest.iter().take_while(|byte| byte.is_ascii_digit()).count();
    let rest = &rest[generation..];
    let gap_after = rest.iter().take_while(|byte| **byte == b' ').count();
    object > 0
        && gap > 0
        && generation > 0
        && gap_after > 0
        && rest[gap_after..].starts_with(b"obj")
}

fn skip_pdf_whitespace(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !is_pdf_whitespace(*byte))
        .unwrap_or(bytes.len());
    &bytes[start..]
}

fn is_pdf_whitespace(byte: u8) -> bool {
    matches!(byte, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

pub(crate) fn scan_sqlite<S: CarveSource>(
    reader: &mut CarveReader<'_, S>,
) -> anyhow::Result<Option<FormatScanResult>> {
    let header = reader.read_vec(100)?;
    if &header[..16] != b"SQLite format 3\0" {
        bail!("invalid SQLite signature");
    }
    let raw_page_size = u16::from_be_bytes([header[16], header[17]]);
    let page_size = if raw_page_size == 1 {
        65_536u64
    } else {
        u64::from(raw_page_size)
    };
    if !page_size.is_power_of_two() || !(512..=65_536).contains(&page_size) {
        bail!("invalid SQLite page size");
    }
    let file_change_counter = u32::from_be_bytes(header[24..28].try_into()?);
    let file_size_in_pages = u32::from_be_bytes(header[28..32].try_into()?);
    let version_valid_for = u32::from_be_bytes(header[92..96].try_into()?);
    let size = if file_size_in_pages != 0 && file_change_counter == version_valid_for {
        u64::from(file_size_in_pages) * page_size
    } else {
        0
    };
    Ok(Some(FormatScanResult::sized(size)))
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

    fn carve_pdf(bytes: &[u8]) -> Option<u64> {
        let source = Bytes(Arc::new(bytes.to_vec()));
        let cancellation = CancellationToken::new();
        let mut lookahead = Vec::new();
        let mut reader = CarveReader::new(
            &source,
            0,
            bytes.len() as u64,
            &bytes[..bytes.len().min(700)],
            &mut lookahead,
            &cancellation,
        );
        scan_pdf(&mut reader)
            .ok()
            .flatten()
            .map(|result| result.size)
    }

    /// A PDF whose last xref section sits at `xref` bytes into the file.
    fn document(body: &str) -> Vec<u8> {
        let mut bytes = format!("%PDF-1.4\n1 0 obj\n<<{body}>>\nendobj\n").into_bytes();
        let xref = bytes.len();
        bytes.extend_from_slice(
            format!("xref\n0 2\ntrailer\n<<>>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
        );
        bytes
    }

    #[test]
    fn size_covers_the_marker_and_its_line_break() {
        let pdf = document("/Type /Catalog");
        let mut evidence = pdf.clone();
        evidence.extend_from_slice(&[0u8; 300]);
        assert_eq!(carve_pdf(&evidence), Some(pdf.len() as u64));

        let mut crlf = pdf[..pdf.len() - 1].to_vec();
        crlf.extend_from_slice(b"\r\n");
        assert_eq!(carve_pdf(&crlf), Some(crlf.len() as u64));
    }

    #[test]
    fn a_following_file_does_not_extend_the_pdf() {
        let first = document("/A 1");
        let mut evidence = first.clone();
        evidence.extend_from_slice(&[0u8; 512 - 100]);
        evidence.extend_from_slice(&document("/B 2"));
        assert_eq!(carve_pdf(&evidence), Some(first.len() as u64));

        // Directly adjacent: a new header is not an update section either.
        let mut adjacent = first.clone();
        adjacent.extend_from_slice(&document("/C 3"));
        assert_eq!(carve_pdf(&adjacent), Some(first.len() as u64));
    }

    #[test]
    fn incremental_updates_and_linearized_files_are_kept_whole() {
        let mut updated = document("/Base 1");
        let update_xref = updated.len() + "2 0 obj\n<<>>\nendobj\n".len();
        updated.extend_from_slice(
            format!("2 0 obj\n<<>>\nendobj\nxref\n0 1\ntrailer\n<<>>\nstartxref\n{update_xref}\n%%EOF\n")
                .as_bytes(),
        );
        let mut evidence = updated.clone();
        evidence.extend_from_slice(b"\0\0garbage");
        assert_eq!(carve_pdf(&evidence), Some(updated.len() as u64));

        // Linearized: the first-page trailer says `startxref 0`, the final one
        // points back at the first-page xref near the start of the file.
        let mut linearized = b"%PDF-1.5\n1 0 obj\n<</Linearized 1>>\nendobj\n".to_vec();
        let first_xref = linearized.len();
        linearized.extend_from_slice(b"xref\n1 1\ntrailer\n<<>>\nstartxref\n0\n%%EOF\n");
        linearized.extend_from_slice(b"2 0 obj\n<<>>\nendobj\n");
        linearized.extend_from_slice(format!("startxref\n{first_xref}\n%%EOF\n").as_bytes());
        assert_eq!(carve_pdf(&linearized), Some(linearized.len() as u64));

        // Space padding after the first-page section, as some linearizers write.
        let mut padded = b"%PDF-1.5\n1 0 obj\n<<>>\nendobj\n".to_vec();
        let padded_xref = padded.len();
        padded.extend_from_slice(b"xref\n1 1\ntrailer\n<<>>\nstartxref\n0\r\n%%EOF\r\n");
        padded.extend_from_slice(&[b' '; 60]);
        padded.extend_from_slice(b"2 0 obj\n<<>>\nendobj\n");
        padded.extend_from_slice(format!("startxref\n{padded_xref}\n%%EOF\n").as_bytes());
        assert_eq!(carve_pdf(&padded), Some(padded.len() as u64));

        // ExifTool opens its update with a comment line.
        let mut exiftool = document("/Base 2");
        let update = "%BeginExifToolUpdate\n3 0 obj\n<<>>\nendobj\n";
        let update_xref = exiftool.len() + update.len();
        exiftool.extend_from_slice(update.as_bytes());
        exiftool.extend_from_slice(
            format!(
                "xref\n0 1\ntrailer\n<<>>\n%EndExifToolUpdate 9\nstartxref\n{update_xref}\n%%EOF\n"
            )
            .as_bytes(),
        );
        assert_eq!(carve_pdf(&exiftool), Some(exiftool.len() as u64));
    }

    #[test]
    fn an_update_pointing_outside_the_file_is_not_merged() {
        let first = document("/A 1");
        let mut evidence = first.clone();
        evidence.extend_from_slice(b"7 0 obj\n<<>>\nendobj\nstartxref\n999999\n%%EOF\n");
        assert_eq!(carve_pdf(&evidence), Some(first.len() as u64));
    }

    #[test]
    fn a_bare_marker_in_the_body_is_skipped() {
        let mut pdf =
            b"%PDF-1.4\n1 0 obj\n<</Length 6>>stream\n%%EOF\nendstream\nendobj\n".to_vec();
        let xref = pdf.len();
        pdf.extend_from_slice(
            format!("xref\n0 1\ntrailer\n<<>>\nstartxref\n{xref}\n%%EOF").as_bytes(),
        );
        assert_eq!(carve_pdf(&pdf), Some(pdf.len() as u64));

        // Without any startxref the first marker still ends the file.
        let bare = b"%PDF-1.1\nhello\n%%EOF\n".to_vec();
        assert_eq!(carve_pdf(&bare), Some(bare.len() as u64));
        assert_eq!(carve_pdf(b"%PDF-1.1\nno end"), None);
    }
}
