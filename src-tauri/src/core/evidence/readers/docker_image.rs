//! Read-only Docker / OCI image archive composer.
//!
//! Accepts both the OCI image-layout tar (`index.json` + `oci-layout` +
//! `blobs/sha256/...`) and the legacy `docker save` tar (`manifest.json`).
//! Layers may be uncompressed (`vnd.oci.image.layer.v1.tar`), gzipped
//! (`...tar+gzip`) or zstd-compressed (`...tar+zstd`); the loader picks the
//! right decoder by media-type with a magic-byte fallback.
//!
//! The outer tar is mmap'd. For raw layers, file data is served as a slice
//! into the mmap with no copy. For compressed layers, the decoded bytes are
//! kept in an `Arc<Vec<u8>>` shared across `FileEntry` lookups.
//!
//! Whiteout semantics follow the OCI image spec §"Layer / Whiteouts":
//!   * `dir/.wh.<name>` removes `dir/<name>` (and its descendants if it's a
//!     directory) from the accumulated tree before the current layer's own
//!     entries are merged in.
//!   * `dir/.wh..wh..opq` purges every entry inherited from a lower layer
//!     under `dir/`, but does not touch entries the current layer adds in
//!     the same step.
//!
//! Hardlinks resolve at merge-time to the link target's data location;
//! symlinks keep their target string and are resolved on `read()`.
//!
//! Hostile inputs are bounded: per-layer tar walks reject entries whose
//! path escapes the archive (`..`), absolute paths are stripped to
//! relative form, and decoded layer size is capped at 8 GiB.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::core::evidence::readers::docker_manifest::{
    load_manifest, OuterMemberIndex, ResolvedLayer, ResolvedManifest,
};
use crate::core::evidence::readers::tar_image::TarImage;

const MAX_DECODED_LAYER: u64 = 8 * 1024 * 1024 * 1024;

#[cfg(test)]
#[path = "docker_image/gzip_tests.rs"]
mod gzip_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Hardlink,
    CharDevice,
    BlockDevice,
    Fifo,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileEntry {
    pub path: String,
    pub kind: EntryKind,
    pub size: u64,
    pub mode: u32,
    pub mtime: i64,
    pub uid: u64,
    pub gid: u64,
    pub link_target: Option<String>,
    pub layer_index: usize,
    #[serde(skip)]
    location: Option<DataLoc>,
}

#[derive(Debug, Clone)]
struct DataLoc {
    layer_index: usize,
    offset_in_layer: u64,
    size: u64,
}

enum LayerBytes {
    Raw { offset: u64, len: usize },
    Decoded(Arc<Vec<u8>>),
}

pub struct Layer {
    pub digest: String,
    pub media_type: String,
    pub uncompressed_size: u64,
    pub compressed_size: u64,
    bytes: LayerBytes,
}

impl std::fmt::Debug for Layer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Layer")
            .field("digest", &self.digest)
            .field("media_type", &self.media_type)
            .field("uncompressed_size", &self.uncompressed_size)
            .field("compressed_size", &self.compressed_size)
            .finish()
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ImageConfig {
    #[serde(default)]
    pub architecture: Option<String>,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub config: Option<RuntimeConfig>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RuntimeConfig {
    #[serde(rename = "User", default)]
    pub user: Option<String>,
    #[serde(rename = "Env", default)]
    pub env: Vec<String>,
    #[serde(rename = "Cmd", default)]
    pub cmd: Option<Vec<String>>,
    #[serde(rename = "Entrypoint", default)]
    pub entrypoint: Option<Vec<String>>,
    #[serde(rename = "WorkingDir", default)]
    pub working_dir: Option<String>,
    #[serde(rename = "Labels", default)]
    pub labels: serde_json::Map<String, serde_json::Value>,
    #[serde(rename = "ExposedPorts", default)]
    pub exposed_ports: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct HistoryEntry {
    #[serde(default)]
    pub created: Option<String>,
    #[serde(default)]
    pub created_by: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub empty_layer: bool,
}

pub struct DockerImage {
    path: PathBuf,
    outer: Mmap,
    _tempfile: Option<NamedTempFile>,
    layers: Vec<Layer>,
    entries: BTreeMap<String, FileEntry>,
    config: ImageConfig,
    repo_tags: Vec<String>,
}

struct ComposedImage {
    layers: Vec<Layer>,
    entries: BTreeMap<String, FileEntry>,
    config: ImageConfig,
    repo_tags: Vec<String>,
}

impl std::fmt::Debug for DockerImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerImage")
            .field("path", &self.path)
            .field("outer_len", &self.outer.len())
            .field("layers", &self.layers.len())
            .field("entries", &self.entries.len())
            .field("repo_tags", &self.repo_tags)
            .finish()
    }
}

impl DockerImage {
    /// Opens an OCI / docker-save tar archive and merges its layers into a
    /// single read-only filesystem view.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let file = File::open(&path).with_context(|| format!("open {}", path.display()))?;
        let outer =
            unsafe { Mmap::map(&file) }.with_context(|| format!("mmap {}", path.display()))?;
        let outer_index = scan_outer(&outer)?;
        Self::from_backing(path, outer, None, &outer_index)
    }

    /// Compose a Docker/OCI image from the `TarImage` already built by the
    /// `.tar` dispatcher. On failure the original tar view is returned intact
    /// so callers can preserve the plain-tar fallback without another scan.
    pub(crate) fn from_tar(tar: TarImage) -> Result<Self, Box<(TarImage, anyhow::Error)>> {
        let composed = compose_image(tar.outer_bytes(), &tar);
        match composed {
            Ok(composed) => {
                let (path, outer, tempfile) = tar.into_backing();
                Ok(Self {
                    path,
                    outer,
                    _tempfile: tempfile,
                    layers: composed.layers,
                    entries: composed.entries,
                    config: composed.config,
                    repo_tags: composed.repo_tags,
                })
            }
            Err(err) => Err(Box::new((tar, err))),
        }
    }

    fn from_backing<I: OuterMemberIndex + ?Sized>(
        path: PathBuf,
        outer: Mmap,
        tempfile: Option<NamedTempFile>,
        outer_index: &I,
    ) -> Result<Self> {
        let composed = compose_image(&outer, outer_index)?;
        Ok(Self {
            path,
            outer,
            _tempfile: tempfile,
            layers: composed.layers,
            entries: composed.entries,
            config: composed.config,
            repo_tags: composed.repo_tags,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn config(&self) -> &ImageConfig {
        &self.config
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    pub fn repo_tags(&self) -> &[String] {
        &self.repo_tags
    }

    /// Raw bytes of the outer archive (mmap-backed). Lets `core::evidence::image`
    /// expose the same slice to the hex view / sector-source layer so the
    /// rest of the app can still reason about the on-disk file as a
    /// regular byte stream even though we treat its content as a merged
    /// container filesystem.
    pub fn outer_bytes(&self) -> &[u8] {
        &self.outer[..]
    }

    /// Total outer-archive size in bytes.
    pub fn outer_len(&self) -> u64 {
        self.outer.len() as u64
    }

    pub fn lookup(&self, path: &str) -> Option<&FileEntry> {
        self.entries.get(&normalize_key(path))
    }

    /// Direct children of `path` ("" / "/" enumerates the image root).
    pub fn children(&self, path: &str) -> Vec<&FileEntry> {
        let prefix = normalize_key(path);
        let mut out = Vec::new();
        let scan_prefix = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        for (key, entry) in self.entries.range(scan_prefix.clone()..) {
            if !key.starts_with(&scan_prefix) {
                break;
            }
            let suffix = &key[scan_prefix.len()..];
            if suffix.is_empty() || suffix.contains('/') {
                continue;
            }
            out.push(entry);
        }
        out
    }

    /// Iterates every merged entry in lexicographic order.
    pub fn walk(&self) -> impl Iterator<Item = &FileEntry> {
        self.entries.values()
    }

    /// Reads up to `max_bytes` from a regular file. Symlinks are resolved
    /// once (no chains). Returns an error for directories, devices, or
    /// dangling links.
    pub fn read(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let loc = self.resolve_data_loc(path)?;
        self.read_at(loc, max_bytes)
    }

    /// Stream the file at `path` to `visitor` in `chunk_size` windows
    /// without materialising the whole file. The data lives in the
    /// outer-tar mmap, so each chunk is a borrowed slice — a multi-GiB
    /// layer file streams in O(1) heap instead of one giant `Vec`.
    pub fn stream<F>(&self, path: &str, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let loc = self.resolve_data_loc(path)?;
        let layer = self
            .layers
            .get(loc.layer_index)
            .ok_or_else(|| anyhow!("invalid layer index"))?;
        let bytes = layer_slice(&self.outer, &layer.bytes);
        let start = loc.offset_in_layer as usize;
        let end = start
            .checked_add(loc.size as usize)
            .ok_or_else(|| anyhow!("entry size overflow"))?;
        if end > bytes.len() {
            bail!("entry data out of range");
        }
        let data = &bytes[start..end];
        if chunk_size == 0 {
            visitor(data)?;
        } else {
            for window in data.chunks(chunk_size) {
                visitor(window)?;
            }
        }
        Ok(data.len() as u64)
    }

    /// Resolve `path` (following one symlink hop) to the data location of
    /// the regular file it ultimately names. Shared by `read` and `stream`.
    fn resolve_data_loc(&self, path: &str) -> Result<&DataLoc> {
        let entry = self
            .lookup(path)
            .ok_or_else(|| anyhow!("not found: {path}"))?;

        let resolved = match entry.kind {
            EntryKind::File | EntryKind::Hardlink => entry,
            EntryKind::Symlink => {
                let target = entry
                    .link_target
                    .as_deref()
                    .ok_or_else(|| anyhow!("symlink without target: {path}"))?;
                let resolved_path = resolve_symlink(path, target);
                self.lookup(&resolved_path)
                    .ok_or_else(|| anyhow!("dangling symlink: {path} -> {target}"))?
            }
            EntryKind::Dir => bail!("not a regular file: {path} (directory)"),
            EntryKind::CharDevice | EntryKind::BlockDevice | EntryKind::Fifo => {
                bail!("not a regular file: {path} (device/fifo)")
            }
        };

        resolved
            .location
            .as_ref()
            .ok_or_else(|| anyhow!("entry has no data: {path}"))
    }

    fn read_at(&self, loc: &DataLoc, max_bytes: usize) -> Result<Vec<u8>> {
        let layer = self
            .layers
            .get(loc.layer_index)
            .ok_or_else(|| anyhow!("invalid layer index"))?;
        let bytes = layer_slice(&self.outer, &layer.bytes);
        let start = loc.offset_in_layer as usize;
        let end = start
            .checked_add(loc.size as usize)
            .ok_or_else(|| anyhow!("entry size overflow"))?;
        if end > bytes.len() {
            bail!("entry data out of range");
        }
        let take = (loc.size as usize).min(max_bytes);
        Ok(bytes[start..start + take].to_vec())
    }
}

fn scan_outer(outer: &Mmap) -> Result<BTreeMap<String, (u64, u64)>> {
    let mut map = BTreeMap::new();
    let mut archive = tar::Archive::new(Cursor::new(&outer[..]));
    // `entries_with_seek` skips past each member's body with a `seek` instead of
    // reading through it (which plain `entries()` does) — a header-only
    // O(members) scan instead of an O(archive bytes) full read (the latter is
    // ~19 s on a 12.5 GiB tar, far worse off a network drive). `core::evidence::image`
    // builds `TarImage` first and only calls `DockerImage::open` when its index
    // shows docker markers (`index.json` / `manifest.json`), so this scan runs
    // on actual docker archives (small) rather than on every multi-GiB evidence
    // tarball. Safe here: the reader is a `Cursor<&[u8]>` (Read + Seek) and we
    // iterate strictly in archive order, never reading a body out of sequence.
    // Mirrors `tar_image::scan_tar`.
    for entry in archive.entries_with_seek().context("read outer tar")? {
        let entry = entry.context("read outer tar entry")?;
        let header = entry.header();
        if header.entry_type() != tar::EntryType::Regular
            && header.entry_type() != tar::EntryType::Continuous
        {
            continue;
        }
        let raw_path = entry
            .path()
            .context("decode outer tar path")?
            .to_string_lossy()
            .replace('\\', "/");
        let key = raw_path.trim_start_matches("./").to_string();
        let offset = entry.raw_file_position();
        let size = header.size().unwrap_or(0);
        map.insert(key, (offset, size));
    }
    Ok(map)
}

fn compose_image<I: OuterMemberIndex + ?Sized>(outer: &[u8], index: &I) -> Result<ComposedImage> {
    let (manifest, config, repo_tags) = load_manifest(outer, index)?;
    let layers = build_layers(outer, index, &manifest)?;
    let entries = compose_layers(outer, &layers)?;
    Ok(ComposedImage {
        layers,
        entries,
        config,
        repo_tags,
    })
}

fn build_layers<I: OuterMemberIndex + ?Sized>(
    outer: &[u8],
    index: &I,
    manifest: &ResolvedManifest,
) -> Result<Vec<Layer>> {
    manifest
        .layers
        .iter()
        .map(|layer| build_layer(outer, index, layer))
        .collect()
}

fn build_layer<I: OuterMemberIndex + ?Sized>(
    outer: &[u8],
    index: &I,
    layer: &ResolvedLayer,
) -> Result<Layer> {
    let (offset, size) = index
        .member_location(&layer.member)
        .ok_or_else(|| anyhow!("missing layer blob: {}", layer.member))?;
    let start = usize::try_from(offset).context("layer offset out of range")?;
    let size_usize = usize::try_from(size).context("layer size out of range")?;
    let end = start
        .checked_add(size_usize)
        .ok_or_else(|| anyhow!("layer blob size overflow: {}", layer.member))?;
    let raw = outer
        .get(start..end)
        .ok_or_else(|| anyhow!("layer blob out of range: {}", layer.member))?;
    let compression = detect_compression(&layer.media_type, raw);
    let (bytes, uncompressed) = match compression {
        Compression::None => (
            LayerBytes::Raw {
                offset,
                len: size_usize,
            },
            size,
        ),
        Compression::Gzip => {
            let decoded = decode_gzip(raw)?;
            let len = decoded.len() as u64;
            (LayerBytes::Decoded(Arc::new(decoded)), len)
        }
        Compression::Zstd => {
            let decoded = decode_zstd(raw)?;
            let len = decoded.len() as u64;
            (LayerBytes::Decoded(Arc::new(decoded)), len)
        }
    };
    Ok(Layer {
        digest: layer.digest.clone(),
        media_type: layer.media_type.clone(),
        compressed_size: size,
        uncompressed_size: if layer.declared_size != 0 && matches!(compression, Compression::None) {
            layer.declared_size
        } else {
            uncompressed
        },
        bytes,
    })
}

#[derive(Debug, Clone, Copy)]
enum Compression {
    None,
    Gzip,
    Zstd,
}

fn detect_compression(media_type: &str, raw: &[u8]) -> Compression {
    let mt = media_type.to_ascii_lowercase();
    if mt.contains("gzip") {
        return Compression::Gzip;
    }
    if mt.contains("zstd") {
        return Compression::Zstd;
    }
    if gzipf::check_signature(raw) {
        return Compression::Gzip;
    }
    if raw.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return Compression::Zstd;
    }
    Compression::None
}

fn decode_gzip(raw: &[u8]) -> Result<Vec<u8>> {
    let mut options = gzipf::Options::default();
    options.max_uncompressed_bytes = MAX_DECODED_LAYER;
    options.max_compressed_bytes = raw.len() as u64;
    let mut out = Vec::new();
    // No Read::take: reaching an output limit must fail, never masquerade as
    // EOF before later members or their trailers have been checked.
    gzipf::GzipDecoder::with_options(raw, options)
        .read_to_end(&mut out)
        .context("gunzip layer")?;
    Ok(out)
}

fn decode_zstd(raw: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    zstd::stream::Decoder::new(raw)
        .context("zstd init")?
        .take(MAX_DECODED_LAYER + 1)
        .read_to_end(&mut out)
        .context("zstd decode layer")?;
    if out.len() as u64 > MAX_DECODED_LAYER {
        bail!("zstd layer exceeds {MAX_DECODED_LAYER} bytes");
    }
    Ok(out)
}

fn layer_slice<'a>(outer: &'a [u8], bytes: &'a LayerBytes) -> &'a [u8] {
    match bytes {
        LayerBytes::Raw { offset, len } => &outer[*offset as usize..*offset as usize + *len],
        LayerBytes::Decoded(arc) => arc.as_slice(),
    }
}

#[derive(Debug, Clone)]
struct LayerEntry {
    path: String,
    kind: EntryKind,
    size: u64,
    mode: u32,
    mtime: i64,
    uid: u64,
    gid: u64,
    link_target: Option<String>,
    location: Option<DataLoc>,
}

fn walk_layer(outer: &[u8], layer_index: usize, layer: &Layer) -> Result<Vec<LayerEntry>> {
    let bytes = layer_slice(outer, &layer.bytes);
    let mut archive = tar::Archive::new(Cursor::new(bytes));
    let mut out = Vec::new();
    // Indexing only needs headers and data offsets. The reader is seekable, so
    // skip member bodies instead of copying every payload through `Read` just
    // to reach the next header. This is especially important for large raw
    // layers and for already-decoded gzip/zstd layers.
    for entry in archive
        .entries_with_seek()
        .with_context(|| format!("read layer tar {}", layer.digest))?
    {
        let entry = entry.with_context(|| format!("read entry in layer {}", layer.digest))?;
        let header = entry.header();
        let raw_path = entry
            .path()
            .with_context(|| format!("decode path in layer {}", layer.digest))?
            .to_string_lossy()
            .replace('\\', "/");
        let path = match normalize_layer_path(&raw_path) {
            Some(p) => p,
            None => continue,
        };
        let entry_type = header.entry_type();
        let kind = match entry_type {
            tar::EntryType::Regular | tar::EntryType::Continuous => EntryKind::File,
            tar::EntryType::Directory => EntryKind::Dir,
            tar::EntryType::Symlink => EntryKind::Symlink,
            tar::EntryType::Link => EntryKind::Hardlink,
            tar::EntryType::Char => EntryKind::CharDevice,
            tar::EntryType::Block => EntryKind::BlockDevice,
            tar::EntryType::Fifo => EntryKind::Fifo,
            // PAX/global extension headers are surfaced internally by the tar
            // crate; anything else (XGlobalHeader, GNUSparse, ...) we skip.
            _ => continue,
        };
        let size = header.size().unwrap_or(0);
        let mode = header.mode().unwrap_or(0);
        let mtime = header.mtime().unwrap_or(0) as i64;
        let uid = header.uid().unwrap_or(0);
        let gid = header.gid().unwrap_or(0);
        let link_target = header
            .link_name()
            .ok()
            .flatten()
            .map(|p| p.to_string_lossy().replace('\\', "/"));
        let location = if matches!(kind, EntryKind::File) {
            Some(DataLoc {
                layer_index,
                offset_in_layer: entry.raw_file_position(),
                size,
            })
        } else {
            None
        };
        out.push(LayerEntry {
            path,
            kind,
            size,
            mode,
            mtime,
            uid,
            gid,
            link_target,
            location,
        });
    }
    Ok(out)
}

fn normalize_layer_path(raw: &str) -> Option<String> {
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
    // Reject `..` traversal — a hostile layer must not punch out of its
    // own root.
    for segment in path.split('/') {
        if segment == ".." {
            return None;
        }
    }
    Some(path)
}

fn split_whiteout(path: &str) -> Option<Whiteout> {
    let (parent, name) = match path.rsplit_once('/') {
        Some((p, n)) => (p, n),
        None => ("", path),
    };
    if name == ".wh..wh..opq" {
        return Some(Whiteout::Opaque {
            dir: parent.to_string(),
        });
    }
    if let Some(stripped) = name.strip_prefix(".wh.") {
        let target = if parent.is_empty() {
            stripped.to_string()
        } else {
            format!("{parent}/{stripped}")
        };
        return Some(Whiteout::Remove { target });
    }
    None
}

enum Whiteout {
    Remove { target: String },
    Opaque { dir: String },
}

fn compose_layers(outer: &[u8], layers: &[Layer]) -> Result<BTreeMap<String, FileEntry>> {
    let mut acc: BTreeMap<String, FileEntry> = BTreeMap::new();
    for (layer_index, layer) in layers.iter().enumerate() {
        let entries = walk_layer(outer, layer_index, layer)?;
        // Phase 1 — gather whiteout markers for this layer.
        let mut removals: Vec<String> = Vec::new();
        let mut opaque_dirs: Vec<String> = Vec::new();
        let mut regular: Vec<LayerEntry> = Vec::with_capacity(entries.len());
        for entry in entries {
            match split_whiteout(&entry.path) {
                Some(Whiteout::Remove { target }) => removals.push(target),
                Some(Whiteout::Opaque { dir }) => opaque_dirs.push(dir),
                None => regular.push(entry),
            }
        }
        // Phase 2 — apply opaque markers (remove every inherited entry
        // beneath the dir, but not entries we'll add this layer).
        for dir in &opaque_dirs {
            let prefix = if dir.is_empty() {
                String::new()
            } else {
                format!("{dir}/")
            };
            let to_drop: Vec<String> = acc
                .range(prefix.clone()..)
                .take_while(|(key, _)| key.starts_with(&prefix))
                .map(|(key, _)| key.clone())
                .collect();
            for key in to_drop {
                acc.remove(&key);
            }
        }
        // Phase 3 — apply per-entry removals (target + descendants).
        for target in &removals {
            acc.remove(target);
            let prefix = format!("{target}/");
            let to_drop: Vec<String> = acc
                .range(prefix.clone()..)
                .take_while(|(key, _)| key.starts_with(&prefix))
                .map(|(key, _)| key.clone())
                .collect();
            for key in to_drop {
                acc.remove(&key);
            }
        }
        // Phase 4 — merge in this layer's regular entries.
        for entry in regular {
            // Hardlinks: resolve to target's data location now (target
            // must already exist in `acc` per tar semantics — within the
            // same layer or inherited from below).
            let location = match entry.kind {
                EntryKind::Hardlink => entry
                    .link_target
                    .as_deref()
                    .and_then(normalize_layer_path)
                    .and_then(|target_key| acc.get(&target_key))
                    .and_then(|target| target.location.clone()),
                _ => entry.location,
            };
            acc.insert(
                entry.path.clone(),
                FileEntry {
                    path: entry.path,
                    kind: entry.kind,
                    size: entry.size,
                    mode: entry.mode,
                    mtime: entry.mtime,
                    uid: entry.uid,
                    gid: entry.gid,
                    link_target: entry.link_target,
                    layer_index,
                    location,
                },
            );
        }
    }
    Ok(acc)
}

fn normalize_key(path: &str) -> String {
    let mut p = path.replace('\\', "/");
    while p.starts_with('/') {
        p.remove(0);
    }
    while p.ends_with('/') {
        p.pop();
    }
    p
}

fn resolve_symlink(from: &str, target: &str) -> String {
    if target.starts_with('/') {
        return normalize_key(target);
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
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    stack.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_leading_dot_slash() {
        assert_eq!(
            normalize_layer_path("./etc/passwd"),
            Some("etc/passwd".into())
        );
        assert_eq!(
            normalize_layer_path("/etc/passwd"),
            Some("etc/passwd".into())
        );
        assert_eq!(normalize_layer_path("etc/"), Some("etc".into()));
        assert_eq!(normalize_layer_path("./"), None);
        assert_eq!(normalize_layer_path("../escape"), None);
    }

    #[test]
    fn whiteout_detection() {
        match split_whiteout("etc/.wh.passwd").unwrap() {
            Whiteout::Remove { target } => assert_eq!(target, "etc/passwd"),
            _ => panic!(),
        }
        match split_whiteout("var/lib/.wh..wh..opq").unwrap() {
            Whiteout::Opaque { dir } => assert_eq!(dir, "var/lib"),
            _ => panic!(),
        }
        assert!(split_whiteout("etc/passwd").is_none());
    }

    #[test]
    fn symlink_resolves_relative_to_parent() {
        assert_eq!(resolve_symlink("usr/bin/ls", "../sbin/ls"), "usr/sbin/ls");
        assert_eq!(resolve_symlink("a/b/c", "/x/y"), "x/y");
        assert_eq!(resolve_symlink("a/b", "./c"), "a/c");
    }

    #[test]
    fn opens_real_archive_when_env_path_is_set() {
        let Some(path) = std::env::var_os("LOVELYCARVE_DOCKER_TEST") else {
            return;
        };
        let image = DockerImage::open(path).expect("docker archive should open");
        assert!(!image.layers().is_empty());
        assert!(!image.entries.is_empty());
    }
}
