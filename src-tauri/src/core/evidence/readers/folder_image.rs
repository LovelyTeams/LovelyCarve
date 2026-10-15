//! Logical-evidence loader that exposes an on-disk folder tree as a
//! read-only "image". Mirrors [`crate::core::evidence::readers::docker_image::DockerImage`] in
//! shape: a one-shot walk at `open()` builds a `BTreeMap<rel-path, entry>`
//! that powers cheap directory listings and stat queries; file reads hit
//! the underlying disk on demand through the same path the user picked.
//!
//! The folder source has no notion of partitions or sectors — downstream
//! commands that depend on byte-level access (hex view) reject
//! folder-backed images explicitly so the user sees a
//! clear message instead of nonsensical output.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};

use crate::core::evidence::readers::tar_image::{TarEntry, TarEntryKind, TarImage};

mod ios;
#[cfg(test)]
mod ios_tests;
pub use ios::IosBackupInfo;

/// Soft cap on how many entries we'll record when scanning a folder. Keeps
/// pathological inputs (someone pointing at their entire home directory)
/// from making the UI hang for minutes — the user can always switch to a
/// more targeted subfolder.
const MAX_ENTRIES: usize = 250_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderEntryKind {
    File,
    Dir,
    Symlink,
}

#[derive(Debug, Clone)]
pub struct FolderEntry {
    /// Relative path from the folder root, using `/` as separator on every
    /// platform. The root itself uses the empty string.
    pub rel_path: String,
    pub kind: FolderEntryKind,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
    pub accessed: Option<SystemTime>,
    pub status_changed: Option<SystemTime>,
    pub inode: Option<u64>,
    pub mode: Option<u32>,
    pub link_target: Option<String>,
    source: Option<FolderEntrySource>,
}

#[derive(Debug, Clone)]
enum FolderEntrySource {
    Ios(usize),
    Host(PathBuf),
    BakArchive {
        archive_index: usize,
        tar_rel_path: String,
    },
}

#[derive(Debug)]
pub struct FolderImage {
    root: PathBuf,
    label: String,
    entries: BTreeMap<String, FolderEntry>,
    total_size: u64,
    bak_archives: Vec<TarImage>,
    ios_backup: Option<ios::IosBackup>,
}

impl FolderImage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        if let Some(image) = Self::detect_ios(path.as_ref())? {
            return Ok(image);
        }
        Self::open_host(path)
    }

    fn open_host(path: impl AsRef<Path>) -> Result<Self> {
        let root = path.as_ref().to_path_buf();
        let meta =
            fs::metadata(&root).with_context(|| format!("stat folder root: {}", root.display()))?;
        if !meta.is_dir() {
            bail!("not a directory: {}", root.display());
        }
        let label = root
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.display().to_string());
        let mut entries = BTreeMap::new();
        let mut total_size = 0u64;
        entries.insert(
            String::new(),
            FolderEntry {
                rel_path: String::new(),
                kind: FolderEntryKind::Dir,
                size: 0,
                modified: meta.modified().ok(),
                created: meta.created().ok(),
                accessed: meta.accessed().ok(),
                status_changed: None,
                inode: None,
                mode: None,
                link_target: None,
                source: Some(FolderEntrySource::Host(root.clone())),
            },
        );
        let mut stack: Vec<(PathBuf, String)> = vec![(root.clone(), String::new())];
        let mut bak_files: Vec<(String, PathBuf, u64)> = Vec::new();
        while let Some((dir, prefix)) = stack.pop() {
            if entries.len() >= MAX_ENTRIES {
                tracing::warn!(
                    "folder scan stopped at {} entries (cap reached) under {}",
                    entries.len(),
                    root.display()
                );
                break;
            }
            let read_dir = match fs::read_dir(&dir) {
                Ok(r) => r,
                Err(err) => {
                    tracing::warn!("read_dir {}: {err:#}", dir.display());
                    continue;
                }
            };
            for entry in read_dir.flatten() {
                if entries.len() >= MAX_ENTRIES {
                    break;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let rel = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                // `symlink_metadata` here so symlinks don't get followed
                // (especially important on Windows where junctions can
                // create cycles). File reads can still resolve targets later.
                let entry_path = entry.path();
                let meta = match fs::symlink_metadata(&entry_path) {
                    Ok(m) => m,
                    Err(err) => {
                        tracing::debug!("stat {}: {err:#}", entry_path.display());
                        continue;
                    }
                };
                let file_type = meta.file_type();
                let kind = if file_type.is_dir() {
                    FolderEntryKind::Dir
                } else if file_type.is_symlink() {
                    FolderEntryKind::Symlink
                } else {
                    FolderEntryKind::File
                };
                let size = if matches!(kind, FolderEntryKind::File) {
                    meta.len()
                } else {
                    0
                };
                if matches!(kind, FolderEntryKind::File) {
                    total_size = total_size.saturating_add(size);
                    if entry_path
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| {
                            ext.eq_ignore_ascii_case("bak") || ext.eq_ignore_ascii_case("ab")
                        })
                    {
                        bak_files.push((rel.clone(), entry_path.clone(), size));
                    }
                }
                let link_target = if matches!(kind, FolderEntryKind::Symlink) {
                    fs::read_link(&entry_path)
                        .ok()
                        .map(|p| p.to_string_lossy().into_owned())
                } else {
                    None
                };
                entries.insert(
                    rel.clone(),
                    FolderEntry {
                        rel_path: rel.clone(),
                        kind,
                        size,
                        modified: meta.modified().ok(),
                        created: meta.created().ok(),
                        accessed: meta.accessed().ok(),
                        status_changed: None,
                        inode: None,
                        mode: None,
                        link_target,
                        source: Some(FolderEntrySource::Host(entry_path.clone())),
                    },
                );
                if matches!(kind, FolderEntryKind::Dir) {
                    stack.push((entry_path, rel));
                }
            }
        }
        let bak_archives = index_bak_files(&mut entries, &mut total_size, bak_files)?;
        Ok(Self {
            root,
            label,
            entries,
            total_size,
            bak_archives,
            ios_backup: None,
        })
    }

    pub fn root_path(&self) -> &Path {
        &self.root
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    pub fn entry_count(&self) -> usize {
        // Exclude the synthetic root entry at "" from the count surfaced to
        // the UI — users care about how many real files / dirs they're
        // looking at, not the implementation detail.
        self.entries.len().saturating_sub(1)
    }

    pub fn lookup(&self, rel: &str) -> Option<&FolderEntry> {
        self.entries.get(&normalize_rel(rel))
    }

    /// Direct children of `rel` (empty or "/" = root). Returned in
    /// lexicographic order; the dispatcher above sorts directories first.
    pub fn children(&self, rel: &str) -> Vec<&FolderEntry> {
        let prefix = normalize_rel(rel);
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

    /// Reads up to `max_bytes` from a regular file inside the folder
    /// source. Symlinks read the on-disk target (the OS resolves the link);
    /// directories error out.
    pub fn read(&self, rel: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let entry = self
            .lookup(rel)
            .ok_or_else(|| anyhow!("not found in folder: {rel}"))?;
        match entry.kind {
            FolderEntryKind::Dir => bail!("not a regular file: {rel} (directory)"),
            FolderEntryKind::File | FolderEntryKind::Symlink => {}
        }
        match entry.source.as_ref() {
            Some(FolderEntrySource::Ios(index)) => self.read_ios(*index, max_bytes),
            Some(FolderEntrySource::Host(path)) => {
                let mut file = fs::File::open(path).with_context(|| format!("open file: {rel}"))?;
                let want = max_bytes.min(entry.size as usize);
                let mut buf = vec![0u8; want];
                let mut filled = 0;
                while filled < buf.len() {
                    let read = file.read(&mut buf[filled..])?;
                    if read == 0 {
                        break;
                    }
                    filled += read;
                }
                buf.truncate(filled);
                Ok(buf)
            }
            Some(FolderEntrySource::BakArchive {
                archive_index,
                tar_rel_path,
            }) => self
                .bak_archives
                .get(*archive_index)
                .ok_or_else(|| anyhow!("bak archive index out of range: {archive_index}"))?
                .read(tar_rel_path, max_bytes),
            None => bail!("not a regular file: {rel} (no backing source)"),
        }
    }

    pub fn stream_file<F>(&self, rel: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let entry = self
            .lookup(rel)
            .ok_or_else(|| anyhow!("not found in folder: {rel}"))?;
        match entry.kind {
            FolderEntryKind::Dir => bail!("not a regular file: {rel} (directory)"),
            FolderEntryKind::File | FolderEntryKind::Symlink => {}
        }
        match entry.source.as_ref() {
            Some(FolderEntrySource::Ios(index)) => self.stream_ios(*index, chunk_size, visitor),
            Some(FolderEntrySource::Host(path)) => {
                let mut file = fs::File::open(path).with_context(|| format!("open file: {rel}"))?;
                let chunk = chunk_size.max(64 * 1024);
                let mut buf = vec![0u8; chunk];
                let mut total = 0u64;
                loop {
                    let read = file.read(&mut buf)?;
                    if read == 0 {
                        break;
                    }
                    visitor(&buf[..read])?;
                    total += read as u64;
                }
                Ok(total)
            }
            Some(FolderEntrySource::BakArchive {
                archive_index,
                tar_rel_path,
            }) => self
                .bak_archives
                .get(*archive_index)
                .ok_or_else(|| anyhow!("bak archive index out of range: {archive_index}"))?
                .stream_file(tar_rel_path, chunk_size, visitor),
            None => bail!("not a regular file: {rel} (no backing source)"),
        }
    }
}

fn normalize_rel(rel: &str) -> String {
    let trimmed = rel.trim_matches('/');
    trimmed.replace('\\', "/")
}

fn index_bak_files(
    entries: &mut BTreeMap<String, FolderEntry>,
    total_size: &mut u64,
    bak_files: Vec<(String, PathBuf, u64)>,
) -> Result<Vec<TarImage>> {
    let mut archives = Vec::new();
    for (rel, path, bak_size) in bak_files {
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        let label = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        match crate::core::evidence::readers::mi_bak::open_bak_archive(&path, label) {
            Ok(archive) => {
                let archive_index = archives.len();
                *total_size = total_size.saturating_sub(bak_size);
                *total_size = total_size.saturating_add(archive.total_size());
                entries.insert(rel.clone(), bak_root_entry(&rel, &path));
                index_bak_archive_entries(entries, &mut archives, archive, archive_index, &rel);
                tracing::info!(
                    "mi-bak: indexed {} under {} (entries={})",
                    path.display(),
                    rel,
                    archives[archive_index].entry_count()
                );
            }
            Err(err) => {
                tracing::warn!("mi-bak: failed to index {}: {err:#}", path.display());
            }
        }
    }
    Ok(archives)
}

fn index_bak_archive_entries(
    entries: &mut BTreeMap<String, FolderEntry>,
    archives: &mut Vec<TarImage>,
    archive: TarImage,
    archive_index: usize,
    root_rel: &str,
) {
    for tar_entry in archive.entries() {
        if tar_entry.rel_path.is_empty() || entries.len() >= MAX_ENTRIES {
            continue;
        }
        let rel = format!("{root_rel}/{}", tar_entry.rel_path);
        entries.insert(
            rel.clone(),
            bak_entry_from_tar(&rel, tar_entry, archive_index),
        );
    }
    archives.push(archive);
}

fn bak_root_entry(rel: &str, path: &Path) -> FolderEntry {
    let metadata = fs::metadata(path).ok();
    FolderEntry {
        rel_path: rel.to_string(),
        kind: FolderEntryKind::Dir,
        size: 0,
        modified: metadata.as_ref().and_then(|meta| meta.modified().ok()),
        created: metadata.as_ref().and_then(|meta| meta.created().ok()),
        accessed: metadata.as_ref().and_then(|meta| meta.accessed().ok()),
        status_changed: None,
        inode: None,
        mode: None,
        link_target: None,
        source: None,
    }
}

fn bak_entry_from_tar(rel: &str, entry: &TarEntry, archive_index: usize) -> FolderEntry {
    let kind = match entry.kind {
        TarEntryKind::Dir => FolderEntryKind::Dir,
        TarEntryKind::Symlink => FolderEntryKind::Symlink,
        TarEntryKind::File | TarEntryKind::Hardlink | TarEntryKind::Other => FolderEntryKind::File,
    };
    let source = (!matches!(kind, FolderEntryKind::Dir)).then(|| FolderEntrySource::BakArchive {
        archive_index,
        tar_rel_path: entry.rel_path.clone(),
    });
    FolderEntry {
        rel_path: rel.to_string(),
        kind,
        size: entry.size,
        modified: (entry.mtime > 0).then(|| UNIX_EPOCH + Duration::from_secs(entry.mtime as u64)),
        created: None,
        accessed: None,
        status_changed: None,
        inode: None,
        mode: None,
        link_target: entry.link_target.clone(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use std::fs::{create_dir, write};
    use std::io::Write;
    use std::path::Path;

    #[cfg(unix)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(target, link)
    }

    #[cfg(windows)]
    fn create_dir_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_dir(target, link)
    }

    #[test]
    fn walks_nested_folder() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        create_dir(root.join("sub")).unwrap();
        write(root.join("a.txt"), b"hello").unwrap();
        write(root.join("sub/b.bin"), b"abcdef").unwrap();

        let image = FolderImage::open(root).expect("open folder");
        assert_eq!(image.total_size(), 5 + 6);
        let mut names: Vec<_> = image
            .children("")
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        names.sort();
        assert_eq!(names, vec!["a.txt".to_string(), "sub".to_string()]);

        let sub: Vec<_> = image
            .children("sub")
            .iter()
            .map(|e| e.rel_path.clone())
            .collect();
        assert_eq!(sub, vec!["sub/b.bin".to_string()]);

        let bytes = image.read("a.txt", usize::MAX).unwrap();
        assert_eq!(bytes, b"hello");
    }

    #[test]
    fn normalises_keys() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path().join("x"), b"x").unwrap();
        let image = FolderImage::open(tmp.path()).unwrap();
        assert!(image.lookup("x").is_some());
        assert!(image.lookup("/x").is_some());
        assert!(image.lookup("/x/").is_some());
    }

    #[test]
    fn records_directory_symlink_without_descending() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let target = root.join("target");
        create_dir(&target).unwrap();
        write(target.join("inside.txt"), b"hidden").unwrap();

        if create_dir_symlink(&target, &root.join("link")).is_err() {
            return;
        }

        let image = FolderImage::open(root).unwrap();
        let link = image.lookup("link").expect("symlink entry");
        assert_eq!(link.kind, FolderEntryKind::Symlink);
        assert!(link.link_target.is_some());
        assert!(image.lookup("target/inside.txt").is_some());
        assert!(image.lookup("link/inside.txt").is_none());
    }

    #[test]
    fn auto_indexes_xiaomi_bak_files_as_directory_nodes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        write(root.join("app.bak"), build_test_bak()).unwrap();
        let backup = build_test_bak();
        write(
            root.join("phone.ab"),
            backup.strip_prefix(b"xiaomi-prefix").unwrap(),
        )
        .unwrap();

        let image = FolderImage::open(root).expect("open folder");

        let unpacked = "app.bak/apps/com.example/data.txt";
        assert_eq!(
            image.lookup("app.bak").map(|entry| entry.kind),
            Some(FolderEntryKind::Dir)
        );
        assert_eq!(image.read(unpacked, usize::MAX).unwrap(), b"hello");
        assert_eq!(image.lookup("phone.ab").unwrap().kind, FolderEntryKind::Dir);
        assert_eq!(
            image
                .read("phone.ab/apps/com.example/data.txt", usize::MAX)
                .unwrap(),
            b"hello"
        );
    }

    fn build_test_bak() -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            let mut header = tar::Header::new_gnu();
            header.set_size(5);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "apps/com.example/data.txt", &b"hello"[..])
                .unwrap();
            builder.finish().unwrap();
        }

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&tar_bytes).unwrap();
        let compressed = encoder.finish().unwrap();

        let mut bak = Vec::new();
        bak.extend_from_slice(b"xiaomi-prefix");
        bak.extend_from_slice(b"ANDROID BACKUP\n5\n1\nnone\n");
        bak.extend_from_slice(&compressed);
        bak
    }
}
