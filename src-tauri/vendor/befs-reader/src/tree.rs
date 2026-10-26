use crate::{require, streams::Map, Header};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::{HashMap, HashSet};

pub(crate) struct Directory {
    pub entries: Vec<(String, u64)>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Names,
    Attributes,
}
struct Node {
    left: u64,
    right: u64,
    overflow: u64,
    keys: Vec<Vec<u8>>,
    values: Vec<u64>,
}
const NULL: u64 = u64::MAX;
impl Directory {
    pub fn read(
        source: &mut impl BlockSource,
        header: &Header,
        map: &Map,
        owner: u64,
        parent: u64,
        role: Role,
    ) -> Result<Self> {
        let endian = header.endian;
        let mut raw = [0; 40];
        map.exact(source, header, 0, &mut raw)?;
        let node_size = endian.u32(&raw, 4)?;
        let levels = endian.u32(&raw, 8)?;
        require(
            endian.u32(&raw, 0)? == 0x69f6c2e8 && endian.u32(&raw, 12)? == 0,
            "befs: directory B+tree magic/key type",
        )?;
        require(
            (1024..=65536).contains(&node_size)
                && node_size.is_power_of_two()
                && (1..=64).contains(&levels),
            "befs: B+tree node size/height",
        )?;
        let maximum = endian.nonnegative(&raw, 32)?;
        let root = endian.nonnegative(&raw, 16)?;
        let free = endian.u64(&raw, 24)?;
        require(
            maximum <= map.length
                && maximum >= 2 * u64::from(node_size)
                && maximum.is_multiple_of(u64::from(node_size)),
            "befs: B+tree maximum size",
        )?;
        let valid = |pointer: u64| {
            pointer >= u64::from(node_size)
                && pointer.is_multiple_of(u64::from(node_size))
                && pointer
                    .checked_add(u64::from(node_size))
                    .is_some_and(|end| end <= maximum)
        };
        require(
            valid(root) && (free == NULL || valid(free)),
            "befs: B+tree root/free pointer",
        )?;
        let mut pending = vec![(root, levels, None::<Vec<u8>>, None::<Vec<u8>>)];
        let mut visited = HashSet::new();
        let mut all = Vec::new();
        let mut siblings = HashMap::<u32, Vec<(u64, u64, u64)>>::new();
        while let Some((pointer, level, lower, upper)) = pending.pop() {
            require(
                valid(pointer) && visited.insert(pointer) && visited.len() + pending.len() <= 65536,
                "befs: B+tree node bounds/cycle/resource limit",
            )?;
            let mut raw = vec![0; node_size as usize];
            map.exact(source, header, pointer, &mut raw)?;
            let node = Node::parse(header, &raw)?;
            require(
                (node.left == NULL || valid(node.left))
                    && (node.right == NULL || valid(node.right)),
                "befs: B+tree sibling bounds",
            )?;
            siblings
                .entry(level)
                .or_default()
                .push((pointer, node.left, node.right));
            for key in &node.keys {
                require(
                    lower.as_ref().is_none_or(|lower| key > lower)
                        && upper.as_ref().is_none_or(|upper| key <= upper),
                    "befs: B+tree child key bounds",
                )?;
            }
            if node.overflow == NULL {
                require(level == 1, "befs: B+tree premature leaf")?;
                all.extend(node.keys.into_iter().zip(node.values));
                require(all.len() <= 1_000_000, "befs: directory entry limit")?;
            } else {
                require(
                    level > 1 && valid(node.overflow),
                    "befs: B+tree level/overflow pointer",
                )?;
                pending.push((
                    node.overflow,
                    level - 1,
                    node.keys.last().cloned().or_else(|| lower.clone()),
                    upper,
                ));
                for index in (0..node.keys.len()).rev() {
                    require(valid(node.values[index]), "befs: B+tree child pointer")?;
                    let lower = if index == 0 {
                        lower.clone()
                    } else {
                        Some(node.keys[index - 1].clone())
                    };
                    pending.push((
                        node.values[index],
                        level - 1,
                        lower,
                        Some(node.keys[index].clone()),
                    ));
                }
            }
        }
        for nodes in siblings.values() {
            for (index, &(_, left, right)) in nodes.iter().enumerate() {
                require(
                    left == index
                        .checked_sub(1)
                        .map_or(NULL, |previous| nodes[previous].0)
                        && right == nodes.get(index + 1).map_or(NULL, |next| next.0),
                    "befs: B+tree sibling chain disagrees with child tree",
                )?;
            }
        }
        require(
            all.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "befs: B+tree duplicate/unsorted directory key",
        )?;
        // Free nodes are not namespace entries, but their chain must neither
        // loop nor overlap any reachable tree node.
        let mut free = free;
        let mut free_seen = HashSet::new();
        while free != NULL {
            require(
                valid(free)
                    && !visited.contains(&free)
                    && free_seen.insert(free)
                    && free_seen.len() + visited.len() <= 65536,
                "befs: B+tree free chain bounds/cycle/overlap",
            )?;
            let mut bytes = [0; 24];
            map.exact(source, header, free, &mut bytes)?;
            require(
                endian.u64(&bytes, 16)? == u64::MAX - 1,
                "befs: free node marker",
            )?;
            free = endian.u64(&bytes, 0)?;
        }
        let mut entries = Vec::with_capacity(all.len());
        let mut dot = false;
        let mut parent_dot = false;
        for (name, value) in all {
            require(
                value > 0 && value < header.blocks,
                "befs: directory inode pointer/duplicate link tag",
            )?;
            header.data_region(value, 1)?;
            require(
                !name.is_empty()
                    && name.len() <= 255
                    && !name.contains(&0)
                    && (role == Role::Attributes || !name.contains(&b'/')),
                "befs: invalid directory name",
            )?;
            if role == Role::Names && name == b"." {
                require(value == owner, "befs: dot identity")?;
                dot = true;
                continue;
            }
            if role == Role::Names && name == b".." {
                require(value == parent, "befs: parent dot identity")?;
                parent_dot = true;
                continue;
            }
            entries.push((
                String::from_utf8(name)
                    .map_err(|_| FoldError::Unsupported("befs: non-UTF-8 directory name"))?,
                value,
            ));
        }
        require(
            role == Role::Attributes || (dot && parent_dot),
            "befs: directory missing dot entries",
        )?;
        Ok(Self { entries })
    }
}
impl Node {
    fn parse(header: &Header, raw: &[u8]) -> Result<Self> {
        let endian = header.endian;
        let count = usize::from(endian.u16(raw, 24)?);
        let key_bytes = usize::from(endian.u16(raw, 26)?);
        let lengths = (28 + key_bytes).next_multiple_of(8);
        let values = lengths + count * 2;
        require(
            values + count * 8 <= raw.len(),
            "befs: B+tree key/length/value boundaries",
        )?;
        let mut keys = Vec::with_capacity(count);
        let mut pointers = Vec::with_capacity(count);
        let mut start = 0;
        for index in 0..count {
            let end = usize::from(endian.u16(raw, lengths + index * 2)?);
            require(
                end > start && end <= key_bytes && end - start <= 256,
                "befs: B+tree cumulative key length",
            )?;
            keys.push(raw[28 + start..28 + end].to_vec());
            pointers.push(endian.u64(raw, values + index * 8)?);
            start = end;
        }
        require(
            start == key_bytes && keys.windows(2).all(|pair| pair[0] < pair[1]),
            "befs: B+tree key byte count/order",
        )?;
        Ok(Self {
            left: endian.u64(raw, 0)?,
            right: endian.u64(raw, 8)?,
            overflow: endian.u64(raw, 16)?,
            keys,
            values: pointers,
        })
    }
}
