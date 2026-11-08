//! EnCase **Logical** Evidence File (EWF-L01) — the file-entry tree.
//!
//! An L01 shares the EWF v1 container with an E01 and carries something
//! completely different: not disk sectors but a tree of file ENTRIES, selected
//! by the examiner at acquisition time. There is no partition table, no
//! filesystem and no unallocated space, so an L01 cannot be "mounted" the way a
//! disk image can — it is enumerated.
//!
//! The tree lives in an `ltree` section: a 48-byte header followed by a
//! UTF-16LE *text* body of tab-separated, newline-delimited categories
//! (`rec`, `perm`, `srce`, `sub`, `entry`).
//!
//! Reference: libyal, *Expert Witness Compression Format (EWF)*, "Ltree
//! section".

use crate::error::{EwfError, Result};
use safe_read::{le_u32, le_u64};

/// Size of the `ltree` section header in bytes.
pub const LTREE_HEADER_SIZE: usize = 48;

/// Header of an `ltree` section.
///
/// Carries its own integrity values, which is why this type exists rather than
/// the caller indexing bytes: an `ltree` that does not match its stored MD5 is
/// a damaged file tree, and on a logical acquisition the tree IS the evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LtreeHeader {
    /// MD5 of the ltree data that follows the header.
    pub data_md5: [u8; 16],
    /// Size in bytes of the ltree data.
    pub data_size: u64,
    /// Adler-32 stored in the header, computed over the header with the
    /// checksum field itself zeroed.
    pub checksum: u32,
}

impl LtreeHeader {
    /// Parse an `ltree` header from the start of `buf`.
    ///
    /// # Errors
    /// [`EwfError::BufferTooShort`] when fewer than [`LTREE_HEADER_SIZE`] bytes
    /// are available.
    pub fn parse(buf: &[u8]) -> Result<Self> {
        if buf.len() < LTREE_HEADER_SIZE {
            return Err(EwfError::BufferTooShort {
                expected: LTREE_HEADER_SIZE,
                got: buf.len(),
            });
        }
        let mut data_md5 = [0u8; 16];
        data_md5.copy_from_slice(&buf[0..16]);
        Ok(Self {
            data_md5,
            data_size: le_u64(buf, 16),
            checksum: le_u32(buf, 24),
        })
    }

    /// Whether the header's own Adler-32 checks out.
    ///
    /// The stored checksum covers the header bytes with the checksum field
    /// zeroed, so verification must zero the same four bytes rather than skip
    /// them — skipping shortens the buffer and changes the result.
    #[must_use]
    pub fn verify_checksum(&self, buf: &[u8]) -> bool {
        let Some(hdr) = buf.get(..LTREE_HEADER_SIZE) else {
            return false;
        };
        // Zero the checksum field rather than skipping it: skipping would
        // shorten the buffer and change the Adler-32 of everything after it.
        let mut probe = hdr.to_vec();
        probe[24..28].fill(0);
        crate::sections::adler32(&probe) == self.checksum
    }
}

/// One file entry from the `entry` category.
///
/// Fields are the ones an examiner needs to identify and account for a file.
/// Everything the format carries is preserved in [`Self::raw`] so nothing is
/// silently dropped — an L01 is the whole evidence, and a field this reader
/// does not model today may be the one a matter turns on.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct LogicalEntry {
    /// Entry name (`n`).
    pub name: String,
    /// DOS 8.3 alias (`snh`), empty when the acquisition recorded none.
    pub short_name: String,
    /// Whether this entry is a directory (`p` == "1").
    pub is_dir: bool,
    /// Logical size in bytes (`ls`).
    pub size: u64,
    /// MD5 of the file data (`ha`), `None` when unset or all-zero.
    pub md5: Option<String>,
    /// Identifier (`id`).
    pub id: u64,
    /// Indices into the tree's entry vector.
    pub children: Vec<usize>,
    /// Index of the parent entry, `None` for the category root.
    pub parent: Option<usize>,
    /// Every declared field, by indicator name, exactly as stored.
    pub raw: std::collections::BTreeMap<String, String>,
}

/// A parsed `ltree` file-entry tree.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct LogicalTree {
    /// All entries, parents before children.
    pub entries: Vec<LogicalEntry>,
    /// Indicator names declared by this file, in their declared order.
    pub indicators: Vec<String>,
    /// Non-fatal problems found while parsing, for the examiner to see.
    ///
    /// A logical acquisition has no filesystem to cross-check against, so a
    /// field this reader could not make sense of is reported rather than
    /// discarded: silence would be indistinguishable from "nothing was wrong".
    pub warnings: Vec<String>,
}

/// Decode `ltree` data, which is UTF-16LE but **not strict UTF-16**.
///
/// The specification records that it permits unpaired surrogates (U+D800 with
/// no low half, and the like). A strict decoder refuses such input, so a reader
/// that uses one fails on real evidence for a reason that has nothing to do
/// with the evidence. Unpaired halves are replaced, and each replacement is a
/// reported warning rather than a silent substitution.
#[must_use]
pub fn decode_ltree_text(data: &[u8]) -> (String, usize) {
    let units: Vec<u16> = data
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    let mut out = String::with_capacity(units.len());
    let mut replaced = 0usize;
    for r in char::decode_utf16(units.iter().copied()) {
        out.push(r.unwrap_or_else(|_| {
            replaced += 1;
            char::REPLACEMENT_CHARACTER
        }));
    }
    (out, replaced)
}

/// Parse the `entry` category of an `ltree` body into a tree.
///
/// # Errors
/// [`EwfError::MalformedLogicalField`] when no `entry` category is present.
pub fn parse_entry_tree(text: &str) -> Result<LogicalTree> {
    // Lines are newline-delimited; a stray CR is stripped rather than carried
    // into a file name.
    let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();

    let cat = lines.iter().position(|l| *l == "entry").ok_or_else(|| {
        EwfError::MalformedLogicalField {
            field: "ltree category",
            value: "no `entry` category in the ltree text".to_owned(),
        }
    })?;
    // entry / <count>\t1 / <indicators> / then the category root and entries.
    let indicators: Vec<String> = lines
        .get(cat + 2)
        .ok_or_else(|| EwfError::MalformedLogicalField {
            field: "ltree indicators",
            value: format!(
                "ltree ends at line {}, before the indicator row",
                lines.len()
            ),
        })?
        .split('\t')
        .map(str::to_owned)
        .collect();
    if indicators.is_empty() {
        return Err(EwfError::MalformedLogicalField {
            field: "ltree indicators",
            value: "indicator row is empty".to_owned(),
        });
    }

    let mut tree = LogicalTree {
        indicators: indicators.clone(),
        ..LogicalTree::default()
    };

    // The category root is itself an entry pair; its sub-count drives the walk.
    let mut cursor = cat + 3;
    let root_subs = lines
        .get(cursor)
        .and_then(|l| l.split('\t').nth(1))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .ok_or_else(|| EwfError::MalformedLogicalField {
            field: "ltree root sub-count",
            value: lines
                .get(cursor)
                .map_or_else(|| format!("no line at index {cursor}"), |l| (*l).to_owned()),
        })?;
    cursor += 2; // root's two lines

    for _ in 0..root_subs {
        cursor = parse_one(&lines, cursor, None, &indicators, &mut tree);
    }
    Ok(tree)
}

/// Parse one entry pair and, recursively, its declared sub-entries.
///
/// Returns the cursor just past this entry's subtree. A malformed pair advances
/// the cursor rather than looping: a reader that cannot make progress on
/// damaged evidence is worse than one that reports what it could not read.
fn parse_one(
    lines: &[&str],
    mut i: usize,
    parent: Option<usize>,
    indicators: &[String],
    tree: &mut LogicalTree,
) -> usize {
    let Some(counts) = lines.get(i) else {
        return i;
    };
    let nsub = counts
        .split('\t')
        .nth(1)
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let Some(values_line) = lines.get(i + 1) else {
        return i + 1;
    };
    let values: Vec<&str> = values_line.split('\t').collect();
    if values.len() != indicators.len() {
        tree.warnings.push(format!(
            "entry at line {i}: {} values for {} declared indicators",
            values.len(),
            indicators.len()
        ));
    }

    let mut raw = std::collections::BTreeMap::new();
    for (k, v) in indicators.iter().zip(values.iter()) {
        raw.insert(k.clone(), (*v).to_owned());
    }
    let get = |k: &str| raw.get(k).map(String::as_str).unwrap_or_default();

    // "ha" uses 32 zeros as its ABSENT marker; treating that as a real digest
    // would assert the file hashed to zero, which is a different claim.
    let md5 = match get("ha") {
        "" => None,
        h if h.bytes().all(|b| b == b'0') => None,
        h => Some(h.to_owned()),
    };
    // The 8.3 alias is stored as "<size including NUL> <name>". The size is a
    // redundant restatement of the name's length, so a disagreement degrades
    // THIS FIELD and is reported -- it never refuses the acquisition. libewf
    // treats the same mismatch as fatal and cannot open files EnCase writes.
    let raw_snh = get("snh");
    let short_name = match raw_snh.split_once(' ') {
        Some((declared, name)) => {
            if declared.parse::<usize>() != Ok(name.chars().count() + 1) {
                tree.warnings.push(format!(
                    "entry at line {i}: short-name size {declared:?} disagrees with its name length"
                ));
            }
            name.to_owned()
        }
        None => raw_snh.to_owned(),
    };

    let idx = tree.entries.len();
    tree.entries.push(LogicalEntry {
        name: get("n").to_owned(),
        short_name,
        is_dir: get("p") == "1",
        size: get("ls").trim().parse().unwrap_or(0),
        md5,
        id: get("id").trim().parse().unwrap_or(0),
        children: Vec::new(),
        parent,
        raw,
    });
    if let Some(p) = parent {
        tree.entries[p].children.push(idx);
    }

    i += 2;
    for _ in 0..nsub {
        i = parse_one(lines, i, Some(idx), indicators, tree);
    }
    i
}

/// Locate and read the `ltree` section of an L01 set, given any segment path.
///
/// The tree lives in one segment only — normally the last — so every segment is
/// walked until it is found. Segment discovery follows EnCase's extension
/// sequence from the given path.
///
/// The body is returned verbatim; the caller verifies it against
/// [`LtreeHeader::data_md5`]. Verification is deliberately NOT done here:
/// returning bytes the caller has not been given the chance to check would make
/// a damaged tree indistinguishable from a sound one.
///
/// # Errors
/// [`EwfError::Io`] on a read failure; [`EwfError::NotLogicalEvidence`] when no
/// segment carries an `ltree` — which for a valid EVF container means it is a
/// PHYSICAL image, not a damaged one.
pub fn find_ltree(first_segment: &std::path::Path) -> Result<(LtreeHeader, Vec<u8>)> {
    use std::io::{Read as _, Seek as _, SeekFrom};

    let mut searched = 0usize;
    for path in segment_paths(first_segment) {
        searched += 1;
        let mut f = std::fs::File::open(&path)?;
        let mut hdr = [0u8; crate::sections::FILE_HEADER_SIZE];
        if f.read_exact(&mut hdr).is_err() {
            continue;
        }
        // Confirm the container before trusting any offset inside it.
        crate::sections::EwfFileHeader::parse(&hdr)?;

        let mut off = crate::sections::FILE_HEADER_SIZE as u64;
        let mut seen = std::collections::HashSet::new();
        loop {
            if !seen.insert(off) {
                break; // a section list that points at itself is not walkable
            }
            f.seek(SeekFrom::Start(off))?;
            let mut d = [0u8; crate::sections::SECTION_DESCRIPTOR_SIZE];
            if f.read_exact(&mut d).is_err() {
                break;
            }
            let Ok(desc) = crate::sections::SectionDescriptor::parse(&d, off) else {
                break;
            };
            if desc.section_type == "ltree" {
                let payload_len = (desc.section_size as usize)
                    .saturating_sub(crate::sections::SECTION_DESCRIPTOR_SIZE);
                if payload_len < LTREE_HEADER_SIZE {
                    return Err(EwfError::BufferTooShort {
                        expected: LTREE_HEADER_SIZE,
                        got: payload_len,
                    });
                }
                let mut head = vec![0u8; LTREE_HEADER_SIZE];
                f.read_exact(&mut head)?;
                let header = LtreeHeader::parse(&head)?;
                let mut body = vec![0u8; payload_len - LTREE_HEADER_SIZE];
                f.read_exact(&mut body)?;
                // The header's declared size is authoritative; a section padded
                // beyond it must not fold padding into the tree.
                let declared = usize::try_from(header.data_size).unwrap_or(body.len());
                if declared < body.len() {
                    body.truncate(declared);
                }
                return Ok((header, body));
            }
            if desc.next == off || desc.next == 0 || desc.section_type == "done" {
                break;
            }
            off = desc.next;
        }
    }
    // Every segment parsed as a container and none carried an `ltree`. That is
    // not a signature problem: it is what a PHYSICAL image looks like.
    Err(EwfError::NotLogicalEvidence { segments: searched })
}

/// Segment file paths for an EnCase set, derived from any one of them.
///
/// EnCase numbers segments `.L01`, `.L02`, … and the case of the extension
/// follows whatever the acquisition tool wrote, so both are tried.
/// The canonical EnCase segment extension for the 1-based segment `n`.
///
/// EnCase numbers segments `x01`…`x99` and then rolls over to LETTERS:
/// `xAA`, `xAB`, … `xAZ`, `xBA`, … `xZZ`. A reader that stops at 99 does not
/// error — it reports the tail as absent, which on a large acquisition means
/// silently addressing a fraction of the evidence.
///
/// Not hypothetical: a real 478-segment image is `E01`…`E99` then `EAA`…`EOO`,
/// i.e. 99 numeric and **379** alpha segments.
///
/// Returns `None` past `xZZ` (99 + 676 = 775 segments), the end of the scheme.
fn segment_ext(prefix: char, n: u32) -> Option<String> {
    if n == 0 {
        return None;
    }
    if n <= 99 {
        return Some(format!("{prefix}{n:02}"));
    }
    let i = n - 100; // 0-based index into AA..ZZ
    if i >= 26 * 26 {
        return None;
    }
    let hi = u8::try_from(i / 26).ok()? + b'A';
    let lo = u8::try_from(i % 26).ok()? + b'A';
    Some(format!("{prefix}{}{}", hi as char, lo as char))
}

/// Segment file paths for an EnCase set, derived from any one of them.
///
/// Enumeration **stops at the first gap**. A missing middle segment means an
/// incomplete set; jumping the hole and continuing would assemble evidence from
/// a discontiguous run and report nothing wrong.
fn segment_paths(first: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut out = vec![first.to_path_buf()];
    let Some(stem) = first.file_stem() else {
        return out;
    };
    // Follow the case of the input: EnCase writes upper-case extensions, but a
    // copied set may be lower-cased by a case-insensitive transfer.
    let Some(ext0) = first.extension().and_then(|e| e.to_str()) else {
        return out;
    };
    let upper = ext0.chars().next().is_some_and(char::is_uppercase);
    let prefix = if upper { 'L' } else { 'l' };
    let dir = first.parent().unwrap_or(std::path::Path::new("."));
    let stem = stem.to_string_lossy();

    for n in 2..=775u32 {
        let Some(ext) = segment_ext(prefix, n) else {
            break;
        };
        let p = dir.join(format!("{stem}.{ext}"));
        if !p.is_file() {
            break;
        }
        if p != first {
            out.push(p);
        }
    }
    out
}

/// Test-only view of [`segment_paths`].
///
/// The enumeration is a private detail, but it is also where a silent
/// evidence-loss bug lives, so it is reachable from an integration test rather
/// than only exercised through a full image.
#[doc(hidden)]
#[must_use]
pub fn segment_paths_for_test(first: &std::path::Path) -> Vec<std::path::PathBuf> {
    segment_paths(first)
}

/// The authoritative media size for an EWF container.
///
/// For a PHYSICAL image the volume section's total size is the disk size and is
/// trusted; only a zero falls back to the chunk table.
///
/// For a LOGICAL container it is not trusted at all. The specification records
/// that `sector_size x sector_count != total_size` for EWF-L01 and that the real
/// total lives in the ltree section — the volume section carries some other
/// quantity. Observed on a real EnCase 8.08 acquisition: the volume section
/// declares 2,146,959,360 bytes while the chunk table describes 11,940,397,056.
/// Trusting the declaration caps reads at 2 GiB and silently loses ~83% of the
/// evidence, with no error raised — every file past the cap simply becomes
/// unreadable.
///
/// The chunk table spans every segment and describes exactly what was stored, so
/// it is the reliable answer for a logical container.
#[must_use]
pub fn media_size_for(
    kind: crate::sections::EwfKind,
    declared: u64,
    chunks: u64,
    chunk_size: u64,
) -> u64 {
    let from_chunks = chunks.saturating_mul(chunk_size);
    match kind {
        crate::sections::EwfKind::Logical => from_chunks,
        crate::sections::EwfKind::Physical if declared == 0 => from_chunks,
        crate::sections::EwfKind::Physical => declared,
    }
}

/// One extent of a file's data within the media stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Extent {
    /// Offset from the start of the media data.
    pub offset: u64,
    /// Length in bytes.
    pub size: u64,
    /// A sparse extent reads back as zeroes; no media data is stored for it.
    pub sparse: bool,
}

/// Parse a `be` (binary extents) value.
///
/// Format: a count, then per extent an optional type letter (`S` for sparse)
/// followed by a hexadecimal offset and size, all space separated. Offsets are
/// relative to the start of the media data.
///
/// # Errors
/// [`EwfError::MalformedLogicalField`] when the value is not shaped as the format
/// describes.
pub fn parse_binary_extents(value: &str) -> Result<Vec<Extent>> {
    let mut it = value.split_whitespace();
    let count: usize =
        it.next()
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| EwfError::MalformedLogicalField {
                field: "be extent count",
                value: value.to_owned(),
            })?;

    let mut out = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        // A leading non-hex token is a type flag; only "S" (sparse) is defined,
        // and an unknown flag is carried as non-sparse rather than refused --
        // refusing would lose the whole acquisition over one annotation.
        let mut tok = it.next().ok_or_else(|| EwfError::MalformedLogicalField {
            field: "be",
            value: value.to_string(),
        })?;
        let mut sparse = false;
        while tok.eq_ignore_ascii_case("s") || u64::from_str_radix(tok, 16).is_err() {
            sparse |= tok.eq_ignore_ascii_case("s");
            tok = it.next().ok_or_else(|| EwfError::MalformedLogicalField {
                field: "be",
                value: value.to_string(),
            })?;
        }
        let offset = u64::from_str_radix(tok, 16).map_err(|_| EwfError::MalformedLogicalField {
            field: "be offset",
            value: tok.to_string(),
        })?;
        let size_tok = it.next().ok_or_else(|| EwfError::MalformedLogicalField {
            field: "be size",
            value: value.to_string(),
        })?;
        let size =
            u64::from_str_radix(size_tok, 16).map_err(|_| EwfError::MalformedLogicalField {
                field: "be size",
                value: size_tok.to_string(),
            })?;
        out.push(Extent {
            offset,
            size,
            sparse,
        });
    }
    Ok(out)
}

impl LogicalEntry {
    /// The extents locating this entry's data in the media stream.
    ///
    /// # Errors
    /// [`EwfError::MalformedLogicalField`] when the stored `be` value is malformed.
    pub fn extents(&self) -> Result<Vec<Extent>> {
        match self.raw.get("be").map(String::as_str) {
            None | Some("") => Ok(Vec::new()),
            Some(v) => parse_binary_extents(v),
        }
    }
}

impl LogicalTree {
    /// Read one entry's file data from the media stream.
    ///
    /// Sparse extents are zero-filled; stored extents are read at their media
    /// offset. The result is truncated to the entry's logical size, because an
    /// extent is allocated in whole chunks and the tail beyond `ls` is padding
    /// that was never part of the file.
    ///
    /// # Errors
    /// [`EwfError::Io`] on a read failure, or [`EwfError::MalformedLogicalField`]
    /// when the entry's extents are malformed.
    pub fn read_entry<R: std::io::Read + std::io::Seek>(
        &self,
        media: &mut R,
        index: usize,
    ) -> Result<Vec<u8>> {
        // An out-of-range index is a CALLER error, not a damaged image; name
        // both numbers rather than implying the evidence is at fault.
        let entry = self
            .entries
            .get(index)
            .ok_or_else(|| EwfError::MalformedLogicalField {
                field: "entry index",
                value: format!("index {index} of {} entries", self.entries.len()),
            })?;
        if entry.is_dir {
            return Ok(Vec::new());
        }
        // EnCase deduplicates: when identical content appears more than once it
        // is stored ONCE and every later entry carries a 1-byte placeholder
        // extent plus `du`, the duplicate data offset. Reading the placeholder
        // literally returns one byte for a file of any size -- silently, and it
        // looks like success until a hash is checked. Observed here: 25,751 of
        // 64,689 files (40%) are stored this way.
        //
        // `du` is DECIMAL, unlike the hexadecimal offsets in `be`. Established
        // from the evidence rather than assumed: read as decimal every value
        // lands inside the 11.94 GB media, while as hex 25,110 of them point
        // past its end.
        let extents = entry.extents()?;
        let is_placeholder = entry.size > 1 && extents.iter().map(|e| e.size).sum::<u64>() <= 1;
        if is_placeholder {
            if let Some(off) = entry
                .raw
                .get("du")
                .and_then(|v| v.trim().parse::<u64>().ok())
                .filter(|o| *o > 0)
            {
                let want = usize::try_from(entry.size).unwrap_or(usize::MAX);
                let mut buf = vec![0u8; want];
                media.seek(std::io::SeekFrom::Start(off))?;
                media.read_exact(&mut buf)?;
                return Ok(buf);
            }
        }

        let mut out = Vec::new();
        for ex in extents {
            let want = usize::try_from(ex.size).unwrap_or(usize::MAX);
            if ex.sparse {
                // No media data was stored for a sparse extent; it reads as
                // zeroes. Seeking to a media offset for it would return some
                // other file's bytes.
                out.resize(out.len() + want, 0);
                continue;
            }
            media.seek(std::io::SeekFrom::Start(ex.offset))?;
            let start = out.len();
            out.resize(start + want, 0);
            media.read_exact(&mut out[start..])?;
        }
        // `ls` is the file's length; the extent is chunk-aligned and its tail is
        // padding. Returning the padding would change every hash.
        let logical = usize::try_from(entry.size).unwrap_or(usize::MAX);
        if out.len() > logical {
            out.truncate(logical);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a spec-shaped `ltree` header for `data`.
    ///
    /// TIER 3: hand-built from the libyal specification, not minted by EnCase.
    /// It pins our reading of the documented layout and nothing more; the
    /// real-world check is the env-gated test against an actual L01.
    fn ltree_header_for(data: &[u8]) -> Vec<u8> {
        use md5::{Digest as _, Md5};
        let mut h = vec![0u8; LTREE_HEADER_SIZE];
        h[0..16].copy_from_slice(&Md5::digest(data));
        h[16..24].copy_from_slice(&(data.len() as u64).to_le_bytes());
        let sum = crate::sections::adler32(&h);
        h[24..28].copy_from_slice(&sum.to_le_bytes());
        h
    }

    /// RED: the header must yield the integrity values that guard the tree.
    ///
    /// On a logical acquisition the entry tree IS the evidence — there is no
    /// filesystem to fall back on — so a tree that fails its stored MD5 is a
    /// damaged exhibit, not a cosmetic problem. Reading these values is what
    /// makes that detectable at all.
    #[test]
    fn ltree_header_yields_the_integrity_values() {
        let data = b"5\nrec\n";
        let h = ltree_header_for(data);

        let parsed = LtreeHeader::parse(&h).expect("a spec-shaped ltree header must parse");
        assert_eq!(parsed.data_size, data.len() as u64, "data size");
        use md5::{Digest as _, Md5};
        assert_eq!(
            parsed.data_md5,
            <[u8; 16]>::from(Md5::digest(data)),
            "the stored MD5 must be the MD5 of the data"
        );
        assert!(
            parsed.verify_checksum(&h),
            "and the header's own Adler-32 must verify"
        );
    }

    /// RED: a truncated header must be refused, never read past.
    #[test]
    fn ltree_header_refuses_a_short_buffer() {
        assert!(
            LtreeHeader::parse(&[0u8; LTREE_HEADER_SIZE - 1]).is_err(),
            "a short header must be refused rather than over-read"
        );
    }

    /// RED: a corrupted header must fail its checksum. Without this the
    /// verification could be a constant `true` and nothing would notice.
    #[test]
    fn ltree_header_detects_a_corrupted_header() {
        let data = b"5\nrec\n";
        let mut h = ltree_header_for(data);
        h[17] ^= 0xFF; // flip a bit in the declared size
        let parsed = LtreeHeader::parse(&h).expect("still structurally parseable");
        assert!(
            !parsed.verify_checksum(&h),
            "a corrupted header must fail its own checksum"
        );
    }

    /// Build an `entry` category with a caller-chosen indicator ORDER.
    ///
    /// TIER 3, and the order is a parameter on purpose: the format declares its
    /// own field order on the indicator line, so a parser that assumes one is
    /// correct only by luck on the files it was written against.
    fn entry_category(indicators: &[&str], rows: &[(usize, Vec<&str>)]) -> String {
        let mut t = String::from("entry\n");
        t.push_str(&format!("{}\t1\n", rows.len()));
        t.push_str(&indicators.join("\t"));
        t.push('\n');
        t.push_str(&format!("26\t{}\n", rows.len()));
        t.push_str(&vec![""; indicators.len()].join("\t"));
        t.push('\n');
        for (nsub, vals) in rows {
            t.push_str(&format!("0\t{nsub}\n"));
            t.push_str(&vals.join("\t"));
            t.push('\n');
        }
        t
    }

    /// RED: fields must be read by indicator NAME, never by position.
    ///
    /// This is the defect that makes libewf unable to open real EnCase output:
    /// it assumes a layout, and a file declaring a different one is misread.
    /// The two cases below carry identical data in DIFFERENT column orders and
    /// must parse identically — a positional reader passes one and fails the
    /// other.
    #[test]
    fn entries_are_read_by_indicator_name_not_position() {
        let a = entry_category(
            &["p", "n", "id", "ls", "ha"],
            &[(
                0,
                vec![
                    "",
                    "report.doc",
                    "7",
                    "1024",
                    "0123456789abcdef0123456789abcdef",
                ],
            )],
        );
        let b = entry_category(
            &["ha", "ls", "id", "n", "p"],
            &[(
                0,
                vec![
                    "0123456789abcdef0123456789abcdef",
                    "1024",
                    "7",
                    "report.doc",
                    "",
                ],
            )],
        );

        let ta = parse_entry_tree(&a).expect("order A parses");
        let tb = parse_entry_tree(&b).expect("order B parses");

        for (label, t) in [("A", &ta), ("B", &tb)] {
            let e = t
                .entries
                .last()
                .unwrap_or_else(|| panic!("{label}: an entry"));
            assert_eq!(e.name, "report.doc", "{label}: name");
            assert_eq!(e.size, 1024, "{label}: size");
            assert_eq!(e.id, 7, "{label}: id");
            assert!(!e.is_dir, "{label}: a file, not a directory");
            assert_eq!(
                e.md5.as_deref(),
                Some("0123456789abcdef0123456789abcdef"),
                "{label}: md5"
            );
        }
    }

    /// RED: an all-zero MD5 means "not hashed", not "hashed to zero".
    #[test]
    fn an_all_zero_md5_is_reported_as_absent() {
        let t = entry_category(
            &["p", "n", "id", "ls", "ha"],
            &[(
                0,
                vec!["", "x.bin", "1", "5", "00000000000000000000000000000000"],
            )],
        );
        let tree = parse_entry_tree(&t).expect("parses");
        assert_eq!(
            tree.entries.last().expect("entry").md5,
            None,
            "an all-zero hash means the file was not hashed"
        );
    }

    /// RED: sub-entries must nest, because the tree is the evidence's structure.
    #[test]
    fn sub_entries_nest_under_their_parent() {
        let t = entry_category(
            &["p", "n", "id", "ls", "ha"],
            &[
                (1, vec!["1", "Docs", "2", "0", ""]),
                (0, vec!["", "inner.txt", "3", "11", ""]),
            ],
        );
        let tree = parse_entry_tree(&t).expect("parses");
        let dir = tree
            .entries
            .iter()
            .position(|e| e.name == "Docs")
            .expect("the directory");
        assert!(tree.entries[dir].is_dir, "Docs is a directory");
        assert_eq!(tree.entries[dir].children.len(), 1, "one child");
        let child = tree.entries[dir].children[0];
        assert_eq!(tree.entries[child].name, "inner.txt");
        assert_eq!(tree.entries[child].parent, Some(dir));
    }

    /// RED: a field this reader does not model must still be preserved.
    ///
    /// An L01 is the entire evidence. A column dropped because today's struct
    /// has no home for it is evidence destroyed by the reader.
    #[test]
    fn unmodelled_fields_are_preserved_verbatim() {
        let t = entry_category(
            &["p", "n", "id", "ls", "ha", "sha", "spth"],
            &[(0, vec!["", "a", "1", "0", "", "abcd", "\\\\Device\\\\Foo"])],
        );
        let tree = parse_entry_tree(&t).expect("parses");
        let e = tree.entries.last().expect("entry");
        assert_eq!(e.raw.get("sha").map(String::as_str), Some("abcd"));
        assert_eq!(
            e.raw.get("spth").map(String::as_str),
            Some("\\\\Device\\\\Foo")
        );
        assert_eq!(
            tree.indicators.len(),
            7,
            "the declared indicator list is retained"
        );
    }

    /// RED: unpaired surrogates must decode, not abort.
    ///
    /// The specification records that ltree data is not strict UTF-16. A strict
    /// decoder refuses real evidence for a reason unrelated to the evidence, so
    /// lone halves are replaced and each replacement is REPORTED.
    #[test]
    fn lone_surrogates_decode_with_a_count_rather_than_failing() {
        // "A" then a lone high surrogate then "B", UTF-16LE.
        let data: Vec<u8> = vec![0x41, 0x00, 0x00, 0xD8, 0x42, 0x00];
        let (text, replaced) = decode_ltree_text(&data);
        assert!(text.starts_with('A'), "text before the bad unit survives");
        assert!(text.ends_with('B'), "and text after it survives too");
        assert_eq!(replaced, 1, "exactly one unpaired unit was replaced");
    }

    /// RED: a body with no entry category is not a usable tree.
    #[test]
    fn a_body_without_an_entry_category_is_refused() {
        assert!(
            parse_entry_tree("5\nrec\n1\t2\n").is_err(),
            "no entry category means no file tree"
        );
    }

    use crate::sections::EwfKind;

    /// RED: a logical container's declared media size must be ignored.
    ///
    /// This is not a tidy-up. A real EnCase 8.08 L01 declares 2,146,959,360
    /// bytes in its volume section while its chunk table describes
    /// 11,940,397,056. Trusting the declaration caps reads at 2 GiB and loses
    /// ~83% of the evidence with no error raised: the files past the cap simply
    /// cannot be read, which on an exhibit is the worst kind of failure —
    /// silent and total.
    #[test]
    fn a_logical_container_ignores_the_declared_media_size() {
        let declared = 2_146_959_360u64;
        let chunks = 45_549u64;
        let chunk_size = 262_144u64;
        assert_eq!(
            media_size_for(EwfKind::Logical, declared, chunks, chunk_size),
            chunks * chunk_size,
            "the chunk table, not the volume section, describes a logical container"
        );
    }

    /// RED: a physical image must still trust its volume section, so the fix
    /// cannot change how every existing E01 is read.
    #[test]
    fn a_physical_image_still_trusts_its_declared_media_size() {
        assert_eq!(
            media_size_for(EwfKind::Physical, 10_485_760, 320, 32_768),
            10_485_760,
            "an E01's declared disk size is authoritative"
        );
    }

    /// RED: the existing zero-fallback for physical images survives.
    #[test]
    fn a_physical_image_with_no_declared_size_falls_back_to_chunks() {
        assert_eq!(
            media_size_for(EwfKind::Physical, 0, 320, 32_768),
            320 * 32_768
        );
    }

    /// RED: binary extents are hex, and the count must be honoured.
    #[test]
    fn binary_extents_parse_offset_and_size_as_hex() {
        let e = parse_binary_extents("1 1a2b3c 4000").expect("a one-extent value parses");
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].offset, 0x1a_2b3c, "offset is hexadecimal");
        assert_eq!(e[0].size, 0x4000, "and so is size");
        assert!(!e[0].sparse);
    }

    /// RED: multiple extents, because a file need not be contiguous.
    #[test]
    fn binary_extents_parse_multiple_extents() {
        let e = parse_binary_extents("2 0 1000 1000 800").expect("parses");
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].offset, e[0].size), (0, 0x1000));
        assert_eq!((e[1].offset, e[1].size), (0x1000, 0x800));
    }

    /// RED: a sparse extent is marked, not skipped.
    ///
    /// Sparse means no media data was stored and the region reads as zeroes.
    /// Dropping it would shorten the file; treating it as stored data would read
    /// someone else's bytes into this file.
    #[test]
    fn a_sparse_extent_is_flagged_rather_than_dropped() {
        let e = parse_binary_extents("1 S 0 1000").expect("parses");
        assert_eq!(e.len(), 1);
        assert!(e[0].sparse, "S marks a sparse extent");
        assert_eq!(e[0].size, 0x1000, "and it still has a length");
    }

    /// RED: a value whose token count contradicts its declared extent count is
    /// refused rather than half-read.
    #[test]
    fn binary_extents_reject_a_truncated_value() {
        assert!(
            parse_binary_extents("2 0 1000").is_err(),
            "two extents declared but one supplied must be refused"
        );
    }

    /// RED: an empty value means no extents, not an error — a zero-length file
    /// legitimately has none.
    #[test]
    fn binary_extents_accept_an_empty_value_as_no_extents() {
        assert_eq!(parse_binary_extents("0").expect("parses"), vec![]);
    }
}
