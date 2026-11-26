use crate::{
    format::{Allocation, BLOCK_SIZE},
    le16, le32, le64, require, Header,
};
use lamfold::{BlockSource, FoldError, Result};
use std::{
    collections::{HashSet, VecDeque},
    ops::RangeInclusive,
    sync::Arc,
};

pub(crate) const STAT: u16 = 0;
pub(crate) const DIRECTORY: u16 = 2;
const POINTER: u16 = 3;
pub(crate) const EXTENT: u16 = 5;
pub(crate) const TAIL: u16 = 6;
pub(crate) const COMPRESSED_TAIL: u16 = 7;
const MAX_VISITED: usize = 131_072;
const MAX_ITEM_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Key(pub(crate) [u64; 4]);

impl Key {
    pub(crate) const ROOT: Self = Self([0x291, 0, 0x2a, 0]);

    pub(crate) fn parse(bytes: &[u8], large: bool) -> Result<Self> {
        let first = le64(bytes, 0)?;
        if large {
            Ok(Self([
                first,
                le64(bytes, 8)?,
                le64(bytes, 16)?,
                le64(bytes, 24)?,
            ]))
        } else {
            Ok(Self([first, 0, le64(bytes, 8)?, le64(bytes, 16)?]))
        }
    }

    pub(crate) fn object_number(self) -> u64 {
        self.0[2] & 0x0fff_ffff_ffff_ffff
    }

    pub(crate) fn minor(self) -> u64 {
        self.0[0] & 15
    }

    pub(crate) fn directory(self) -> RangeInclusive<Self> {
        let prefix = self.object_number() << 4;
        Self([prefix, 0, 0, 0])..=Self([prefix, u64::MAX, u64::MAX, u64::MAX])
    }

    pub(crate) fn file_body(self) -> RangeInclusive<Self> {
        let mut low = self;
        low.0[0] = (self.0[0] & !15) | 4;
        let mut high = low;
        high.0[3] = u64::MAX;
        low..=high
    }
}

#[derive(Clone, Copy)]
struct Record {
    key: Key,
    offset: usize,
    end: usize,
    plugin: u16,
}

struct Page {
    data: Box<[u8; BLOCK_SIZE]>,
    level: u8,
    records: Vec<Record>,
}

impl Page {
    fn parse(data: Box<[u8; BLOCK_SIZE]>, header: &Header) -> Result<Self> {
        let protected = header.protected_nodes;
        let magic = if protected { 0x1905_1966 } else { 0x5234_4653 };
        require(
            le16(&data[..], 0)? == u16::from(protected) && le32(&data[..], 8)? == magic,
            "reiser4: node magic or plugin",
        )?;
        require(
            le32(&data[..], 12)? == header.mkfs_id && le64(&data[..], 16)? <= header.flushes,
            "reiser4: node generation",
        )?;
        if protected {
            let crc = crc32c::crc32c_append(crc32c::crc32c(&data[..28]), &data[32..]);
            require(!crc == le32(&data[..], 28)?, "reiser4: node checksum")?;
        }
        let header_size = if protected { 32 } else { 28 };
        let level = data[26];
        let count = usize::from(le16(&data[..], 2)?);
        let free = usize::from(le16(&data[..], 4)?);
        let end = usize::from(le16(&data[..], 6)?);
        let key_size = if header.large_keys { 32 } else { 24 };
        let record_size = key_size + 6;
        let headers = BLOCK_SIZE
            .checked_sub(count * record_size)
            .ok_or(FoldError::Corrupt("reiser4: node item count"))?;
        require(
            count > 0 && headers >= end && end >= header_size && free == headers - end,
            "reiser4: node free-space boundaries",
        )?;
        let mut records = Vec::<Record>::with_capacity(count);
        for index in 0..count {
            let offset = BLOCK_SIZE - (index + 1) * record_size;
            let key = Key::parse(&data[offset..offset + key_size], header.large_keys)?;
            let body = usize::from(le16(&data[..], offset + key_size)?);
            let plugin = le16(&data[..], offset + key_size + 4)?;
            require(
                body >= header_size && body < end,
                "reiser4: item data offset",
            )?;
            if let Some(previous) = records.last_mut() {
                require(
                    previous.offset < body && previous.key <= key,
                    "reiser4: item ordering",
                )?;
                previous.end = body;
            } else {
                require(body == header_size, "reiser4: start of item data")?;
            }
            require(
                match level {
                    1 => plugin != POINTER && plugin != EXTENT,
                    2 => plugin == POINTER || plugin == EXTENT,
                    _ => plugin == POINTER,
                },
                "reiser4: item at unexpected node level",
            )?;
            records.push(Record {
                key,
                offset: body,
                end,
                plugin,
            });
        }
        Ok(Self {
            data,
            level,
            records,
        })
    }
}

pub(crate) struct Item {
    pub(crate) key: Key,
    pub(crate) plugin: u16,
    pub(crate) id: u64,
    data: Vec<u8>,
}

impl Item {
    fn from_record(page: &Page, block: u64, record: Record) -> Self {
        Self {
            key: record.key,
            plugin: record.plugin,
            id: block * BLOCK_SIZE as u64 + record.offset as u64,
            data: page.data[record.offset..record.end].to_vec(),
        }
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.data
    }
}

#[derive(Default)]
pub(crate) struct Tree {
    pages: VecDeque<(u64, Arc<Page>)>,
    pub(crate) allocation: Allocation,
}

impl Tree {
    fn page(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        block: u64,
    ) -> Result<Arc<Page>> {
        if let Some(index) = self.pages.iter().position(|(saved, _)| *saved == block) {
            let entry = self
                .pages
                .remove(index)
                .expect("position comes from the same cache");
            let page = Arc::clone(&entry.1);
            self.pages.push_front(entry);
            return Ok(page);
        }
        self.allocation.check(source, header, block, 1)?;
        let mut data = Box::new([0; BLOCK_SIZE]);
        source.read_at(block * BLOCK_SIZE as u64, data.as_mut_slice())?;
        let page = Arc::new(Page::parse(data, header)?);
        if self.pages.len() == 128 {
            self.pages.pop_back();
        }
        self.pages.push_front((block, Arc::clone(&page)));
        Ok(page)
    }

    pub(crate) fn stat_at(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        id: u64,
    ) -> Result<Item> {
        let block = id / BLOCK_SIZE as u64;
        let offset = (id % BLOCK_SIZE as u64) as usize;
        let page = self.page(source, header, block)?;
        let record = page
            .records
            .iter()
            .find(|record| {
                record.offset == offset
                    && record.plugin == STAT
                    && record.key.minor() == 1
                    && record.key.0[3] == 0
            })
            .ok_or(FoldError::NotFound)?;
        Ok(Item::from_record(&page, block, *record))
    }

    pub(crate) fn items(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        query: RangeInclusive<Key>,
    ) -> Result<Vec<Item>> {
        let mut stack = vec![(header.root, header.height, None, None)];
        let mut visited = HashSet::new();
        let mut items = Vec::new();
        let mut bytes = 0usize;
        while let Some((block, level, lower, upper)) = stack.pop() {
            require(
                visited.len() < MAX_VISITED && visited.insert(block),
                "reiser4: tree cycle or traversal limit",
            )?;
            let page = self.page(source, header, block)?;
            require(page.level == level, "reiser4: wrong tree level")?;
            let first = page.records[0].key;
            let last = page.records[page.records.len() - 1].key;
            require(
                lower.is_none_or(|key| first == key) && upper.is_none_or(|key| last <= key),
                "reiser4: child outside its key interval",
            )?;
            for (index, record) in page.records.iter().enumerate().rev() {
                if record.plugin == POINTER {
                    let right = page.records.get(index + 1).map(|r| r.key).or(upper);
                    if record.key <= *query.end() && right.is_none_or(|key| key >= *query.start()) {
                        require(
                            level > 1 && record.end - record.offset == 8,
                            "reiser4: child pointer size",
                        )?;
                        let child = le64(&page.data[..], record.offset)?;
                        stack.push((child, level - 1, Some(record.key), right));
                    }
                } else if query.contains(&record.key) {
                    bytes += record.end - record.offset;
                    require(bytes <= MAX_ITEM_BYTES, "reiser4: object item data limit")?;
                    items.push(Item::from_record(&page, block, *record));
                }
            }
        }
        items.sort_by_key(|item| item.key);
        Ok(items)
    }
}
