//! Read-only loader that exposes a plain (uncompressed) `.tar` archive as a
//! logical file tree. Mirrors [`crate::core::evidence::readers::docker_image::DockerImage`] and
//! [`crate::core::evidence::readers::folder_image::FolderImage`] in shape: the outer archive is
//! mmap'd, a one-shot header walk at `open()` builds a `BTreeMap<path, entry>`
//! that powers cheap directory listings / stat queries, and regular-file reads
//! are served as zero-copy slices into the mmap.
//!
//! Unlike the Docker composer there are no layers or whiteouts — a tar is a
//! single flat sequence of members. Two robustness measures keep arbitrary
//! archives navigable:
//!   * implicit ancestor directories are synthesised, so an archive that only
//!     stores `a/b/c.txt` (no `a/` or `a/b/` member) still renders a tree;
//!   * `..` traversal is rejected and absolute paths are stripped to relative
//!     form, so a hostile archive cannot punch out of its own root.
//!
//! Symlinks keep their target string and are resolved one hop on `read()`;
//! hardlinks resolve to the link target's data location. Device / fifo members
//! are surfaced as zero-byte nodes that are not readable.
//!
//! A tar is uncompressed by definition; a `.tar`-named file that is actually
//! gzip/zstd (or otherwise not a tar) yields no members and [`TarImage::open`]
//! bails, letting `core::evidence::image` fall back to its RAW probe.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use gzipf::GzipDecoder;
use memmap2::Mmap;

/// Soft cap on recorded members. A 63 GiB Android system tar holds a few
/// hundred thousand entries; this bound keeps a pathological / corrupt archive
/// from exhausting memory while staying far above any real evidence tar.
const MAX_ENTRIES: usize = 2_000_000;

#[cfg(test)]
#[path = "tar_image/gzip_tests.rs"]
mod gzip_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TarEntryKind {
    File,
    Dir,
    Symlink,
    /// Tar hardlink member (`link_target` names another archived file).
    Hardlink,
    /// Character / block device or FIFO — surfaced for completeness but has no
    /// readable byte content.
    Other,
}

#[derive(Debug, Clone)]
pub struct TarEntry {
    /// Normalised archive path, `/`-separated, no leading/trailing slash. The
    /// synthetic root member uses the empty string.
    pub rel_path: String,
    pub kind: TarEntryKind,
    pub size: u64,
    pub mode: u32,
    /// Modification time as a Unix epoch second (0 when the header omits it).
    pub mtime: i64,
    pub uid: u64,
    pub gid: u64,
    pub link_target: Option<String>,
    /// Absolute `(offset, size)` of the member's data within the outer mmap,
    /// for regular files with content. `None` for directories, empty files,
    /// links, and device nodes.
    location: Option<(u64, u64)>,
}

pub struct TarImage {
    path: PathBuf,
    label: String,
    outer: Mmap,
    /// Anonymous delete-on-close file holding a decoded tar. The OS removes it
    /// once its handle closes, even when the process exits without dropping
    /// this image (the desktop app ends without unloading its evidence).
    _tempfile: Option<File>,
    entries: BTreeMap<String, TarEntry>,
    total_size: u64,
}

impl std::fmt::Debug for TarImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TarImage")
            .field("path", &self.path)
            .field("outer_len", &self.outer.len())
            .field("entries", &self.entries.len())
            .field("total_size", &self.total_size)
            .finish()
    }
}

impl TarImage {
    /// mmap a `.tar` file and index its members. Returns an error (so the
    /// caller can fall back to a RAW probe) when the file holds no usable tar
    /// members — e.g. a compressed or non-tar payload that happens to carry a
    /// `.tar` extension.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let outer =
            unsafe { Mmap::map(&file) }.with_context(|| format!("mmap {}", path.display()))?;

        let label = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        Self::from_mmap(path, label, outer, None)
    }

    /// Decompress a `.tar.gz` / `.tgz` into a tempfile and index the resulting
    /// tar stream. The original gzip path stays as the evidence path / label;
    /// only the internal tar bytes back archive navigation and hex reads.
    pub fn open_gzip(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        // Drain every GZIP member and validate all trailers before publishing
        // the tempfile-backed tar view. Errors drop the unpublished tempfile.
        let mut decoder = GzipDecoder::new(file);
        Self::open_from_tar_reader(path, None, &mut decoder)
    }

    /// Materialise a tar byte stream into a tempfile and index it. Used by
    /// logical containers such as Android Backup where the tar is embedded
    /// behind another header/compression layer.
    pub fn open_from_tar_reader(
        path: PathBuf,
        label: Option<String>,
        reader: &mut impl Read,
    ) -> Result<Self> {
        let mut tmp = tempfile::tempfile().context("create tempfile for decoded tar")?;
        let mut buf = vec![0u8; 1024 * 1024];
        loop {
            let read = reader
                .read(&mut buf)
                .with_context(|| format!("decompress {}", path.display()))?;
            if read == 0 {
                break;
            }
            tmp.write_all(&buf[..read])
                .with_context(|| format!("write decompressed tar for {}", path.display()))?;
        }
        tmp.flush()?;
        let outer = unsafe { Mmap::map(&tmp) }.with_context(|| "mmap decompressed tar")?;

        let label = label.unwrap_or_else(|| {
            path.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string())
        });

        Self::from_mmap(path, label, outer, Some(tmp))
    }

    fn from_mmap(
        path: PathBuf,
        label: String,
        outer: Mmap,
        tempfile: Option<File>,
    ) -> Result<Self> {
        let (entries, total_size) =
            scan_tar(&outer).with_context(|| format!("scan tar archive: {}", path.display()))?;
        if entries.len() <= 1 {
            // Only the synthetic root: not a usable tar (empty, compressed, or
            // mis-named). Bail so `core::evidence::image` keeps probing as RAW.
            bail!("no tar members found in {}", path.display());
        }

        Ok(Self {
            path,
            label,
            outer,
            _tempfile: tempfile,
            entries,
            total_size,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// Raw bytes of the outer archive (mmap-backed). Lets `core::evidence::image` expose
    /// the same slice to the hex view / sector-source layer so the on-disk file
    /// is still inspectable as a byte stream.
    pub fn outer_bytes(&self) -> &[u8] {
        &self.outer[..]
    }

    pub fn outer_len(&self) -> u64 {
        self.outer.len() as u64
    }

    /// Consume the logical tar view and return its mmap backing. Docker/OCI
    /// detection first builds this index so ordinary multi-GiB tar evidence is
    /// scanned only once; a confirmed container archive can then reuse the
    /// same mmap while composing layers instead of reopening and rescanning it.
    pub(crate) fn into_backing(self) -> (PathBuf, Mmap, Option<File>) {
        (self.path, self.outer, self._tempfile)
    }

    /// Summed size of every regular-file member (logical content, not the
    /// padded on-disk archive footprint).
    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    /// Member count surfaced to the UI, excluding the synthetic root at "".
    pub fn entry_count(&self) -> usize {
        self.entries.len().saturating_sub(1)
    }

    pub fn lookup(&self, rel: &str) -> Option<&TarEntry> {
        self.entries.get(&normalize_key(rel))
    }

    pub fn entries(&self) -> impl Iterator<Item = &TarEntry> {
        self.entries.values()
    }

    /// Absolute `(offset, size)` of `rel`'s bytes within the outer tar (one
    /// symlink / hardlink hop resolved), or `None` for directories, empty
    /// files, devices, and dangling links. Lets the FS-tree adapter surface a
    /// real data-run so the hex view can jump to a file's bytes in the archive.
    pub fn data_location(&self, rel: &str) -> Option<(u64, u64)> {
        self.resolve_location(rel).ok()
    }

    /// Direct children of `rel` (empty or "/" = root), in lexicographic order;
    /// the adapter above sorts directories first.
    pub fn children(&self, rel: &str) -> Vec<&TarEntry> {
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

    /// Read up to `max_bytes` from a regular file. Symlinks resolve one hop,
    /// hardlinks resolve to their target; directories / devices error out.
    pub fn read(&self, rel: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let (offset, size) = self.resolve_location(rel)?;
        let bytes = self.slice(offset, size)?;
        let take = (size as usize).min(max_bytes);
        Ok(bytes[..take].to_vec())
    }

    /// Stream the member at `rel` to `visitor` in `chunk_size` windows. The
    /// data lives in the outer mmap, so each chunk is a borrowed slice and a
    /// multi-GiB member streams in O(1) heap.
    pub fn stream_file<F>(&self, rel: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let (offset, size) = self.resolve_location(rel)?;
        let data = self.slice(offset, size)?;
        if chunk_size == 0 {
            visitor(data)?;
        } else {
            for window in data.chunks(chunk_size) {
                visitor(window)?;
            }
        }
        Ok(data.len() as u64)
    }

    /// Resolve `rel` (one symlink / hardlink hop) to the `(offset, size)` of
    /// the regular file it ultimately names.
    fn resolve_location(&self, rel: &str) -> Result<(u64, u64)> {
        let entry = self
            .lookup(rel)
            .ok_or_else(|| anyhow!("not found in tar: {rel}"))?;
        let resolved = match entry.kind {
            TarEntryKind::File => entry,
            TarEntryKind::Symlink => {
                let target = entry
                    .link_target
                    .as_deref()
                    .ok_or_else(|| anyhow!("symlink without target: {rel}"))?;
                // `resolve_symlink` returns `None` when the target climbs above
                // the archive root (more `..` than depth) — such a link pointed
                // outside the captured tree on the real device, so it must NOT
                // be clamped to a coincidental in-archive file. Treat as dangling.
                let resolved_path = resolve_symlink(rel, target).ok_or_else(|| {
                    anyhow!("symlink target escapes archive root: {rel} -> {target}")
                })?;
                self.lookup(&resolved_path)
                    .ok_or_else(|| anyhow!("dangling symlink: {rel} -> {target}"))?
            }
            TarEntryKind::Hardlink => {
                let target = entry
                    .link_target
                    .as_deref()
                    .ok_or_else(|| anyhow!("hardlink without target: {rel}"))?;
                // Hardlink targets are stored archive-root-relative but GNU tar
                // emits a `./` prefix whenever the archive was built from a
                // `./`-relative root (`tar -C / -cf x.tar ./system`, the usual
                // Android extraction). Member keys were stored via
                // `normalize_tar_path` (which strips `./`), so the raw target
                // would never match — normalise it the same way before lookup.
                let target_key = normalize_tar_path(target).ok_or_else(|| {
                    anyhow!("hardlink target escapes archive root: {rel} -> {target}")
                })?;
                self.lookup(&target_key)
                    .ok_or_else(|| anyhow!("dangling hardlink: {rel} -> {target}"))?
            }
            TarEntryKind::Dir => bail!("not a regular file: {rel} (directory)"),
            TarEntryKind::Other => bail!("not a regular file: {rel} (device/fifo)"),
        };
        resolved
            .location
            .ok_or_else(|| anyhow!("entry has no data: {rel}"))
    }

    fn slice(&self, offset: u64, size: u64) -> Result<&[u8]> {
        let start = usize::try_from(offset).context("tar offset out of range")?;
        let end = start
            .checked_add(usize::try_from(size).context("tar size out of range")?)
            .ok_or_else(|| anyhow!("tar entry size overflow"))?;
        self.outer
            .get(start..end)
            .ok_or_else(|| anyhow!("tar entry data out of range"))
    }
}

/// Walk the outer tar once, returning the indexed members (including a
/// synthetic root and synthesised ancestor directories) and the summed
/// regular-file size.
fn scan_tar(outer: &Mmap) -> Result<(BTreeMap<String, TarEntry>, u64)> {
    let mut entries: BTreeMap<String, TarEntry> = BTreeMap::new();
    entries.insert(String::new(), dir_entry(String::new()));

    let mut archive = tar::Archive::new(Cursor::new(&outer[..]));
    let mut capped = false;
    let mut sparse_count = 0usize;
    // `entries_with_seek` skips past each member's data with a `seek` instead of
    // reading through it (which plain `entries()` does). On a multi-GiB archive
    // that turns the index walk from an O(archive bytes) full read into an
    // O(members) header-only scan — opening a 63 GiB phone tar drops from
    // minutes (reading every byte) to seconds. Safe here because we iterate
    // strictly in archive order and never read a member body out of sequence.
    for entry in archive.entries_with_seek().context("read tar entries")? {
        if entries.len() >= MAX_ENTRIES {
            capped = true;
            break;
        }
        let mut entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                tracing::warn!("tar: skipping unreadable entry: {err:#}");
                continue;
            }
        };
        // PAX 1.0 GNU sparse metadata (if any) must be read before we take the
        // immutable `entry.header()` borrow below — `pax_extensions` needs
        // `&mut entry`. `entry_type` is `Copy`, so reading it here drops its
        // borrow immediately.
        let entry_type = entry.header().entry_type();
        let pax_sparse = gnu_sparse_pax(&mut entry);

        let header = entry.header();
        let header_path = match entry.path() {
            Ok(path) => path.to_string_lossy().replace('\\', "/"),
            Err(err) => {
                tracing::debug!("tar: skipping entry with undecodable path: {err:#}");
                continue;
            }
        };
        // GNU sparse members — old-format 'S' typeflag, or PAX (formats 0.0/0.1
        // store the real name in the header / `GNU.sparse.name` respectively and
        // the logical length in `GNU.sparse.size`; format 1.0 uses a
        // `GNUSparseFile.NNNN/` placeholder + `GNU.sparse.realsize`). The `tar`
        // crate cannot reconstruct sparse content through our zero-copy
        // mmap-slice reads, so rather than silently dropping 'S' members (the old
        // `_ => continue`) or exposing condensed/placeholder bytes as "content",
        // surface the member under its REAL name and logical size as a visible
        // but NON-READABLE node (kind `Other`, no `location`), and count it for
        // an audit warning. Full sparse reconstruction is a separate follow-up.
        let is_sparse = entry_type == tar::EntryType::GNUSparse || pax_sparse.is_some();
        // Skip extension/global headers — the tar crate folds GNU long-name /
        // PAX metadata into the member they describe, so we never see those as
        // standalone entries here.
        let kind = if is_sparse {
            TarEntryKind::Other
        } else {
            match entry_type {
                tar::EntryType::Regular | tar::EntryType::Continuous => TarEntryKind::File,
                tar::EntryType::Directory => TarEntryKind::Dir,
                tar::EntryType::Symlink => TarEntryKind::Symlink,
                tar::EntryType::Link => TarEntryKind::Hardlink,
                tar::EntryType::Char | tar::EntryType::Block | tar::EntryType::Fifo => {
                    TarEntryKind::Other
                }
                _ => continue,
            }
        };
        // PAX 0.1/1.0 sparse replace the header name with a `GNUSparseFile.NNNN/`
        // placeholder and carry the real name in `GNU.sparse.name`; PAX 0.0 and
        // old-GNU 'S' keep the real name in the header. Prefer the PAX name when
        // present, otherwise the header path.
        let raw_path = match pax_sparse.as_ref().and_then(|s| s.name.as_deref()) {
            Some(name) => name.replace('\\', "/"),
            None => header_path,
        };
        let Some(rel) = normalize_tar_path(&raw_path) else {
            continue;
        };
        if is_sparse {
            sparse_count += 1;
        }

        // Logical content size: the PAX sparse length (`GNU.sparse.realsize` for
        // format 1.0, `GNU.sparse.size` for 0.0/0.1) when present; the
        // crate-resolved `entry.size()` for old 'S' sparse (it rewrites the size
        // to the real length while leaving the header type 'S'); the header size
        // otherwise.
        let size = if let Some(logical) = pax_sparse.as_ref().and_then(|s| s.size) {
            logical
        } else if entry_type == tar::EntryType::GNUSparse {
            entry.size()
        } else {
            header.size().unwrap_or(0)
        };
        let mode = header.mode().unwrap_or(0);
        let mtime = header.mtime().unwrap_or(0) as i64;
        let uid = header.uid().unwrap_or(0);
        let gid = header.gid().unwrap_or(0);
        let link_target = header
            .link_name()
            .ok()
            .flatten()
            .map(|p| p.to_string_lossy().replace('\\', "/"));
        // Regular files always carry a location — including 0-byte files, which
        // resolve to a valid empty slice `(pos, 0)` so a legitimately empty
        // member reads / streams / exports as empty rather than erroring with
        // "entry has no data". Directories, links, devices, and sparse members
        // (kind `Other`) carry no location.
        let location = if matches!(kind, TarEntryKind::File) {
            Some((entry.raw_file_position(), size))
        } else {
            None
        };

        ensure_ancestors(&mut entries, &rel);
        let node = TarEntry {
            rel_path: rel.clone(),
            kind,
            size: if matches!(kind, TarEntryKind::Dir) {
                0
            } else {
                size
            },
            mode,
            mtime,
            uid,
            gid,
            link_target,
            location,
        };
        // Collision policy: a directory (real or synthesised, with possible
        // descendants) must never be demoted to a file by a colliding non-dir
        // member, or the subtree under it would be orphaned (and dropped from a
        // recursive export). Other collisions — duplicate exact paths, or two
        // distinct members whose paths collapse to the same key under
        // lossy-UTF-8 decoding — are last-wins, but logged so the overwrite is
        // auditable rather than silent.
        match entries.get(&rel) {
            Some(existing)
                if existing.kind == TarEntryKind::Dir && node.kind != TarEntryKind::Dir =>
            {
                tracing::warn!(
                    "tar: member {rel:?} collides with a directory; keeping the directory"
                );
            }
            Some(existing) if existing.kind != TarEntryKind::Dir => {
                tracing::warn!(
                    "tar: duplicate / colliding member {rel:?} overwrites an earlier entry"
                );
                entries.insert(rel, node);
            }
            // Existing None, or a real directory replacing its synthesised
            // placeholder (both Dir): take the new node.
            _ => {
                entries.insert(rel, node);
            }
        }
    }

    if capped {
        tracing::warn!(
            "tar scan stopped at {} entries (cap reached); archive truncated in the UI",
            MAX_ENTRIES
        );
    }
    if sparse_count > 0 {
        tracing::warn!(
            "tar: {sparse_count} GNU sparse member(s) listed as non-readable \
             (sparse reconstruction is not supported; raw bytes remain visible in hex)"
        );
    }
    // Sum logical file size from the FINAL map so collisions / skipped members
    // can't double-count (an earlier member's size must not survive after a
    // later member overwrote — or was rejected at — its key).
    let total_size = entries
        .values()
        .filter(|entry| matches!(entry.kind, TarEntryKind::File))
        .map(|entry| entry.size)
        .fold(0u64, |acc, size| acc.saturating_add(size));
    Ok((entries, total_size))
}

/// Insert a `Dir` placeholder for every ancestor of `rel` that is not already
/// present. Keeps the tree navigable when an archive omits explicit directory
/// members. Never overwrites an existing entry (a real dir member that arrives
/// later replaces its placeholder via the normal insert in `scan_tar`).
fn ensure_ancestors(entries: &mut BTreeMap<String, TarEntry>, rel: &str) {
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

fn dir_entry(rel_path: String) -> TarEntry {
    TarEntry {
        rel_path,
        kind: TarEntryKind::Dir,
        size: 0,
        mode: 0,
        mtime: 0,
        uid: 0,
        gid: 0,
        link_target: None,
        location: None,
    }
}

/// GNU sparse metadata recovered from a member's PAX extended header.
struct GnuSparse {
    /// Real member name from `GNU.sparse.name` (PAX 0.1/1.0, where the header
    /// holds a `GNUSparseFile.NNNN/` placeholder). `None` for format 0.0, whose
    /// header already carries the real name.
    name: Option<String>,
    /// Logical (expanded) file length: `GNU.sparse.realsize` (format 1.0) or
    /// `GNU.sparse.size` (formats 0.0/0.1). `None` only on a malformed header.
    size: Option<u64>,
}

/// Detect a PAX GNU sparse member from its extended header. Returns `Some` when
/// any `GNU.sparse.*` key is present — covering all PAX sparse formats (0.0,
/// 0.1, 1.0), which the `tar` crate exposes via `pax_extensions` but does not
/// act on, so the underlying member is a Regular entry whose stored bytes are
/// the compacted (non-sparse) data, not the logical file. Returns `None` for
/// non-sparse members (old-format 'S' sparse is detected separately via the
/// header typeflag). Does not read the member body, so it leaves the seek
/// iterator position untouched.
fn gnu_sparse_pax<R: std::io::Read>(entry: &mut tar::Entry<'_, R>) -> Option<GnuSparse> {
    let exts = entry.pax_extensions().ok().flatten()?;
    let mut is_sparse = false;
    let mut name: Option<String> = None;
    let mut size: Option<u64> = None;
    for ext in exts {
        let Ok(ext) = ext else { continue };
        let Ok(key) = ext.key() else { continue };
        if !key.starts_with("GNU.sparse.") {
            continue;
        }
        is_sparse = true;
        match key {
            "GNU.sparse.name" => name = ext.value().ok().map(|v| v.to_string()),
            // `realsize` (format 1.0) and `size` (0.0/0.1) both carry the logical
            // length; prefer `realsize` when both appear, regardless of order.
            "GNU.sparse.realsize" => {
                if let Some(v) = ext.value().ok().and_then(|v| v.trim().parse::<u64>().ok()) {
                    size = Some(v);
                }
            }
            "GNU.sparse.size" if size.is_none() => {
                size = ext.value().ok().and_then(|v| v.trim().parse::<u64>().ok());
            }
            _ => {}
        }
    }
    is_sparse.then_some(GnuSparse { name, size })
}

/// Normalise a member path read from a tar header: strip `./` / leading `/` /
/// trailing `/`, reject `..` traversal, and map the archive root to `None`
/// (callers skip it — the synthetic root is inserted separately).
fn normalize_tar_path(raw: &str) -> Option<String> {
    let mut path = raw
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string();
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
fn normalize_key(rel: &str) -> String {
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
/// `..` segment would climb above the archive root (the link pointed outside the
/// captured tree), so the caller can treat it as dangling instead of silently
/// clamping it to a coincidental in-archive file.
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
                // Popping past the root means the target escaped the archive.
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
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    use tempfile::NamedTempFile;

    /// Build a tar on disk from `(path, kind, data)` triples and return the
    /// tempfile so the mmap stays backed for the test's lifetime.
    fn build_tar(members: &[(&str, tar::EntryType, &[u8])]) -> NamedTempFile {
        let mut tmp = NamedTempFile::new().expect("tempfile");
        {
            let mut builder = tar::Builder::new(tmp.as_file_mut());
            for (path, kind, data) in members {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_entry_type(*kind);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, path, &data[..])
                    .expect("append");
            }
            builder.finish().expect("finish tar");
        }
        tmp.as_file_mut().flush().expect("flush");
        tmp
    }

    #[test]
    fn walks_nested_archive() {
        let tmp = build_tar(&[
            ("a.txt", tar::EntryType::Regular, b"hello"),
            ("sub/", tar::EntryType::Directory, b""),
            ("sub/b.bin", tar::EntryType::Regular, b"abcdef"),
        ]);
        let image = TarImage::open(tmp.path()).expect("open tar");
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

        assert_eq!(image.read("a.txt", usize::MAX).unwrap(), b"hello");
        assert_eq!(image.read("sub/b.bin", usize::MAX).unwrap(), b"abcdef");
    }

    #[test]
    fn synthesises_missing_parent_dirs() {
        // Only a deep file, no explicit dir members.
        let tmp = build_tar(&[("a/b/c.txt", tar::EntryType::Regular, b"deep")]);
        let image = TarImage::open(tmp.path()).expect("open tar");

        let top: Vec<_> = image
            .children("")
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert_eq!(top, vec!["a".to_string()]);
        assert_eq!(image.lookup("a").map(|e| e.kind), Some(TarEntryKind::Dir));
        assert_eq!(image.lookup("a/b").map(|e| e.kind), Some(TarEntryKind::Dir));

        let mid: Vec<_> = image
            .children("a/b")
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert_eq!(mid, vec!["a/b/c.txt".to_string()]);
        assert_eq!(image.read("a/b/c.txt", usize::MAX).unwrap(), b"deep");
    }

    #[test]
    fn reads_empty_member_as_empty() {
        // A legitimately empty (0-byte) file must read / stream as empty, not
        // error with "entry has no data".
        let tmp = build_tar(&[
            ("empty", tar::EntryType::Regular, b""),
            ("full", tar::EntryType::Regular, b"data"),
        ]);
        let image = TarImage::open(tmp.path()).expect("open tar");
        assert_eq!(image.lookup("empty").map(|e| e.size), Some(0));
        assert_eq!(image.read("empty", usize::MAX).unwrap(), b"");
        let mut streamed = Vec::new();
        let n = image
            .stream_file("empty", 4096, |chunk| {
                streamed.extend_from_slice(chunk);
                Ok(())
            })
            .expect("stream empty");
        assert_eq!(n, 0);
        assert!(streamed.is_empty());
    }

    #[test]
    fn hardlink_with_dot_slash_target_resolves() {
        // GNU tar built from a `./` root (the usual Android `/system`
        // extraction) emits hardlink targets with a `./` prefix. The member key
        // is stored without it, so the target must be normalised the same way
        // before lookup — otherwise busybox/toybox-style hardlinks dangle.
        let mut tmp = NamedTempFile::new().expect("tempfile");
        {
            let mut builder = tar::Builder::new(tmp.as_file_mut());
            let mut file = tar::Header::new_gnu();
            file.set_size(7);
            file.set_entry_type(tar::EntryType::Regular);
            file.set_mode(0o755);
            file.set_cksum();
            builder
                .append_data(&mut file, "system/bin/toybox", &b"toybox!"[..])
                .expect("append file");
            let mut link = tar::Header::new_gnu();
            link.set_size(0);
            link.set_entry_type(tar::EntryType::Link);
            link.set_mode(0o755);
            builder
                .append_link(&mut link, "system/bin/sh", "./system/bin/toybox")
                .expect("append hardlink");
            builder.finish().expect("finish");
        }
        tmp.as_file_mut().flush().expect("flush");

        let image = TarImage::open(tmp.path()).expect("open tar");
        assert_eq!(
            image.lookup("system/bin/sh").map(|e| e.kind),
            Some(TarEntryKind::Hardlink)
        );
        assert_eq!(image.read("system/bin/sh", usize::MAX).unwrap(), b"toybox!");
        // The hardlink also exposes a real data run (the target's bytes).
        assert!(image.data_location("system/bin/sh").is_some());
    }

    #[test]
    fn directory_wins_over_colliding_file_member() {
        // A regular file named `a` arriving AFTER a deep member under `a/` must
        // not demote the synthesised directory — that would orphan the subtree
        // and drop it from a recursive export.
        let tmp = build_tar(&[
            ("a/b.txt", tar::EntryType::Regular, b"keep"),
            ("a", tar::EntryType::Regular, b"clobber?"),
        ]);
        let image = TarImage::open(tmp.path()).expect("open tar");
        assert_eq!(image.lookup("a").map(|e| e.kind), Some(TarEntryKind::Dir));
        let kids: Vec<_> = image
            .children("a")
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert_eq!(kids, vec!["a/b.txt".to_string()]);
        assert_eq!(image.read("a/b.txt", usize::MAX).unwrap(), b"keep");
        // total_size must reflect only the surviving file, not the rejected one.
        assert_eq!(image.total_size(), 4);
    }

    #[test]
    fn normalize_tar_path_handles_dot_and_traversal() {
        assert_eq!(
            normalize_tar_path("./system/bin/sh"),
            Some("system/bin/sh".to_string())
        );
        assert_eq!(
            normalize_tar_path("/system/bin/sh"),
            Some("system/bin/sh".to_string())
        );
        assert_eq!(normalize_tar_path("system/"), Some("system".to_string()));
        assert_eq!(normalize_tar_path("../escape"), None);
        assert_eq!(normalize_tar_path("a/../b"), None);
        assert_eq!(normalize_tar_path("./"), None);
    }

    #[test]
    fn resolve_symlink_resolves_and_flags_escape() {
        assert_eq!(
            resolve_symlink("a/b/c", "../sibling"),
            Some("a/sibling".to_string())
        );
        // `..` count exactly equal to depth lands at the root — still in-archive.
        assert_eq!(resolve_symlink("a/b/c", "../../x"), Some("x".to_string()));
        assert_eq!(
            resolve_symlink("a/b", "/etc/passwd"),
            Some("etc/passwd".to_string())
        );
        // Climbing above the root is an escape → None (dangling, not clamped).
        assert_eq!(resolve_symlink("topfile", "../secret"), None);
        assert_eq!(resolve_symlink("a", "../../etc"), None);
    }

    #[test]
    fn key_normalisation_is_lenient() {
        let tmp = build_tar(&[("x", tar::EntryType::Regular, b"x")]);
        let image = TarImage::open(tmp.path()).expect("open tar");
        assert!(image.lookup("x").is_some());
        assert!(image.lookup("/x").is_some());
        assert!(image.lookup("/x/").is_some());
        assert!(image.lookup("\\x").is_some());
    }

    #[test]
    fn rejects_non_tar_payload() {
        let mut tmp = NamedTempFile::new().expect("tempfile");
        tmp.write_all(&[0u8; 4096]).expect("write zeros");
        tmp.flush().expect("flush");
        // A block of zeros decodes to zero members → open must bail so the
        // caller falls back to RAW.
        assert!(TarImage::open(tmp.path()).is_err());
    }

    #[test]
    fn opens_gzip_compressed_tar() {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "hello.txt", &b"hello"[..])
                .expect("append");
            builder.finish().expect("finish tar");
        }

        let mut tmp = NamedTempFile::new().expect("tempfile");
        {
            let mut encoder = GzEncoder::new(tmp.as_file_mut(), Compression::default());
            encoder.write_all(&tar_bytes).expect("write gzip");
            encoder.finish().expect("finish gzip");
        }
        tmp.as_file_mut().flush().expect("flush");

        let image = TarImage::open_gzip(tmp.path()).expect("open tar.gz");
        assert_eq!(image.entry_count(), 1);
        assert_eq!(image.read("hello.txt", usize::MAX).unwrap(), b"hello");
    }

    #[test]
    fn opens_real_tar_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_TAR_TEST") else {
            return;
        };
        let image = TarImage::open(&path).expect("real tar should open");
        assert!(image.entry_count() > 0);
        // Root must list at least one child.
        assert!(!image.children("").is_empty());
    }
}
