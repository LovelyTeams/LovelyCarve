//! Read-only loader that exposes a `.7z` archive as a logical evidence tree.
//!
//! The archive header is indexed at open time; member bytes are decompressed on
//! demand through `sevenz-rust2`. Solid archives may need to decode earlier
//! members before the requested one, so reads are streaming and bounded by the
//! caller instead of materialising the whole archive unless export asks for it.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use memmap2::Mmap;
use parking_lot::Mutex;
use sevenz_rust2::{Archive, ArchiveEntry, ArchiveReader, Password};

const MAX_ENTRIES: usize = 2_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SevenzEntryKind {
    File,
    Dir,
    Other,
}

#[derive(Debug, Clone)]
pub struct SevenzEntry {
    pub rel_path: String,
    pub kind: SevenzEntryKind,
    pub size: u64,
    pub mode: u32,
    pub modified: Option<i64>,
    pub accessed: Option<i64>,
    pub created: Option<i64>,
}

pub struct SevenzImage {
    path: PathBuf,
    label: String,
    outer: Mmap,
    reader: Mutex<ArchiveReader<File>>,
    entries: BTreeMap<String, SevenzEntry>,
    total_size: u64,
}

impl std::fmt::Debug for SevenzImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SevenzImage")
            .field("path", &self.path)
            .field("outer_len", &self.outer.len())
            .field("entries", &self.entries.len())
            .field("total_size", &self.total_size)
            .finish()
    }
}

impl SevenzImage {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let outer = {
            let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
            unsafe { Mmap::map(&file) }.with_context(|| format!("mmap {}", path.display()))?
        };
        let reader = ArchiveReader::open(&path, Password::empty())
            .with_context(|| format!("read 7z archive: {}", path.display()))?;

        let label = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let (entries, total_size) = scan_archive(reader.archive())
            .with_context(|| format!("scan 7z: {}", path.display()))?;
        if entries.len() <= 1 {
            bail!("no 7z members found in {}", path.display());
        }

        Ok(Self {
            path,
            label,
            outer,
            reader: Mutex::new(reader),
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

    pub fn outer_bytes(&self) -> &[u8] {
        &self.outer[..]
    }

    pub fn outer_len(&self) -> u64 {
        self.outer.len() as u64
    }

    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    pub fn entry_count(&self) -> usize {
        self.entries.len().saturating_sub(1)
    }

    pub fn lookup(&self, rel: &str) -> Option<&SevenzEntry> {
        self.entries.get(&normalize_key(rel))
    }

    pub fn children(&self, rel: &str) -> Vec<&SevenzEntry> {
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

    pub fn read(&self, rel: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let entry = self
            .lookup(rel)
            .ok_or_else(|| anyhow!("not found in 7z: {rel}"))?;
        if !matches!(entry.kind, SevenzEntryKind::File) {
            bail!("not a regular file: {rel}");
        }
        if entry.size == 0 || max_bytes == 0 {
            return Ok(Vec::new());
        }
        let target = normalize_key(rel);
        let mut out = Vec::new();
        self.visit_member(&target, |reader| {
            Read::take(reader, max_bytes as u64)
                .read_to_end(&mut out)
                .context("decompress 7z member")?;
            Ok(())
        })?;
        Ok(out)
    }

    pub fn stream_file<F>(&self, rel: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let entry = self
            .lookup(rel)
            .ok_or_else(|| anyhow!("not found in 7z: {rel}"))?;
        if !matches!(entry.kind, SevenzEntryKind::File) {
            bail!("not a regular file: {rel}");
        }
        if entry.size == 0 {
            return Ok(0);
        }
        let target = normalize_key(rel);
        let mut total = 0u64;
        self.visit_member(&target, |reader| {
            let window = if chunk_size == 0 {
                1024 * 1024
            } else {
                chunk_size
            };
            let mut buf = vec![0u8; window];
            loop {
                let read = reader.read(&mut buf).context("decompress 7z member")?;
                if read == 0 {
                    break;
                }
                visitor(&buf[..read])?;
                total += read as u64;
            }
            Ok(())
        })?;
        Ok(total)
    }

    fn visit_member<F>(&self, target: &str, mut visit: F) -> Result<()>
    where
        F: FnMut(&mut dyn Read) -> Result<()>,
    {
        let mut found = false;
        let mut visit_error: Option<anyhow::Error> = None;
        let mut reader = self.reader.lock();
        reader
            .for_each_entries(|entry, stream| {
                if normalize_7z_path(entry.name()).as_deref() == Some(target) {
                    if let Err(err) = visit(stream) {
                        visit_error = Some(err);
                    }
                    found = true;
                    return Ok(false);
                }
                let mut sink = std::io::sink();
                std::io::copy(stream, &mut sink)?;
                Ok(true)
            })
            .map_err(|err| anyhow!("decompress 7z archive: {err}"))?;
        if let Some(err) = visit_error {
            return Err(err);
        }
        if found {
            Ok(())
        } else {
            bail!("not found in 7z: {target}")
        }
    }
}

fn scan_archive(archive: &Archive) -> Result<(BTreeMap<String, SevenzEntry>, u64)> {
    let mut entries: BTreeMap<String, SevenzEntry> = BTreeMap::new();
    entries.insert(String::new(), dir_entry(String::new()));

    let mut capped = false;
    for raw in &archive.files {
        if entries.len() >= MAX_ENTRIES {
            capped = true;
            break;
        }
        let Some(rel) = normalize_7z_path(raw.name()) else {
            continue;
        };
        let kind = entry_kind(raw);
        ensure_ancestors(&mut entries, &rel);
        let node = SevenzEntry {
            rel_path: rel.clone(),
            kind,
            size: if matches!(kind, SevenzEntryKind::Dir) {
                0
            } else {
                raw.size
            },
            mode: raw.windows_attributes,
            modified: raw
                .has_last_modified_date
                .then(|| nt_time_epoch(raw.last_modified_date()))
                .flatten(),
            accessed: raw
                .has_access_date
                .then(|| nt_time_epoch(raw.access_date()))
                .flatten(),
            created: raw
                .has_creation_date
                .then(|| nt_time_epoch(raw.creation_date()))
                .flatten(),
        };
        match entries.get(&rel) {
            Some(existing)
                if existing.kind == SevenzEntryKind::Dir && node.kind != SevenzEntryKind::Dir =>
            {
                tracing::warn!(
                    "7z: member {rel:?} collides with a directory; keeping the directory"
                );
            }
            Some(existing) if existing.kind != SevenzEntryKind::Dir => {
                tracing::warn!(
                    "7z: duplicate / colliding member {rel:?} overwrites an earlier entry"
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
            "7z scan stopped at {} entries (cap reached); archive truncated in the UI",
            MAX_ENTRIES
        );
    }

    let total_size = entries
        .values()
        .filter(|entry| matches!(entry.kind, SevenzEntryKind::File))
        .map(|entry| entry.size)
        .fold(0u64, |acc, size| acc.saturating_add(size));
    Ok((entries, total_size))
}

fn entry_kind(entry: &ArchiveEntry) -> SevenzEntryKind {
    if entry.is_directory() {
        SevenzEntryKind::Dir
    } else if entry.is_anti_item() {
        SevenzEntryKind::Other
    } else {
        SevenzEntryKind::File
    }
}

fn ensure_ancestors(entries: &mut BTreeMap<String, SevenzEntry>, rel: &str) {
    let mut acc = String::new();
    let mut segments = rel.split('/').peekable();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            break;
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

fn dir_entry(rel_path: String) -> SevenzEntry {
    SevenzEntry {
        rel_path,
        kind: SevenzEntryKind::Dir,
        size: 0,
        mode: 0,
        modified: None,
        accessed: None,
        created: None,
    }
}

fn nt_time_epoch(value: sevenz_rust2::NtTime) -> Option<i64> {
    let time: std::time::SystemTime = value.into();
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_secs()).ok())
}

fn normalize_7z_path(raw: &str) -> Option<String> {
    let mut path = raw.replace('\\', "/");
    while path.starts_with("./") {
        path = path[2..].to_string();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_7z_path_handles_dot_and_traversal() {
        assert_eq!(
            normalize_7z_path("./system/bin/sh"),
            Some("system/bin/sh".to_string())
        );
        assert_eq!(normalize_7z_path("/abs/x"), Some("abs/x".to_string()));
        assert_eq!(normalize_7z_path("dir/"), Some("dir".to_string()));
        assert_eq!(normalize_7z_path("../escape"), None);
        assert_eq!(normalize_7z_path("a/../b"), None);
    }

    #[test]
    fn opens_real_7z_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_7Z_TEST") else {
            return;
        };
        let image = SevenzImage::open(&path).expect("real 7z should open");
        assert!(image.entry_count() > 0);
        assert!(!image.children("").is_empty());
    }
}
