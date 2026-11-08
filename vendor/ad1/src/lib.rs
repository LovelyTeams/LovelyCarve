//! `ad1` — pure-Rust reader for the **AccessData AD1** logical image container
//! (FTK Imager "Custom Content Image").
//!
//! AD1 is a *logical* file/folder container — NOT a sector-level disk image. It
//! stores a tree of files + per-file metadata (name, timestamps, attributes) +
//! the file data in zlib-compressed chunks + stored MD5/SHA1 hashes, across one
//! or more segments (`.ad1`, `.ad2`, …). So this reader exposes a **virtual
//! filesystem** (path → bytes + metadata), like a zip/tar reader — there is no
//! block device / partition / filesystem layer underneath it.
//!
//! The on-disk layout follows the al3ks1s/AD1-tools reverse-engineered reference
//! (see `docs/format.md`). All integers are little-endian; tree addresses are
//! logical offsets handled internally by `segment::SegmentSet` (private).
//!
//! [`Ad1Reader`] is `Send + Sync`: segment files are read with positioned
//! reads, so one reader serves concurrent callers.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod segment;

#[cfg(feature = "testfix")]
pub mod testfix;

use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use flate2::read::ZlibDecoder;
use safe_read::{le_u32, le_u64};
use segment::SegmentSet;

/// Marker string present in every AD1 segment header.
pub const AD1_SEGMENTED_MARKER: &[u8] = b"ADSEGMENTEDFILE\x00";

// --- hardening limits (Paranoid Gatekeeper) ------------------------------
/// Physical bytes read up front to parse the segment + logical headers.
const HEADER_WINDOW: usize = 0x300;
/// Largest accepted item-name length.
const MAX_NAME_LEN: usize = 4096;
/// Largest accepted metadata record payload.
const MAX_META_DATA: usize = 65_536;
/// Largest accepted metadata-record count per item.
const MAX_META_RECORDS: usize = 4096;
/// Largest accepted total tree-node count.
const MAX_ENTRIES: usize = 5_000_000;
/// Largest accepted zlib chunk size (64 MiB; the format's typical value is 64 KiB).
const MAX_CHUNK_SIZE: u32 = 64 * 1024 * 1024;

/// Item-type value for a folder node (`AD1_FOLDER_SIGNATURE`).
const ITEM_TYPE_FOLDER: u32 = 0x05;

/// Errors from reading an AD1 image.
#[derive(Debug, thiserror::Error)]
pub enum Ad1Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not an AD1 image: {0}")]
    NotAd1(String),
    #[error("unsupported AD1 feature: {0}")] // e.g. ADCRYPT (encrypted)
    Unsupported(String),
    #[error("malformed AD1 structure: {0}")]
    Malformed(String),
}

/// One node in the AD1 logical file tree.
#[derive(Debug, Clone)]
pub struct Ad1Entry {
    /// Logical path within the image (POSIX-style, `/`-separated).
    pub path: String,
    /// True for a directory node (no file data).
    pub is_dir: bool,
    /// Uncompressed file size in bytes (0 for directories).
    pub size: u64,
    /// Raw item-type code from the header (0 = file, 5 = folder).
    pub item_type: u32,
    /// Stored MD5 (lowercase hex) if the image carries one for this file.
    pub md5: Option<String>,
    /// Stored SHA1 (lowercase hex) if present.
    pub sha1: Option<String>,
    /// Stored modified timestamp (`YYYYMMDDThhmmss`) if present.
    pub modified: Option<String>,
    /// Stored accessed timestamp if present.
    pub accessed: Option<String>,
    /// Stored changed/created timestamp if present.
    pub changed: Option<String>,
    /// Index of the parent folder in [`Ad1Reader::entries`]; `None` for a
    /// top-level item.
    pub parent: Option<usize>,
    /// Logical address of this file's zlib chunk table (0 if none).
    pub(crate) zlib_addr: u64,
}

/// A reader over an AD1 logical image (its first segment plus any `.ad2`…).
#[derive(Debug)]
pub struct Ad1Reader {
    segments: SegmentSet,
    image_version: u32,
    chunk_size: u32,
    segment_count: u32,
    entries: Vec<Ad1Entry>,
}

/// Parsed item-header fields needed to walk the tree.
struct RawItem {
    next_item_addr: u64,
    first_child_addr: u64,
    first_metadata_addr: u64,
    zlib_metadata_addr: u64,
    decompressed_size: u64,
    item_type: u32,
    name: String,
}

impl Ad1Reader {
    /// Open an AD1 image given the path to its **first** segment (`*.ad1`);
    /// subsequent segments are discovered alongside it.
    ///
    /// # Errors
    /// - [`Ad1Error::NotAd1`] if the signature is absent (the bytes are shown),
    /// - [`Ad1Error::Unsupported`] for `ADCRYPT` (encrypted) images,
    /// - [`Ad1Error::Malformed`] / [`Ad1Error::Io`] for structural / I/O faults.
    pub fn open(first_segment: &Path) -> Result<Self, Ad1Error> {
        // `read_to_end` keeps reading until the window is full or the file
        // ends; a single `read` may legally return fewer bytes.
        let mut head = Vec::with_capacity(HEADER_WINDOW);
        File::open(first_segment)?
            .take(HEADER_WINDOW as u64)
            .read_to_end(&mut head)?;

        // --- detection --------------------------------------------------
        if head.len() >= 8 && &head[0..7] == b"ADCRYPT" {
            return Err(Ad1Error::Unsupported(format!(
                "ADCRYPT (encrypted AD1) — decryption is out of scope; signature {}",
                hex_preview(&head)
            )));
        }
        if head.len() < 15 || &head[0..15] != b"ADSEGMENTEDFILE" {
            return Err(Ad1Error::NotAd1(format!(
                "expected ADSEGMENTEDFILE, found signature {}",
                hex_preview(&head)
            )));
        }

        // --- segment header ---------------------------------------------
        let segment_count = le_u32(&head, 0x1c).max(1);
        let fragments_size = le_u32(&head, 0x22);
        if fragments_size == 0 {
            return Err(Ad1Error::Malformed(
                "segment header fragments_size is 0".into(),
            ));
        }

        // --- logical header ---------------------------------------------
        let image_version = le_u32(&head, 0x210);
        let chunk_size = le_u32(&head, 0x218);
        let first_item_addr = le_u64(&head, 0x224);
        if chunk_size == 0 || chunk_size > MAX_CHUNK_SIZE {
            return Err(Ad1Error::Malformed(format!(
                "implausible zlib chunk size {chunk_size} (image version {image_version})"
            )));
        }

        let segments = SegmentSet::open(first_segment, segment_count, fragments_size)?;

        let mut entries = Vec::new();
        if first_item_addr != 0 {
            walk_tree(&segments, first_item_addr, &mut entries)?;
        }

        Ok(Self {
            segments,
            image_version,
            chunk_size,
            segment_count,
            entries,
        })
    }

    /// The logical file tree (depth-first, directories before their children).
    #[must_use]
    pub fn entries(&self) -> &[Ad1Entry] {
        &self.entries
    }

    /// AD1 format version recorded in the logical header (commonly 3 or 4).
    #[must_use]
    pub fn image_version(&self) -> u32 {
        self.image_version
    }

    /// Maximum decompressed bytes per zlib data chunk.
    #[must_use]
    pub fn chunk_size(&self) -> u32 {
        self.chunk_size
    }

    /// Number of segments declared by the image header.
    #[must_use]
    pub fn segment_count(&self) -> u32 {
        self.segment_count
    }

    /// 1-based indices of segments the header declares but that are absent on
    /// disk (feeds the `AD1-SEGMENT-MISSING` audit). Empty for a complete image.
    #[must_use]
    pub fn missing_segments(&self) -> Vec<u32> {
        self.segments.missing()
    }

    /// Path of every segment the header declares (`.ad1`, `.ad2`, …), present
    /// or not, in order.
    #[must_use]
    pub fn segment_paths(&self) -> &[PathBuf] {
        self.segments.paths()
    }

    /// Read up to `buf.len()` decompressed bytes of `entry` starting at
    /// `offset`, inflating only the zlib chunks the range overlaps.
    ///
    /// Returns the number of bytes written (0 at or past end of file, or for a
    /// directory).
    ///
    /// # Errors
    /// [`Ad1Error::Malformed`] / [`Ad1Error::Io`] on a corrupt chunk table or
    /// decompression failure.
    pub fn read_at(
        &self,
        entry: &Ad1Entry,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<usize, Ad1Error> {
        if entry.is_dir || entry.size == 0 || offset >= entry.size || buf.is_empty() {
            return Ok(0);
        }
        let want_total = (buf.len() as u64).min(entry.size - offset) as usize;
        if want_total == 0 {
            return Ok(0);
        }
        if entry.zlib_addr == 0 {
            return Err(Ad1Error::Malformed(format!(
                "file '{}' has {} bytes but no chunk table",
                entry.path, entry.size
            )));
        }

        let cs = u64::from(self.chunk_size);
        let count = le_u64(&self.segments.read(entry.zlib_addr, 8)?, 0);
        // A chunk table cannot have more addresses than the image has bytes/8.
        // `count >= max_addrs` is the overflow-safe form of `count + 1 > max_addrs`.
        let max_addrs = self.segments.capacity() / 8 + 2;
        if count == 0 || count >= max_addrs {
            return Err(Ad1Error::Malformed(format!(
                "file '{}' declares implausible chunk count {count}",
                entry.path
            )));
        }
        let end = offset.saturating_add(want_total as u64);
        // Read only the addresses of the chunks the range overlaps: chunk `i`
        // spans `[address[i], address[i + 1])`, so chunks `first..=last` need
        // addresses `first..=last + 1`. A whole-table read per call would make a
        // sequential stream of a large file quadratic.
        let first = offset / cs;
        if first >= count {
            return Ok(0);
        }
        let last = ((end - 1) / cs).min(count - 1);
        let table_len = ((last - first + 2) as usize).saturating_mul(8);
        let table_addr = entry
            .zlib_addr
            .saturating_add(8)
            .saturating_add(first.saturating_mul(8));
        let addr_bytes = self.segments.read(table_addr, table_len)?;
        let addr = |i: u64| le_u64(&addr_bytes, ((i - first).saturating_mul(8)) as usize);

        let mut produced = 0usize;
        let mut cur = offset;
        let mut ci = first;
        while cur < end && ci <= last {
            let (start, stop) = (addr(ci), addr(ci + 1));
            if stop < start {
                return Err(Ad1Error::Malformed(format!(
                    "file '{}' chunk {ci} has non-monotonic addresses",
                    entry.path
                )));
            }
            let comp = self.segments.read(start, (stop - start) as usize)?;
            let raw = inflate(&comp, self.chunk_size as usize)?;
            let chunk_base = ci.saturating_mul(cs);
            let chunk_end = chunk_base.saturating_add(raw.len() as u64);
            // A chunk that inflated to fewer than `cs` bytes leaves a hole before
            // this chunk's base: the declared size lies. Stop with what we have
            // (the auditor flags the short read as AD1-SIZE-LIE).
            if cur < chunk_base {
                break;
            }
            if cur < chunk_end {
                let from = (cur - chunk_base) as usize;
                let n = (raw.len() - from).min((end - cur) as usize);
                buf[produced..produced + n].copy_from_slice(&raw[from..from + n]);
                produced += n;
                cur += n as u64;
            }
            ci += 1;
        }
        Ok(produced)
    }
}

/// Format up to the first 16 bytes of `buf` as lowercase hex for diagnostics.
fn hex_preview(buf: &[u8]) -> String {
    use std::fmt::Write as _;
    let take = buf.len().min(16);
    let mut s = String::with_capacity(take * 2);
    for b in &buf[..take] {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Inflate one zlib chunk, capping output at `max` bytes (decompression-bomb guard).
fn inflate(comp: &[u8], max: usize) -> Result<Vec<u8>, Ad1Error> {
    let mut out = Vec::new();
    ZlibDecoder::new(comp)
        .take(max as u64)
        .read_to_end(&mut out)?;
    Ok(out)
}

/// Read-ahead for the tree walk. An item's header, name and metadata records
/// are a few hundred bytes read field by field; serving them from small
/// windows replaces a read per field with about one read per item. Two
/// windows, replaced least-recently-used, follow both a layout that interleaves
/// each item with its metadata (FTK) and one that stores all items first and
/// all metadata after them. Data chunks are never read through it.
struct ReadAhead<'a> {
    segments: &'a SegmentSet,
    windows: [Window; 2],
    /// Index of the window that served the latest read.
    recent: usize,
}

#[derive(Default)]
struct Window {
    start: u64,
    bytes: Vec<u8>,
}

impl Window {
    fn slice(&self, offset: u64, len: usize) -> Option<&[u8]> {
        let from = usize::try_from(offset.checked_sub(self.start)?).ok()?;
        self.bytes.get(from..from.checked_add(len)?)
    }
}

impl<'a> ReadAhead<'a> {
    const WINDOW: usize = 8 * 1024;

    fn new(segments: &'a SegmentSet) -> Self {
        Self {
            segments,
            windows: [Window::default(), Window::default()],
            recent: 0,
        }
    }

    /// Exactly `len` bytes at logical `offset`, with the same errors as
    /// `SegmentSet::read`.
    fn read(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, Ad1Error> {
        if len > Self::WINDOW {
            return self.segments.read(offset, len);
        }
        for (index, window) in self.windows.iter().enumerate() {
            if let Some(bytes) = window.slice(offset, len) {
                self.recent = index;
                return Ok(bytes.to_vec());
            }
        }
        let readable = self.segments.readable_from(offset);
        if readable < len as u64 {
            // Let the direct read report where the data runs out.
            return self.segments.read(offset, len);
        }
        let fill = readable.min(Self::WINDOW as u64) as usize;
        let victim = 1 - self.recent;
        self.windows[victim] = Window {
            start: offset,
            bytes: self.segments.read(offset, fill)?,
        };
        self.recent = victim;
        self.windows[victim]
            .slice(offset, len)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| {
                Ad1Error::Malformed(format!("read-ahead missed {len} bytes at {offset}"))
            })
    }
}

/// Read the item-header fields at logical `addr`.
fn read_item(view: &mut ReadAhead<'_>, addr: u64) -> Result<RawItem, Ad1Error> {
    let head = view.read(addr, 0x30)?;
    let name_len = le_u32(&head, 0x2c) as usize;
    if name_len > MAX_NAME_LEN {
        return Err(Ad1Error::Malformed(format!(
            "item at {addr:#x} declares name length {name_len} (> {MAX_NAME_LEN})"
        )));
    }
    let name_bytes = view.read(addr + 0x30, name_len)?;
    // Match the reference: map '/' to '_' so names never break path joins.
    let name: String = String::from_utf8_lossy(&name_bytes)
        .chars()
        .map(|c| if c == '/' { '_' } else { c })
        .collect();
    Ok(RawItem {
        next_item_addr: le_u64(&head, 0x00),
        first_child_addr: le_u64(&head, 0x08),
        first_metadata_addr: le_u64(&head, 0x10),
        zlib_metadata_addr: le_u64(&head, 0x18),
        decompressed_size: le_u64(&head, 0x20),
        item_type: le_u32(&head, 0x28),
        name,
    })
}

/// Metadata fields collected for one item.
#[derive(Default)]
struct Meta {
    md5: Option<String>,
    sha1: Option<String>,
    modified: Option<String>,
    accessed: Option<String>,
    changed: Option<String>,
}

/// Walk an item's metadata linked list, collecting the fields we surface.
fn read_metadata(view: &mut ReadAhead<'_>, first_addr: u64) -> Result<Meta, Ad1Error> {
    let mut meta = Meta::default();
    let mut addr = first_addr;
    let mut seen = HashSet::new();
    let mut count = 0usize;
    while addr != 0 {
        if !seen.insert(addr) {
            break; // cycle in the metadata chain — stop, keep what we have
        }
        count += 1;
        // cov:unreachable in unit tests — DoS backstop against an unbounded
        // metadata chain; exercised by the fuzz target rather than a contrived
        // 4097-record fixture.
        if count > MAX_META_RECORDS {
            return Err(Ad1Error::Malformed(format!(
                "metadata chain exceeds {MAX_META_RECORDS} records"
            )));
        }
        let h = view.read(addr, 0x14)?;
        let next = le_u64(&h, 0x00);
        let category = le_u32(&h, 0x08);
        let key = le_u32(&h, 0x0c);
        let dlen = le_u32(&h, 0x10) as usize;
        if dlen > MAX_META_DATA {
            return Err(Ad1Error::Malformed(format!(
                "metadata record at {addr:#x} declares data length {dlen} (> {MAX_META_DATA})"
            )));
        }
        let data = view.read(addr + 0x14, dlen)?;
        let as_str = || {
            String::from_utf8_lossy(&data)
                .trim_end_matches('\0')
                .to_string()
        };
        match (category, key) {
            (0x01, 0x5001) => meta.md5 = Some(as_str()),
            (0x01, 0x5002) => meta.sha1 = Some(as_str()),
            (0x05, 0x07) => meta.accessed = Some(as_str()),
            (0x05, 0x08) => meta.modified = Some(as_str()),
            (0x05, 0x09) => meta.changed = Some(as_str()),
            _ => {}
        }
        addr = next;
    }
    Ok(meta)
}

/// A pending tree-walk step: an item address plus its parent's path and index.
type WalkStep = (u64, Option<String>, Option<usize>);

/// Walk the file tree from `first_item_addr`, producing entries in DFS preorder.
///
/// Iterative (explicit stack) so a deep or wide tree cannot overflow the call
/// stack, with a visited-set guard against cyclic `next`/`child` pointers.
fn walk_tree(
    seg: &SegmentSet,
    first_item_addr: u64,
    entries: &mut Vec<Ad1Entry>,
) -> Result<(), Ad1Error> {
    let mut view = ReadAhead::new(seg);
    let mut stack: Vec<WalkStep> = vec![(first_item_addr, None, None)];
    let mut seen = HashSet::new();
    while let Some((addr, parent_path, parent)) = stack.pop() {
        if addr == 0 {
            continue;
        }
        if !seen.insert(addr) {
            return Err(Ad1Error::Malformed(format!(
                "tree cycle: item at {addr:#x} visited twice"
            )));
        }
        // cov:unreachable in unit tests — DoS backstop against an unbounded tree;
        // exercised by the fuzz target rather than a 5M-node fixture.
        if entries.len() >= MAX_ENTRIES {
            return Err(Ad1Error::Malformed(format!(
                "tree exceeds {MAX_ENTRIES} entries"
            )));
        }
        let item = read_item(&mut view, addr)?;
        let path = match &parent_path {
            None => item.name.clone(),
            Some(p) => format!("{p}/{}", item.name),
        };
        let is_dir = item.item_type == ITEM_TYPE_FOLDER;
        let meta = read_metadata(&mut view, item.first_metadata_addr)?;
        let index = entries.len();
        entries.push(Ad1Entry {
            path: path.clone(),
            is_dir,
            size: item.decompressed_size,
            item_type: item.item_type,
            md5: meta.md5,
            sha1: meta.sha1,
            modified: meta.modified,
            accessed: meta.accessed,
            changed: meta.changed,
            parent,
            zlib_addr: item.zlib_metadata_addr,
        });
        // Push sibling first, then child, so the child subtree is emitted first.
        if item.next_item_addr != 0 {
            stack.push((item.next_item_addr, parent_path.clone(), parent));
        }
        if item.first_child_addr != 0 {
            stack.push((item.first_child_addr, Some(path), Some(index)));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_is_the_documented_string() {
        assert_eq!(&AD1_SEGMENTED_MARKER[..15], b"ADSEGMENTEDFILE");
    }

    #[test]
    fn open_missing_file_is_io_error() {
        assert!(matches!(
            Ad1Reader::open(Path::new("/nonexistent.ad1")),
            Err(Ad1Error::Io(_))
        ));
    }

    #[test]
    fn hex_preview_caps_at_16_bytes() {
        let buf = [0xabu8; 32];
        assert_eq!(hex_preview(&buf).len(), 32); // 16 bytes -> 32 hex chars
    }
}
