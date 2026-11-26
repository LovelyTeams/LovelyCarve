use std::collections::{BTreeMap, HashSet};
use std::ops::Range;

use lamfold::{BlockSource, FileKind, FoldError, Result};

use crate::allocation::Allocation;
use crate::format::{
    DIRENT_FORMAT, Header, INODE_BYTES, Inode, Number, file_kind, meta, require, u16be, u32be,
    u64be,
};
use crate::stream::Stream;

const ENTRY_BYTES: usize = 40;
const LEAF_BYTES: usize = 104;
const MAX_ENTRIES: usize = 1_000_000;

#[derive(Clone)]
pub(crate) struct Entry {
    pub number: Number,
    pub raw_kind: u16,
    pub kind: FileKind,
}

pub(crate) type Directory = BTreeMap<String, Entry>;

pub(crate) fn read(
    source: &mut impl BlockSource,
    header: &Header,
    allocation: &mut Allocation,
    inode: &Inode,
) -> Result<Directory> {
    if inode.kind != FileKind::Directory {
        return Err(FoldError::NotDirectory);
    }
    require(
        inode.entries as usize <= MAX_ENTRIES,
        "GFS1 directory entry limit",
    )?;
    let mut output = BTreeMap::new();
    if inode.flags & 2 == 0 {
        require(
            inode.height == 0 && inode.depth == 0 && inode.payload_format == DIRENT_FORMAT,
            "invalid GFS1 linear directory layout",
        )?;
        records(&inode.raw[INODE_BYTES..], &mut output, None)?;
    } else {
        let bucket_count = 1_usize << inode.depth;
        require(
            inode.size == (bucket_count * 8) as u64,
            "GFS1 exhash directory size differs from its depth",
        )?;
        let stream = Stream::parse(source, header, inode, Some(allocation))?;
        let mut hash_table = vec![0; bucket_count * 8];
        require(
            stream.read(source, header, 0, &mut hash_table)? == hash_table.len(),
            "short GFS1 exhash directory table",
        )?;
        let mut leaves = HashSet::new();
        let table = hash_table.as_chunks::<8>().0;
        let mut bucket = 0;
        while bucket < table.len() {
            let mut leaf = u64be(&table[bucket], 0)?;
            require(leaf != 0, "missing GFS1 exhash leaf")?;
            let start = bucket;
            while bucket < table.len() && table[bucket] == table[start] {
                bucket += 1;
            }
            let width = bucket - start;
            while leaf != 0 {
                require(
                    leaves.len() < 65536 && leaves.insert(leaf) && !stream.owned.contains(&leaf),
                    "duplicate or excessive GFS1 leaf blocks",
                )?;
                allocation.check(source, header, leaf, true)?;
                let raw = header.read_block(source, leaf)?;
                meta(&raw, 6, 600)?;
                let leaf_depth = u16be(&raw, 24)?;
                require(
                    leaf_depth <= inode.depth
                        && width == 1_usize << (inode.depth - leaf_depth)
                        && start.is_multiple_of(width)
                        && u32be(&raw, 28)? == DIRENT_FORMAT,
                    "invalid GFS1 exhash leaf span or format",
                )?;
                let before = output.len();
                records(
                    &raw[LEAF_BYTES..],
                    &mut output,
                    Some((inode.depth, start..bucket)),
                )?;
                require(
                    output.len() - before == usize::from(u16be(&raw, 26)?),
                    "GFS1 leaf directory count differs from its header",
                )?;
                leaf = u64be(&raw, 32)?;
            }
        }
        require(
            (leaves.len() + stream.owned.len()) as u64 <= inode.blocks,
            "GFS1 exhash allocations exceed inode block count",
        )?;
    }
    require(
        output.len() == inode.entries as usize,
        "GFS1 directory count differs from its inode",
    )?;
    require(
        output
            .get(".")
            .is_some_and(|entry| entry.number == inode.number && entry.kind == FileKind::Directory)
            && output.get("..").is_some_and(|entry| {
                entry.kind == FileKind::Directory
                    && (inode.number != header.root || entry.number == header.root)
            }),
        "GFS1 directory self or parent reference",
    )?;
    output.remove(".");
    output.remove("..");
    Ok(output)
}

fn records(
    mut raw: &[u8],
    output: &mut Directory,
    bucket: Option<(u16, Range<usize>)>,
) -> Result<()> {
    let mut first = true;
    while !raw.is_empty() {
        let number = Number::parse(raw, 0)?;
        let length = usize::from(u16be(raw, 20)?);
        require(
            length >= ENTRY_BYTES && length <= raw.len() && length.is_multiple_of(8),
            "invalid GFS1 directory record length",
        )?;
        if number.formal == 0 {
            require(first, "misplaced GFS1 blank directory record")?;
        } else {
            let name_length = usize::from(u16be(raw, 22)?);
            require(
                number.block != 0
                    && (1..=255).contains(&name_length)
                    && ENTRY_BYTES + name_length <= length,
                "invalid GFS1 directory name or inode address",
            )?;
            let bytes = &raw[ENTRY_BYTES..ENTRY_BYTES + name_length];
            let hash = crc32fast::hash(bytes);
            require(
                hash == u32be(raw, 16)? && !bytes.iter().any(|&byte| matches!(byte, 0 | b'/')),
                "invalid GFS1 directory name or hash",
            )?;
            if let Some((depth, span)) = &bucket {
                let index = hash.checked_shr(32 - u32::from(*depth)).unwrap_or(0) as usize;
                require(
                    span.contains(&index),
                    "GFS1 directory name outside exhash bucket",
                )?;
            }
            let name = std::str::from_utf8(bytes)
                .map_err(|_| FoldError::Unsupported("non-UTF-8 GFS1 directory name"))?;
            let raw_kind = u16be(raw, 24)?;
            require(
                output.len() < MAX_ENTRIES
                    && output
                        .insert(
                            name.into(),
                            Entry {
                                number,
                                raw_kind,
                                kind: file_kind(raw_kind)?,
                            },
                        )
                        .is_none(),
                "duplicate or excessive GFS1 directory entries",
            )?;
        }
        first = false;
        raw = &raw[length..];
    }
    Ok(())
}
