use crate::{le16, le32, le64, require, tree::Item};
use lamfold::{DirEntry, FileKind, FileTimes, FoldError, Result};
use std::collections::HashSet;

#[derive(Clone)]
pub(crate) struct Stat {
    pub mode: u32,
    pub size: u64,
    pub times: FileTimes,
}
impl Stat {
    pub fn parse(items: &[Item]) -> Result<Self> {
        let item = items.first().ok_or(FoldError::NotFound)?;
        require(
            item.key.kind == 0 && item.key.offset == 0,
            "reiserfs: missing stat-data key",
        )?;
        let body = &item.body;
        let (size, time_offset, links) = match item.version {
            0 => {
                require(body.len() == 32, "reiserfs: v1 stat-data size")?;
                (u64::from(le32(body, 8)?), 12, u32::from(le16(body, 2)?))
            }
            1 => {
                require(body.len() == 44, "reiserfs: v2 stat-data size")?;
                (le64(body, 8)?, 24, le32(body, 4)?)
            }
            _ => return Err(FoldError::Unsupported("reiserfs: stat-data version")),
        };
        require(
            links > 0 && size <= i64::MAX as u64,
            "reiserfs: unlinked/invalid-size object",
        )?;
        Ok(Self {
            mode: u32::from(le16(body, 0)?),
            size,
            times: FileTimes {
                accessed: Some((i64::from(le32(body, time_offset)?), 0)),
                modified: Some((i64::from(le32(body, time_offset + 4)?), 0)),
                changed: Some((i64::from(le32(body, time_offset + 8)?), 0)),
                created: None,
            },
        })
    }
    pub fn kind(&self) -> FileKind {
        match self.mode & 0xf000 {
            0x8000 => FileKind::Regular,
            0x4000 => FileKind::Directory,
            0xa000 => FileKind::Symlink,
            _ => FileKind::Other,
        }
    }
}
pub(crate) struct Directory {
    pub entries: Vec<DirEntry>,
    pub parent: u64,
}
pub(crate) fn directory(node: u64, stat: &Stat, items: &[Item]) -> Result<Directory> {
    require(
        stat.kind() == FileKind::Directory,
        "reiserfs: not a directory",
    )?;
    let mut entries = Vec::new();
    let mut names = HashSet::new();
    let mut dot = None;
    let mut parent = None;
    let mut total = 0u64;
    let mut previous_offset = 0;
    for item in items.iter().skip(1) {
        require(
            item.key.kind == 3 && item.version == 0,
            "reiserfs: non-directory item in directory",
        )?;
        let count = usize::from(item.count);
        require(
            count > 0 && count <= item.body.len() / 16,
            "reiserfs: directory header count",
        )?;
        let mut name_end = item.body.len();
        for index in 0..count {
            let position = index * 16;
            let offset = le32(&item.body, position)?;
            require(
                offset > previous_offset && (index != 0 || u64::from(offset) == item.key.offset),
                "reiserfs: directory hash/order/item key",
            )?;
            previous_offset = offset;
            let target = (u64::from(le32(&item.body, position + 4)?) << 32)
                | u64::from(le32(&item.body, position + 8)?);
            let location = usize::from(le16(&item.body, position + 12)?);
            let state = le16(&item.body, position + 14)?;
            if state & 1 != 0 {
                return Err(FoldError::Unsupported(
                    "reiserfs: embedded directory stat-data",
                ));
            }
            require(
                state & !4 == 0 && location >= count * 16 && location < name_end,
                "reiserfs: directory name/state bounds",
            )?;
            let raw = &item.body[location..name_end];
            name_end = location;
            let length = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
            require(
                length > 0 && length <= 255 && raw[length..].iter().all(|byte| *byte == 0),
                "reiserfs: name length/padding",
            )?;
            if state & 4 == 0 {
                continue;
            }
            let name = std::str::from_utf8(&raw[..length]).map_err(|_| {
                FoldError::Unsupported(
                    "reiserfs: non-UTF-8 filename cannot be represented losslessly",
                )
            })?;
            require(
                (target >> 32 != 0 && target as u32 != 0)
                    || (node == (1u64 << 32) | 2 && name == ".." && target == 1),
                "reiserfs: directory target identity",
            )?;
            require(!name.contains('/'), "reiserfs: slash in name")?;
            require(
                names.insert(name.to_owned()),
                "reiserfs: duplicate visible name",
            )?;
            match name {
                "." => {
                    require(offset == 1 && target == node, "reiserfs: dot identity")?;
                    dot = Some(target);
                }
                ".." => {
                    require(offset == 2, "reiserfs: dotdot offset")?;
                    parent = Some(target);
                }
                _ => {
                    require(
                        offset > 2 && target != node,
                        "reiserfs: directory self edge",
                    )?;
                    entries.push(DirEntry {
                        name: name.to_owned(),
                        node: target,
                        kind: FileKind::Other,
                    });
                }
            }
            if entries.len() > 1_000_000 {
                return Err(FoldError::Unsupported(
                    "reiserfs: directory exceeds entry limit",
                ));
            }
        }
        require(
            name_end == count * 16,
            "reiserfs: unaccounted directory name bytes",
        )?;
        total += item.body.len() as u64;
    }
    require(
        total == stat.size && dot.is_some() && parent.is_some(),
        "reiserfs: directory size/dot entries",
    )?;
    Ok(Directory {
        entries,
        parent: parent.ok_or(FoldError::Corrupt("reiserfs: missing parent"))?,
    })
}

pub(crate) enum Content {
    Direct(Vec<u8>),
    Block(u32),
}
pub(crate) struct Segment {
    pub start: u64,
    pub length: u64,
    pub content: Content,
}
pub(crate) fn content(stat: &Stat, items: &[Item], block_size: u32) -> Result<Vec<Segment>> {
    require(
        matches!(stat.kind(), FileKind::Regular | FileKind::Symlink),
        "reiserfs: object has no readable content",
    )?;
    let mut segments: Vec<Segment> = Vec::new();
    let mut next = 0u64;
    for item in items.iter().skip(1) {
        require(
            item.key.offset > 0 && item.key.offset - 1 == next && next < stat.size,
            "reiserfs: file gap/overlap/extra item",
        )?;
        match item.key.kind {
            1 => {
                require(
                    next.is_multiple_of(u64::from(block_size)) && item.body.len().is_multiple_of(4),
                    "reiserfs: indirect alignment",
                )?;
                let free = if item.version == 0 {
                    u64::from(item.count)
                } else {
                    0
                };
                require(
                    free < u64::from(block_size),
                    "reiserfs: indirect free-space bounds",
                )?;
                for index in 0..item.body.len() / 4 {
                    let block = le32(&item.body, index * 4)?;
                    let length = u64::from(block_size)
                        - if index + 1 == item.body.len() / 4 {
                            free
                        } else {
                            0
                        };
                    require(next < stat.size, "reiserfs: indirect pointer beyond EOF")?;
                    let contiguous = segments.last().is_some_and(|last| match last.content {
                        Content::Block(0) => block == 0,
                        Content::Block(previous) => {
                            block != 0
                                && last.length.is_multiple_of(u64::from(block_size))
                                && u64::from(previous) + last.length / u64::from(block_size)
                                    == u64::from(block)
                        }
                        Content::Direct(_) => false,
                    });
                    if contiguous {
                        if let Some(last) = segments.last_mut() {
                            last.length += length;
                        }
                    } else {
                        if segments.len() == 1_000_000 {
                            return Err(FoldError::Unsupported(
                                "reiserfs: file map exceeds one million fragments",
                            ));
                        }
                        segments.push(Segment {
                            start: next,
                            length,
                            content: Content::Block(block),
                        });
                    }
                    next = next
                        .checked_add(length)
                        .ok_or(FoldError::Corrupt("reiserfs: logical offset overflow"))?;
                }
            }
            2 => {
                // Tail and symlink items can be padded to eight bytes. Only
                // bytes inside the recorded logical length are returned.
                let length = item.body.len() as u64;
                require(
                    next / u64::from(block_size) == (next + length - 1) / u64::from(block_size),
                    "reiserfs: direct item crosses tail block",
                )?;
                segments.push(Segment {
                    start: next,
                    length,
                    content: Content::Direct(item.body.clone()),
                });
                next += length;
            }
            _ => return Err(FoldError::Corrupt("reiserfs: invalid file item type")),
        }
    }
    require(
        next >= stat.size,
        "reiserfs: incomplete file logical coverage",
    )?;
    if let Some(last) = segments.last() {
        let excess = next - stat.size;
        match last.content {
            Content::Direct(_) => require(excess < 8, "reiserfs: excessive direct tail padding")?,
            Content::Block(_) => require(
                excess < u64::from(block_size),
                "reiserfs: excessive indirect tail",
            )?,
        }
    }
    let mut physical: Vec<_> = segments
        .iter()
        .filter_map(|segment| match segment.content {
            Content::Block(block) if block != 0 => Some((
                u64::from(block),
                u64::from(block) + segment.length.div_ceil(u64::from(block_size)),
            )),
            Content::Block(_) | Content::Direct(_) => None,
        })
        .collect();
    physical.sort_unstable();
    require(
        physical.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "reiserfs: duplicate/overlapping physical content blocks",
    )?;
    Ok(segments)
}
