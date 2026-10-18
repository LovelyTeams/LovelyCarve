use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use flate2::read::ZlibDecoder;
use lru::LruCache;
use safe_read::{le_u32, le_u64};

use crate::chunk_table::{
    parse_table_section, ChunkTable, LazyChunkTable, SectionMeta, TableSectionRef,
    DEFAULT_SECTION_CACHE,
};
use crate::error::{EwfError, Result};
use crate::ewf2;
use crate::parse::{parse_error2_data, parse_header_text};
use crate::sections::{
    Chunk, EwfFileHeader, EwfVolume, SectionDescriptor, DEFAULT_LRU_SIZE, FILE_HEADER_SIZE,
    SECTION_DESCRIPTOR_SIZE,
};
use crate::segment_source::{SegmentCursor, SegmentSource};
#[cfg(feature = "verify")]
use crate::types::VerifyResult;
use crate::types::{AcquisitionError, EwfMetadata, StoredHashes};

#[path = "reader/bulk.rs"]
mod bulk;

// ---------------------------------------------------------------------------
// Positioned read (thread-safe, cursor-free)
// ---------------------------------------------------------------------------

/// Fill `buf` from `file` starting at `offset`, returning the bytes read (short
/// only at end of file).
///
/// Uses the OS positioned-read primitive — `pread(2)` on Unix, `seek_read`
/// (a `ReadFile` carrying its own `OVERLAPPED` offset) on Windows — so it takes
/// `&File` and never touches a shared cursor. That makes it safe to call
/// concurrently from many threads on one handle: each call carries its own
/// offset, so there is no read/seek race. Keeps `forbid(unsafe)` (no mmap).
pub(crate) fn pread(file: &File, buf: &mut [u8], offset: u64) -> io::Result<usize> {
    #[cfg(unix)]
    use std::os::unix::fs::FileExt;
    #[cfg(windows)]
    use std::os::windows::fs::FileExt;

    let mut total = 0usize;
    while total < buf.len() {
        #[cfg(unix)]
        let res = file.read_at(&mut buf[total..], offset + total as u64);
        #[cfg(windows)]
        let res = file.seek_read(&mut buf[total..], offset + total as u64);
        match res {
            Ok(0) => break,
            Ok(n) => total += n,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(total)
}

// ---------------------------------------------------------------------------
// Segment file discovery
// ---------------------------------------------------------------------------

/// Discover all segment files for an EWF image (E01, L01, Ex01, or Lx01).
///
/// Detects the extension prefix from the input path:
/// - 3-char (v1): `.E01`..`.EZZ`, `.L01`..`.LZZ`
/// - 4-char (v2): `.Ex01`..`.EzZZ`, `.Lx01`..`.LzZZ`
///
/// The directory to glob for sibling segment files of `first`.
///
/// `Path::parent()` returns `Some("")` — not `None` — for a bare filename, so a
/// naive `unwrap_or_else(|| ".")` leaves an empty directory and roots the glob at
/// the filesystem root. Map both the empty and missing cases to the current
/// directory so `ingest <bare.E01>` works from the evidence directory.
fn segment_dir(first: &Path) -> &Path {
    match first.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

/// Returns paths sorted by expected segment order.
pub fn discover_segments(first: &Path) -> Result<Vec<PathBuf>> {
    let stem = first
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| EwfError::NoSegments(first.display().to_string()))?;
    let parent = segment_dir(first);

    // `Path::extension()` yields `Some("")` — not `None` — for a name ending in
    // a bare `.`, which would slip past the `unwrap_or` fallback and leave an
    // empty extension. Treat an empty extension as absent, which is what the
    // E01 default is for.
    let ext = first
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty())
        .unwrap_or("E01");

    if !matches!(
        ext.to_ascii_lowercase().as_str(),
        "e01" | "l01" | "s01" | "ex01" | "lx01"
    ) {
        return Ok(vec![first.to_path_buf()]);
    }
    let escaped_stem = glob::Pattern::escape(stem);
    let parent_str = glob::Pattern::escape(&parent.to_string_lossy());
    let mut paths: Vec<PathBuf> = Vec::new();

    // `ext` is non-empty by construction above, so the default never fires.
    let prefix = ext.chars().next().map_or('E', |c| c.to_ascii_uppercase());
    let lc = prefix.to_ascii_lowercase();

    if ext.len() == 4 {
        // EWF2: 4-char extensions like Ex01, Lx01
        for pattern in &[
            format!("{parent_str}/{escaped_stem}.[{prefix}{lc}][x-zX-Z][0-9][0-9]"),
            format!("{parent_str}/{escaped_stem}.[{prefix}{lc}][x-zX-Z][A-Za-z][A-Za-z]"),
        ] {
            if let Ok(entries) = glob::glob(pattern) {
                paths.extend(entries.filter_map(std::result::Result::ok));
            }
        }
    } else {
        // EWF v1: 3-char extensions like E01, L01
        for pattern in &[
            format!("{parent_str}/{escaped_stem}.[{prefix}{lc}][0-9][0-9]"),
            format!("{parent_str}/{escaped_stem}.[{prefix}{lc}][A-Za-z][A-Za-z]"),
        ] {
            if let Ok(entries) = glob::glob(pattern) {
                paths.extend(entries.filter_map(std::result::Result::ok));
            }
        }
    }

    if paths.is_empty() {
        return Err(EwfError::NoSegments(first.display().to_string()));
    }

    // Sort by extension for natural segment order
    paths.sort_by(|a, b| {
        let ext_a = a.extension().and_then(|e| e.to_str()).unwrap_or("");
        let ext_b = b.extension().and_then(|e| e.to_str()).unwrap_or("");
        ext_a.to_ascii_uppercase().cmp(&ext_b.to_ascii_uppercase())
    });

    Ok(paths)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Maximum section data size we'll read into memory (`DoS` guard).
const MAX_SECTION_DATA_SIZE: u64 = 1_000_000;

/// Maximum bytes accepted from a zlib-decompressed stream (deflate-bomb guard).
/// EWF header metadata is plain text; 10 MB is already extremely generous.
const MAX_DECOMPRESSED_SIZE: u64 = 10 * MAX_SECTION_DATA_SIZE;

/// Maximum entries in a SINGLE table section (`DoS` guard on the per-section
/// `entries_buf` allocation: 4M × 4 B = 16 MB). This is NOT an image-size limit —
/// an image's chunks are spread across many table sections, and the total chunk
/// count is bounded by real file bytes via `read_exact`, not by this constant.
/// (The former `MAX_CHUNK_COUNT` rejected `vol.chunk_count > 4M`, i.e. any image
/// over 4M × 32 KB = 128 GB — a too-low ceiling from a 1000× units slip in its
/// "128 TB" comment. Removed: the header count is an untrusted hint, not a limit.)
const MAX_TABLE_ENTRIES: usize = 4_000_000;

/// Maximum chunk size in bytes. EWF typically uses 32 KB; 128 MB is a generous cap.
const MAX_CHUNK_SIZE: u64 = 128 * 1024 * 1024;

/// Default EWF2 chunk size when `device_info` is absent or unparseable.
const DEFAULT_V2_CHUNK_SIZE: u64 = 32768;

/// Validate that segment numbers are sequential (1, 2, 3, ...) and reorder the
/// backing segments to match. Generic over the segment type so the v1 path can
/// reorder [`SegmentSource`]s and the v2 path raw `File`s through one routine.
pub(crate) fn validate_and_reorder_segments<T>(
    segments: Vec<T>,
    segment_numbers: Vec<u32>,
) -> Result<Vec<T>> {
    let mut indexed: Vec<(usize, u32)> = segment_numbers.into_iter().enumerate().collect();
    indexed.sort_by_key(|&(_, seg)| seg);

    // Validate sequential segment numbers (1, 2, 3, ...)
    for (expected_pos, &(_, seg_num)) in indexed.iter().enumerate() {
        let expected = (expected_pos + 1) as u32;
        if seg_num != expected {
            return Err(EwfError::SegmentGap {
                expected,
                got: seg_num,
            });
        }
    }

    // Reorder backing segments to match segment order
    let mut slots: Vec<Option<T>> = segments.into_iter().map(Some).collect();
    let mut ordered = Vec::with_capacity(slots.len());
    for &(idx, _) in &indexed {
        if let Some(seg) = slots[idx].take() {
            ordered.push(seg);
        }
    }
    Ok(ordered)
}

// ---------------------------------------------------------------------------
// EWF2 helpers
// ---------------------------------------------------------------------------

/// Walk the EWF2 backward-linked section list and return descriptors in
/// forward (file) order.
///
/// EWF2 layout per section: `[data bytes][descriptor 64 B]`.  The terminal
/// section (Done/Next) sits at the very end of the file with `data_size = 0`.
/// Each descriptor's `previous_offset` is the absolute file offset of the
/// preceding descriptor; the first section has `previous_offset = 0`.
fn collect_ewf2_descriptors(
    file: &mut File,
    file_len: u64,
) -> Result<Vec<ewf2::Ewf2SectionDescriptor>> {
    const DS: u64 = ewf2::SECTION_DESCRIPTOR_SIZE as u64;
    if file_len < DS {
        return Ok(Vec::new());
    }
    let mut descriptors = Vec::new();
    let mut visited = std::collections::HashSet::new();
    let mut desc_offset = file_len - DS;

    loop {
        if !visited.insert(desc_offset) {
            return Err(EwfError::Parse(
                "EWF2 section descriptor list contains a cycle".to_string(),
            ));
        }
        file.seek(SeekFrom::Start(desc_offset))?;
        let mut buf = [0u8; ewf2::SECTION_DESCRIPTOR_SIZE];
        file.read_exact(&mut buf)?;
        let desc = ewf2::Ewf2SectionDescriptor::parse(&buf, desc_offset)?;
        let prev = desc.previous_offset;
        descriptors.push(desc);
        if prev == 0 {
            break;
        }
        if prev >= file_len {
            return Err(EwfError::Parse(format!(
                "EWF2 previous_offset {prev:#x} exceeds file length {file_len:#x}"
            )));
        }
        desc_offset = prev;
    }

    descriptors.reverse();
    Ok(descriptors)
}

/// Return a zlib-decompressed copy of `raw` when it starts with a zlib magic
/// byte (`0x78`), otherwise return a copy of the raw bytes unchanged.
///
/// Reads at most `MAX_DECOMPRESSED_SIZE` bytes to guard against deflate bombs.
fn maybe_zlib_decompress(raw: &[u8]) -> Result<Vec<u8>> {
    if raw.len() >= 2 && raw[0] == 0x78 {
        let mut out = Vec::with_capacity(raw.len() * 4);
        ZlibDecoder::new(raw)
            .take(MAX_DECOMPRESSED_SIZE)
            .read_to_end(&mut out)
            .map_err(|e| EwfError::Parse(format!("EWF2 zlib decompress failed: {e}")))?;
        Ok(out)
    } else {
        Ok(raw.to_vec())
    }
}

// ---------------------------------------------------------------------------
// EwfReader - main public API
// ---------------------------------------------------------------------------

/// A reader for Expert Witness Format (E01/EWF) forensic disk images.
///
/// Implements `Read` and `Seek` over the logical disk image stored across
/// one or more `.E01`/`.E02`/... segment files.
///
/// # Example
/// ```no_run
/// use std::io::Read;
/// let mut reader = ewf::EwfReader::open("disk.E01").unwrap();
/// let mut buf = [0u8; 512];
/// reader.read_exact(&mut buf).unwrap(); // read first sector
/// ```
pub struct EwfReader {
    compression_method: ewf2::CompressionMethod,
    is_v2: bool,
    // Note: LruCache does not implement Debug, so we cannot derive Debug.
    // We provide a manual impl below.
    /// Ordered segment sources (loose file, in-archive sub-range, or in-RAM
    /// buffer). EWF v2 always populates this with [`SegmentSource::File`].
    segments: Vec<SegmentSource>,
    /// Chunk table: chunk[i] covers logical bytes [i*`chunk_size`, (i+1)*`chunk_size`).
    /// Either eager (flat vec, parsed at open) or lazy (paged section index).
    chunks: ChunkTable,
    /// Chunk size in bytes (typically 32 KB).
    chunk_size: u64,
    /// Total logical image size in bytes.
    total_size: u64,
    /// Current read position (for Read + Seek).
    position: u64,
    /// LRU cache: `chunk_id` -> decompressed chunk data. `Mutex`-guarded so the
    /// reader can serve positioned reads through a shared `&self` from many
    /// threads — the cache is the only interior mutation on the read path.
    cache: Mutex<LruCache<usize, Vec<u8>>>,
    /// MD5 from hash/digest section (16 bytes), if present.
    stored_md5: Option<[u8; 16]>,
    /// SHA-1 from digest section (20 bytes), if present.
    stored_sha1: Option<[u8; 20]>,
    /// Case and acquisition metadata from header sections.
    metadata: EwfMetadata,
    /// Sectors with read errors during acquisition (from error2 section).
    acquisition_errors: Vec<AcquisitionError>,
}

impl EwfReader {
    /// Open an EWF image from a path to the first segment file (e.g. `image.E01`).
    ///
    /// Automatically discovers and opens all additional segment files.
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let paths = discover_segments(path.as_ref())?;
        Self::open_segments(&paths)
    }

    /// Open an EWF image with a custom LRU cache size.
    ///
    /// `cache_size` is the number of decompressed chunks to keep in memory.
    /// Each chunk is typically 32 KB, so 100 chunks ≈ 3.2 MB, 1000 ≈ 32 MB.
    pub fn open_with_cache_size<P: AsRef<Path>>(path: P, cache_size: usize) -> Result<Self> {
        let paths = discover_segments(path.as_ref())?;
        Self::open_segments_with_cache_size(&paths, cache_size)
    }

    /// Open an EWF image from explicit segment file paths (must be in order).
    pub fn open_segments(paths: &[PathBuf]) -> Result<Self> {
        Self::open_segments_with_cache_size(paths, DEFAULT_LRU_SIZE)
    }

    /// Open from explicit segment paths with a custom LRU cache size.
    pub fn open_segments_with_cache_size(paths: &[PathBuf], cache_size: usize) -> Result<Self> {
        if paths.is_empty() {
            return Err(EwfError::NoSegments("empty path list".into()));
        }

        // Peek at the first 8 bytes to determine format version
        {
            let mut probe = File::open(&paths[0])?;
            let mut sig = [0u8; 8];
            probe.read_exact(&mut sig)?;
            if sig == ewf2::EVF2_SIGNATURE || sig == ewf2::LEF2_SIGNATURE {
                return Self::open_segments_v2(paths, cache_size);
            }
        }

        // EWF v1: each loose segment file becomes a `SegmentSource::File`.
        let sources = paths
            .iter()
            .map(|p| {
                File::open(p)
                    .map(SegmentSource::File)
                    .map_err(EwfError::from)
            })
            .collect::<Result<Vec<_>>>()?;
        Self::open_v1(sources, cache_size, None)
    }

    /// Open an EWF v1 image from explicit [`SegmentSource`]s with an EAGER chunk
    /// table. Each source may be a loose file, an in-archive sub-range, or an
    /// in-RAM buffer — sources are ordered by their parsed EWF segment number.
    ///
    /// EWF v2 (Ex01/Lx01) sources are not supported here (the v2 path is
    /// File/path-only for now); a v2 signature is rejected with a clear error.
    pub fn open_from_sources(sources: Vec<SegmentSource>) -> Result<Self> {
        Self::open_v1(sources, DEFAULT_LRU_SIZE, None)
    }

    /// Open an EWF v1 image from explicit [`SegmentSource`]s with a LAZY (paged)
    /// chunk table. The source-backed counterpart to [`open_lazy`](Self::open_lazy).
    pub fn open_lazy_from_sources(sources: Vec<SegmentSource>) -> Result<Self> {
        Self::open_v1(sources, DEFAULT_LRU_SIZE, Some(DEFAULT_SECTION_CACHE))
    }

    /// Unified EWF v1 open over [`SegmentSource`] backings.
    ///
    /// `lazy_section_cache` selects the chunk-table strategy: `None` builds the
    /// EAGER flat table (every entry parsed up front); `Some(n)` builds the LAZY
    /// paged index keeping `n` parsed table sections resident. Both share this
    /// one descriptor-walk + section-handling body, so eager and lazy stay in
    /// lockstep and the three source kinds are exercised identically.
    fn open_v1(
        sources: Vec<SegmentSource>,
        cache_size: usize,
        lazy_section_cache: Option<usize>,
    ) -> Result<Self> {
        if sources.is_empty() {
            return Err(EwfError::NoSegments("empty source list".into()));
        }

        // Reject an EWF2 source: the v2 path is path/File-only for now.
        {
            let mut sig = [0u8; 8];
            sources[0].read_at(&mut sig, 0)?;
            if sig == ewf2::EVF2_SIGNATURE || sig == ewf2::LEF2_SIGNATURE {
                return Err(EwfError::Parse(
                    "EWF2 (Ex01/Lx01) is not supported via SegmentSource; use open(path)".into(),
                ));
            }
        }

        // Parse each source's 13-byte file header for its segment number.
        let mut headers = Vec::with_capacity(sources.len());
        for src in &sources {
            let mut hdr_buf = [0u8; FILE_HEADER_SIZE];
            src.read_at(&mut hdr_buf, 0)?;
            headers.push(EwfFileHeader::parse(&hdr_buf)?);
        }
        let segment_numbers: Vec<u32> = headers
            .iter()
            .map(|h| u32::from(h.segment_number))
            .collect();
        let ordered_segments = validate_and_reorder_segments(sources, segment_numbers)?;

        // Walk section descriptors in each segment
        let mut chunk_size: u64 = 0;
        let mut total_size: u64 = 0;
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut index: Vec<SectionMeta> = Vec::new();
        let mut next_chunk_id: usize = 0;
        let lazy = lazy_section_cache.is_some();
        let mut stored_md5: Option<[u8; 16]> = None;
        let mut stored_sha1: Option<[u8; 20]> = None;
        let mut metadata = EwfMetadata::default();
        let mut acquisition_errors: Vec<AcquisitionError> = Vec::new();

        for (seg_idx, source) in ordered_segments.iter().enumerate() {
            // A mutable cursor over this source lets the existing seek/read_exact
            // descriptor-walk code run unchanged over any of the three backings.
            let mut file = SegmentCursor::new(source);
            let mut desc_offset: u64 = FILE_HEADER_SIZE as u64;
            let mut descriptors = Vec::new();

            let file_len = file.seek(SeekFrom::End(0))?;
            while let Some(chain_end) = desc_offset.checked_add(SECTION_DESCRIPTOR_SIZE as u64) {
                // checked_add handles u64::MAX overflow — loop ends if offset wraps.
                if chain_end > file_len {
                    log::debug!("truncated chain at {desc_offset}, EOF {file_len}");
                    break;
                }

                file.seek(SeekFrom::Start(desc_offset))?;
                let mut desc_buf = [0u8; SECTION_DESCRIPTOR_SIZE];
                file.read_exact(&mut desc_buf)?;
                let desc = SectionDescriptor::parse(&desc_buf, desc_offset)?;
                let next = desc.next;
                descriptors.push(desc);

                if next == 0 || next <= desc_offset {
                    break;
                }
                desc_offset = next;
            }

            // Prefer "table" over "table2"
            let has_table = descriptors.iter().any(|d| d.section_type == "table");
            let table_type = if has_table { "table" } else { "table2" };

            for desc in &descriptors {
                let sectors_data_end = crate::v1_tables::data_end(desc, &descriptors, file_len)?;
                match desc.section_type.as_str() {
                    "volume" | "disk" => {
                        let mut vol_buf = [0u8; 94];
                        file.seek(SeekFrom::Start(
                            desc.offset + SECTION_DESCRIPTOR_SIZE as u64,
                        ))?;
                        file.read_exact(&mut vol_buf)?;
                        let vol = EwfVolume::parse(&vol_buf)?;
                        let cs = vol.chunk_size();
                        if cs > MAX_CHUNK_SIZE {
                            return Err(EwfError::InvalidChunkSize(
                                cs.min(u64::from(u32::MAX)) as u32
                            ));
                        }
                        metadata.bytes_per_sector = Some(vol.bytes_per_sector);
                        metadata.media_type = Some(vol.media_type);
                        metadata.compression = Some(match vol_buf[52] {
                            0 => "无压缩".into(),
                            1 => "快速压缩".into(),
                            2 => "最佳压缩".into(),
                            level => format!("级别 {level}"),
                        });
                        chunk_size = cs;
                        total_size = vol.total_size();
                        if total_size == 0 {
                            total_size = chunk_size * u64::from(vol.chunk_count);
                        }
                        // `vol.chunk_count` is an UNTRUSTED header hint, NOT a hard
                        // limit: a real 2 TB image has ~67M chunks (well past any
                        // fixed ceiling). Bound the up-front reservation by what this
                        // segment file can physically describe — every chunk costs at
                        // least a 4-byte table entry, so chunk_count <= file_len / 4.
                        // A forged giant count thus reserves only what the bytes allow
                        // (no OOM); the table sections (each bounded by `read_exact`)
                        // remain the source of truth for the real chunk count.
                        // (Eager only — the lazy index never holds the flat table.)
                        if !lazy {
                            let plausible_max = (file.segment_len() / 4) as usize;
                            chunks.reserve((vol.chunk_count as usize).min(plausible_max));
                        }
                    }
                    t if t == table_type => {
                        if lazy {
                            // LIGHT pass: read only the 24-byte header (entry_count
                            // + base_offset); per-entry bytes are NOT read here.
                            let section_ref = TableSectionRef {
                                desc_offset: desc.offset,
                                sectors_data_end,
                            };
                            let meta = section_ref.read_header(
                                source,
                                seg_idx,
                                next_chunk_id,
                                MAX_TABLE_ENTRIES,
                            )?;
                            next_chunk_id += meta.entry_count;
                            index.push(meta);
                        } else {
                            let desc_offset = desc.offset;
                            file.seek(SeekFrom::Start(
                                desc_offset + SECTION_DESCRIPTOR_SIZE as u64,
                            ))?;
                            let mut tbl_hdr = [0u8; 24];
                            file.read_exact(&mut tbl_hdr)?;

                            let entry_count = le_u32(&tbl_hdr, 0) as usize;
                            if entry_count > MAX_TABLE_ENTRIES {
                                return Err(EwfError::Parse(format!(
                                    "table entry count {entry_count} exceeds maximum {MAX_TABLE_ENTRIES}"
                                )));
                            }
                            let base_offset = le_u64(&tbl_hdr, 8);

                            let entries_offset = desc_offset + SECTION_DESCRIPTOR_SIZE as u64 + 24;
                            file.seek(SeekFrom::Start(entries_offset))?;
                            let mut entries_buf = vec![0u8; entry_count * 4];
                            file.read_exact(&mut entries_buf)?;

                            // Parse this section's entries via the SHARED routine,
                            // so the eager table is byte-identical to the lazy path.
                            let section = parse_table_section(
                                &entries_buf,
                                entry_count,
                                base_offset,
                                seg_idx,
                                chunk_size,
                                sectors_data_end,
                            )?;
                            chunks.extend(section);
                        }
                    }
                    "hash" => {
                        let data_offset = desc.offset + SECTION_DESCRIPTOR_SIZE as u64;
                        file.seek(SeekFrom::Start(data_offset))?;
                        let mut hash_buf = [0u8; 16];
                        file.read_exact(&mut hash_buf)?;
                        if stored_md5.is_none() {
                            stored_md5 = Some(hash_buf);
                        }
                        log::debug!("parsed hash section: MD5 = {hash_buf:02x?}");
                    }
                    "digest" => {
                        let data_offset = desc.offset + SECTION_DESCRIPTOR_SIZE as u64;
                        file.seek(SeekFrom::Start(data_offset))?;
                        let mut digest_buf = [0u8; 36];
                        file.read_exact(&mut digest_buf)?;
                        let mut md5 = [0u8; 16];
                        let mut sha1 = [0u8; 20];
                        md5.copy_from_slice(&digest_buf[0..16]);
                        sha1.copy_from_slice(&digest_buf[16..36]);
                        // EnCase writes the digest section with the fields it
                        // did not compute left as ZERO. An all-zero digest is
                        // therefore an ABSENCE, not a value: storing it as
                        // Some(..) makes every later comparison fail against a
                        // hash the acquisition never recorded, which reports a
                        // sound image as mismatched.
                        stored_md5 = (md5 != [0u8; 16]).then_some(md5);
                        stored_sha1 = (sha1 != [0u8; 20]).then_some(sha1);
                        log::debug!("parsed digest section: MD5 = {md5:02x?}, SHA-1 = {sha1:02x?}");
                    }
                    "header" | "header2" => {
                        let data_offset = desc.offset + SECTION_DESCRIPTOR_SIZE as u64;
                        let data_size = desc
                            .section_size
                            .saturating_sub(SECTION_DESCRIPTOR_SIZE as u64);
                        if data_size > 0 && data_size < MAX_SECTION_DATA_SIZE {
                            file.seek(SeekFrom::Start(data_offset))?;
                            let mut compressed = vec![0u8; data_size as usize];
                            file.read_exact(&mut compressed)?;
                            // Limit decompressed output — a crafted stream could expand 1 MB
                            // compressed input into gigabytes (deflate bomb).
                            let mut decompressed = Vec::new();
                            let mut limited = std::io::Read::take(
                                flate2::read::ZlibDecoder::new(&compressed[..]),
                                MAX_DECOMPRESSED_SIZE,
                            );
                            if std::io::Read::read_to_end(&mut limited, &mut decompressed).is_ok() {
                                let text = if desc.section_type == "header2" {
                                    crate::acquisition::utf16_text(&decompressed)
                                } else {
                                    String::from_utf8_lossy(&decompressed).into_owned()
                                };
                                parse_header_text(&text, &mut metadata);
                            }
                        }
                    }
                    "error2" => {
                        let data_offset = desc.offset + SECTION_DESCRIPTOR_SIZE as u64;
                        let data_size = desc
                            .section_size
                            .saturating_sub(SECTION_DESCRIPTOR_SIZE as u64);
                        if data_size > 0 && data_size < MAX_SECTION_DATA_SIZE {
                            file.seek(SeekFrom::Start(data_offset))?;
                            let mut buf = vec![0u8; data_size as usize];
                            file.read_exact(&mut buf)?;
                            acquisition_errors.extend(parse_error2_data(&buf));
                            log::debug!(
                                "parsed error2 section: {} entries",
                                acquisition_errors.len()
                            );
                        }
                    }
                    _ => {}
                }
            }
        }

        if chunk_size == 0 {
            return Err(EwfError::MissingVolume);
        }

        let chunks = match lazy_section_cache {
            Some(section_cache) => {
                ChunkTable::Lazy(LazyChunkTable::new(index, chunk_size, section_cache))
            }
            None => ChunkTable::Eager(chunks),
        };

        // `unwrap_or(MIN)` expresses the same clamp as the former
        // `.max(1).unwrap()`, but as a total conversion: a 0 cache size becomes
        // A LOGICAL container's volume section does not describe the media.
        // The spec records sector_size x sector_count != total_size for EWF-L01;
        // observed on a real EnCase 8.08 acquisition, the declaration is 2 GiB
        // against a chunk table describing 11.9 GB. Trusting it caps reads at
        // 2 GiB and loses most of the evidence with no error raised.
        let kind = headers
            .first()
            .map_or(crate::sections::EwfKind::Physical, |h| h.kind);
        let total_size =
            crate::logical::media_size_for(kind, total_size, chunks.len() as u64, chunk_size);

        // 1 by the type's own floor rather than by a runtime assertion.
        let cache = Mutex::new(LruCache::new(
            std::num::NonZeroUsize::new(cache_size).unwrap_or(std::num::NonZeroUsize::MIN),
        ));

        Ok(Self {
            compression_method: ewf2::CompressionMethod::Zlib,
            is_v2: false,
            segments: ordered_segments,
            chunks,
            chunk_size,
            total_size,
            position: 0,
            cache,
            stored_md5,
            stored_sha1,
            metadata,
            acquisition_errors,
        })
    }

    /// Open an EWF v1 image with a LAZY (paged, on-demand) chunk table.
    ///
    /// Identical bytes to [`open`](Self::open) — the lazy table parses each
    /// `table`/`table2` section's per-entry bytes only when a chunk in it is
    /// first read, via the SAME `parse_table_section` routine (crate-private) the eager path
    /// uses. Trades a small per-read section lookup (binary search + an
    /// occasional section parse on cache miss) for not holding the full
    /// `chunk_count * size_of::<Chunk>()` table resident.
    ///
    /// EWF v2 (Ex01/Lx01) images fall back to the eager path: lazy is v1-only.
    pub fn open_lazy<P: AsRef<Path>>(path: P) -> Result<Self> {
        let paths = discover_segments(path.as_ref())?;
        Self::open_segments_lazy(&paths, DEFAULT_LRU_SIZE, DEFAULT_SECTION_CACHE)
    }

    /// Open EWF v1 from explicit segment paths with a lazy chunk table.
    ///
    /// `cache_size` is the decompressed-chunk LRU (as elsewhere); `section_cache`
    /// is how many parsed table sections the lazy index keeps resident. EWF2
    /// sources fall back to the eager v2 open (lazy is v1-only).
    pub fn open_segments_lazy(
        paths: &[PathBuf],
        cache_size: usize,
        section_cache: usize,
    ) -> Result<Self> {
        if paths.is_empty() {
            return Err(EwfError::NoSegments("empty path list".into()));
        }

        // Version probe: EWF2 has no lazy path yet — defer to the eager v2 open.
        {
            let mut probe = File::open(&paths[0])?;
            let mut sig = [0u8; 8];
            probe.read_exact(&mut sig)?;
            if sig == ewf2::EVF2_SIGNATURE || sig == ewf2::LEF2_SIGNATURE {
                return Self::open_segments_v2(paths, cache_size);
            }
        }

        let sources = paths
            .iter()
            .map(|p| {
                File::open(p)
                    .map(SegmentSource::File)
                    .map_err(EwfError::from)
            })
            .collect::<Result<Vec<_>>>()?;
        Self::open_v1(sources, cache_size, Some(section_cache))
    }

    /// Open EWF2 (Ex01/Lx01) segments.
    fn open_segments_v2(paths: &[PathBuf], cache_size: usize) -> Result<Self> {
        // Open all segment files and parse v2 headers
        let mut segments = Vec::with_capacity(paths.len());
        let mut v2_headers = Vec::with_capacity(paths.len());
        for path in paths {
            let mut f = File::open(path)?;
            let mut hdr_buf = [0u8; ewf2::FILE_HEADER_SIZE];
            f.read_exact(&mut hdr_buf)?;
            v2_headers.push(ewf2::Ewf2FileHeader::parse(&hdr_buf)?);
            segments.push(f);
        }

        let compression_method = v2_headers
            .first()
            .map_or(ewf2::CompressionMethod::Zlib, |header| {
                header.compression_method
            });
        if v2_headers.iter().any(|header| {
            header.compression_method != compression_method
                || header.set_identifier != v2_headers[0].set_identifier
        }) {
            return Err(EwfError::Parse(
                "inconsistent EWF2 segment set or compression methods".into(),
            ));
        }
        let mut ordered_segments = validate_and_reorder_segments(
            segments,
            v2_headers.iter().map(|h| h.segment_number).collect(),
        )?;

        let mut chunk_size: u64 = 0;
        let mut total_size: u64 = 0;
        let mut chunks: Vec<Chunk> = Vec::new();
        let mut stored_md5: Option<[u8; 16]> = None;
        let mut stored_sha1: Option<[u8; 20]> = None;
        let mut metadata = EwfMetadata::default();
        let acquisition_errors: Vec<AcquisitionError> = Vec::new();
        metadata.compression = Some(format!("{compression_method:?}"));

        for (seg_idx, file) in ordered_segments.iter_mut().enumerate() {
            let file_len = file.seek(SeekFrom::End(0))?;

            // EWF2 uses a backward-linked list: each section is [data][descriptor].
            // Traverse from the terminal Done/Next descriptor at the end of the file
            // backward via `previous_offset`, then process descriptors forward.
            let descriptors = collect_ewf2_descriptors(file, file_len)?;

            for desc in &descriptors {
                if desc.is_encrypted() {
                    return Err(EwfError::EncryptedNotSupported);
                }

                // Section data immediately precedes the descriptor:
                //   data_offset = desc.offset - desc.data_size
                match desc.section_type {
                    ewf2::Ewf2SectionType::CaseData
                        if desc.data_size > 0
                            && desc.data_size < MAX_SECTION_DATA_SIZE
                            && metadata.case_number.is_none() =>
                    {
                        let data_offset =
                            desc.offset.checked_sub(desc.data_size).ok_or_else(|| {
                                EwfError::Parse(format!(
                                    "EWF2 case_data offset underflow: desc={:#x} size={:#x}",
                                    desc.offset, desc.data_size
                                ))
                            })?;
                        file.seek(SeekFrom::Start(data_offset))?;
                        let mut buf = vec![0u8; desc.data_size as usize];
                        file.read_exact(&mut buf)?;
                        let raw = if compression_method == ewf2::CompressionMethod::Bzip2 {
                            let mut output = Vec::new();
                            bzip2::read::BzDecoder::new(buf.as_slice())
                                .take(MAX_DECOMPRESSED_SIZE + 1)
                                .read_to_end(&mut output)?;
                            if output.len() as u64 > MAX_DECOMPRESSED_SIZE {
                                return Err(EwfError::Parse("EWF2 metadata too large".into()));
                            }
                            output
                        } else {
                            maybe_zlib_decompress(&buf)?
                        };
                        parse_ewf2_case_data(&raw, &mut metadata);
                        log::debug!("parsed v2 case_data: case={:?}", metadata.case_number);
                    }
                    ewf2::Ewf2SectionType::DeviceInfo
                        if desc.data_size > 0
                            && desc.data_size < MAX_SECTION_DATA_SIZE
                            && chunk_size == 0 =>
                    {
                        let data_offset =
                            desc.offset.checked_sub(desc.data_size).ok_or_else(|| {
                                EwfError::Parse(format!(
                                    "EWF2 device_info offset underflow: desc={:#x} size={:#x}",
                                    desc.offset, desc.data_size
                                ))
                            })?;
                        file.seek(SeekFrom::Start(data_offset))?;
                        let mut buf = vec![0u8; desc.data_size as usize];
                        file.read_exact(&mut buf)?;
                        let raw = if compression_method == ewf2::CompressionMethod::Bzip2 {
                            let mut output = Vec::new();
                            bzip2::read::BzDecoder::new(buf.as_slice())
                                .take(MAX_DECOMPRESSED_SIZE + 1)
                                .read_to_end(&mut output)?;
                            if output.len() as u64 > MAX_DECOMPRESSED_SIZE {
                                return Err(EwfError::Parse("EWF2 metadata too large".into()));
                            }
                            output
                        } else {
                            maybe_zlib_decompress(&buf)?
                        };
                        parse_ewf2_device_info(&raw, &mut chunk_size, &mut total_size);
                        crate::acquisition::device_metadata(&raw, &mut metadata)?;
                        log::debug!(
                            "parsed v2 device_info: chunk_size={chunk_size}, total_size={total_size}"
                        );
                    }
                    ewf2::Ewf2SectionType::SectorTable => {
                        let data_offset =
                            desc.offset.checked_sub(desc.data_size).ok_or_else(|| {
                                EwfError::Parse(format!(
                                    "EWF2 sector_table offset underflow: desc={:#x} size={:#x}",
                                    desc.offset, desc.data_size
                                ))
                            })?;
                        file.seek(SeekFrom::Start(data_offset))?;
                        // EWF2 table header is 32 bytes: first_chunk(8) + entry_count(4)
                        // + 20 bytes of reserved/checksum fields. Entries follow the header;
                        // a 16-byte trailing table checksum closes the section data.
                        let mut tbl_hdr_buf = [0u8; 32];
                        file.read_exact(&mut tbl_hdr_buf)?;
                        let tbl_hdr = ewf2::Ewf2TableHeader::parse(&tbl_hdr_buf)?;
                        if tbl_hdr.first_chunk != chunks.len() as u64 {
                            return Err(EwfError::Parse("non-contiguous EWF2 sector table".into()));
                        }

                        let entry_count = tbl_hdr.entry_count as usize;
                        if entry_count > MAX_TABLE_ENTRIES {
                            return Err(EwfError::Parse(format!(
                                "table entry count {entry_count} exceeds maximum {MAX_TABLE_ENTRIES}"
                            )));
                        }
                        let entries_offset = data_offset + 32;
                        file.seek(SeekFrom::Start(entries_offset))?;
                        let mut entries_buf = vec![0u8; entry_count * ewf2::TABLE_ENTRY_SIZE];
                        file.read_exact(&mut entries_buf)?;

                        log::debug!(
                            "parsed v2 sector_table: first_chunk={}, entries={entry_count}",
                            tbl_hdr.first_chunk
                        );

                        for i in 0..entry_count {
                            let start = i * ewf2::TABLE_ENTRY_SIZE;
                            let end = start + ewf2::TABLE_ENTRY_SIZE;
                            let entry = ewf2::Ewf2TableEntry::parse(&entries_buf[start..end])?;
                            if entry.flags & !7 != 0 || entry.chunk_data_offset >= (1u64 << 63) {
                                return Err(EwfError::Parse(
                                    "unsupported EWF2 chunk flags or offset".into(),
                                ));
                            }
                            chunks.push(
                                Chunk::new(
                                    seg_idx,
                                    entry.is_compressed(),
                                    entry.chunk_data_offset,
                                    u64::from(entry.chunk_data_size),
                                )
                                .with_v2_flags(entry.flags),
                            );
                        }
                    }
                    ewf2::Ewf2SectionType::Md5Hash if desc.data_size >= 16 => {
                        let data_offset =
                            desc.offset.checked_sub(desc.data_size).ok_or_else(|| {
                                EwfError::Parse(format!(
                                    "EWF2 md5_hash offset underflow: desc={:#x} size={:#x}",
                                    desc.offset, desc.data_size
                                ))
                            })?;
                        file.seek(SeekFrom::Start(data_offset))?;
                        let mut hash = [0u8; 16];
                        file.read_exact(&mut hash)?;
                        stored_md5 = Some(hash);
                        log::debug!("parsed v2 md5_hash section: {hash:02x?}");
                    }
                    ewf2::Ewf2SectionType::Sha1Hash if desc.data_size >= 20 => {
                        let data_offset =
                            desc.offset.checked_sub(desc.data_size).ok_or_else(|| {
                                EwfError::Parse(format!(
                                    "EWF2 sha1_hash offset underflow: desc={:#x} size={:#x}",
                                    desc.offset, desc.data_size
                                ))
                            })?;
                        file.seek(SeekFrom::Start(data_offset))?;
                        let mut hash = [0u8; 20];
                        file.read_exact(&mut hash)?;
                        stored_sha1 = Some(hash);
                        log::debug!("parsed v2 sha1_hash section: {hash:02x?}");
                    }
                    // Done / Next are terminal markers and any unrecognized
                    // section type is a no-op here (data already extracted above).
                    _ => {}
                }
            }
        }

        // Default chunk_size if device_info didn't provide it
        if chunk_size == 0 {
            chunk_size = DEFAULT_V2_CHUNK_SIZE;
        }
        if chunk_size > MAX_CHUNK_SIZE {
            return Err(EwfError::Parse("invalid EWF2 chunk size".into()));
        }
        if total_size == 0 {
            total_size = chunks.len() as u64 * chunk_size;
        }

        // `unwrap_or(MIN)` expresses the same clamp as the former
        // `.max(1).unwrap()`, but as a total conversion: a 0 cache size becomes
        // 1 by the type's own floor rather than by a runtime assertion.
        let cache = Mutex::new(LruCache::new(
            std::num::NonZeroUsize::new(cache_size).unwrap_or(std::num::NonZeroUsize::MIN),
        ));

        // The v2 reader walks raw `File`s; wrap them as `SegmentSource::File` for
        // the unified data path. (v2 is File-only for now.)
        let segments = ordered_segments
            .into_iter()
            .map(SegmentSource::File)
            .collect();

        Ok(Self {
            compression_method,
            is_v2: true,
            segments,
            chunks: ChunkTable::Eager(chunks),
            chunk_size,
            total_size,
            position: 0,
            cache,
            stored_md5,
            stored_sha1,
            metadata,
            acquisition_errors,
        })
    }

    /// Reported media sector size, with the EWF default when absent.
    pub fn bytes_per_sector(&self) -> u32 {
        self.metadata.bytes_per_sector.unwrap_or(512)
    }
    /// Whether this is the version 2 container format.
    pub fn is_v2(&self) -> bool {
        self.is_v2
    }

    /// Total logical size of the disk image in bytes.
    #[must_use]
    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    /// Chunk size in bytes (typically 32768).
    #[must_use]
    pub fn chunk_size(&self) -> u64 {
        self.chunk_size
    }

    /// Number of chunks in the image.
    #[must_use]
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Resident bytes of the chunk table (benchmark instrumentation).
    ///
    /// Eager = `chunk_count * size_of::<Chunk>()`; lazy = section index +
    /// currently-cached parsed sections.
    #[must_use]
    pub fn resident_table_bytes(&self) -> usize {
        self.chunks.resident_table_bytes()
    }

    /// Whether this reader uses the lazy (paged) chunk table.
    #[must_use]
    pub fn is_lazy(&self) -> bool {
        matches!(self.chunks, ChunkTable::Lazy(_))
    }

    /// Access raw chunk metadata (for testing/diagnostics). Returns an owned
    /// [`Chunk`] — lazy tables materialize on demand, so a borrow is not stable.
    #[cfg(test)]
    pub(crate) fn chunk_meta(&self, idx: usize) -> Chunk {
        self.chunks
            .get(idx, &self.segments)
            .expect("chunk_meta: chunk id in range")
    }

    /// Diagnostic accessor: `(file_offset, on_disk_size, compressed, segment_idx)`
    /// for chunk `idx`. Used by the eager-vs-lazy byte-identity test to assert
    /// per-chunk metadata parity (offset/size/flag/segment), not just decoded
    /// bytes. Eager clones from the vec; lazy materializes the chunk on demand.
    ///
    /// # Errors
    /// Returns an error if `idx` is out of range or a lazy section read fails.
    #[doc(hidden)]
    pub fn debug_chunk(&self, idx: usize) -> Result<(u64, u64, bool, usize)> {
        let c = self.chunks.get(idx, &self.segments)?;
        Ok((c.offset(), c.size(), c.compressed(), c.segment_idx()))
    }

    /// Returns the integrity hashes stored within the EWF image by the acquisition tool.
    ///
    /// The `hash` section (`EnCase` 1+) stores an MD5 of the acquired media.
    /// The `digest` section (`EnCase` 6.12+) stores both MD5 and SHA-1.
    /// If neither section is present (e.g. some FTK Imager images), both fields will be `None`.
    #[must_use]
    pub fn stored_hashes(&self) -> StoredHashes {
        StoredHashes {
            md5: self.stored_md5,
            sha1: self.stored_sha1,
        }
    }

    /// Returns case and acquisition metadata from the EWF header sections.
    #[must_use]
    pub fn metadata(&self) -> &EwfMetadata {
        &self.metadata
    }

    /// Returns sectors that had read errors during acquisition.
    ///
    /// Empty for clean acquisitions. Populated from the `error2` section when present.
    #[must_use]
    pub fn acquisition_errors(&self) -> &[AcquisitionError] {
        &self.acquisition_errors
    }

    /// Verify image integrity by streaming all media data through MD5 (and SHA-1 if
    /// a stored SHA-1 exists) and comparing against the hashes stored in the image.
    ///
    /// Returns a [`VerifyResult`] with the computed hashes and match status.
    /// If the image has no stored hashes, the computed hashes are still returned
    /// but the match fields will be `None`.
    ///
    /// Requires the `verify` feature (enabled by default).
    ///
    /// # Example
    ///
    /// ```no_run
    /// let mut reader = ewf::EwfReader::open("image.E01").unwrap();
    /// let result = reader.verify().unwrap();
    /// if let Some(true) = result.md5_match {
    ///     println!("Image integrity verified (MD5 match)");
    /// }
    /// ```
    #[cfg(feature = "verify")]
    pub fn verify(&self) -> Result<VerifyResult> {
        use md5::Digest;
        use rayon::prelude::*;

        let mut md5_hasher = md5::Md5::new();
        let mut sha1_hasher = if self.stored_sha1.is_some() {
            Some(sha1::Sha1::new())
        } else {
            None
        };

        // Hashing is serial (MD5/SHA1 chain their state), but zlib decompression
        // — the CPU cost of a full-image hash — is not. Decompress chunks in
        // parallel BATCHES, then feed them to the hashers IN ORDER. A batch of
        // `threads * 4` chunks keeps every core busy while bounding peak memory
        // to one batch of decompressed chunks. `decompress_chunk` is cacheless,
        // so streaming the whole image neither pollutes nor contends the LRU.
        let chunk_count = self.chunks.len();
        let batch = rayon::current_num_threads().saturating_mul(4).max(1);
        let mut hashed: u64 = 0;

        for start in (0..chunk_count).step_by(batch) {
            let end = (start + batch).min(chunk_count);
            let pages: Vec<Vec<u8>> = (start..end)
                .into_par_iter()
                .map(|ci| self.decompress_chunk(ci))
                .collect::<Result<Vec<_>>>()?;
            for page in pages {
                // Trim the final chunk to the image's true length: a chunk
                // decompresses into a full chunk_size buffer, but the last one
                // may back fewer logical bytes.
                let remaining = self.total_size.saturating_sub(hashed);
                let take = (page.len() as u64).min(remaining) as usize;
                md5_hasher.update(&page[..take]);
                if let Some(ref mut h) = sha1_hasher {
                    h.update(&page[..take]);
                }
                hashed += take as u64;
            }
        }

        let computed_md5: [u8; 16] = md5_hasher.finalize().into();
        let computed_sha1: Option<[u8; 20]> = sha1_hasher.map(|h| h.finalize().into());

        let md5_match = self.stored_md5.map(|stored| stored == computed_md5);
        let sha1_match = match (self.stored_sha1, computed_sha1) {
            (Some(stored), Some(computed)) => Some(stored == computed),
            _ => None,
        };

        // What the chunk table can actually address, against what the volume
        // section says the media IS. A short table means segments (or their
        // tables) are missing: the hash below is then computed over a PARTIAL
        // image and will not match the acquisition digest. Reporting only the
        // mismatch would blame the evidence for the reader's incomplete view.
        let addressable = self.chunk_size() * self.chunk_count() as u64;
        let declared = self.total_size();

        Ok(VerifyResult {
            computed_md5,
            computed_sha1,
            md5_match,
            sha1_match,
            addressable_bytes: addressable,
            declared_bytes: declared,
        })
    }

    /// Read and decompress a single chunk by its index.
    ///
    /// Takes `&self`: the compressed bytes are fetched with a positioned read
    /// (no shared cursor) and decompressed WITHOUT holding the cache lock, so
    /// distinct chunks decompress in parallel across threads. The lock is held
    /// only for the brief cache probe and insert.
    fn read_chunk(&self, chunk_id: usize) -> Result<Vec<u8>> {
        // Fast path: serve from cache. Recover a poisoned lock rather than
        // panic — a poisoned cache is still readable and a panic here would
        // take down every concurrent reader.
        {
            let mut cache = self
                .cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(cached) = cache.get(&chunk_id) {
                return Ok(cached.clone());
            }
        }

        let page = self.decompress_chunk(chunk_id)?;

        let mut cache = self
            .cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache.put(chunk_id, page.clone());
        Ok(page)
    }

    /// Decompress a chunk by index WITHOUT touching the cache.
    ///
    /// The cacheless counterpart to [`read_chunk`]: used by the parallel
    /// [`verify`](Self::verify), which streams every chunk exactly once and so
    /// must neither pollute the LRU nor serialize on its lock. Positioned read
    /// + decompress, all through `&self`.
    fn decompress_chunk(&self, chunk_id: usize) -> Result<Vec<u8>> {
        let chunk = self.chunks.get(chunk_id, &self.segments)?;
        let source = self.segments.get(chunk.segment_idx()).ok_or_else(|| {
            EwfError::Parse(format!("chunk {chunk_id} references missing segment"))
        })?;
        let required = self
            .total_size
            .saturating_sub(chunk_id as u64 * self.chunk_size)
            .min(self.chunk_size);
        crate::chunk_decode::decode(
            source,
            &chunk,
            self.compression_method,
            self.chunk_size,
            required,
        )
    }

    /// Read bytes at an arbitrary logical offset through a shared `&self`.
    ///
    /// Positioned + thread-safe: many threads may call this concurrently on one
    /// `EwfReader` (e.g. parallel full-image hashing), each decompressing its
    /// own chunks. Returns the number of bytes read (short only at end of
    /// image). This is the concurrency-safe counterpart to the cursor-based
    /// [`Read`] impl, which layers position tracking on top.
    /// Requests of at least 1 MiB decode in up to four coarse parallel jobs,
    /// bypassing the decompressed-chunk LRU to preserve hot metadata pages.
    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> Result<usize> {
        if buf.len() >= 1024 * 1024 {
            return self.read_bulk_at(buf, offset);
        }
        let mut buf_idx = 0usize;
        let mut off = offset;

        loop {
            let remaining_image = self.total_size.saturating_sub(off);
            let remaining_buf = buf.len() - buf_idx;
            let in_chunk = self.chunk_size - (off % self.chunk_size);

            let to_read = in_chunk.min(remaining_image).min(remaining_buf as u64) as usize;

            if to_read == 0 {
                break;
            }

            let chunk_id = (off / self.chunk_size) as usize;
            let page = self.read_chunk(chunk_id)?;

            let page_offset = (off % self.chunk_size) as usize;
            buf[buf_idx..buf_idx + to_read]
                .copy_from_slice(&page[page_offset..page_offset + to_read]);

            off += to_read as u64;
            buf_idx += to_read;
        }

        Ok(buf_idx)
    }
}

impl Read for EwfReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.read_at(buf, self.position).map_err(io::Error::other)?;
        self.position += n as u64;
        Ok(n)
    }
}

impl Seek for EwfReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(p) => p as i64,
            SeekFrom::End(p) => self.total_size as i64 + p,
            SeekFrom::Current(p) => self.position as i64 + p,
        };
        if new_pos < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek to negative position",
            ));
        }
        self.position = new_pos as u64;
        Ok(self.position)
    }
}

/// Parse EWF2 `device_info` section data (UTF-16LE tab-separated text) to extract
/// `bytes_per_sector`, `sectors_per_chunk`, and `total_sectors` for media geometry.
///
/// Format:
///   Line 1: version ("2")
///   Line 2: section name ("main")
///   Line 3: field names (tab-separated, e.g. "b\tsc\tts")
///   Line 4: field values (tab-separated)
pub(crate) fn parse_ewf2_device_info(raw: &[u8], chunk_size: &mut u64, total_size: &mut u64) {
    // Decode UTF-16LE to String
    if raw.len() < 2 {
        return;
    }
    let u16_iter = raw
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    let text: String = char::decode_utf16(u16_iter)
        .filter_map(std::result::Result::ok)
        .collect();

    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 4 {
        return;
    }

    let names: Vec<&str> = lines[2].split('\t').collect();
    let values: Vec<&str> = lines[3].split('\t').collect();

    let mut bytes_per_sector: u64 = 512;
    let mut sectors_per_chunk: u64 = 64;
    let mut total_sectors: u64 = 0;

    for (i, &name) in names.iter().enumerate() {
        if let Some(&val_str) = values.get(i) {
            match name {
                "b" => {
                    if let Ok(v) = val_str.parse::<u64>() {
                        bytes_per_sector = v;
                    }
                }
                "sc" => {
                    if let Ok(v) = val_str.parse::<u64>() {
                        sectors_per_chunk = v;
                    }
                }
                "ts" => {
                    if let Ok(v) = val_str.parse::<u64>() {
                        total_sectors = v;
                    }
                }
                _ => {}
            }
        }
    }

    let computed_chunk_size = bytes_per_sector
        .checked_mul(sectors_per_chunk)
        .unwrap_or(u64::MAX);
    if computed_chunk_size > 0 {
        *chunk_size = computed_chunk_size;
    }
    if total_sectors > 0 && bytes_per_sector > 0 {
        *total_size = bytes_per_sector
            .checked_mul(total_sectors)
            .unwrap_or(u64::MAX);
    }
}

/// Parse EWF2 `case_data` section (UTF-16LE tab-separated) to extract case metadata.
///
/// Field codes: `cn`=`case_number`, `en`=`evidence_number`, `ex`=examiner,
/// `de`=description, `nt`=notes, `av`=`acquiry_software`, `ov`=`os_version`,
/// `at`=`acquiry_date`, `tt`=`system_date` (decimal POSIX seconds, matching
/// libewf_case_data.c); `ad`/`sd` remain accepted for older synthetic fixtures.
pub(crate) fn parse_ewf2_case_data(raw: &[u8], metadata: &mut EwfMetadata) {
    if raw.len() < 2 {
        return;
    }
    let u16_iter = raw
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    let text: String = char::decode_utf16(u16_iter)
        .filter_map(std::result::Result::ok)
        .collect();

    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 4 {
        return;
    }

    let names: Vec<&str> = lines[2].split('\t').collect();
    let values: Vec<&str> = lines[3].split('\t').collect();

    for (i, &name) in names.iter().enumerate() {
        if let Some(&val) = values.get(i) {
            if val.is_empty() {
                continue;
            }
            match name {
                "cn" => metadata.case_number = Some(val.to_string()),
                "en" => metadata.evidence_number = Some(val.to_string()),
                "ex" => metadata.examiner = Some(val.to_string()),
                "de" => metadata.description = Some(val.to_string()),
                "nt" => metadata.notes = Some(val.to_string()),
                "av" => metadata.acquiry_software = Some(val.to_string()),
                "ov" => metadata.os_version = Some(val.to_string()),
                "at" | "ad" => metadata.acquiry_date = Some(val.to_string()),
                "tt" | "sd" => metadata.system_date = Some(val.to_string()),
                _ => {}
            }
        }
    }
}

impl std::fmt::Debug for EwfReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EwfReader")
            .field("chunk_size", &self.chunk_size)
            .field("total_size", &self.total_size)
            .field("position", &self.position)
            .field("chunk_count", &self.chunks.len())
            .field("segment_count", &self.segments.len())
            .field(
                "cached_chunks",
                &self
                    .cache
                    .try_lock()
                    .map_or_else(|_| "<locked>".to_string(), |c| c.len().to_string()),
            )
            .field("stored_md5", &self.stored_md5)
            .field("stored_sha1", &self.stored_sha1)
            .field("metadata", &self.metadata)
            .field("acquisition_errors", &self.acquisition_errors)
            .finish()
    }
}

#[cfg(test)]
mod segment_dir_tests {
    use super::segment_dir;
    use std::path::Path;

    #[test]
    fn bare_filename_globs_current_dir_not_root() {
        // `Path::parent()` returns Some("") — not None — for a bare filename, so
        // the segment glob must fall back to the current directory, not "/".
        // Reproduces finding F1: `ingest <bare.E01>` from the evidence dir.
        assert_eq!(segment_dir(Path::new("bare.E01")), Path::new("."));
    }

    #[test]
    fn directory_qualified_filename_keeps_its_parent() {
        assert_eq!(
            segment_dir(Path::new("/evidence/case/bare.E01")),
            Path::new("/evidence/case")
        );
        assert_eq!(segment_dir(Path::new("sub/bare.E01")), Path::new("sub"));
    }
}
