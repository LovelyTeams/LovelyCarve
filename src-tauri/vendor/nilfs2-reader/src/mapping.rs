//! Checked NILFS direct/B-tree records. Pointers are interpreted by the caller:
//! DAT uses physical blocks; ordinary and other metadata files use virtual ones.
use crate::{le16, le64, require};
use lamfold::{FoldError, Result};

pub(crate) struct Node {
    pub level: u8,
    pub keys: Vec<u64>,
    pub pointers: Vec<u64>,
}
impl Node {
    pub fn parse(raw: &[u8], root: bool, level: Option<u8>) -> Result<Self> {
        let start = if root { 8 } else { 16 };
        require(
            raw.len() >= start && raw[0] == u8::from(root),
            "nilfs2: B-tree node flags/header",
        )?;
        let actual_level = raw[1];
        require(
            (1..14).contains(&actual_level) && level.is_none_or(|level| actual_level == level),
            "nilfs2: B-tree level",
        )?;
        let capacity = (raw.len() - start) / 16;
        let used = le16(raw, 2)? as usize;
        require(
            (used != 0 || root && actual_level == 1) && used <= capacity,
            "nilfs2: B-tree child count",
        )?;
        let mut keys = Vec::with_capacity(used);
        let mut pointers = Vec::with_capacity(used);
        for index in 0..used {
            let key = le64(raw, start + index * 8)?;
            let pointer = le64(raw, start + capacity * 8 + index * 8)?;
            require(
                pointer != 0 && keys.last().is_none_or(|previous| *previous < key),
                "nilfs2: B-tree zero pointer/key order",
            )?;
            keys.push(key);
            pointers.push(pointer);
        }
        Ok(Self {
            level: actual_level,
            keys,
            pointers,
        })
    }
    pub fn child(&self, key: u64) -> Option<(u64, u64, Option<u64>)> {
        let index = self
            .keys
            .partition_point(|first| *first <= key)
            .checked_sub(1)?;
        Some((
            self.keys[index],
            self.pointers[index],
            self.keys.get(index + 1).copied(),
        ))
    }
}
pub(crate) enum Root {
    Direct([u64; 6]),
    Tree(Node),
}
impl Root {
    pub fn parse(raw: &[u8]) -> Result<Self> {
        let raw = raw
            .get(56..112)
            .ok_or(FoldError::Corrupt("nilfs2: inode bmap span"))?;
        if raw[0] & 1 != 0 {
            return Ok(Self::Tree(Node::parse(raw, true, None)?));
        }
        require(raw[0] == 0, "nilfs2: direct-map flags")?;
        let mut pointers = [0; 6];
        for (index, pointer) in pointers.iter_mut().enumerate() {
            *pointer = le64(raw, 8 + index * 8)?;
        }
        Ok(Self::Direct(pointers))
    }
}
