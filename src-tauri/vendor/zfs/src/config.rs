//! On-disk XDR nvlists and the pool topology stored in a verified leaf label.

use std::collections::BTreeMap;

const UINT64: u32 = 8;
const STRING: u32 = 9;
const UINT64_ARRAY: u32 = 16;
const NVLIST: u32 = 19;
const NVLIST_ARRAY: u32 = 20;

#[derive(Clone, Copy)]
struct Value<'a> {
    kind: u32,
    count: usize,
    bytes: &'a [u8],
}

pub(crate) struct NvList<'a>(BTreeMap<&'a [u8], Value<'a>>);

fn word(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?))
}

impl<'a> NvList<'a> {
    pub(crate) fn from_packed(bytes: &'a [u8]) -> Option<Self> {
        (bytes.first() == Some(&1)).then_some(())?;
        Self::parse(bytes.get(4..)?).map(|(list, _)| list)
    }

    fn parse(mut bytes: &'a [u8]) -> Option<(Self, &'a [u8])> {
        (word(bytes)? == 0).then_some(())?;
        bytes = bytes.get(8..)?;
        let mut entries = BTreeMap::new();
        loop {
            let size = word(bytes)? as usize;
            let decoded = word(bytes.get(4..)?)?;
            if size == 0 && decoded == 0 {
                return Some((Self(entries), &bytes[8..]));
            }
            (size >= 20 && size % 4 == 0 && decoded != 0).then_some(())?;
            let (pair, remaining) = bytes.split_at_checked(size)?;
            bytes = remaining;
            let name_len = word(&pair[8..])? as usize;
            let name = pair.get(12..)?.get(..name_len)?;
            let data = pair.get(12..)?.get(name_len.checked_add(3)? & !3..)?;
            let value = Value {
                kind: word(data)?,
                count: word(data.get(4..)?)? as usize,
                bytes: data.get(8..)?,
            };
            if entries.insert(name, value).is_some() {
                return None;
            }
        }
    }

    fn value(&self, name: &[u8], kind: u32) -> Option<Value<'a>> {
        self.0.get(name).copied().filter(|value| value.kind == kind)
    }

    pub(crate) fn number(&self, name: &[u8]) -> Option<u64> {
        let value = self.value(name, UINT64)?;
        (value.count == 1).then_some(())?;
        Some(u64::from_be_bytes(value.bytes.try_into().ok()?))
    }

    fn string(&self, name: &[u8]) -> Option<&'a str> {
        let value = self.value(name, STRING)?;
        (value.count == 1).then_some(())?;
        let count = word(value.bytes)? as usize;
        std::str::from_utf8(value.bytes.get(4..)?.get(..count)?).ok()
    }

    fn list(&self, name: &[u8]) -> Option<Self> {
        let value = self.value(name, NVLIST)?;
        let (list, tail) = Self::parse(value.bytes)?;
        (value.count == 1 && tail.is_empty()).then_some(list)
    }

    fn lists(&self, name: &[u8]) -> Option<Vec<Self>> {
        let value = self.value(name, NVLIST_ARRAY)?;
        (value.count <= value.bytes.len() / 16).then_some(())?;
        let mut list = Vec::with_capacity(value.count);
        let mut bytes = value.bytes;
        for _ in 0..value.count {
            let (child, tail) = Self::parse(bytes)?;
            list.push(child);
            bytes = tail;
        }
        bytes.is_empty().then_some(list)
    }

    fn numbers(&self, name: &[u8]) -> Option<Vec<u64>> {
        let value = self.value(name, UINT64_ARRAY)?;
        let bytes = value.bytes.get(4..)?;
        (word(value.bytes)? as usize == value.count
            && bytes.len() == value.count.checked_mul(8)?)
        .then_some(())?;
        Some(
            bytes
                .chunks_exact(8)
                .map(|item| u64::from_be_bytes(item.try_into().unwrap()))
                .collect(),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vdev {
    pub id: u32,
    pub guid: u64,
    pub kind: String,
    pub ashift: Option<u8>,
    pub allocated_size: Option<u64>,
    pub parity: Option<u8>,
    pub children: Vec<Vdev>,
}

impl Vdev {
    fn parse(list: &NvList<'_>, depth: u8) -> Option<Self> {
        (depth <= 8).then_some(())?;
        let children = if list.0.contains_key(b"children".as_slice()) {
            list.lists(b"children")?
                .iter()
                .map(|child| Self::parse(child, depth + 1))
                .collect::<Option<Vec<_>>>()?
        } else {
            Vec::new()
        };
        Some(Self {
            id: list.number(b"id")?.try_into().ok()?,
            guid: list.number(b"guid")?,
            kind: list.string(b"type")?.to_owned(),
            ashift: list.number(b"ashift").map(u8::try_from).transpose().ok()?,
            allocated_size: list.number(b"asize"),
            parity: list.number(b"nparity").map(u8::try_from).transpose().ok()?,
            children,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub name: String,
    pub txg: u64,
    pub top_count: Option<u32>,
    pub holes: Vec<u32>,
    pub top: Vdev,
    pub expanded_raidz: bool,
}

impl Config {
    pub(crate) fn parse(list: &NvList<'_>) -> Option<Option<Self>> {
        if !list.0.contains_key(b"vdev_tree".as_slice()) {
            return Some(None);
        }
        let top = Vdev::parse(&list.list(b"vdev_tree")?, 0)?;
        (top.guid != 0 && top.guid == list.number(b"top_guid")?).then_some(())?;
        let holes = if list.0.contains_key(b"hole_array".as_slice()) {
            list.numbers(b"hole_array")?
                .into_iter()
                .map(u32::try_from)
                .collect::<Result<_, _>>()
                .ok()?
        } else {
            Vec::new()
        };
        let expanded_raidz = list.list(b"features_for_read").is_some_and(|features| {
            features
                .0
                .contains_key(b"org.openzfs:raidz_expansion".as_slice())
        });
        Some(Some(Self {
            name: list.string(b"name")?.to_owned(),
            txg: list.number(b"txg")?,
            top_count: list
                .number(b"vdev_children")
                .map(u32::try_from)
                .transpose()
                .ok()?,
            holes,
            top,
            expanded_raidz,
        }))
    }
}
