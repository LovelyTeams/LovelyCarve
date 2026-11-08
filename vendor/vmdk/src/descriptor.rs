//! Text VMDK descriptor parsing (Virtual Disk Format 1.1 §4.3).

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::error::{Result, VmdkError};

/// Resolve a descriptor-controlled extent/parent filename against `base_dir`,
/// refusing any path that escapes that directory (absolute, root/prefix, or a
/// `..` component). VMDK descriptors reference sibling files by design; a path
/// that climbs out is a crafted-image attempt to read arbitrary host files, so
/// the safe default is to refuse it.
pub(crate) fn resolve_extent_path(base_dir: &Path, filename: &str) -> io::Result<PathBuf> {
    let rel = Path::new(filename);
    let escapes = rel.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if escapes {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refusing descriptor-controlled path that escapes the image directory: {filename}"
            ),
        ));
    }
    Ok(base_dir.join(rel))
}

/// Parsed text VMDK descriptor.
pub(crate) struct TextDescriptor {
    pub create_type: Box<str>,
    /// Content ID (hex field `CID=`); `0xffff_ffff` when absent.
    pub cid: u32,
    /// Parent content ID (hex field `parentCID=`); `0xffff_ffff` when absent or base image.
    pub parent_cid: u32,
    /// Raw descriptor text (NUL bytes stripped; used by `VmdkReader::descriptor_text()`).
    /// `parentFileNameHint` is recovered from this text by the chain reader when needed.
    pub raw_text: Box<str>,
    /// FLAT extents (twoGbMaxExtentFlat, monolithicFlat).
    pub extents: Vec<ExtentEntry>,
    /// SPARSE extents (twoGbMaxExtentSparse) — each is a binary VMDK with its own GD/GT.
    pub sparse_extents: Vec<SparseEntry>,
    /// Sum of all FLAT extent sector counts.
    pub capacity_sectors: u64,
    /// Sum of all SPARSE extent sector counts.
    pub sparse_capacity_sectors: u64,
}

/// A single flat extent entry from the descriptor.
pub(crate) struct ExtentEntry {
    pub size_sectors: u64,
    pub filename: Box<str>,
    /// Byte offset into the extent file where this extent's data begins.
    pub file_byte_offset: u64,
    /// `true` for a `ZERO` extent: no backing file, reads as zeros.
    pub is_zero: bool,
}

/// A single sparse extent entry from the descriptor (twoGbMaxExtentSparse).
///
/// Each extent is an independent binary VMDK file with its own sparse header and GD/GT.
pub(crate) struct SparseEntry {
    pub size_sectors: u64,
    pub filename: Box<str>,
}

/// Parse a VMDK text descriptor, collecting all metadata fields and extent entries.
pub(crate) fn parse_text_descriptor(text: &str) -> Result<TextDescriptor> {
    let mut create_type = Box::from("");
    let mut cid: u32 = 0xffff_ffff;
    let mut parent_cid: u32 = 0xffff_ffff;
    let mut extents = Vec::new();
    let mut sparse_extents = Vec::new();
    let mut capacity_sectors = 0u64;
    let mut sparse_capacity_sectors = 0u64;

    // Strip NUL padding (embedded descriptors are zero-padded to a sector boundary).
    let text_clean = {
        let end = text.find('\0').unwrap_or(text.len());
        &text[..end]
    };

    for line in text_clean.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim();
            match key.trim() {
                "createType" => {
                    create_type = Box::from(value.trim_matches('"'));
                    continue;
                }
                "CID" | "parentCID" => {
                    let parsed = u32::from_str_radix(value, 16)
                        .map_err(|_| VmdkError::MalformedDescriptor("invalid content ID"))?;
                    if key.trim() == "CID" {
                        cid = parsed;
                    } else {
                        parent_cid = parsed;
                    }
                    continue;
                }
                _ => {}
            }
        }
        if let Some(ext) = try_parse_flat_extent(line) {
            capacity_sectors = capacity_sectors
                .checked_add(ext.size_sectors)
                .ok_or(VmdkError::GeometryOverflow { field: "capacity" })?;
            extents.push(ext);
            continue;
        }
        if let Some(ext) = try_parse_sparse_extent(line) {
            sparse_capacity_sectors = sparse_capacity_sectors
                .checked_add(ext.size_sectors)
                .ok_or(VmdkError::GeometryOverflow { field: "capacity" })?;
            sparse_extents.push(ext);
            continue;
        }
        if matches!(
            line.split_whitespace().next(),
            Some("RW" | "RDONLY" | "NOACCESS")
        ) {
            return Err(VmdkError::MalformedDescriptor(
                "invalid or unsupported extent",
            ));
        }
    }

    Ok(TextDescriptor {
        create_type,
        cid,
        parent_cid,
        raw_text: Box::from(text_clean),
        extents,
        sparse_extents,
        capacity_sectors,
        sparse_capacity_sectors,
    })
}

/// Parse a flat-style extent line:
///   `RW <n> FLAT "<file>" <sector_offset>` — preallocated raw extent
///   `RW <n> VMFS "<file>"`                 — `ESXi` flat extent (offset implied 0)
///   `RW <n> ZERO`                          — backing-file-less zero-filled extent
///
/// Returns `None` for sparse/other types, blank lines, comments, or malformed input.
fn try_parse_flat_extent(line: &str) -> Option<ExtentEntry> {
    let mut rest = line;

    // Access token: RW | RDONLY | NOACCESS. NOACCESS marks an inaccessible hole,
    // which we treat like ZERO (reads as zeros) so the virtual geometry is preserved.
    let (access, tail) = split_token(rest)?;
    if !matches!(access, "RW" | "RDONLY" | "NOACCESS") {
        return None;
    }
    rest = tail;

    // Sector count
    let (sectors_str, tail) = split_token(rest)?;
    let size_sectors: u64 = sectors_str.parse().ok()?;
    rest = tail;

    // Extent type — FLAT/VMFS/VMFSRAW/VMFSRDM are file-backed. VMFSRAW maps a raw
    // LUN; VMFSRDM is a Raw Device Mapping pointing at a mapped physical device.
    // ZERO has no file.
    let (ext_type, tail) = split_token(rest)?;
    if !matches!(ext_type, "FLAT" | "VMFS" | "VMFSRAW" | "VMFSRDM" | "ZERO") {
        return None;
    }
    rest = tail.trim_start();

    // ZERO (and NOACCESS) extents carry no filename or offset.
    if ext_type == "ZERO" || access == "NOACCESS" {
        return Some(ExtentEntry {
            size_sectors,
            filename: Box::from(""),
            file_byte_offset: 0,
            is_zero: true,
        });
    }

    // Filename: bare word or double-quoted (may contain spaces)
    let (filename, remaining) = if let Some(inner) = rest.strip_prefix('"') {
        let close = inner.find('"')?;
        (&inner[..close], &inner[close + 1..])
    } else {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        (&rest[..end], &rest[end..])
    };

    let offset_token = remaining.split('#').next()?.split_whitespace().next();
    let file_sector_offset: u64 = match offset_token {
        Some(token) => token.parse().ok()?,
        None => 0,
    };

    Some(ExtentEntry {
        size_sectors,
        filename: Box::from(filename),
        file_byte_offset: file_sector_offset.checked_mul(512)?,
        is_zero: false,
    })
}

/// Parse `<access> <n> SPARSE "<file>"` lines.
///
/// Returns `None` for non-SPARSE types or malformed input.
fn try_parse_sparse_extent(line: &str) -> Option<SparseEntry> {
    let mut rest = line;

    let (access, tail) = split_token(rest)?;
    if !matches!(access, "RW" | "RDONLY") {
        return None;
    }
    rest = tail;

    let (sectors_str, tail) = split_token(rest)?;
    let size_sectors: u64 = sectors_str.parse().ok()?;
    rest = tail;

    let (ext_type, tail) = split_token(rest)?;
    // SPARSE = standard VMDK4 sparse; VMFSSPARSE = COWD-based ESXi sparse;
    // SESPARSE = vSphere 6.5+ space-efficient sparse. All resolve to a binary
    // extent whose own magic selects the reader.
    if !matches!(ext_type, "SPARSE" | "VMFSSPARSE" | "SESPARSE") {
        return None;
    }
    rest = tail.trim_start();

    let filename = if let Some(inner) = rest.strip_prefix('"') {
        let close = inner.find('"')?;
        &inner[..close]
    } else {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        &rest[..end]
    };

    Some(SparseEntry {
        size_sectors,
        filename: Box::from(filename),
    })
}

/// Split leading non-whitespace token from `s`; return `(token, rest_trimmed)`.
fn split_token(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    if s.is_empty() {
        return None;
    }
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    Some((&s[..end], &s[end..]))
}

/// Decode raw descriptor bytes to text, honoring a declared `ddb.encoding`.
///
/// VMDK descriptors may be written in a non-UTF-8 encoding (the spec lists
/// `windows-1252`, `Shift_JIS`, `GBK`, `Big5`). The structural keywords are ASCII,
/// so the declared encoding is recoverable from the raw bytes; the *values*
/// (filenames, parent hints) may be non-ASCII. Decoding never silently empties:
/// an undecodable byte becomes U+FFFD, not a dropped descriptor.
pub(crate) fn decode_descriptor(bytes: &[u8]) -> String {
    // Embedded descriptors are NUL-padded to a sector boundary.
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let bytes = &bytes[..end];

    // The `ddb.encoding` declaration is itself ASCII, so it survives a lossy pass
    // and can be read even when the body bytes are not valid UTF-8.
    let label = declared_encoding(&String::from_utf8_lossy(bytes));
    decode_bytes(bytes, label.as_deref())
}

/// Default decoder (no `full-encoding` feature): UTF-8 + windows-1252, dependency-free.
/// Any other declared encoding degrades to lossy UTF-8 (U+FFFD) rather than dropping
/// the descriptor — fail visible, not silent. Enable `full-encoding` for the full set
/// (`Shift_JIS` / `GBK` / `Big5` / …) via `encoding_rs`.
#[cfg(not(feature = "full-encoding"))]
fn decode_bytes(bytes: &[u8], label: Option<&str>) -> String {
    if label.is_some_and(|e| e.eq_ignore_ascii_case("windows-1252")) {
        return decode_windows_1252(bytes);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// Full decoder (`full-encoding` feature): decode per the declared `ddb.encoding`
/// using `encoding_rs`, covering the WHATWG label set (`windows-1252`,
/// `Shift_JIS`, `GBK`, `Big5`, `UTF-8`, …). An unrecognized/absent label defaults
/// to UTF-8; undecodable bytes become U+FFFD (never a dropped descriptor). The
/// declared encoding is honored verbatim — no BOM sniffing overrides it.
#[cfg(feature = "full-encoding")]
fn decode_bytes(bytes: &[u8], label: Option<&str>) -> String {
    let encoding = label
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    let (decoded, _had_errors) = encoding.decode_without_bom_handling(bytes);
    decoded.into_owned()
}

/// The value of a `…encoding … = "<label>"` line, if the descriptor declares one.
fn declared_encoding(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        if !line.to_ascii_lowercase().contains("encoding") {
            return None;
        }
        let (_, rhs) = line.split_once('=')?;
        let value = rhs.trim().trim_matches('"').trim();
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Decode windows-1252 (CP-1252) bytes. 0x00–0x7F are ASCII and 0xA0–0xFF map to
/// U+00A0–U+00FF (Latin-1); 0x80–0x9F carry the CP-1252-specific punctuation
/// (undefined slots map to their C1 control code point, matching the WHATWG table).
#[cfg(not(feature = "full-encoding"))]
fn decode_windows_1252(bytes: &[u8]) -> String {
    const C1: [char; 32] = [
        '\u{20AC}', '\u{0081}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}',
        '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{008D}',
        '\u{017D}', '\u{008F}', '\u{0090}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}',
        '\u{2022}', '\u{2013}', '\u{2014}', '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}',
        '\u{0153}', '\u{009D}', '\u{017E}', '\u{0178}',
    ];
    bytes
        .iter()
        .map(|&b| match b {
            0x80..=0x9F => C1[(b - 0x80) as usize],
            _ => b as char,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_windows_1252_descriptor_per_ddb_encoding() {
        // 0xE9 = é and 0x80 = € in windows-1252. 0x80 is the discriminator: in
        // windows-1252 it is the Euro sign (U+20AC), in Latin-1 a control char —
        // so this proves a real cp1252 decode, not a naive byte cast.
        let bytes = b"createType=\"monolithicFlat\"\nddb.encoding = \"windows-1252\"\nparentFileNameHint=\"caf\xE9\x80.vmdk\"\n";
        let s = decode_descriptor(bytes);
        assert!(s.contains('\u{00E9}'), "0xE9 -> é: {s:?}");
        assert!(
            s.contains('\u{20AC}'),
            "0x80 -> € (cp1252, not Latin-1): {s:?}"
        );
        assert!(
            s.contains("monolithicFlat"),
            "ASCII structure preserved: {s:?}"
        );
    }

    #[test]
    fn decode_invalid_utf8_without_hint_is_lossy_not_empty() {
        // No/unknown encoding declared + invalid UTF-8 → never silently empty;
        // the ASCII structure survives and the bad byte becomes U+FFFD.
        let bytes = b"createType=\"monolithicSparse\"\nname=\xE9 raw\n";
        let s = decode_descriptor(bytes);
        assert!(!s.is_empty(), "must never silently empty the descriptor");
        assert!(
            s.contains("monolithicSparse"),
            "ASCII structure kept: {s:?}"
        );
        assert!(s.contains('\u{FFFD}'), "undecodable byte -> U+FFFD: {s:?}");
    }

    #[test]
    fn decode_valid_utf8_unchanged() {
        let s = decode_descriptor("createType=\"x\"\nlabel=café\n".as_bytes());
        assert!(s.contains("café"), "valid UTF-8 round-trips: {s:?}");
    }

    #[cfg(feature = "full-encoding")]
    #[test]
    fn decode_shift_jis_with_full_encoding_feature() {
        // Shift_JIS bytes 0x82 0xA0 = あ (U+3042). Multibyte — only the
        // `full-encoding` (encoding_rs) path can decode it; the default build
        // degrades to lossy. Guarded so it only runs when the feature is enabled.
        let bytes = b"createType=\"monolithicFlat\"\nddb.encoding = \"shift_jis\"\nname=\x82\xA0\n";
        let s = decode_descriptor(bytes);
        assert!(s.contains('\u{3042}'), "Shift_JIS あ decoded: {s:?}");
    }

    #[test]
    fn parse_two_gb_max_extent_flat_descriptor() {
        let text = r#"# Disk DescriptorFile
version=1
CID=49bcfa17
parentCID=ffffffff
createType="twoGbMaxExtentFlat"

# Extent description
RW 2048 FLAT "flat-f001.vmdk" 0
"#;
        let d = parse_text_descriptor(text).expect("parse");
        assert_eq!(d.create_type.as_ref(), "twoGbMaxExtentFlat");
        assert_eq!(d.capacity_sectors, 2048);
        assert_eq!(d.extents.len(), 1);
        assert_eq!(d.extents[0].filename.as_ref(), "flat-f001.vmdk");
        assert_eq!(d.extents[0].size_sectors, 2048);
        assert_eq!(d.extents[0].file_byte_offset, 0);
    }

    #[test]
    fn parse_sparse_extents() {
        let text = "createType=\"twoGbMaxExtentSparse\"\nRW 8192 SPARSE \"disk-s001.vmdk\"\nRW 8192 SPARSE \"disk-s002.vmdk\"\n";
        let d = parse_text_descriptor(text).expect("parse");
        assert_eq!(d.extents.len(), 0);
        assert_eq!(d.sparse_extents.len(), 2);
        assert_eq!(d.sparse_extents[0].filename.as_ref(), "disk-s001.vmdk");
        assert_eq!(d.sparse_extents[0].size_sectors, 8192);
        assert_eq!(d.sparse_capacity_sectors, 16384);
    }

    #[test]
    fn parse_quoted_filename_with_spaces() {
        let text = "createType=\"twoGbMaxExtentFlat\"\nRW 512 FLAT \"my disk.vmdk\" 0\n";
        let d = parse_text_descriptor(text).expect("parse");
        assert_eq!(d.extents[0].filename.as_ref(), "my disk.vmdk");
    }

    #[test]
    fn parse_nonzero_sector_offset() {
        let text = "createType=\"twoGbMaxExtentFlat\"\nRW 512 FLAT \"ext.vmdk\" 4096\n";
        let d = parse_text_descriptor(text).expect("parse");
        assert_eq!(d.extents[0].file_byte_offset, 4096 * 512);
    }

    #[test]
    fn parse_flat_sector_offset_no_multiply_overflow() {
        // Attacker-controlled sector offset near u64::MAX must fail, not panic
        // under overflow-checks (fuzz_recover crash-c237a5d8: `* 512` overflowed).
        let text = "createType=\"monolithicFlat\"\nRW 512 FLAT \"ext.vmdk\" 18446744073709551615\n";
        assert!(parse_text_descriptor(text).is_err());
    }

    #[test]
    fn parse_flat_bare_word_filename() {
        // Unquoted (bare-word) filename — exercises the non-quoted branch.
        let text = "createType=\"monolithicFlat\"\nRW 2048 FLAT bare-f001.vmdk 0\n";
        let d = parse_text_descriptor(text).expect("parse");
        assert_eq!(d.extents.len(), 1);
        assert_eq!(d.extents[0].filename.as_ref(), "bare-f001.vmdk");
    }

    #[test]
    fn parse_sparse_bare_word_filename() {
        let text = "createType=\"twoGbMaxExtentSparse\"\nRW 2048 SPARSE bare-s001.vmdk\n";
        let d = parse_text_descriptor(text).expect("parse");
        assert_eq!(d.sparse_extents.len(), 1);
        assert_eq!(d.sparse_extents[0].filename.as_ref(), "bare-s001.vmdk");
    }

    #[test]
    fn parse_vmfsrdm_raw_device_mapping_extent() {
        // A Raw Device Mapping (RDM) disk points a VMFSRDM extent at the mapped
        // physical LUN (here a `vml.*` device identifier). libvmdk recognises
        // VMFSRDM as a first-class extent type; dropping it loses the record of
        // which physical device the VM had passthrough access to.
        let text = "createType=\"vmfsRawDeviceMap\"\nRW 20971520 VMFSRDM \"vml.02000000006006016015301d00\"\n";
        let d = parse_text_descriptor(text).expect("parse");
        assert_eq!(d.create_type.as_ref(), "vmfsRawDeviceMap");
        assert_eq!(d.extents.len(), 1);
        assert_eq!(
            d.extents[0].filename.as_ref(),
            "vml.02000000006006016015301d00"
        );
        assert_eq!(d.extents[0].size_sectors, 20_971_520);
        assert!(!d.extents[0].is_zero);
        assert_eq!(d.capacity_sectors, 20_971_520);
    }

    #[test]
    fn malformed_extent_lines_are_rejected() {
        // Do not silently drop extents and shift virtual disk offsets.
        // "RW" alone exercises split_token's empty-remainder branch.
        let text =
            "createType=\"custom\"\nRW 100 BOGUS \"x.vmdk\"\nRDONLY notanumber FLAT \"y\"\nRW\n";
        assert!(parse_text_descriptor(text).is_err());
    }

    #[test]
    fn sparse_extent_capacity_overflow_is_rejected() {
        // Two near-u64::MAX SPARSE extents overflow the running capacity sum.
        let big = u64::MAX;
        let text = format!(
            "createType=\"twoGbMaxExtentSparse\"\nRW {big} SPARSE \"a\"\nRW {big} SPARSE \"b\"\n"
        );
        assert!(matches!(
            parse_text_descriptor(&text),
            Err(VmdkError::GeometryOverflow { field: "capacity" })
        ));
    }

    #[test]
    fn flat_extent_capacity_overflow_is_rejected() {
        let big = u64::MAX;
        let text = format!(
            "createType=\"twoGbMaxExtentFlat\"\nRW {big} FLAT \"a\" 0\nRW {big} FLAT \"b\" 0\n"
        );
        assert!(matches!(
            parse_text_descriptor(&text),
            Err(VmdkError::GeometryOverflow { field: "capacity" })
        ));
    }
}
