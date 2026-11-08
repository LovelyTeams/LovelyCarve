//! Slot-linked UTF-16 directory trees, including partially allocated external
//! leaves and both persistent-index and legacy leaf-head layouts.
use crate::{
    exact, le16, le32, le64,
    metadata::{Header, Physical},
    require,
};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub name: String,
    pub units: Vec<u16>,
    pub inode: u32,
    pub index: Option<u32>,
    pub page: u64,
    pub slot: u8,
}
pub(crate) struct Directory {
    pub entries: Vec<Entry>,
    pub pages: Vec<Physical>,
}
impl Directory {
    pub fn read(source: &mut impl BlockSource, header: &Header, root: &[u8]) -> Result<Self> {
        let mut walker = Walker {
            source,
            header,
            seen: HashSet::new(),
            levels: BTreeMap::new(),
            units: 0,
            directory: Self {
                entries: Vec::new(),
                pages: Vec::new(),
            },
        };
        walker.visit(root, None, 0)?;
        require(
            walker.levels.values().all(|(_, next, _)| *next == 0),
            "jfs: unterminated dtree siblings",
        )?;
        for pair in walker.directory.entries.windows(2) {
            require(
                pair[0].units < pair[1].units,
                "jfs: duplicate/unordered cross-leaf names",
            )?;
        }
        Ok(walker.directory)
    }
}
struct Walker<'a, S> {
    source: &'a mut S,
    header: &'a Header,
    seen: HashSet<u64>,
    levels: BTreeMap<usize, (u64, u64, Vec<u16>)>,
    units: usize,
    directory: Directory,
}
impl<S: BlockSource> Walker<'_, S> {
    fn visit(
        &mut self,
        bytes: &[u8],
        address: Option<Physical>,
        depth: usize,
    ) -> Result<(Vec<u16>, Vec<u16>)> {
        require(
            depth <= 8 && bytes.len() >= 32,
            "jfs: dtree depth/header size",
        )?;
        let leaf = bytes[16] & 2 != 0;
        let count = usize::from(bytes[17]);
        let (slots, table, table_slots) = match address {
            None => (9, 24, 0),
            Some(expected) => {
                let slots = usize::from(bytes[20]);
                let table = usize::from(bytes[21]);
                let table_slots = slots.div_ceil(32);
                require(
                    matches!(slots, 16 | 32 | 64 | 128)
                        && slots * 32 == bytes.len()
                        && table > 0
                        && table + table_slots <= slots
                        && Physical::parse(bytes, 24, self.header)? == expected,
                    "jfs: dtree page/table/self geometry",
                )?;
                (slots, table * 32, table_slots)
            }
        };
        require(
            bytes[16] & !0xb7 == 0
                && bytes[16] & 6 == if leaf { 2 } else { 4 }
                && bytes[16] & 1 == u8::from(address.is_none())
                && slots * 32 <= bytes.len()
                && count <= if address.is_none() { 8 } else { slots }
                && table + count <= slots * 32,
            "jfs: dtree node flags/count/table",
        )?;
        let mut occupied = [false; 128];
        occupied[0] = true;
        if address.is_some() {
            occupied[table / 32..table / 32 + table_slots].fill(true);
        }
        let mut minimum = None;
        let mut previous = None;
        let mut maximum: Option<Vec<u16>> = None;
        for order in 0..count {
            let slot = usize::from(bytes[table + order]);
            require(
                slot < slots && !occupied[slot],
                "jfs: duplicate/out-of-range dtree slot",
            )?;
            occupied[slot] = true;
            let head = &bytes[slot * 32..slot * 32 + 32];
            let (name_at, next_at, head_units) = if leaf {
                (
                    6,
                    4,
                    if self.header.flags & 0x00200000 != 0 {
                        11
                    } else {
                        13
                    },
                )
            } else {
                (10, 8, 11)
            };
            let length = usize::from(head[next_at + 1]);
            require(!leaf || length != 0, "jfs: empty filename")?;
            self.units += length;
            require(
                self.units <= 8 * 1024 * 1024,
                "jfs: directory name memory limit",
            )?;
            let mut name = Vec::with_capacity(length);
            for index in 0..length.min(head_units) {
                name.push(le16(head, name_at + index * 2)?);
            }
            let mut next = head[next_at];
            while name.len() < length {
                let slot = usize::from(next);
                require(
                    slot < slots && !occupied[slot],
                    "jfs: directory name continuation cycle/bounds",
                )?;
                occupied[slot] = true;
                let continuation = &bytes[slot * 32..slot * 32 + 32];
                require(
                    continuation[1] == 1,
                    "jfs: directory continuation slot count",
                )?;
                for index in 0..(length - name.len()).min(15) {
                    name.push(le16(continuation, 2 + index * 2)?);
                }
                next = continuation[0];
            }
            require(
                next == 255 && !name.iter().any(|unit| matches!(*unit, 0 | 47)),
                "jfs: filename terminator/units",
            )?;
            require(
                previous.as_ref().is_none_or(|previous| *previous < name),
                "jfs: unordered directory keys",
            )?;
            if !leaf {
                require(
                    maximum.as_ref().is_none_or(|maximum| *maximum < name),
                    "jfs: directory separator upper bound",
                )?;
            }
            previous = Some(name.clone());
            if leaf {
                require(
                    self.directory.entries.len() < 262144 && name != [46] && name != [46, 46],
                    "jfs: directory entry limit/reserved name",
                )?;
                let displayed = String::from_utf16(&name)
                    .map_err(|_| FoldError::Unsupported("jfs: invalid UTF-16 name"))?;
                minimum.get_or_insert_with(|| name.clone());
                maximum = Some(name.clone());
                self.directory.entries.push(Entry {
                    name: displayed,
                    units: name,
                    inode: le32(head, 0)?,
                    index: if self.header.flags & 0x00200000 != 0 {
                        Some(le32(head, 28)?)
                    } else {
                        None
                    },
                    page: address.map_or(0, |extent| extent.start),
                    slot: order as u8,
                });
            } else {
                let child = Physical::parse(head, 0, self.header)?;
                let length = child.length(self.header);
                require(
                    matches!(length, 512 | 1024 | 2048 | 4096)
                        && self.seen.len() < 65536
                        && self.seen.insert(child.start),
                    "jfs: dtree child size/cycle/page limit",
                )?;
                self.directory.pages.push(child);
                let mut bytes = vec![0; length as usize];
                exact(self.source, child.byte(self.header), &mut bytes)?;
                let (lower, last) = self.visit(&bytes, Some(child), depth + 1)?;
                require(name <= lower, "jfs: directory separator lower bound")?;
                minimum.get_or_insert(lower);
                maximum = Some(last);
            }
        }
        require(leaf || count > 0, "jfs: empty directory index")?;
        let mut next = bytes[19];
        let mut free = 0;
        while next != 255 {
            let slot = usize::from(next);
            require(
                slot < slots && !occupied[slot],
                "jfs: directory free-slot cycle/overlap",
            )?;
            occupied[slot] = true;
            free += 1;
            next = bytes[slot * 32];
        }
        require(
            free == usize::from(bytes[18]) && occupied[..slots].iter().all(|value| *value),
            "jfs: directory slot partition/free count",
        )?;
        if let Some(address) = address {
            let first = minimum
                .as_ref()
                .ok_or(FoldError::Corrupt("jfs: empty external directory page"))?;
            let last = maximum
                .as_ref()
                .ok_or(FoldError::Corrupt("jfs: empty external directory page"))?;
            if let Some((previous, next, key)) = self.levels.get(&depth) {
                require(
                    *next == address.start && le64(bytes, 8)? == *previous && key < first,
                    "jfs: dtree sibling links/key order",
                )?;
            } else {
                require(le64(bytes, 8)? == 0, "jfs: dtree first sibling")?;
            }
            self.levels
                .insert(depth, (address.start, le64(bytes, 0)?, last.clone()));
        }
        Ok((minimum.unwrap_or_default(), maximum.unwrap_or_default()))
    }
}
