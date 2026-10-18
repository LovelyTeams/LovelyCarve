//! Parse an in-memory `.zip` archive's central directory into a structured
//! preview report, and decompress a single member on demand.
//!
//! This is the "原类型视图" backend for a ZIP file that lives *inside* a loaded
//! disk image (e.g. a `.zip` stored on an NTFS volume inside a reconstructed
//! RAID). Unlike [`crate::core::evidence::readers::zip_image::ZipImage`] — which mmaps a standalone
//! `.zip` to mount it as a logical evidence source — here the archive has no
//! on-disk path: the caller has already materialised its bytes (bounded by a
//! read cap, since the central directory sits at the *end* of the file and the
//! file-node read primitives have no ranged/seek read). We therefore parse a
//! borrowed `&[u8]` through a `Cursor`.
//!
//! Listing uses `by_index_raw` so it never decompresses and so encrypted /
//! unsupported-method members are surfaced (visible but non-readable) rather
//! than dropped — mirroring `ZipImage::scan_zip`. Member content is only
//! decompressed by [`read_member`], on explicit user click, bounded by
//! `max_bytes` so a preview never inflates a multi-GiB member.

use std::io::{Cursor, Read};

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use zip::{CompressionMethod, ZipArchive};

use crate::core::evidence::readers::zip_image::{normalize_key, normalize_zip_path, zip_dt_unix};

/// Cap on members walked for aggregate stats. The 128 MiB read cap already
/// bounds the central-directory size (and thus member count), but a pathological
/// archive can still pack millions of tiny entries; stop here and flag it.
const MAX_SCAN: usize = 200_000;

/// Cap on members returned in the detail list. Aggregate stats cover every
/// scanned member; only the per-entry list truncates (the UI tree stays
/// responsive). Far above any real evidence archive's useful display size.
const MAX_LIST: usize = 50_000;

/// Symlink targets are tiny path strings; cap the body read that recovers them.
const MAX_SYMLINK_TARGET: u64 = 4096;

/// One member of the archive, as surfaced to the tree view. Kinds mirror
/// [`crate::core::evidence::readers::zip_image::ZipEntryKind`]: `other` = encrypted or
/// unsupported-compression (visible but not previewable).
#[derive(Debug, Clone, Serialize)]
pub struct ZipPreviewEntry {
    /// Normalised archive path, `/`-separated, no leading/trailing slash.
    pub path: String,
    /// Final path segment (display name).
    pub name: String,
    /// `"file"` / `"dir"` / `"symlink"` / `"other"`.
    pub kind: String,
    /// Uncompressed (logical) size; 0 for directories.
    pub size: u64,
    /// Stored (compressed) size; 0 for directories.
    pub compressed_size: u64,
    /// Compression-method label, e.g. `"Stored"` / `"Deflate"` / `"Bzip2"`.
    pub method: String,
    /// Member uses traditional/AES encryption (not previewable).
    pub encrypted: bool,
    /// CRC-32 of the uncompressed content as 8 lowercase hex digits; `None` for
    /// directories.
    pub crc32: Option<String>,
    /// Modification time as an ISO-8601 UTC string, or `None` when the header
    /// omits it / the MS-DOS timestamp is out of range.
    pub modified: Option<String>,
    /// For symlink members, the recovered target path; `None` otherwise.
    pub link_target: Option<String>,
}

/// Whole-archive preview: every member plus rolled-up statistics.
#[derive(Debug, Clone, Serialize)]
pub struct ZipPreview {
    pub entries: Vec<ZipPreviewEntry>,
    /// Total real members scanned (files + dirs + symlinks + non-readable),
    /// which may exceed `entries.len()` when the detail list truncates.
    pub entry_count: usize,
    /// Readable regular-file members.
    pub file_count: usize,
    /// Explicit directory members (the tree synthesises missing ancestors).
    pub dir_count: usize,
    /// Summed uncompressed size over every non-directory member.
    pub total_uncompressed: u64,
    /// Summed compressed size over every non-directory member.
    pub total_compressed: u64,
    /// Members flagged encrypted.
    pub encrypted_count: u64,
    /// Archive-level comment (lossy-decoded), if non-empty.
    pub comment: Option<String>,
    /// Detail list capped at `MAX_LIST` (or scan capped at `MAX_SCAN`).
    pub entries_truncated: bool,
    /// The caller only had a prefix of the file (file exceeded the read cap);
    /// since the central directory is at the end, a successful parse here means
    /// the prefix happened to contain it — surfaced for transparency.
    pub truncated_bytes: bool,
    pub warnings: Vec<String>,
}

/// Parse the central directory of an in-memory archive. Returns an error (so the
/// command can give a targeted message) when the bytes are not a usable ZIP —
/// including the common case where `truncated_bytes` chopped off the trailing
/// end-of-central-directory record.
pub fn parse(data: &[u8], _real_size: u64, truncated_bytes: bool) -> Result<ZipPreview> {
    let mut archive = ZipArchive::new(Cursor::new(data)).context("read zip central directory")?;

    let mut warnings: Vec<String> = Vec::new();

    // Archive-level comment is raw bytes; decode lossily, keep only if non-empty.
    let comment = {
        let raw = archive.comment();
        if raw.is_empty() {
            None
        } else {
            Some(String::from_utf8_lossy(raw).into_owned())
        }
    };

    let count = archive.len();
    let scan = count.min(MAX_SCAN);
    if count > MAX_SCAN {
        warnings.push(format!(
            "压缩包成员数 {count} 超过扫描上限 {MAX_SCAN}，仅统计/列出前 {MAX_SCAN} 项"
        ));
    }

    let mut entries: Vec<ZipPreviewEntry> = Vec::new();
    let mut entry_count = 0usize;
    let mut file_count = 0usize;
    let mut dir_count = 0usize;
    let mut total_uncompressed = 0u64;
    let mut total_compressed = 0u64;
    let mut encrypted_count = 0u64;
    let mut unreadable_count = 0usize;

    for index in 0..scan {
        // `by_index_raw` reads metadata without a decompressor / password check,
        // so it succeeds for encrypted members and unsupported methods — the
        // very members that must stay visible-but-non-readable.
        let file = match archive.by_index_raw(index) {
            Ok(file) => file,
            Err(err) => {
                tracing::warn!("zip preview: skipping unreadable entry {index}: {err:#}");
                continue;
            }
        };
        // `enclosed_name` is the traversal-safe accessor (None for `..` / absolute).
        let Some(name) = file.enclosed_name() else {
            continue;
        };
        let raw = name.to_string_lossy().replace('\\', "/");
        let Some(rel) = normalize_zip_path(&raw) else {
            continue;
        };

        let is_dir = file.is_dir();
        let is_symlink = file.is_symlink();
        let method = file.compression();
        let encrypted = file.encrypted();
        let size = file.size();
        let compressed_size = file.compressed_size();
        let crc = file.crc32();
        let mtime_dt = file.last_modified();
        drop(file);

        // Only STORED + DEFLATE are compiled in; anything else (or encrypted) is
        // surfaced as a non-readable `other` member, never silently dropped.
        let supported = !encrypted
            && matches!(
                method,
                CompressionMethod::Stored | CompressionMethod::Deflated
            );
        let kind = if is_dir {
            "dir"
        } else if is_symlink {
            "symlink"
        } else if supported {
            "file"
        } else {
            "other"
        };

        entry_count += 1;
        if is_dir {
            dir_count += 1;
        } else {
            total_uncompressed = total_uncompressed.saturating_add(size);
            total_compressed = total_compressed.saturating_add(compressed_size);
            if kind == "file" {
                file_count += 1;
            }
        }
        if encrypted {
            encrypted_count += 1;
        }
        if kind == "other" {
            unreadable_count += 1;
        }

        if entries.len() < MAX_LIST {
            // Symlink targets live in the (small) member body — read through the
            // decompressing reader (raw bytes would be compressed for a DEFLATE
            // link). Gated on the list cap so a pathological archive with tens of
            // thousands of symlinks past MAX_LIST never pays the decompress cost
            // for entries the UI will not show.
            let link_target = if is_symlink {
                archive.by_index(index).ok().and_then(|mut reader| {
                    let mut buf = Vec::new();
                    let read = Read::take(&mut reader, MAX_SYMLINK_TARGET).read_to_end(&mut buf);
                    (read.is_ok() && !buf.is_empty())
                        .then(|| String::from_utf8_lossy(&buf).replace('\\', "/"))
                })
            } else {
                None
            };
            let display_name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            let modified = mtime_dt
                .map(zip_dt_unix)
                .filter(|&secs| secs > 0)
                .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
                .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string());
            entries.push(ZipPreviewEntry {
                path: rel,
                name: display_name,
                kind: kind.to_string(),
                size: if is_dir { 0 } else { size },
                compressed_size: if is_dir { 0 } else { compressed_size },
                method: method_label(method),
                encrypted,
                crc32: if is_dir {
                    None
                } else {
                    Some(format!("{crc:08x}"))
                },
                modified,
                link_target,
            });
        }
    }

    if unreadable_count > 0 {
        warnings.push(format!(
            "{unreadable_count} 个成员不可预览（加密或未编译支持的压缩方法），仅可在十六进制视图查看原始字节"
        ));
    }

    let entries_truncated = entry_count > entries.len();
    if entries_truncated && count <= MAX_SCAN {
        warnings.push(format!(
            "成员条目较多，仅列出前 {MAX_LIST} 项（统计数据按全部成员计算）"
        ));
    }
    if truncated_bytes {
        warnings.push(
            "文件超过结构解析读取上限，仅读取了前缀字节；若结构看似完整则中央目录恰好落在已读范围内".into(),
        );
    }

    Ok(ZipPreview {
        entries,
        entry_count,
        file_count,
        dir_count,
        total_uncompressed,
        total_compressed,
        encrypted_count,
        comment,
        entries_truncated,
        truncated_bytes,
        warnings,
    })
}

/// Decompress a single regular-file member up to `max_bytes`. The `member_path`
/// is the normalised path the preview surfaced; we re-normalise it and match it
/// against each member's traversal-safe `enclosed_name`. Errors (rather than
/// returning garbage) for directories, symlinks, encrypted members, and members
/// using an uncompiled compression method.
pub fn read_member(data: &[u8], member_path: &str, max_bytes: usize) -> Result<Vec<u8>> {
    let want = normalize_key(member_path);
    if want.is_empty() {
        bail!("空的成员路径");
    }
    let mut archive = ZipArchive::new(Cursor::new(data)).context("read zip central directory")?;

    // Find the matching index first (the raw borrow must end before by_index).
    let mut target: Option<usize> = None;
    let count = archive.len();
    for index in 0..count {
        let file = match archive.by_index_raw(index) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let Some(name) = file.enclosed_name() else {
            continue;
        };
        let raw = name.to_string_lossy().replace('\\', "/");
        if normalize_zip_path(&raw).as_deref() != Some(want.as_str()) {
            continue;
        }
        if file.is_dir() {
            bail!("该成员是目录，无法预览内容：{want}");
        }
        if file.is_symlink() {
            bail!("该成员是符号链接，仅显示其目标，不读取内容：{want}");
        }
        if file.encrypted() {
            bail!("该成员已加密，无法解压预览：{want}");
        }
        if !matches!(
            file.compression(),
            CompressionMethod::Stored | CompressionMethod::Deflated
        ) {
            bail!("该成员使用了未编译支持的压缩方法，无法解压预览：{want}");
        }
        target = Some(index);
        break;
    }

    let index = target.ok_or_else(|| anyhow!("压缩包内未找到成员：{want}"))?;
    let mut reader = archive.by_index(index)?;
    // `take` bounds decompression to the requested window: a preview never
    // inflates the whole member.
    let mut buf = Vec::new();
    Read::take(&mut reader, max_bytes as u64)
        .read_to_end(&mut buf)
        .context("decompress zip member")?;
    Ok(buf)
}

/// Human label for a compression method. `CompressionMethod` is `#[non_exhaustive]`,
/// so the two we compile in are named and everything else falls back to its Debug
/// form (e.g. `Bzip2`, `Zstd`, `Unsupported(20)`).
fn method_label(m: CompressionMethod) -> String {
    match m {
        CompressionMethod::Stored => "Stored".to_string(),
        CompressionMethod::Deflated => "Deflate".to_string(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    /// Build a zip into an in-memory buffer from `(path, data, deflate)` triples.
    /// A trailing `/` in `path` marks an explicit directory member.
    fn build_zip(members: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut cursor);
            for (path, data, deflate) in members {
                let opts: SimpleFileOptions =
                    SimpleFileOptions::default().compression_method(if *deflate {
                        CompressionMethod::Deflated
                    } else {
                        CompressionMethod::Stored
                    });
                if path.ends_with('/') {
                    writer.add_directory(*path, opts).expect("add_directory");
                } else {
                    writer.start_file(*path, opts).expect("start_file");
                    writer.write_all(data).expect("write member");
                }
            }
            writer.finish().expect("finish zip");
        }
        cursor.into_inner()
    }

    #[test]
    fn lists_stored_and_deflate_members() {
        let data = build_zip(&[
            ("a.txt", b"hello", false),
            ("sub/", b"", false),
            ("sub/b.bin", b"abcdef", true),
        ]);
        let report = parse(&data, data.len() as u64, false).expect("parse");
        assert_eq!(report.file_count, 2);
        assert_eq!(report.dir_count, 1);
        assert_eq!(report.total_uncompressed, 5 + 6);
        assert!(!report.entries_truncated);

        let a = report
            .entries
            .iter()
            .find(|e| e.path == "a.txt")
            .expect("a.txt");
        assert_eq!(a.kind, "file");
        assert_eq!(a.method, "Stored");
        assert_eq!(a.size, 5);
        assert!(a.crc32.is_some());
    }

    #[test]
    fn reads_member_bounded() {
        let big = vec![b'x'; 1 << 16];
        let data = build_zip(&[("big.txt", &big, true)]);
        let head = read_member(&data, "big.txt", 16).expect("bounded read");
        assert_eq!(head, vec![b'x'; 16]);
        let full = read_member(&data, "big.txt", usize::MAX).expect("full read");
        assert_eq!(full.len(), big.len());
    }

    #[test]
    fn read_member_rejects_directory_and_missing() {
        let data = build_zip(&[("dir/", b"", false), ("dir/f.txt", b"hi", false)]);
        assert!(read_member(&data, "dir", usize::MAX).is_err());
        assert!(read_member(&data, "nope.txt", usize::MAX).is_err());
        assert_eq!(read_member(&data, "dir/f.txt", usize::MAX).unwrap(), b"hi");
    }

    #[test]
    fn truncated_tail_fails_to_parse() {
        let data = build_zip(&[("a.txt", b"hello", true)]);
        // Lop off the trailing end-of-central-directory record.
        let prefix = &data[..data.len() / 2];
        assert!(parse(prefix, data.len() as u64, true).is_err());
    }

    #[test]
    fn surfaces_archive_comment() {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut cursor);
            writer.set_comment("evidence bundle").unwrap();
            let opts = SimpleFileOptions::default();
            writer.start_file("a.txt", opts).expect("start");
            writer.write_all(b"x").expect("write");
            writer.finish().expect("finish");
        }
        let data = cursor.into_inner();
        let report = parse(&data, data.len() as u64, false).expect("parse");
        assert_eq!(report.comment.as_deref(), Some("evidence bundle"));
    }
}
