//! Read-only loader that exposes an AccessData AD1 logical image (FTK Imager
//! "Custom Content Image") as a logical evidence tree.
//!
//! The vendored `ad1-core` reader parses the item tree and the stored per-file
//! metadata at open time. File data is stored as independent zlib chunks of
//! `chunk_size` bytes, so a ranged read inflates only the chunks it overlaps.
//! Every segment the header declares (`.ad1`, `.ad2`, …) must be present. The
//! segment files are also memory-mapped back to back so the hex view and image
//! hashing see the raw evidence bytes, as for the other archive sources.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use ad1::{Ad1Entry, Ad1Error, Ad1Reader};
use anyhow::{anyhow, bail, Context, Result};
use memmap2::Mmap;

/// Decompressed bytes handed out per read while streaming a whole file.
const STREAM_WINDOW: u64 = 1024 * 1024;

/// A segment file as it was when the image was opened.
#[derive(Debug, Clone)]
pub struct Ad1SegmentFile {
    pub path: PathBuf,
    pub len: u64,
    pub modified_unix_ms: Option<i64>,
    pub created_unix_ms: Option<i64>,
}

pub struct Ad1Image {
    path: PathBuf,
    label: String,
    reader: Ad1Reader,
    /// Indices of the top-level items.
    roots: Vec<usize>,
    /// Child indices of every entry, in image order.
    children: Vec<Vec<usize>>,
    /// The segment files, mapped in order.
    segments: Vec<Mmap>,
    segment_files: Vec<Ad1SegmentFile>,
    outer_len: u64,
    total_size: u64,
}

fn unix_ms(time: std::io::Result<SystemTime>) -> Option<i64> {
    let since = time.ok()?.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(since.as_millis()).ok()
}

impl std::fmt::Debug for Ad1Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ad1Image")
            .field("path", &self.path)
            .field("segments", &self.segments.len())
            .field("outer_len", &self.outer_len)
            .field("entries", &self.reader.entries().len())
            .field("total_size", &self.total_size)
            .finish()
    }
}

/// Whether `error` (from [`Ad1Image::open`]) means the file is not an AD1
/// image at all, so the caller may try other formats.
pub fn is_not_ad1(error: &anyhow::Error) -> bool {
    matches!(error.downcast_ref::<Ad1Error>(), Some(Ad1Error::NotAd1(_)))
}

impl Ad1Image {
    /// Open an AD1 image by its first segment (`*.ad1`).
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let reader = match Ad1Reader::open(&path) {
            Ok(reader) => reader,
            Err(Ad1Error::Unsupported(detail)) => {
                bail!("暂不支持加密的 AD1 镜像（ADCRYPT）：{detail}")
            }
            Err(error) => {
                return Err(anyhow::Error::new(error))
                    .with_context(|| format!("read AD1 image: {}", path.display()))
            }
        };
        let missing = reader.missing_segments();
        if !missing.is_empty() {
            let names = missing
                .iter()
                .filter_map(|&segment| reader.segment_paths().get(segment as usize - 1))
                .map(|segment| segment.display().to_string())
                .collect::<Vec<_>>()
                .join("、");
            bail!("AD1 镜像缺少分段：{names}");
        }

        let mut segments = Vec::with_capacity(reader.segment_paths().len());
        let mut segment_files = Vec::with_capacity(reader.segment_paths().len());
        for segment in reader.segment_paths() {
            let file = File::open(segment)
                .with_context(|| format!("open AD1 segment {}", segment.display()))?;
            let meta = file
                .metadata()
                .with_context(|| format!("stat AD1 segment {}", segment.display()))?;
            // SAFETY: evidence files are opened read-only and not modified while
            // the image is loaded; same contract as the other mmap sources.
            let map = unsafe { Mmap::map(&file) }
                .with_context(|| format!("mmap AD1 segment {}", segment.display()))?;
            segment_files.push(Ad1SegmentFile {
                path: segment.clone(),
                len: map.len() as u64,
                modified_unix_ms: unix_ms(meta.modified()),
                created_unix_ms: unix_ms(meta.created()),
            });
            segments.push(map);
        }
        let outer_len = segments.iter().map(|map| map.len() as u64).sum();

        let entries = reader.entries();
        let mut roots = Vec::new();
        let mut children = vec![Vec::new(); entries.len()];
        for (index, entry) in entries.iter().enumerate() {
            match entry.parent.and_then(|parent| children.get_mut(parent)) {
                Some(siblings) => siblings.push(index),
                None => roots.push(index),
            }
        }
        let total_size = entries
            .iter()
            .filter(|entry| !entry.is_dir)
            .fold(0u64, |total, entry| total.saturating_add(entry.size));
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());

        Ok(Self {
            path,
            label,
            reader,
            roots,
            children,
            segments,
            segment_files,
            outer_len,
            total_size,
        })
    }

    /// Every segment file, as it was at open.
    pub fn segment_files(&self) -> &[Ad1SegmentFile] {
        &self.segment_files
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    /// Summed size of all segment files.
    pub fn outer_len(&self) -> u64 {
        self.outer_len
    }

    /// Summed declared size of all files.
    pub fn total_size(&self) -> u64 {
        self.total_size
    }

    pub fn entry_count(&self) -> usize {
        self.reader.entries().len()
    }

    pub fn entry(&self, index: usize) -> Option<&Ad1Entry> {
        self.reader.entries().get(index)
    }

    /// Indices of the top-level items.
    pub fn roots(&self) -> &[usize] {
        &self.roots
    }

    /// Indices of the items directly inside entry `index`.
    pub fn children(&self, index: usize) -> &[usize] {
        self.children.get(index).map_or(&[], Vec::as_slice)
    }

    /// Copy raw segment bytes at `offset` of the concatenated segment files
    /// into `dst`; returns how many bytes were available.
    pub fn read_outer(&self, offset: u64, dst: &mut [u8]) -> usize {
        let mut copied = 0usize;
        let mut position = offset;
        let mut base = 0u64;
        for segment in &self.segments {
            let len = segment.len() as u64;
            if copied == dst.len() {
                break;
            }
            if position < base + len {
                // `position - base < len`, which fits in usize like the mapping.
                let from = (position - base) as usize;
                let take = (segment.len() - from).min(dst.len() - copied);
                dst[copied..copied + take].copy_from_slice(&segment[from..from + take]);
                copied += take;
                position += take as u64;
            }
            base += len;
        }
        copied
    }

    fn file_entry(&self, index: usize) -> Result<&Ad1Entry> {
        let entry = self
            .entry(index)
            .ok_or_else(|| anyhow!("AD1 entry {index} does not exist"))?;
        if entry.is_dir {
            bail!("not a regular file: {}", entry.path);
        }
        Ok(entry)
    }

    /// Bytes `[offset, offset + length)` of file `index`, truncated at the end
    /// of the file. Data that ends before the declared size is an error rather
    /// than a short result, so a caller never mistakes it for end of file.
    pub fn read_range(&self, index: usize, offset: u64, length: usize) -> Result<Vec<u8>> {
        let entry = self.file_entry(index)?;
        if offset >= entry.size || length == 0 {
            return Ok(Vec::new());
        }
        let want = usize::try_from((entry.size - offset).min(length as u64))
            .context("AD1 read does not fit in memory")?;
        let mut buf = vec![0u8; want];
        let read = self
            .reader
            .read_at(entry, offset, &mut buf)
            .with_context(|| format!("read AD1 file {}", entry.path))?;
        if read < want {
            bail!(
                "AD1 文件数据不完整：{} 在偏移 {offset} 处只解出 {read} / {want} 字节",
                entry.path
            );
        }
        Ok(buf)
    }

    /// Up to `max_bytes` from the start of file `index`.
    pub fn read(&self, index: usize, max_bytes: usize) -> Result<Vec<u8>> {
        self.read_range(index, 0, max_bytes)
    }

    /// Stream file `index` from its start in slabs of at most `chunk_size`
    /// bytes. Fails unless exactly the declared size was delivered.
    pub fn stream_file<F>(&self, index: usize, chunk_size: usize, mut visitor: F) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let entry = self.file_entry(index)?;
        let slab = chunk_size.max(1);
        // Whole zlib chunks per read, so no chunk is inflated twice.
        let chunk = u64::from(self.reader.chunk_size().max(1));
        let window = STREAM_WINDOW.max(slab as u64).div_ceil(chunk) * chunk;
        let mut offset = 0u64;
        while offset < entry.size {
            let want = usize::try_from(window.min(entry.size - offset))?;
            let bytes = self.read_range(index, offset, want)?;
            for piece in bytes.chunks(slab) {
                visitor(piece)?;
            }
            offset += bytes.len() as u64;
        }
        Ok(offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ad1::testfix::{self, Node};

    fn write(dir: &Path, segments: &[Vec<u8>]) -> PathBuf {
        for (index, bytes) in segments.iter().enumerate() {
            std::fs::write(dir.join(format!("case.ad{}", index + 1)), bytes).unwrap();
        }
        dir.join("case.ad1")
    }

    fn data_of<'a>(built: &'a testfix::Built, path: &str) -> &'a [u8] {
        built
            .expected
            .iter()
            .find(|entry| entry.path == path)
            .and_then(|entry| entry.data.as_deref())
            .unwrap()
    }

    fn index_of(image: &Ad1Image, path: &str) -> usize {
        (0..image.entry_count())
            .find(|&index| image.entry(index).unwrap().path == path)
            .unwrap()
    }

    fn split_three(built: &testfix::Built) -> Vec<Vec<u8>> {
        let fragments_size = 2u32;
        let stride = fragments_size as usize * 0x1_0000 - testfix::MARGIN;
        let count = (built.bytes.len() - testfix::MARGIN).div_ceil(stride) as u32;
        testfix::split(&built.bytes, count, fragments_size)
    }

    #[test]
    fn tree_ranges_streams_and_outer_bytes_follow_the_segments() {
        let built = testfix::build(testfix::sample_tree());
        let segments = split_three(&built);
        assert!(segments.len() >= 2);
        let dir = tempfile::tempdir().unwrap();
        let image = Ad1Image::open(write(dir.path(), &segments)).unwrap();

        assert_eq!(image.roots(), [0]);
        let names = |indices: &[usize]| -> Vec<String> {
            indices
                .iter()
                .map(|&index| image.entry(index).unwrap().path.clone())
                .collect()
        };
        assert_eq!(names(image.children(0)), ["root/hello.txt", "root/sub"]);
        assert_eq!(image.total_size(), 12 + 200_000);

        let large = index_of(&image, "root/sub/a.bin");
        let data = data_of(&built, "root/sub/a.bin");
        assert_eq!(
            image.read_range(large, 65_530, 20).unwrap(),
            &data[65_530..65_550]
        );
        assert_eq!(
            image.read_range(large, 199_990, 64).unwrap(),
            &data[199_990..]
        );
        assert!(image.read_range(large, 200_000, 64).unwrap().is_empty());
        let mut streamed = Vec::new();
        let total = image
            .stream_file(large, 4096, |piece| {
                assert!(piece.len() <= 4096);
                streamed.extend_from_slice(piece);
                Ok(())
            })
            .unwrap();
        assert_eq!(total, 200_000);
        assert_eq!(streamed, data);

        let joined: Vec<u8> = segments.concat();
        assert_eq!(image.outer_len(), joined.len() as u64);
        let mut window = vec![0u8; 4096];
        let start = segments[0].len() - 100;
        assert_eq!(image.read_outer(start as u64, &mut window), 4096);
        assert_eq!(window, joined[start..start + 4096]);
        assert_eq!(image.read_outer(joined.len() as u64 - 10, &mut window), 10);
    }

    #[test]
    fn missing_segments_and_short_data_are_refused() {
        let built = testfix::build(testfix::sample_tree());
        let segments = split_three(&built);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("gap.ad1"), &segments[0]).unwrap();
        let error = Ad1Image::open(dir.path().join("gap.ad1")).unwrap_err();
        assert!(
            format!("{error:#}").contains("AD1 镜像缺少分段"),
            "{error:#}"
        );

        // Replace the first (non-final) chunk of a.bin with a zlib stream that
        // inflates to 16 bytes: the file is then short of its declared size,
        // and reads and streams fail instead of ending early.
        let mut bytes = built.bytes.clone();
        let item = find_item(&bytes, "a.bin");
        let table = testfix::MARGIN + read_u64(&bytes, item + 0x18) as usize;
        let chunk0 = testfix::MARGIN + read_u64(&bytes, table + 8) as usize;
        let short = {
            use flate2::write::ZlibEncoder;
            use std::io::Write as _;
            let mut encoder = ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(&[0u8; 16]).unwrap();
            encoder.finish().unwrap()
        };
        bytes[chunk0..chunk0 + short.len()].copy_from_slice(&short);
        let short_dir = tempfile::tempdir().unwrap();
        let image = Ad1Image::open(write(short_dir.path(), &[bytes])).unwrap();
        let large = index_of(&image, "root/sub/a.bin");
        let error = image.stream_file(large, 65_536, |_| Ok(())).unwrap_err();
        assert!(
            format!("{error:#}").contains("AD1 文件数据不完整"),
            "{error:#}"
        );
        assert!(image.read_range(large, 0, 100).is_err());
    }

    fn read_u64(bytes: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
    }

    /// Physical offset of the item record named `name` in a single-segment image.
    fn find_item(bytes: &[u8], name: &str) -> usize {
        let mut stack = vec![read_u64(bytes, testfix::MARGIN + 0x24)];
        while let Some(addr) = stack.pop() {
            if addr == 0 {
                continue;
            }
            let base = testfix::MARGIN + addr as usize;
            let len = u32::from_le_bytes(bytes[base + 0x2c..base + 0x30].try_into().unwrap());
            if &bytes[base + 0x30..base + 0x30 + len as usize] == name.as_bytes() {
                return base;
            }
            stack.push(read_u64(bytes, base));
            stack.push(read_u64(bytes, base + 8));
        }
        panic!("item {name} not found");
    }

    #[test]
    fn non_ad1_files_are_reported_as_such() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.ad1");
        std::fs::write(&path, vec![0x5au8; 4096]).unwrap();
        assert!(is_not_ad1(&Ad1Image::open(&path).unwrap_err()));

        let encrypted = dir.path().join("locked.ad1");
        let mut bytes = vec![0u8; 512];
        bytes[..8].copy_from_slice(b"ADCRYPT\0");
        std::fs::write(&encrypted, bytes).unwrap();
        let error = Ad1Image::open(&encrypted).unwrap_err();
        assert!(!is_not_ad1(&error));
        assert!(format!("{error:#}").contains("ADCRYPT"));
    }

    #[test]
    fn wide_folders_list_children_in_order() {
        let files = (0..40)
            .map(|index| {
                Node::File(
                    Box::leak(format!("f{index:02}.txt").into_boxed_str()),
                    vec![b'x'; index],
                )
            })
            .collect();
        let built = testfix::build(Node::Dir("root", files));
        let dir = tempfile::tempdir().unwrap();
        let image = Ad1Image::open(write(dir.path(), &[built.bytes])).unwrap();
        assert_eq!(image.children(0).len(), 40);
        assert!(image.children(0).windows(2).all(|pair| pair[0] < pair[1]));
        assert!(image.children(1).is_empty());
    }
}
