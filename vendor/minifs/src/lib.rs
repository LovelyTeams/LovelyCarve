//! Read-only MINIFS v3 metadata and file access, independent of any application.
//!
//! Offsets are relative to the MINIFS signature. Directories are reconstructed
//! from file paths; the format supplies no timestamps, modes or deleted records.
//! See README.md for format references, resource limits and unsupported variants.

mod codec;
#[cfg(test)]
mod tests;

use anyhow::{ensure, Context, Result};
use std::{collections::BTreeMap, io};

pub const MAGIC: &[u8; 16] = b"MINIFS\0\0\0\0\0\0\0\0\0\0";

/// An immutable random-access source. Implementers must report short reads as
/// errors and keep `len()` and source bytes stable for the reader's lifetime.
pub trait ReadAt {
    fn len(&self) -> u64;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()>;
}

impl ReadAt for &[u8] {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }
    fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        let start = usize::try_from(offset).map_err(|_| io::ErrorKind::InvalidInput)?;
        let end = start
            .checked_add(buffer.len())
            .ok_or(io::ErrorKind::InvalidInput)?;
        let source = self.get(start..end).ok_or(io::ErrorKind::UnexpectedEof)?;
        buffer.copy_from_slice(source);
        Ok(())
    }
}

/// Per-reader allocation and traversal limits, applied before allocating.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub files: usize,
    pub metadata_bytes: usize,
    pub chunk_bytes: usize,
    pub dictionary_bytes: u32,
    pub path_bytes: usize,
    pub path_depth: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            files: 100_000,
            metadata_bytes: 16 << 20,
            chunk_bytes: 32 << 20,
            dictionary_bytes: 16 << 20,
            path_bytes: 4096,
            path_depth: 256,
        }
    }
}

/// A file's logical placement in one shared, independently compressed chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub chunk: usize,
    pub offset: u64,
    pub size: u64,
}

#[derive(Debug, Clone)]
struct Chunk {
    offset: u64,
    packed: usize,
    unpacked: usize,
}

/// Validated metadata index; no file data is decoded while opening it.
pub struct Index {
    files: Vec<FileEntry>,
    chunks: Vec<Chunk>,
    data_offset: u64,
    image_length: u64,
    limits: Limits,
    entries: BTreeMap<String, Option<usize>>,
    children: BTreeMap<String, Vec<String>>,
}

/// A directory entry. `file == None` identifies a synthesized directory.
#[derive(Debug, Clone, Copy)]
pub struct Entry<'a> {
    pub path: &'a str,
    pub file: Option<&'a FileEntry>,
}

pub fn has_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(MAGIC)
}

fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    let bytes = bytes
        .get(offset..offset + 4)
        .context("truncated MINIFS integer")?;
    Ok(u32::from_be_bytes(bytes.try_into()?))
}

fn read(source: &impl ReadAt, offset: u64, length: usize) -> Result<Vec<u8>> {
    ensure!(
        offset
            .checked_add(length as u64)
            .is_some_and(|end| end <= source.len()),
        "MINIFS read exceeds source"
    );
    let mut bytes = vec![0; length];
    source
        .read_exact_at(offset, &mut bytes)
        .context("MINIFS source read")?;
    Ok(bytes)
}

fn name(bytes: &[u8], offset: u32) -> Result<&str> {
    let offset = offset as usize;
    ensure!(
        offset == 0 || bytes.get(offset - 1) == Some(&0),
        "MINIFS name points inside a string"
    );
    let bytes = bytes
        .get(offset..)
        .context("MINIFS name offset outside table")?;
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .context("unterminated MINIFS name")?;
    std::str::from_utf8(&bytes[..end]).context("MINIFS name is not UTF-8")
}

/// Canonical absolute path used by the library. Empty string denotes the root.
pub fn valid_path(path: &str) -> bool {
    path.is_empty()
        || (path.starts_with('/')
            && !path.contains(['\\', '\0'])
            && path
                .split('/')
                .skip(1)
                .all(|part| !part.is_empty() && part != "." && part != ".."))
}

impl Index {
    pub fn open(source: &impl ReadAt, limits: Limits) -> Result<Self> {
        let header = read(source, 0, 32)?;
        ensure!(has_magic(&header), "invalid MINIFS signature");
        ensure!(
            word(&header, 16)? == 3,
            "unsupported MINIFS version (only v3 is supported)"
        );
        let count = word(&header, 20)? as usize;
        let names_length = word(&header, 28)? as usize;
        ensure!(
            count > 0 && count <= limits.files,
            "MINIFS file count limit"
        );
        let files_length = count
            .checked_mul(20)
            .context("MINIFS file table overflow")?;
        let metadata_length = names_length
            .checked_add(files_length)
            .context("MINIFS metadata overflow")?;
        ensure!(
            metadata_length <= limits.metadata_bytes,
            "MINIFS metadata limit"
        );
        let metadata = read(source, 32, metadata_length)?;
        let names = &metadata[..names_length];
        let records = &metadata[names_length..];
        let mut files = Vec::with_capacity(count);
        let mut chunk_count = 0;
        let mut path_bytes = 0usize;
        for record in records.as_chunks::<20>().0 {
            let directory = name(names, word(record, 0)?)?;
            let filename = name(names, word(record, 4)?)?;
            ensure!(
                !filename.is_empty() && !filename.contains('/'),
                "invalid MINIFS file name"
            );
            let path = if directory.is_empty() {
                format!("/{filename}")
            } else {
                format!("/{directory}/{filename}")
            };
            ensure!(
                valid_path(&path)
                    && path.len() <= limits.path_bytes
                    && path.split('/').count() - 1 <= limits.path_depth,
                "invalid MINIFS path or path limit"
            );
            path_bytes = path_bytes
                .checked_add(path.len())
                .context("MINIFS path budget overflow")?;
            ensure!(
                path_bytes <= limits.metadata_bytes,
                "MINIFS expanded paths limit"
            );
            let chunk = word(record, 8)? as usize;
            chunk_count = chunk_count.max(
                chunk
                    .checked_add(1)
                    .context("MINIFS chunk index overflow")?,
            );
            files.push(FileEntry {
                path,
                chunk,
                offset: word(record, 12)? as u64,
                size: word(record, 16)? as u64,
            });
        }
        ensure!(
            chunk_count <= count,
            "MINIFS chunk count exceeds file count"
        );
        let chunks_length = chunk_count
            .checked_mul(12)
            .context("MINIFS chunk table overflow")?;
        ensure!(
            metadata_length
                .checked_add(chunks_length)
                .is_some_and(|length| length <= limits.metadata_bytes),
            "MINIFS metadata limit"
        );
        let chunk_table_offset = 32 + metadata_length as u64;
        let data_offset = chunk_table_offset + chunks_length as u64;
        let records = read(source, chunk_table_offset, chunks_length)?;
        let mut chunks = Vec::with_capacity(chunk_count);
        let mut end = 0;
        for record in records.as_chunks::<12>().0 {
            let offset = word(record, 0)? as u64;
            let packed = word(record, 4)? as usize;
            let unpacked = word(record, 8)? as usize;
            ensure!(
                packed >= 18 && packed <= limits.chunk_bytes && unpacked <= limits.chunk_bytes,
                "MINIFS chunk size limit"
            );
            ensure!(offset == end, "MINIFS chunk ranges overlap or contain gaps");
            end = offset
                .checked_add(packed as u64)
                .context("MINIFS chunk overflow")?;
            ensure!(
                data_offset
                    .checked_add(end)
                    .is_some_and(|end| end <= source.len()),
                "MINIFS chunk exceeds source"
            );
            chunks.push(Chunk {
                offset,
                packed,
                unpacked,
            });
        }
        for file in &files {
            let chunk = chunks.get(file.chunk).context("MINIFS missing chunk")?;
            ensure!(
                file.offset
                    .checked_add(file.size)
                    .is_some_and(|end| end <= chunk.unpacked as u64),
                "MINIFS file exceeds decoded chunk"
            );
        }
        let mut entries = BTreeMap::from([(String::new(), None)]);
        for (index, file) in files.iter().enumerate() {
            ensure!(
                entries.insert(file.path.clone(), Some(index)).is_none(),
                "duplicate MINIFS path"
            );
        }
        for file in &files {
            let mut path = file.path.as_str();
            while let Some((parent, _)) = path.rsplit_once('/') {
                ensure!(
                    !matches!(entries.get(parent), Some(Some(_))),
                    "MINIFS file/directory collision"
                );
                if !entries.contains_key(parent) {
                    path_bytes = path_bytes
                        .checked_add(parent.len())
                        .context("MINIFS path budget overflow")?;
                    ensure!(
                        path_bytes <= limits.metadata_bytes,
                        "MINIFS directory paths limit"
                    );
                    entries.insert(parent.to_owned(), None);
                }
                path = parent;
            }
        }
        let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for path in entries.keys().filter(|path| !path.is_empty()) {
            let (parent, _) = path
                .rsplit_once('/')
                .context("invalid indexed MINIFS path")?;
            children
                .entry(parent.to_owned())
                .or_default()
                .push(path.clone());
        }
        Ok(Self {
            files,
            chunks,
            data_offset,
            image_length: data_offset + end,
            limits,
            entries,
            children,
        })
    }

    /// Length through the end of the final compressed chunk, excluding wrapper padding.
    pub fn image_length(&self) -> u64 {
        self.image_length
    }
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
    pub fn files(&self) -> &[FileEntry] {
        &self.files
    }

    pub fn entry(&self, path: &str) -> Result<Entry<'_>> {
        let (path, index) = self
            .entries
            .get_key_value(path)
            .context("MINIFS path not found")?;
        Ok(Entry {
            path,
            file: index.map(|index| &self.files[index]),
        })
    }

    pub fn children(&self, path: &str) -> Result<Vec<Entry<'_>>> {
        ensure!(
            self.entry(path)?.file.is_none(),
            "MINIFS entry is not a directory"
        );
        self.children
            .get(path)
            .into_iter()
            .flatten()
            .map(|path| self.entry(path))
            .collect()
    }
}

/// Reader with at most one decoded chunk cached. Errors never publish partial data.
pub struct Reader<S> {
    source: S,
    index: Index,
    cache: Option<(usize, Vec<u8>)>,
}

impl<S: ReadAt> Reader<S> {
    pub fn open(source: S, limits: Limits) -> Result<Self> {
        let index = Index::open(&source, limits)?;
        Ok(Self {
            source,
            index,
            cache: None,
        })
    }
    pub fn index(&self) -> &Index {
        &self.index
    }

    pub fn read_range(&mut self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        self.read_range_with(path, offset, length, || Ok(()))
    }

    /// `check` runs before reading and every 64 KiB of decoder input/output;
    /// returning an error cancels the operation and does not populate the cache.
    pub fn read_range_with(
        &mut self,
        path: &str,
        offset: u64,
        length: usize,
        mut check: impl FnMut() -> Result<()>,
    ) -> Result<Vec<u8>> {
        check()?;
        let file = self
            .index
            .entry(path)?
            .file
            .context("MINIFS directory has no content")?
            .clone();
        ensure!(
            offset <= file.size,
            "MINIFS file offset exceeds logical length"
        );
        let count = (file.size - offset).min(length as u64) as usize;
        if count == 0 {
            return Ok(Vec::new());
        }
        if !self
            .cache
            .as_ref()
            .is_some_and(|(index, _)| *index == file.chunk)
        {
            self.cache = None;
            let chunk = &self.index.chunks[file.chunk];
            let bytes = read(
                &self.source,
                self.index.data_offset + chunk.offset,
                chunk.packed,
            )?;
            let decoded = codec::decode(
                &bytes,
                chunk.unpacked,
                self.index.limits.dictionary_bytes,
                &mut check,
            )
            .with_context(|| format!("MINIFS chunk {}", file.chunk))?;
            self.cache = Some((file.chunk, decoded));
        }
        let (_, bytes) = self.cache.as_ref().context("MINIFS cache missing")?;
        let start = usize::try_from(file.offset + offset)?;
        Ok(bytes
            .get(start..start + count)
            .context("MINIFS decoded file bounds")?
            .to_vec())
    }
}
