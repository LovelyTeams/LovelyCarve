use crate::{
    format::BLOCK_SIZE,
    le16, le32, le64, require,
    tree::{Item, Key, COMPRESSED_TAIL, DIRECTORY, EXTENT, STAT, TAIL},
};
use lamfold::{FileKind, FileTimes, FoldError, Metadata, Result};
use std::collections::HashSet;

pub(crate) struct Stat {
    pub(crate) key: Key,
    pub(crate) metadata: Metadata,
    pub(crate) times: FileTimes,
    pub(crate) target: Option<Vec<u8>>,
    pub(crate) encrypted: bool,
}

impl Stat {
    pub(crate) fn parse(item: &Item) -> Result<Self> {
        require(
            item.plugin == STAT && item.key.minor() == 1 && item.key.0[3] == 0,
            "reiser4: invalid stat-data key or plugin",
        )?;
        let bytes = item.bytes();
        let mask = le16(bytes, 0)?;
        if mask & !0x01bf != 0 {
            return Err(FoldError::Unsupported("reiser4: stat-data extensions"));
        }
        require(mask & 1 != 0, "reiser4: missing lightweight stat-data")?;
        let mode = u32::from(le16(bytes, 2)?);
        let kind = match mode & 0xf000 {
            0x4000 => FileKind::Directory,
            0x8000 => FileKind::Regular,
            0xa000 => FileKind::Symlink,
            _ => FileKind::Other,
        };
        let size = le64(bytes, 8)?;
        require(le32(bytes, 4)? > 0, "reiser4: stat-data has no links")?;
        let mut cursor = 16;
        let mut times = FileTimes::default();
        if mask & 2 != 0 {
            times = times_at(bytes, cursor + 8)?;
            cursor += 28;
        }
        if mask & 4 != 0 {
            times = times_at(bytes, cursor)?;
            cursor += 12;
        }
        let target = if mask & 8 != 0 {
            require(
                kind == FileKind::Symlink && size > 0 && size < BLOCK_SIZE as u64,
                "reiser4: symlink geometry",
            )?;
            let size = size as usize;
            let raw = bytes
                .get(cursor..cursor + size + 1)
                .ok_or(FoldError::Corrupt("reiser4: truncated symlink"))?;
            require(
                raw[size] == 0 && !raw[..size].contains(&0),
                "reiser4: invalid symlink target",
            )?;
            cursor += size + 1;
            Some(raw[..size].to_vec())
        } else {
            require(kind != FileKind::Symlink, "reiser4: missing symlink target")?;
            None
        };
        let mut encrypted = mask & 128 != 0;
        if mask & 16 != 0 {
            let count = usize::from(le16(bytes, cursor)?);
            cursor += 2;
            require((1..=14).contains(&count), "reiser4: stat-data plugin count")?;
            let mut seen = 0u16;
            for _ in 0..count {
                let slot = le16(bytes, cursor)?;
                let plugin = le16(bytes, cursor + 2)?;
                cursor += 4;
                require(
                    slot < 14 && seen & (1 << slot) == 0,
                    "reiser4: stat-data plugin slot",
                )?;
                seen |= 1 << slot;
                if slot == 8 && plugin != 0 {
                    encrypted = true;
                }
            }
        }
        if mask & 32 != 0 {
            cursor += 4;
        }
        if !encrypted && mask & 256 != 0 {
            let count = usize::from(le16(bytes, cursor)?);
            require(
                (1..=14).contains(&count),
                "reiser4: stat-data inherited plugin count",
            )?;
            cursor += 2 + 4 * count;
        }
        require(
            cursor <= bytes.len() && (encrypted || cursor == bytes.len()),
            "reiser4: stat-data length",
        )?;
        Ok(Self {
            key: item.key,
            metadata: Metadata { kind, size, mode },
            times,
            target,
            encrypted,
        })
    }
}

fn times_at(bytes: &[u8], offset: usize) -> Result<FileTimes> {
    Ok(FileTimes {
        accessed: Some((i64::from(le32(bytes, offset)?), 0)),
        modified: Some((i64::from(le32(bytes, offset + 4)?), 0)),
        changed: Some((i64::from(le32(bytes, offset + 8)?), 0)),
        created: None,
    })
}

pub(crate) struct Directory {
    pub(crate) parent: Key,
    pub(crate) entries: Vec<(String, Key)>,
}

impl Directory {
    pub(crate) fn parse(key: Key, large: bool, items: &[Item]) -> Result<Self> {
        let words = if large { 3 } else { 2 };
        let object_size = words * 8;
        let header_size = object_size + 2;
        let mut names = HashSet::new();
        let mut parent = None;
        let mut dot = false;
        let mut result = Vec::new();
        for item in items {
            if item.plugin != DIRECTORY {
                return Err(FoldError::Unsupported("reiser4: directory item plugin"));
            }
            let bytes = item.bytes();
            let count = usize::from(le16(bytes, 0)?);
            let start = 2 + count * header_size;
            require(
                count > 0 && start < bytes.len(),
                "reiser4: directory item header bounds",
            )?;
            let mut previous_end = start;
            for index in 0..count {
                let entry = 2 + index * header_size;
                let body = usize::from(le16(bytes, entry + object_size)?);
                let end = if index + 1 < count {
                    usize::from(le16(bytes, entry + header_size + object_size)?)
                } else {
                    bytes.len()
                };
                require(
                    body == previous_end && end <= bytes.len() && body + object_size <= end,
                    "reiser4: directory record bounds",
                )?;
                previous_end = end;
                let mut target_bytes = [0; 32];
                target_bytes[..object_size].copy_from_slice(&bytes[body..body + object_size]);
                let target = Key::parse(&target_bytes, large)?;
                require(
                    target.minor() == 1 && target.object_number() != 0,
                    "reiser4: directory target key",
                )?;
                let name = name(bytes, entry, object_size, body + object_size, end)?;
                match name.as_slice() {
                    b"." => {
                        require(!dot && target == key, "reiser4: directory self reference")?;
                        dot = true;
                    }
                    b".." => {
                        require(
                            parent.replace(target).is_none(),
                            "reiser4: duplicate directory parent",
                        )?;
                    }
                    _ => {
                        require(
                            !name.is_empty() && name.len() <= 255 && !name.contains(&b'/'),
                            "reiser4: invalid directory name",
                        )?;
                        let name = String::from_utf8(name).map_err(|_| {
                            FoldError::Unsupported("reiser4: non-UTF-8 directory name")
                        })?;
                        require(
                            names.insert(name.clone()),
                            "reiser4: duplicate directory name",
                        )?;
                        result.push((name, target));
                    }
                }
            }
        }
        require(dot, "reiser4: missing directory self reference")?;
        let parent = parent.ok_or(FoldError::Corrupt("reiser4: missing directory parent"))?;
        Ok(Self {
            parent,
            entries: result,
        })
    }
}

fn name(bytes: &[u8], entry: usize, key_size: usize, start: usize, end: usize) -> Result<Vec<u8>> {
    let first = le64(bytes, entry)?;
    if first & 0x0100_0000_0000_0000 != 0 {
        let name = bytes[start..end]
            .strip_suffix(&[0])
            .ok_or(FoldError::Corrupt("reiser4: missing long-name terminator"))?;
        require(!name.contains(&0), "reiser4: embedded long-name terminator")?;
        return Ok(name.to_vec());
    }
    require(start == end, "reiser4: unexpected inline name storage")?;
    let mut name = Vec::with_capacity(key_size);
    name.extend(
        first.to_be_bytes()[1..]
            .iter()
            .copied()
            .filter(|byte| *byte != 0),
    );
    for field in (8..key_size).step_by(8) {
        name.extend(
            le64(bytes, entry + field)?
                .to_be_bytes()
                .into_iter()
                .filter(|byte| *byte != 0),
        );
    }
    if name.is_empty() {
        name.push(b'.');
    }
    Ok(name)
}

pub(crate) enum Content {
    Inline(Vec<u8>),
    Blocks(u64),
    Hole,
}

pub(crate) struct Segment {
    pub(crate) start: u64,
    pub(crate) end: u64,
    pub(crate) content: Content,
}

pub(crate) fn file_segments(items: Vec<Item>, size: u64) -> Result<Vec<Segment>> {
    let mut covered = 0u64;
    let mut segments = Vec::new();
    for item in items {
        let start = item.key.0[3];
        require(
            start == covered && start < size,
            "reiser4: missing, overlapping or trailing data item",
        )?;
        match item.plugin {
            TAIL => {
                let length = item.bytes().len() as u64;
                let end = start
                    .checked_add(length)
                    .filter(|end| *end <= size)
                    .ok_or(FoldError::Corrupt("reiser4: tail length"))?;
                segments.push(Segment {
                    start,
                    end,
                    content: Content::Inline(item.bytes().to_vec()),
                });
                covered = end;
            }
            EXTENT => {
                let (runs, remainder) = item.bytes().as_chunks::<16>();
                require(
                    start % BLOCK_SIZE as u64 == 0 && remainder.is_empty(),
                    "reiser4: extent item geometry",
                )?;
                for run in runs {
                    let block = le64(run, 0)?;
                    let blocks = le64(run, 8)?;
                    let end = blocks
                        .checked_mul(BLOCK_SIZE as u64)
                        .and_then(|length| covered.checked_add(length))
                        .ok_or(FoldError::Corrupt("reiser4: extent length overflow"))?;
                    require(
                        blocks > 0 && covered < size && end - (BLOCK_SIZE as u64) < size,
                        "reiser4: extent exceeds file length",
                    )?;
                    let content = match block {
                        0 => Content::Hole,
                        1 => return Err(FoldError::Corrupt("reiser4: uncommitted extent")),
                        _ => Content::Blocks(block),
                    };
                    let end = end.min(size);
                    segments.push(Segment {
                        start: covered,
                        end,
                        content,
                    });
                    covered = end;
                }
            }
            COMPRESSED_TAIL => {
                return Err(FoldError::Unsupported("reiser4: compressed file items"))
            }
            _ => return Err(FoldError::Unsupported("reiser4: file item plugin")),
        }
    }
    require(covered == size, "reiser4: incomplete file extent coverage")?;
    Ok(segments)
}
