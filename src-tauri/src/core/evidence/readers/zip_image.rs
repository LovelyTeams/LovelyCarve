//! Read-only loader that exposes a `.zip` archive as a logical file tree.
//! Mirrors [`crate::core::evidence::readers::tar_image::TarImage`] in shape and intent — a forensic
//! ZIP (KAPE triage output, a logical extraction, a collected-evidence bundle)
//! is treated as a merged read-only filesystem rather than a block device.
//!
//! The crucial difference from a tar is **compression**. A tar stores its
//! members uncompressed and contiguous, so `TarImage` serves every read as a
//! zero-copy slice into the outer mmap. ZIP members are usually DEFLATE-
//! compressed, so a read has to decompress. We keep two cheap fast paths:
//!   * STORED (method 0) members ARE contiguous uncompressed bytes inside the
//!     archive, so they read as a zero-copy `outer` slice and surface a real
//!     `data_location` the hex view can jump to;
//!   * DEFLATE members decompress on demand through the `zip` crate, bounded by
//!     the caller's `max_bytes` so a 4 KiB preview never inflates a 1 GiB member.
//!
//! Robustness measures match `TarImage`:
//!   * implicit ancestor directories are synthesised, so an archive that only
//!     stores `a/b/c.txt` still renders a tree;
//!   * `enclosed_name()` (the `zip` crate's traversal-safe accessor) rejects
//!     `..` / absolute paths, so a hostile archive cannot punch out of its root;
//!   * symlinks keep their target (read from the member body, Unix-style) and
//!     resolve one hop on read; encrypted members and members compressed with a
//!     method we did not build support for (only STORED + DEFLATE are enabled —
//!     see `Cargo.toml`) are surfaced as visible but NON-readable nodes rather
//!     than silently dropped or exposed as garbage.
//!
//! A `.zip`-named file that is not a valid ZIP yields a `ZipArchive::new` error
//! and [`ZipImage::open`] bails, letting `core::evidence::image` fall back to its RAW probe.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use memmap2::Mmap;
use parking_lot::Mutex;
use zip::{CompressionMethod, ZipArchive};

use crate::core::evidence::readers::tar_image::TarImage;

mod bak;

/// Soft cap on recorded members — keeps a pathological / corrupt archive from
/// exhausting memory while staying far above any real evidence ZIP. Matches the
/// tar loader's bound.
const MAX_ENTRIES: usize = 2_000_000;

/// Symlink targets are tiny path strings; cap the body read that recovers them
/// so a member mislabelled as a symlink can't pull a large blob into memory.
const MAX_SYMLINK_TARGET: u64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZipEntryKind {
    File,
    Dir,
    Symlink,
    /// Encrypted, unsupported-compression, or otherwise non-readable member.
    /// Surfaced for completeness (the name / size still inform the tree) but
    /// has no readable byte content.
    Other,
}

#[derive(Debug, Clone)]
pub struct ZipEntry {
    /// Normalised archive path, `/`-separated, no leading/trailing slash. The
    /// synthetic root member uses the empty string.
    pub rel_path: String,
    pub kind: ZipEntryKind,
    /// Uncompressed (logical) size. 0 for directories.
    pub size: u64,
    pub mode: u32,
    /// Modification time as a Unix epoch second (0 when the header omits it or
    /// the MS-DOS timestamp is out of range).
    pub mtime: i64,
    pub link_target: Option<String>,
    /// Index of this member inside the backing [`ZipArchive`], for
    /// `by_index`-based decompression. `Some` only for readable [`ZipEntryKind::File`]
    /// members; `None` for directories, links, and non-readable members.
    archive_index: Option<usize>,
    /// Absolute `(offset, size)` of the member's UNCOMPRESSED data within the
    /// outer mmap. `Some` only for STORED files — where the archived bytes equal
    /// the logical content — so the hex view can jump to a file's real bytes.
    /// `None` for DEFLATE members (their archived bytes are compressed),
    /// directories, links, and non-readable members.
    stored_location: Option<(u64, u64)>,
    /// Member in a decoded backup tar, with paths relative to that backup.
    bak_source: Option<(usize, String)>,
}

pub struct ZipImage {
    path: PathBuf,
    label: String,
    /// Raw bytes of the on-disk archive, for the hex view / sector-source layer.
    outer: Mmap,
    /// Backing archive for decompressing reads. A second, independent mmap of
    /// the same file lives inside the cursor (cheap — both map the same read-only
    /// pages), keeping `outer` free to hand out borrowed slices. Guarded by a
    /// mutex because `ZipArchive::by_index` needs `&mut self`.
    archive: Mutex<ZipArchive<Cursor<Mmap>>>,
    entries: BTreeMap<String, ZipEntry>,
    total_size: u64,
    bak_archives: Vec<TarImage>,
}

impl std::fmt::Debug for ZipImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZipImage")
            .field("path", &self.path)
            .field("outer_len", &self.outer.len())
            .field("entries", &self.entries.len())
            .field("total_size", &self.total_size)
            .finish()
    }
}

impl ZipImage {
    /// mmap a `.zip` file and index its central directory. Returns an error (so
    /// the caller can fall back to a RAW probe) when the file is not a usable ZIP
    /// or holds no members.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let outer = {
            let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
            unsafe { Mmap::map(&file) }.with_context(|| format!("mmap {}", path.display()))?
        };
        // Second independent mapping moved into the archive cursor, so the
        // archive owns its bytes while `outer` keeps handing out slices.
        let archive = {
            let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
            let mmap =
                unsafe { Mmap::map(&file) }.with_context(|| format!("mmap {}", path.display()))?;
            ZipArchive::new(Cursor::new(mmap))
                .with_context(|| format!("read zip central directory: {}", path.display()))?
        };

        let label = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        let mut archive = archive;
        let (mut entries, mut total_size) = scan_zip(&mut archive)
            .with_context(|| format!("scan zip archive: {}", path.display()))?;
        if entries.len() <= 1 {
            // Only the synthetic root: an empty / unusable archive. Bail so
            // `core::evidence::image` keeps probing as RAW.
            bail!("no zip members found in {}", path.display());
        }

        let bak_archives = bak::index_backups(&mut archive, &mut entries, &mut total_size, &path);
        Ok(Self {
            path,
            label,
            outer,
            archive: Mutex::new(archive),
            entries,
            total_size,
            bak_archives,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// Raw bytes of the outer archive (mmap-backed). Lets `core::evidence::image` expose
    /// the same slice to the hex view / sector-source layer.
    pub fn outer_bytes(&self) -> &[u8] {
        &self.outer[..]
    }

    pub fn outer_len(&self) -> u64 {
        self.outer.len() as u64
    }

    /// Summed uncompressed size of every regular-file member.
    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    /// Member count surfaced to the UI, excluding the synthetic root at "".
    pub fn entry_count(&self) -> usize {
        self.entries.len().saturating_sub(1)
    }

    pub fn lookup(&self, rel: &str) -> Option<&ZipEntry> {
        self.entries.get(&normalize_key(rel))
    }

    /// Absolute `(offset, size)` of `rel`'s uncompressed bytes within the outer
    /// archive (one symlink hop resolved), or `None` for DEFLATE members
    /// (compressed in place), directories, devices, and dangling links. Lets the
    /// FS-tree adapter surface a real data-run so the hex view can jump to a
    /// STORED file's bytes in the archive.
    pub fn data_location(&self, rel: &str) -> Option<(u64, u64)> {
        let entry = self.resolve(rel).ok()?;
        entry.stored_location
    }

    /// Direct children of `rel` (empty or "/" = root), in lexicographic order;
    /// the adapter above sorts directories first.
    pub fn children(&self, rel: &str) -> Vec<&ZipEntry> {
        let prefix = normalize_key(rel);
        let scan = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        let mut out = Vec::new();
        for (key, entry) in self.entries.range(scan.clone()..) {
            if !key.starts_with(&scan) {
                break;
            }
            let suffix = &key[scan.len()..];
            if suffix.is_empty() || suffix.contains('/') {
                continue;
            }
            out.push(entry);
        }
        out
    }

    /// Read up to `max_bytes` from a regular file. Symlinks resolve one hop;
    /// directories / non-readable members error out.
    pub fn read(&self, rel: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let entry = self.resolve(rel)?;
        self.read_entry(entry, max_bytes)
    }

    /// Stream the member at `rel` to `visitor` in `chunk_size` windows. STORED
    /// members stream as borrowed slices into the outer mmap (O(1) heap); DEFLATE
    /// members decompress through a bounded buffer, so a multi-GiB member streams
    /// without materialising the whole expansion.
    pub fn stream_file<F>(&self, rel: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let entry = self.resolve(rel)?;
        if let Some((index, path)) = &entry.bak_source {
            return self
                .bak_archives
                .get(*index)
                .context("backup archive index out of range")?
                .stream_file(path, chunk_size, visitor);
        }
        if let Some((offset, size)) = entry.stored_location {
            let data = self.slice(offset, size)?;
            if chunk_size == 0 {
                visitor(data)?;
            } else {
                for window in data.chunks(chunk_size) {
                    visitor(window)?;
                }
            }
            return Ok(data.len() as u64);
        }
        let idx = entry
            .archive_index
            .ok_or_else(|| anyhow!("entry has no data: {}", entry.rel_path))?;
        let window = if chunk_size == 0 { 1 << 20 } else { chunk_size };
        let mut archive = self.archive.lock();
        let mut reader = archive.by_index(idx)?;
        let mut buf = vec![0u8; window];
        let mut total = 0u64;
        loop {
            let read = reader.read(&mut buf).context("decompress zip member")?;
            if read == 0 {
                break;
            }
            visitor(&buf[..read])?;
            total += read as u64;
        }
        Ok(total)
    }

    /// Resolve `rel` (one symlink hop) to the regular-file entry it ultimately
    /// names. Hardlinks do not exist in ZIP, so only symlinks are followed.
    fn resolve(&self, rel: &str) -> Result<&ZipEntry> {
        let entry = self
            .lookup(rel)
            .ok_or_else(|| anyhow!("not found in zip: {rel}"))?;
        match entry.kind {
            ZipEntryKind::File => Ok(entry),
            ZipEntryKind::Symlink if entry.bak_source.is_some() => Ok(entry),
            ZipEntryKind::Symlink => {
                let target = entry
                    .link_target
                    .as_deref()
                    .ok_or_else(|| anyhow!("symlink without target: {rel}"))?;
                // `None` here means the target climbed above the archive root —
                // it pointed outside the captured tree, so treat it as dangling
                // rather than clamping to a coincidental in-archive file.
                let resolved_path = resolve_symlink(rel, target).ok_or_else(|| {
                    anyhow!("symlink target escapes archive root: {rel} -> {target}")
                })?;
                let resolved = self
                    .lookup(&resolved_path)
                    .ok_or_else(|| anyhow!("dangling symlink: {rel} -> {target}"))?;
                // A symlink to a symlink is not followed further (one hop only);
                // require the resolved entry to be a regular file.
                if resolved.kind == ZipEntryKind::File {
                    Ok(resolved)
                } else {
                    bail!("symlink does not resolve to a regular file: {rel} -> {target}")
                }
            }
            ZipEntryKind::Dir => bail!("not a regular file: {rel} (directory)"),
            ZipEntryKind::Other => {
                bail!("member is not readable (encrypted or unsupported compression): {rel}")
            }
        }
    }

    fn read_entry(&self, entry: &ZipEntry, max_bytes: usize) -> Result<Vec<u8>> {
        if let Some((index, path)) = &entry.bak_source {
            return self
                .bak_archives
                .get(*index)
                .context("backup archive index out of range")?
                .read(path, max_bytes);
        }
        if let Some((offset, size)) = entry.stored_location {
            let data = self.slice(offset, size)?;
            let take = (size as usize).min(max_bytes);
            return Ok(data[..take].to_vec());
        }
        let idx = entry
            .archive_index
            .ok_or_else(|| anyhow!("entry has no data: {}", entry.rel_path))?;
        let mut archive = self.archive.lock();
        let mut reader = archive.by_index(idx)?;
        // `take` bounds the decompression to what the caller asked for: a 4 KiB
        // preview never inflates the whole member.
        let mut buf = Vec::new();
        Read::take(&mut reader, max_bytes as u64)
            .read_to_end(&mut buf)
            .context("decompress zip member")?;
        Ok(buf)
    }

    fn slice(&self, offset: u64, size: u64) -> Result<&[u8]> {
        let start = usize::try_from(offset).context("zip offset out of range")?;
        let end = start
            .checked_add(usize::try_from(size).context("zip size out of range")?)
            .ok_or_else(|| anyhow!("zip entry size overflow"))?;
        self.outer
            .get(start..end)
            .ok_or_else(|| anyhow!("zip entry data out of range"))
    }
}

/// Walk the archive's central directory once, returning the indexed members
/// (including a synthetic root and synthesised ancestor directories) and the
/// summed uncompressed regular-file size.
fn scan_zip(archive: &mut ZipArchive<Cursor<Mmap>>) -> Result<(BTreeMap<String, ZipEntry>, u64)> {
    let mut entries: BTreeMap<String, ZipEntry> = BTreeMap::new();
    entries.insert(String::new(), dir_entry(String::new()));

    let count = archive.len();
    let mut capped = false;
    let mut unreadable_count = 0usize;
    for index in 0..count {
        if entries.len() >= MAX_ENTRIES {
            capped = true;
            break;
        }
        // `by_index_raw` reads the member's central-directory metadata + local
        // header WITHOUT constructing a decompressor or checking a password, so
        // it succeeds for encrypted members and members using a compression
        // method we did not compile in (bzip2/zstd/lzma/…) — the very cases that
        // must be surfaced as visible-but-non-readable nodes rather than dropped.
        // (Plain `by_index` would return `Err` for both, skipping them entirely.)
        let file = match archive.by_index_raw(index) {
            Ok(file) => file,
            Err(err) => {
                tracing::warn!("zip: skipping unreadable entry {index}: {err:#}");
                continue;
            }
        };

        // `enclosed_name` is the traversal-safe accessor: it returns `None` for
        // `..` escapes / absolute paths, which is the security boundary.
        let Some(name) = file.enclosed_name() else {
            tracing::debug!(
                "zip: skipping entry {index} with unsafe/absolute path: {:?}",
                file.name_raw()
            );
            continue;
        };
        let raw = name.to_string_lossy().replace('\\', "/");
        let is_dir = file.is_dir();
        let Some(rel) = normalize_zip_path(&raw) else {
            continue;
        };

        let is_symlink = file.is_symlink();
        let method = file.compression();
        let encrypted = file.encrypted();
        let size = file.size();
        let compressed_size = file.compressed_size();
        let data_start = file.data_start();
        let mode = file.unix_mode().unwrap_or(0);
        let mtime = file.last_modified().map(zip_dt_unix).unwrap_or(0);
        drop(file);

        // Only STORED + DEFLATE are compiled in (see `Cargo.toml`); an encrypted
        // member or one using bzip2/zstd/lzma/etc. cannot be decompressed, so it
        // is surfaced as a visible non-readable node instead of erroring on every
        // read or masquerading as content.
        let supported = !encrypted
            && matches!(
                method,
                CompressionMethod::Stored | CompressionMethod::Deflated
            );

        let kind = if is_dir {
            ZipEntryKind::Dir
        } else if is_symlink {
            ZipEntryKind::Symlink
        } else if supported {
            ZipEntryKind::File
        } else {
            ZipEntryKind::Other
        };

        // STORED files map straight onto the archive bytes, so reads are zero-copy
        // and the hex view can jump there — but ONLY when the run is provably the
        // real content. The zip central directory does not enforce
        // `compressed_size == uncompressed_size` for a STORED member; a corrupt /
        // hostile archive can declare an inflated `size`, and slicing
        // `outer[data_start .. data_start+size]` would then bleed adjacent
        // members' bytes into the read. When they disagree, drop the fast path and
        // let reads fall back to the crate's `by_index` reader (which bounds the
        // member by `compressed_size` and verifies its CRC), yielding faithful
        // bytes or a clean error rather than neighbouring archive data.
        let stored_location = if matches!(kind, ZipEntryKind::File)
            && method == CompressionMethod::Stored
            && compressed_size == size
        {
            data_start.map(|offset| (offset, size))
        } else {
            None
        };
        let archive_index = matches!(kind, ZipEntryKind::File).then_some(index);

        // Symlink targets live in the (small) member body, Unix-style. Read it
        // through the DECOMPRESSING reader (the raw reader above would hand back
        // compressed bytes for a DEFLATE symlink); bounded and links are rare.
        let link_target = if matches!(kind, ZipEntryKind::Symlink) {
            archive.by_index(index).ok().and_then(|mut reader| {
                let mut buf = Vec::new();
                let read = Read::take(&mut reader, MAX_SYMLINK_TARGET).read_to_end(&mut buf);
                (read.is_ok() && !buf.is_empty())
                    .then(|| String::from_utf8_lossy(&buf).replace('\\', "/"))
            })
        } else {
            None
        };
        if matches!(kind, ZipEntryKind::Other) {
            unreadable_count += 1;
        }

        ensure_ancestors(&mut entries, &rel);
        let node = ZipEntry {
            rel_path: rel.clone(),
            kind,
            size: if matches!(kind, ZipEntryKind::Dir) {
                0
            } else {
                size
            },
            mode,
            mtime,
            link_target,
            archive_index,
            stored_location,
            bak_source: None,
        };
        // Collision policy mirrors the tar loader: a directory (real or
        // synthesised, possibly with descendants) is never demoted to a file by
        // a colliding non-dir member, or its subtree would be orphaned. Other
        // collisions are last-wins but logged.
        match entries.get(&rel) {
            Some(existing)
                if existing.kind == ZipEntryKind::Dir && node.kind != ZipEntryKind::Dir =>
            {
                tracing::warn!(
                    "zip: member {rel:?} collides with a directory; keeping the directory"
                );
            }
            Some(existing) if existing.kind != ZipEntryKind::Dir => {
                tracing::warn!(
                    "zip: duplicate / colliding member {rel:?} overwrites an earlier entry"
                );
                entries.insert(rel, node);
            }
            _ => {
                entries.insert(rel, node);
            }
        }
    }

    if capped {
        tracing::warn!(
            "zip scan stopped at {} entries (cap reached); archive truncated in the UI",
            MAX_ENTRIES
        );
    }
    if unreadable_count > 0 {
        tracing::warn!(
            "zip: {unreadable_count} member(s) listed as non-readable \
             (encrypted or unsupported compression; raw bytes remain visible in hex)"
        );
    }
    // Sum from the FINAL map so collisions / rejected members can't double-count.
    let total_size = entries
        .values()
        .filter(|entry| matches!(entry.kind, ZipEntryKind::File))
        .map(|entry| entry.size)
        .fold(0u64, |acc, size| acc.saturating_add(size));
    Ok((entries, total_size))
}

/// Insert a `Dir` placeholder for every ancestor of `rel` not already present,
/// so the tree stays navigable when an archive omits explicit directory members.
fn ensure_ancestors(entries: &mut BTreeMap<String, ZipEntry>, rel: &str) {
    let mut acc = String::new();
    let mut segments = rel.split('/').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            break; // last segment is the entry itself, not an ancestor
        }
        if acc.is_empty() {
            acc.push_str(segment);
        } else {
            acc.push('/');
            acc.push_str(segment);
        }
        entries
            .entry(acc.clone())
            .or_insert_with(|| dir_entry(acc.clone()));
    }
}

fn dir_entry(rel_path: String) -> ZipEntry {
    ZipEntry {
        rel_path,
        kind: ZipEntryKind::Dir,
        size: 0,
        mode: 0,
        mtime: 0,
        link_target: None,
        archive_index: None,
        stored_location: None,
        bak_source: None,
    }
}

/// Convert a `zip::DateTime` to a Unix epoch second via its plain y/m/d h:m:s
/// accessors (which need no zip crate feature), treating it as UTC. Returns 0
/// for an out-of-range MS-DOS timestamp.
pub(crate) fn zip_dt_unix(dt: zip::DateTime) -> i64 {
    chrono::NaiveDate::from_ymd_opt(dt.year() as i32, dt.month() as u32, dt.day() as u32)
        .and_then(|date| date.and_hms_opt(dt.hour() as u32, dt.minute() as u32, dt.second() as u32))
        .map(|naive| naive.and_utc().timestamp())
        .unwrap_or(0)
}

/// Normalise a member path: strip leading `/` / trailing `/`, map backslashes to
/// `/`, reject `..` traversal (defence-in-depth behind `enclosed_name`), and map
/// the archive root to `None` (the synthetic root is inserted separately).
pub(crate) fn normalize_zip_path(raw: &str) -> Option<String> {
    let mut path = raw.replace('\\', "/");
    while path.starts_with('/') {
        path.remove(0);
    }
    while path.ends_with('/') {
        path.pop();
    }
    if path.is_empty() {
        return None;
    }
    for segment in path.split('/') {
        if segment == ".." {
            return None;
        }
    }
    Some(path)
}

/// Normalise a lookup key supplied by the dispatcher (already archive-relative,
/// but may carry stray leading/trailing slashes / backslashes).
pub(crate) fn normalize_key(rel: &str) -> String {
    let mut p = rel.replace('\\', "/");
    while p.starts_with('/') {
        p.remove(0);
    }
    while p.ends_with('/') {
        p.pop();
    }
    p
}

/// Resolve a relative symlink `target` declared at `from` to an archive key.
/// Absolute targets are treated as archive-root-relative. Returns `None` when a
/// `..` segment would climb above the archive root.
fn resolve_symlink(from: &str, target: &str) -> Option<String> {
    if target.starts_with('/') {
        return Some(normalize_key(target));
    }
    let parent = match from.rsplit_once('/') {
        Some((p, _)) => p.to_string(),
        None => String::new(),
    };
    let mut stack: Vec<&str> = if parent.is_empty() {
        Vec::new()
    } else {
        parent.split('/').collect()
    };
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                stack.pop()?;
            }
            other => stack.push(other),
        }
    }
    Some(stack.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    /// Build a zip on disk from `(path, data, deflate)` triples and return the
    /// tempfile so the mmap stays backed for the test's lifetime. A trailing `/`
    /// in `path` marks an explicit directory member.
    fn build_zip(members: &[(&str, &[u8], bool)]) -> NamedTempFile {
        let tmp = NamedTempFile::new().expect("tempfile");
        {
            let mut writer = ZipWriter::new(tmp.reopen().expect("reopen"));
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
        tmp
    }

    #[test]
    fn walks_nested_archive_stored_and_deflate() {
        let tmp = build_zip(&[
            ("a.txt", b"hello", false),
            ("sub/", b"", false),
            ("sub/b.bin", b"abcdef", true),
        ]);
        let image = ZipImage::open(tmp.path()).expect("open zip");
        assert_eq!(image.total_size(), 5 + 6);

        let mut roots: Vec<_> = image
            .children("")
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        roots.sort();
        assert_eq!(roots, vec!["a.txt".to_string(), "sub".to_string()]);

        let sub: Vec<_> = image
            .children("sub")
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert_eq!(sub, vec!["sub/b.bin".to_string()]);

        // STORED reads through the zero-copy outer slice; DEFLATE decompresses.
        assert_eq!(image.read("a.txt", usize::MAX).unwrap(), b"hello");
        assert_eq!(image.read("sub/b.bin", usize::MAX).unwrap(), b"abcdef");
        // Only the STORED member exposes a physical data location.
        assert!(image.data_location("a.txt").is_some());
        assert!(image.data_location("sub/b.bin").is_none());
    }

    #[test]
    fn deflate_preview_is_bounded() {
        let big = vec![b'x'; 1 << 16];
        let tmp = build_zip(&[("big.txt", &big, true)]);
        let image = ZipImage::open(tmp.path()).expect("open zip");
        let head = image.read("big.txt", 16).expect("bounded read");
        assert_eq!(head, vec![b'x'; 16]);
        assert_eq!(image.read("big.txt", usize::MAX).unwrap().len(), big.len());
    }

    #[test]
    fn synthesises_missing_parent_dirs() {
        let tmp = build_zip(&[("a/b/c.txt", b"deep", true)]);
        let image = ZipImage::open(tmp.path()).expect("open zip");

        let top: Vec<_> = image
            .children("")
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert_eq!(top, vec!["a".to_string()]);
        assert_eq!(image.lookup("a").map(|e| e.kind), Some(ZipEntryKind::Dir));
        assert_eq!(image.lookup("a/b").map(|e| e.kind), Some(ZipEntryKind::Dir));
        assert_eq!(image.read("a/b/c.txt", usize::MAX).unwrap(), b"deep");
    }

    #[test]
    fn reads_empty_member_as_empty() {
        let tmp = build_zip(&[("empty", b"", false), ("full", b"data", true)]);
        let image = ZipImage::open(tmp.path()).expect("open zip");
        assert_eq!(image.lookup("empty").map(|e| e.size), Some(0));
        assert_eq!(image.read("empty", usize::MAX).unwrap(), b"");
        let mut streamed = Vec::new();
        let n = image
            .stream_file("full", 2, |chunk| {
                streamed.extend_from_slice(chunk);
                Ok(())
            })
            .expect("stream");
        assert_eq!(n, 4);
        assert_eq!(streamed, b"data");
    }

    #[test]
    fn directory_wins_over_colliding_file_member() {
        let tmp = build_zip(&[("a/b.txt", b"keep", true), ("a", b"clobber?", true)]);
        let image = ZipImage::open(tmp.path()).expect("open zip");
        assert_eq!(image.lookup("a").map(|e| e.kind), Some(ZipEntryKind::Dir));
        let kids: Vec<_> = image
            .children("a")
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert_eq!(kids, vec!["a/b.txt".to_string()]);
        assert_eq!(image.read("a/b.txt", usize::MAX).unwrap(), b"keep");
        assert_eq!(image.total_size(), 4);
    }

    #[test]
    fn symlink_resolves_one_hop() {
        // Symlinks store their target as the member body; resolution reads it
        // through the decompressing reader and follows one hop to a regular file.
        let tmp = NamedTempFile::new().expect("tempfile");
        {
            let mut writer = ZipWriter::new(tmp.reopen().expect("reopen"));
            let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
            writer.start_file("dir/real.txt", opts).expect("start");
            writer.write_all(b"payload").expect("write");
            writer
                .add_symlink("dir/link", "real.txt", SimpleFileOptions::default())
                .expect("add_symlink");
            writer.finish().expect("finish");
        }
        let image = ZipImage::open(tmp.path()).expect("open zip");
        assert_eq!(
            image.lookup("dir/link").map(|e| e.kind),
            Some(ZipEntryKind::Symlink)
        );
        assert_eq!(
            image.lookup("dir/link").and_then(|e| e.link_target.clone()),
            Some("real.txt".to_string())
        );
        // Reading the symlink follows the relative target to its sibling file.
        assert_eq!(image.read("dir/link", usize::MAX).unwrap(), b"payload");
    }

    #[test]
    fn normalize_zip_path_handles_traversal() {
        assert_eq!(
            normalize_zip_path("system/bin/sh"),
            Some("system/bin/sh".to_string())
        );
        assert_eq!(normalize_zip_path("/abs/x"), Some("abs/x".to_string()));
        assert_eq!(normalize_zip_path("dir/"), Some("dir".to_string()));
        assert_eq!(normalize_zip_path("../escape"), None);
        assert_eq!(normalize_zip_path("a/../b"), None);
    }

    #[test]
    fn resolve_symlink_resolves_and_flags_escape() {
        assert_eq!(
            resolve_symlink("a/b/c", "../sibling"),
            Some("a/sibling".to_string())
        );
        assert_eq!(resolve_symlink("a/b/c", "../../x"), Some("x".to_string()));
        assert_eq!(
            resolve_symlink("a/b", "/etc/passwd"),
            Some("etc/passwd".to_string())
        );
        assert_eq!(resolve_symlink("topfile", "../secret"), None);
    }

    #[test]
    fn rejects_non_zip_payload() {
        let mut tmp = NamedTempFile::new().expect("tempfile");
        tmp.write_all(&[0u8; 4096]).expect("write zeros");
        tmp.flush().expect("flush");
        assert!(ZipImage::open(tmp.path()).is_err());
    }

    #[test]
    fn opens_real_zip_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_ZIP_TEST") else {
            return;
        };
        let image = ZipImage::open(&path).expect("real zip should open");
        assert!(image.entry_count() > 0);
        assert!(!image.children("").is_empty());
    }
}
