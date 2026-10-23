//! Read-only SquashFS 1.0 and 2.0/2.1, sharing backhand's tree/read contracts.
//! Wire layouts are independently implemented from documented format fields.
#[path = "legacy_inode.rs"]
mod inode;
#[path = "legacy_wire.rs"]
mod wire;
use crate::traits::filesystem::{BackhandNode, BackhandSquashfsFileReader};
use crate::{BackhandError, BufReadSeek, FilesystemReaderTrait};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use wire::{Header, Metadata, decode_data, metadata_block, read_at, require};

type Result<T> = std::result::Result<T, BackhandError>;
/// Open a legacy source through the version-neutral tree interface. Keep the
/// concrete parser, its allocation and vtable inside the reader crate.
// Keep this boundary when emitting Windows CodeView inline-site information.
#[inline(never)]
pub fn open<'a>(reader: Box<dyn BufReadSeek + 'a>) -> Result<Box<dyn FilesystemReaderTrait + 'a>> {
    Ok(Box::new(FilesystemReader::from_boxed_reader(reader)?))
}
fn invalid(message: &str) -> BackhandError {
    BackhandError::StdIo(io::Error::new(
        io::ErrorKind::InvalidData,
        format!("SquashFS v1/v2: {message}"),
    ))
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Block {
    start: u64,
    size: usize,
    uncompressed: bool,
    logical: usize,
}
#[derive(Debug, Clone, PartialEq, Eq)]
struct Plan {
    descriptor: BackhandSquashfsFileReader,
    blocks: Vec<Block>,
    fragment: Option<(Block, usize, usize)>,
}
type FileKey = (u64, u32, u32, u32);
fn file_key(file: &BackhandSquashfsFileReader) -> Result<FileKey> {
    match file {
        BackhandSquashfsFileReader::Basic {
            blocks_start,
            frag_index,
            block_offset,
            file_size,
            ..
        } => Ok((*blocks_start, *frag_index, *block_offset, *file_size)),
        BackhandSquashfsFileReader::Extended { .. } => {
            Err(invalid("extended file descriptor is not a legacy file"))
        }
    }
}

/// Immutable legacy tree with lazy file data decoding. The source is held behind
/// a mutex like the v3/v4 readers and is never extracted into host files.
pub struct FilesystemReader<'a> {
    reader: Mutex<Box<dyn BufReadSeek + 'a>>,
    header: Header,
    nodes: Vec<BackhandNode>,
    plans: BTreeMap<FileKey, Plan>,
}
impl<'a> FilesystemReader<'a> {
    pub fn from_reader(reader: impl BufReadSeek + 'a) -> Result<Self> {
        Self::from_boxed_reader(Box::new(reader))
    }

    /// Parse an already-erased bounded source, sharing one parser implementation
    /// across partition, firmware and reconstructed-volume cursor types.
    pub fn from_boxed_reader(mut reader: Box<dyn BufReadSeek + 'a>) -> Result<Self> {
        let header = Header::read(&mut *reader)?;
        let (fragments, directory_end) = read_fragments(&mut *reader, &header)?;
        let mut budget = 0;
        let inodes = Metadata::read(
            &mut *reader,
            header.inode_start,
            header.directory_start,
            &header,
            &mut budget,
        )?;
        let directories = Metadata::read(
            &mut *reader,
            header.directory_start,
            directory_end,
            &header,
            &mut budget,
        )?;
        let mut cache = BTreeMap::new();
        let mut occupied: BTreeMap<usize, usize> = BTreeMap::new();
        let mut pending = vec![(PathBuf::from("/"), header.root, 1u8, 0usize)];
        let mut seen_directories = BTreeSet::new();
        let mut nodes = Vec::new();
        let mut plans: BTreeMap<FileKey, Plan> = BTreeMap::new();
        let mut path_bytes = 0usize;
        let mut index_bytes = 0usize;
        while let Some((path, reference, expected, depth)) = pending.pop() {
            require(
                depth <= 256 && nodes.len() < 1_000_000,
                "tree depth/node budget",
            )?;
            if let std::collections::btree_map::Entry::Vacant(entry) = cache.entry(reference) {
                let parsed = inode::parse(reference, &inodes, &directories, &header, &fragments)?;
                index_bytes = index_bytes
                    .checked_add(
                        256 + parsed
                            .plan
                            .as_ref()
                            .map_or(0, |plan| plan.blocks.len() * 128),
                    )
                    .ok_or_else(|| invalid("index budget overflow"))?;
                require(index_bytes <= 256 * 1024 * 1024, "index budget exceeded")?;
                if let Some((_, end)) = occupied.range(..=parsed.start).next_back() {
                    require(*end <= parsed.start, "overlapping inode records")?;
                }
                if let Some((start, _)) = occupied.range(parsed.start..).next() {
                    require(parsed.end <= *start, "overlapping inode records")?;
                }
                occupied.insert(parsed.start, parsed.end);
                entry.insert(parsed);
                require(
                    cache.len() <= header.count as usize,
                    "inode count exceeds superblock",
                )?;
            }
            let parsed = cache
                .get(&reference)
                .ok_or_else(|| invalid("missing inode cache"))?;
            require(expected == parsed.kind, "directory type and inode disagree")?;
            if let Some(directory) = &parsed.directory {
                require(
                    seen_directories.insert(reference),
                    "directory cycle or hardlink",
                )?;
                let entries = inode::directory(directory, &directories, &header)?;
                for entry in entries.into_iter().rev() {
                    path_bytes = path_bytes
                        .checked_add(path.as_os_str().len() + entry.name.len() + 1)
                        .ok_or_else(|| invalid("path budget overflow"))?;
                    require(path_bytes <= 256 * 1024 * 1024, "path budget exceeded")?;
                    pending.push((
                        path.join(entry.name),
                        entry.reference,
                        entry.kind,
                        depth + 1,
                    ));
                }
            }
            if let Some(plan) = &parsed.plan {
                let key = file_key(&plan.descriptor)?;
                if let Some(previous) = plans.get(&key) {
                    require(previous == plan, "conflicting shared file descriptor")?;
                } else {
                    plans.insert(key, plan.clone());
                }
            }
            nodes.push(BackhandNode {
                fullpath: path,
                header: parsed.header,
                inner: parsed.inner.clone(),
            });
        }
        let mut fragment_windows: BTreeMap<(u64, usize), usize> = BTreeMap::new();
        for plan in plans.values() {
            if let Some((block, within, count)) = &plan.fragment {
                let window = fragment_windows
                    .entry((block.start, block.size))
                    .or_default();
                *window = (*window).max(within + count);
            }
        }
        for plan in plans.values_mut() {
            if let Some((block, _, _)) = &mut plan.fragment {
                block.logical = *fragment_windows
                    .get(&(block.start, block.size))
                    .ok_or_else(|| invalid("missing fragment window"))?;
            }
        }
        Ok(Self {
            reader: Mutex::new(reader),
            header,
            nodes,
            plans,
        })
    }
    fn plan(&self, file: &BackhandSquashfsFileReader) -> Result<&Plan> {
        let plan = self
            .plans
            .get(&file_key(file)?)
            .ok_or_else(|| invalid("unknown file descriptor"))?;
        require(
            plan.descriptor == *file,
            "file descriptor differs from parsed inode",
        )?;
        Ok(plan)
    }
    fn decode(&self, reader: &mut dyn BufReadSeek, block: &Block) -> Result<Vec<u8>> {
        let bytes = read_at(reader, block.start, block.size, self.header.inode_start)?;
        if block.uncompressed {
            Ok(bytes)
        } else {
            decode_data(&bytes, self.header.size, block.logical)
        }
    }
}
impl FilesystemReaderTrait for FilesystemReader<'_> {
    fn files(&self) -> Box<dyn Iterator<Item = BackhandNode> + '_> {
        Box::new(self.nodes.iter().cloned())
    }
    fn file_data(&self, file: &BackhandSquashfsFileReader) -> Result<Vec<u8>> {
        self.file_data_range(file, 0, file.file_len())
    }
    fn file_data_range(
        &self,
        file: &BackhandSquashfsFileReader,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>> {
        let plan = self.plan(file)?;
        let length = (length as u64).min((file.file_len() as u64).saturating_sub(offset)) as usize;
        require(length <= 256 * 1024 * 1024, "single read exceeds budget")?;
        let mut output = vec![0; length];
        if length == 0 {
            return Ok(output);
        }
        let mut reader = self
            .reader
            .lock()
            .map_err(|_| BackhandError::MutexPoisoned)?;
        let end = offset + length as u64;
        let first = (offset / self.header.size as u64) as usize;
        for (number, block) in plan.blocks.iter().enumerate().skip(first) {
            let start = number as u64 * self.header.size as u64;
            if start >= end {
                break;
            }
            let decoded = self.decode(&mut **reader, block)?;
            require(decoded.len() == block.logical, "file block logical length")?;
            let from = start.max(offset);
            let to = (start + block.logical as u64).min(end);
            require(from <= to, "file block outside logical request")?;
            output[(from - offset) as usize..(to - offset) as usize]
                .copy_from_slice(&decoded[(from - start) as usize..(to - start) as usize]);
        }
        if let Some((block, within, count)) = &plan.fragment {
            let start = plan.blocks.len() as u64 * self.header.size as u64;
            if start < end && start + *count as u64 > offset {
                let decoded = self.decode(&mut **reader, block)?;
                require(
                    within
                        .checked_add(*count)
                        .is_some_and(|end| end <= decoded.len()),
                    "fragment outside decoded block",
                )?;
                let from = start.max(offset);
                let to = (start + *count as u64).min(end);
                output[(from - offset) as usize..(to - offset) as usize].copy_from_slice(
                    &decoded[within + (from - start) as usize..within + (to - start) as usize],
                );
            }
        }
        Ok(output)
    }
    fn file_data_to_writer(
        &self,
        file: &BackhandSquashfsFileReader,
        writer: &mut dyn Write,
    ) -> Result<u64> {
        self.plan(file)?;
        let mut offset = 0u64;
        while offset < file.file_len() as u64 {
            let bytes = self.file_data_range(file, offset, 1024 * 1024)?;
            require(!bytes.is_empty(), "short logical stream")?;
            writer.write_all(&bytes)?;
            offset += bytes.len() as u64;
        }
        require(
            offset == file.file_len() as u64,
            "incomplete logical stream",
        )?;
        Ok(offset)
    }
}

fn read_fragments(reader: &mut dyn BufReadSeek, header: &Header) -> Result<(Vec<Block>, u64)> {
    if header.fragments == 0 {
        return Ok((Vec::new(), header.uid_start));
    }
    let total = header.fragments as usize * 8;
    let count = total.div_ceil(8192);
    require(
        header.fragment_start.checked_add((count * 4) as u64) == Some(header.uid_start),
        "fragment index table length",
    )?;
    let pointers = read_at(reader, header.fragment_start, count * 4, header.uid_start)?;
    let mut result = Vec::with_capacity(header.fragments as usize);
    let mut first = None;
    let mut expected = None;
    for index in 0..count {
        let start = u64::from(header.order.u32(&pointers, index * 4)?);
        require(
            start >= header.directory_start
                && start < header.fragment_start
                && expected.is_none_or(|next| next == start),
            "fragment metadata pointer ordering",
        )?;
        if first.is_none() {
            first = Some(start);
        }
        let (bytes, next) = metadata_block(reader, start, header.fragment_start, header)?;
        require(
            bytes.len() == (total - index * 8192).min(8192),
            "fragment table decoded length",
        )?;
        for entry in bytes.as_chunks::<8>().0 {
            result.push(block(
                u64::from(header.order.u32(entry, 0)?),
                header.order.u32(entry, 4)?,
                header,
                header.size,
            )?);
        }
        expected = Some(next);
    }
    require(
        expected == Some(header.fragment_start),
        "fragment metadata extent",
    )?;
    Ok((
        result,
        first.ok_or_else(|| invalid("empty fragment table"))?,
    ))
}
fn block(start: u64, raw: u32, header: &Header, logical: usize) -> Result<Block> {
    require(raw & 0xfe00_0000 == 0, "unknown data block flag")?;
    let size = (raw & 0x00ff_ffff) as usize;
    require(
        (1..=header.size).contains(&size)
            && start >= header.header_size
            && start
                .checked_add(size as u64)
                .is_some_and(|end| end <= header.inode_start),
        "data block source extent",
    )?;
    Ok(Block {
        start,
        size,
        uncompressed: raw & 0x0100_0000 != 0,
        logical,
    })
}

#[cfg(test)]
#[path = "legacy_tests.rs"]
mod tests;
