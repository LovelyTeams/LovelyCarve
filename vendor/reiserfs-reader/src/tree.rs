use crate::{exact, le16, le32, le64, require, Header};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::{HashSet, VecDeque};

/// CPU-order key. Directory/locality identifiers remain an inseparable pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Key {
    pub node: u64,
    pub offset: u64,
    pub kind: u8,
}
impl Key {
    fn parse(bytes: &[u8], version: Option<u16>) -> Result<Self> {
        let raw = le64(bytes, 8)?;
        // Internal delimiters have no version field. Linux's le_key_version
        // treats types 1..3 as v2, and stat-data/v1 uniqueness as v1.
        let version = version.unwrap_or(u16::from(matches!(raw >> 60, 1..=3)));
        let (offset, kind) = match version {
            0 => (
                u64::from(le32(bytes, 8)?),
                match le32(bytes, 12)? {
                    0 => 0,
                    0xffff_fffe => 1,
                    0xffff_ffff => 2,
                    500 => 3,
                    _ => return Err(FoldError::Unsupported("reiserfs: unknown v1 key type")),
                },
            ),
            1 => (raw & ((1u64 << 60) - 1), (raw >> 60) as u8),
            _ => {
                return Err(FoldError::Unsupported(
                    "reiserfs: unknown/unreachable item version",
                ))
            }
        };
        require(kind <= 3, "reiserfs: key type")?;
        Ok(Self {
            node: (u64::from(le32(bytes, 0)?) << 32) | u64::from(le32(bytes, 4)?),
            offset,
            kind,
        })
    }
}
#[derive(Clone)]
pub(crate) struct Item {
    pub key: Key,
    pub version: u16,
    pub count: u16,
    pub body: Vec<u8>,
}
pub(crate) struct Tree {
    bitmaps: VecDeque<(u32, Vec<u8>)>,
    pages: VecDeque<(u32, Vec<u8>)>,
}
impl Tree {
    pub fn new() -> Self {
        Self {
            bitmaps: VecDeque::new(),
            pages: VecDeque::new(),
        }
    }
    pub fn allocated(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        block: u32,
    ) -> Result<()> {
        header.data_block(block)?;
        let index = block / (header.block_size * 8);
        if !self.bitmaps.iter().any(|(cached, _)| *cached == index) {
            let mut bytes = vec![0; header.block_size as usize];
            exact(
                source,
                u64::from(header.bitmap(index)) * u64::from(header.block_size),
                &mut bytes,
            )?;
            if self.bitmaps.len() == 8 {
                self.bitmaps.pop_front();
            }
            self.bitmaps.push_back((index, bytes));
        }
        let bytes = &self
            .bitmaps
            .iter()
            .find(|(cached, _)| *cached == index)
            .ok_or(FoldError::Corrupt("reiserfs: missing bitmap"))?
            .1;
        let bit = block % (header.block_size * 8);
        require(
            bytes[bit as usize / 8] & (1 << (bit % 8)) != 0,
            "reiserfs: referenced free block",
        )
    }
    fn page(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        block: u32,
    ) -> Result<Vec<u8>> {
        self.allocated(source, header, block)?;
        if let Some((_, bytes)) = self.pages.iter().find(|(cached, _)| *cached == block) {
            return Ok(bytes.clone());
        }
        let mut bytes = vec![0; header.block_size as usize];
        exact(
            source,
            u64::from(block) * u64::from(header.block_size),
            &mut bytes,
        )?;
        if self.pages.len() == 64 {
            self.pages.pop_front();
        }
        self.pages.push_back((block, bytes.clone()));
        Ok(bytes)
    }
    /// Prune unrelated key ranges at every internal node. Only this object's
    /// items are materialized, with a 64 MiB cap; page caches stay bounded.
    pub fn object(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        node: u64,
    ) -> Result<Vec<Item>> {
        self.items(source, header, node, false)
    }
    pub fn stat(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        node: u64,
    ) -> Result<Vec<Item>> {
        self.items(source, header, node, true)
    }
    fn items(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        node: u64,
        stat_only: bool,
    ) -> Result<Vec<Item>> {
        require(
            node >> 32 != 0 && node as u32 != 0,
            "reiserfs: invalid object identity",
        )?;
        let mut stack = vec![(header.root, header.height - 1, None, None, None)];
        let mut visited = HashSet::new();
        let mut items = Vec::new();
        let mut materialized = 0usize;
        while let Some((block, expected_level, lower, upper, used)) = stack.pop() {
            require(
                visited.insert(block),
                "reiserfs: cyclic/repeated tree child",
            )?;
            if visited.len() > 65536 {
                return Err(FoldError::Unsupported(
                    "reiserfs: object tree exceeds traversal limit",
                ));
            }
            let bytes = self.page(source, header, block)?;
            let level = le16(&bytes, 0)?;
            let count = usize::from(le16(&bytes, 2)?);
            let free = usize::from(le16(&bytes, 4)?);
            require(
                level == expected_level && count > 0 && free <= bytes.len() - 24,
                "reiserfs: tree level/count/free space",
            )?;
            let actual_used = bytes.len() - 24 - free;
            if let Some(used) = used {
                require(
                    actual_used == used,
                    "reiserfs: parent child-used-size mismatch",
                )?;
            }
            let mut previous = None;
            if level == 1 {
                require(
                    count <= (bytes.len() - 24) / 24,
                    "reiserfs: leaf item count",
                )?;
                let mut body_end = bytes.len();
                for index in 0..count {
                    let position = 24 + index * 24;
                    let version = le16(&bytes, position + 22)?;
                    let key = Key::parse(&bytes[position..position + 16], Some(version))?;
                    require(
                        index != 0 || lower.is_none_or(|lower| key == lower),
                        "reiserfs: delimiter is not child minimum key",
                    )?;
                    require(
                        previous.is_none_or(|previous| previous < key)
                            && lower.is_none_or(|lower| key >= lower)
                            && upper.is_none_or(|upper| key < upper),
                        "reiserfs: leaf keys/order/parent bounds",
                    )?;
                    previous = Some(key);
                    let length = usize::from(le16(&bytes, position + 18)?);
                    let location = usize::from(le16(&bytes, position + 20)?);
                    require(
                        length > 0
                            && location >= 24 + count * 24
                            && location.checked_add(length) == Some(body_end),
                        "reiserfs: leaf body bounds/overlap/packing",
                    )?;
                    body_end = location;
                    if key.node == node && (!stat_only || key.kind == 0) {
                        materialized += length + 24;
                        if materialized > 64 * 1024 * 1024 {
                            return Err(FoldError::Unsupported(
                                "reiserfs: object metadata exceeds 64 MiB limit",
                            ));
                        }
                        items.push(Item {
                            key,
                            version,
                            count: le16(&bytes, position + 16)?,
                            body: bytes[location..location + length].to_vec(),
                        });
                    }
                }
                require(
                    body_end == 24 + count * 24 + free,
                    "reiserfs: leaf free-space mismatch",
                )?;
            } else {
                require(
                    level <= 5
                        && count <= (bytes.len() - 32) / 24
                        && actual_used == count * 16 + (count + 1) * 8,
                    "reiserfs: internal node geometry",
                )?;
                let mut keys = Vec::with_capacity(count);
                for index in 0..count {
                    let key = Key::parse(&bytes[24 + index * 16..40 + index * 16], None)?;
                    require(
                        previous.is_none_or(|previous| previous < key)
                            && lower.is_none_or(|lower| key >= lower)
                            && upper.is_none_or(|upper| key < upper),
                        "reiserfs: internal key order/parent bounds",
                    )?;
                    previous = Some(key);
                    keys.push(key);
                }
                let mut children = HashSet::new();
                for index in (0..=count).rev() {
                    let position = 24 + count * 16 + index * 8;
                    let child = le32(&bytes, position)?;
                    header.data_block(child)?;
                    require(children.insert(child), "reiserfs: duplicate child pointer")?;
                    let child_lower = if index == 0 {
                        lower
                    } else {
                        Some(keys[index - 1])
                    };
                    let child_upper = keys.get(index).copied().or(upper);
                    // Inclusive on the upper object's identity: it may span
                    // several leaves even when a delimiter has the same ID.
                    let relevant = if stat_only {
                        let target = Key {
                            node,
                            offset: 0,
                            kind: 0,
                        };
                        child_lower.is_none_or(|key| key <= target)
                            && child_upper.is_none_or(|key| key > target)
                    } else {
                        child_lower.is_none_or(|key| key.node <= node)
                            && child_upper.is_none_or(|key| key.node >= node)
                    };
                    if relevant {
                        stack.push((
                            child,
                            level - 1,
                            child_lower,
                            child_upper,
                            Some(usize::from(le16(&bytes, position + 4)?)),
                        ));
                    }
                }
            }
        }
        require(
            items.windows(2).all(|pair| pair[0].key < pair[1].key),
            "reiserfs: overlapping object leaves",
        )?;
        Ok(items)
    }
}
