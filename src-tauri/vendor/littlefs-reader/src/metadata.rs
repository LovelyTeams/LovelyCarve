use crc32fast::Hasher;
use lamfold::{FoldError, Result};

use crate::format::{Tag, TagType};
use crate::{Pair, le32, require};

#[derive(Clone, Copy)]
pub(crate) struct TagBody<'a> {
    pub tag: Tag,
    pub body: &'a [u8],
}

pub(crate) struct Block<'a> {
    pub revision: u32,
    pub tags: Vec<TagBody<'a>>,
}

impl<'a> Block<'a> {
    pub fn committed(bytes: &'a [u8]) -> Option<Self> {
        let revision_bytes = bytes.get(..4)?;
        let mut checksum = Hasher::new();
        checksum.update(revision_bytes);
        let mut offset = 4;
        let mut previous = u32::MAX;
        let mut tags = Vec::new();
        let mut committed = None;
        while let Some(encoded) = bytes.get(offset..offset + 4) {
            let tag = Tag(u32::from_be_bytes(encoded.try_into().ok()?) ^ previous);
            if !tag.is_valid() {
                break;
            }
            let Some(body) = bytes.get(offset + 4..offset + tag.dsize()) else {
                break;
            };
            checksum.update(encoded);
            previous = tag.0;
            if let Some(chunk) = tag.ccrc_chunk() {
                let finalized = !std::mem::take(&mut checksum).finalize();
                if body
                    .get(..4)
                    .is_none_or(|expected| le32(expected) != finalized)
                {
                    break;
                }
                committed = Some(tags.len());
                previous ^= u32::from(chunk & 1) << 31;
            } else {
                checksum.update(body);
                tags.push(TagBody { tag, body });
            }
            offset += tag.dsize();
        }
        tags.truncate(committed?);
        Some(Self {
            revision: le32(revision_bytes),
            tags,
        })
    }

    pub fn select(first: &'a [u8], second: &'a [u8]) -> Option<(usize, Self)> {
        match (Self::committed(first), Self::committed(second)) {
            (None, None) => None,
            (Some(a), Some(b)) if (b.revision.wrapping_sub(a.revision) as i32) > 0 => Some((1, b)),
            (Some(a), _) => Some((0, a)),
            (None, Some(b)) => Some((1, b)),
        }
    }

    pub fn latest(&self, matches: impl Fn(TagType) -> bool) -> Option<TagBody<'a>> {
        self.tags
            .iter()
            .rev()
            .copied()
            .find(|entry| matches(entry.tag.tag_type()))
    }

    pub fn tail(&self) -> Result<Option<(Pair, bool)>> {
        let Some(entry) = self.latest(|kind| matches!(kind, TagType::HardTail | TagType::SoftTail))
        else {
            return Ok(None);
        };
        let pair = decode_pair(entry.body)?;
        if pair == [u32::MAX; 2] {
            return Ok(None);
        }
        Ok(Some((pair, entry.tag.tag_type() == TagType::HardTail)))
    }
}

pub(crate) fn decode_pair(bytes: &[u8]) -> Result<Pair> {
    require(bytes.len() == 8, "invalid LittleFS metadata pair length")?;
    let pair = [le32(bytes), le32(&bytes[4..])];
    if pair != [u32::MAX; 2] && (pair[0] == pair[1] || pair.contains(&u32::MAX)) {
        return Err(FoldError::Corrupt(
            "invalid LittleFS metadata pair addresses",
        ));
    }
    Ok(pair)
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Slot<'a> {
    pub name: Option<(TagType, &'a [u8])>,
    pub layout: Option<(TagType, &'a [u8])>,
}

pub(crate) fn slots<'a>(block: &Block<'a>) -> Result<Vec<Slot<'a>>> {
    let mut slots = Vec::<Slot<'a>>::new();
    for entry in &block.tags {
        let kind = entry.tag.tag_type();
        let i = usize::from(entry.tag.id());
        match kind {
            TagType::Create => {
                require(
                    i <= slots.len() && slots.len() < 1023,
                    "invalid LittleFS create slot",
                )?;
                slots.insert(i, Slot::default());
            }
            TagType::Delete => {
                require(i < slots.len(), "invalid LittleFS delete slot")?;
                slots.remove(i);
            }
            TagType::RegularFile | TagType::Directory | TagType::Superblock => {
                require(i < 1023, "invalid LittleFS name slot")?;
                if i >= slots.len() {
                    slots.resize(i + 1, Slot::default());
                }
                slots[i].name = (!entry.tag.is_special_length()).then_some((kind, entry.body));
            }
            TagType::DirStruct | TagType::InlineStruct | TagType::CtzStruct => {
                require(i < 1023, "invalid LittleFS structure slot")?;
                if i >= slots.len() {
                    slots.resize(i + 1, Slot::default());
                }
                slots[i].layout = (!entry.tag.is_special_length()).then_some((kind, entry.body));
            }
            _ => {}
        }
    }
    Ok(slots)
}
