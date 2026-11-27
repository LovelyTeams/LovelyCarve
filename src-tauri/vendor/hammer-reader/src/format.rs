use std::fmt::Write;

use lamfold::{BlockSource, FileKind, FileTimes, FoldError, Result};

use crate::require;
use crate::undo::Undo;

const MAGIC: u64 = 0xc841_4d4d_c552_3031;
const REVERSE_MAGIC: u64 = MAGIC.swap_bytes();
pub const PROBE_BYTES: usize = 1928;
pub const BTREE_BYTES: usize = 4096;
pub const BUFFER_BYTES: u64 = 16384;
pub const DATA_MAX: usize = 65536;
pub(crate) const BUFFER_OFFSET_MASK: u64 = 0x000f_ffff_ffff_ffff;

pub const INODE: u16 = 1;
pub const DATA: u16 = 0x10;
pub const DIRENT: u16 = 0x11;
pub const FIX: u16 = 0x14;
pub const SNAPSHOT: u16 = 0x16;
pub const LOCAL_INODE: u32 = 1;
pub const LOCAL_MISC: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Order {
    Little,
    Big,
}

impl Order {
    fn detect(bytes: &[u8]) -> Option<Self> {
        let word = Self::Little.u64(bytes.get(..8)?);
        match word {
            MAGIC => Some(Self::Little),
            REVERSE_MAGIC => Some(Self::Big),
            _ => None,
        }
    }

    pub fn u16(self, bytes: &[u8]) -> u16 {
        let bytes = [bytes[0], bytes[1]];
        match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        }
    }

    pub fn u32(self, bytes: &[u8]) -> u32 {
        let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        }
    }

    pub fn u64(self, bytes: &[u8]) -> u64 {
        let bytes: [u8; 8] = bytes[..8].try_into().expect("fixed width");
        match self {
            Self::Little => u64::from_le_bytes(bytes),
            Self::Big => u64::from_be_bytes(bytes),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Checksum {
    version: u32,
}

impl Checksum {
    pub fn valid(self, stored: u32, bytes: &[u8]) -> bool {
        if self.version >= 7 && crc32c::crc32c(bytes) == stored {
            return true;
        }
        crc32fast::hash(bytes) == stored
    }

    pub fn valid_split(self, stored: u32, start: &[u8], end: &[u8]) -> bool {
        if self.version >= 7 && stored == crc32c::crc32c(start) ^ crc32c::crc32c(end) {
            return true;
        }
        stored == crc32fast::hash(start) ^ crc32fast::hash(end)
    }
}

#[derive(Clone)]
pub struct Header {
    pub order: Order,
    pub version: u32,
    pub volume_id: u32,
    pub volumes: u32,
    pub label: String,
    pub checksum_present: bool,
    pub(crate) root: u64,
    start: u64,
    end: u64,
    dirty: bool,
    undo: Option<Undo>,
}

impl Header {
    pub fn parse(head: &[u8], length: u64) -> Result<Option<Self>> {
        let Some(order) = Order::detect(head) else {
            return Ok(None);
        };
        let bytes = head
            .get(..PROBE_BYTES)
            .ok_or(FoldError::Corrupt("truncated HAMMER volume header"))?;
        let version = order.u32(&bytes[152..]);
        if !(1..=7).contains(&version) {
            return Err(FoldError::Unsupported("HAMMER volume version"));
        }
        let stored = order.u32(&bytes[156..]);
        // Standalone newfs_hammer leaves this field unset. Its tree and record CRCs still apply.
        require(
            stored == 0 || (Checksum { version }).valid_split(stored, &bytes[..156], &bytes[160..]),
            "HAMMER volume header CRC mismatch",
        )?;
        let volume_id = order.u32(&bytes[144..]);
        let volumes = order.u32(&bytes[148..]);
        let boot = order.u64(&bytes[8..]);
        let log = order.u64(&bytes[16..]);
        let start = order.u64(&bytes[24..]);
        let end = order.u64(&bytes[32..]);
        require(
            volumes > 0
                && volumes <= 256
                && volume_id < volumes
                && order.u32(&bytes[164..]) == 0
                && boot >= BUFFER_BYTES
                && boot <= log
                && log <= start
                && start < end
                && end <= length
                && start.is_multiple_of(BUFFER_BYTES)
                && end.is_multiple_of(BUFFER_BYTES),
            "invalid HAMMER volume geometry",
        )?;
        let label = &bytes[80..144];
        let label = &label[..label
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(label.len())];
        let flags = order.u32(&bytes[160..]);
        let undo = (volume_id == 0 && volumes == 1)
            .then(|| Undo::parse(bytes, order))
            .transpose()?;
        let header = Self {
            order,
            version,
            volume_id,
            volumes,
            label: display_name(label),
            checksum_present: stored != 0,
            root: order.u64(&bytes[240..]),
            start,
            end,
            dirty: flags & 4 != 0,
            undo,
        };
        if volume_id == 0 && volumes == 1 {
            header.physical(header.root, BTREE_BYTES, &[8], BTREE_BYTES as u64)?;
        }
        Ok(Some(header))
    }

    pub(crate) fn validate_mount(&self, source: &mut impl BlockSource) -> Result<()> {
        let undo = self
            .undo
            .as_ref()
            .ok_or(FoldError::Unsupported("multi-volume HAMMER filesystem"))?;
        if self.dirty {
            return Err(FoldError::Unsupported(
                "HAMMER volume requires undo or redo recovery",
            ));
        }
        undo.validate_clean(self, source)
    }

    pub(crate) fn checksum(&self) -> Checksum {
        Checksum {
            version: self.version,
        }
    }

    pub(crate) fn physical(
        &self,
        address: u64,
        size: usize,
        zones: &[u64],
        alignment: u64,
    ) -> Result<u64> {
        let offset = address & BUFFER_OFFSET_MASK;
        let volume = (address >> 52) & 255;
        require(
            volume == u64::from(self.volume_id)
                && zones.contains(&(address >> 60))
                && offset.is_multiple_of(alignment),
            "invalid HAMMER zone, member or physical offset",
        )?;
        let begin = self
            .start
            .checked_add(offset)
            .ok_or(FoldError::Corrupt("HAMMER physical address overflow"))?;
        require(
            begin
                .checked_add(size as u64)
                .is_some_and(|end| end <= self.end),
            "HAMMER physical allocation exceeds the volume",
        )?;
        Ok(begin)
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct Key {
    pub localization: u32,
    pub object: i64,
    pub kind: u16,
    pub position: i64,
}

impl Key {
    pub fn decode(bytes: &[u8], order: Order) -> Self {
        Self {
            localization: order.u32(&bytes[36..]),
            object: order.u64(bytes) as i64,
            kind: order.u16(&bytes[32..]),
            position: order.u64(&bytes[8..]) as i64,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Record {
    pub key: Key,
    pub created: u64,
    pub deleted: u64,
    pub object_kind: u8,
    pub address: u64,
    pub length: usize,
    pub crc: u32,
}

impl Record {
    pub fn decode(bytes: &[u8], order: Order) -> Result<Self> {
        let length = order.u32(&bytes[56..]) as usize;
        let created = order.u64(&bytes[16..]);
        let deleted = order.u64(&bytes[24..]);
        let key = Key::decode(bytes, order);
        require(
            length <= DATA_MAX && (deleted == 0 || deleted >= created),
            "invalid HAMMER record length or transaction interval",
        )?;
        Ok(Self {
            key,
            created,
            deleted,
            object_kind: bytes[34],
            address: order.u64(&bytes[48..]),
            length,
            crc: order.u32(&bytes[60..]),
        })
    }

    pub fn visible_at(self, as_of: u64) -> bool {
        self.created != 0 && self.created <= as_of && (self.deleted == 0 || as_of < self.deleted)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Inode {
    pub kind: FileKind,
    pub raw_kind: u8,
    pub local_dirents: bool,
    pub size: u64,
    pub mode: u32,
    pub times: FileTimes,
    pub link: Option<Vec<u8>>,
}

impl Inode {
    pub fn decode(bytes: &[u8], order: Order) -> Result<Self> {
        require(
            bytes.len() == 128 && order.u16(bytes) == 1,
            "invalid HAMMER inode version or size",
        )?;
        let raw_kind = bytes[64];
        let kind = match raw_kind {
            1 => FileKind::Directory,
            2 => FileKind::Regular,
            7 => FileKind::Symlink,
            _ => FileKind::Other,
        };
        let size = order.u64(&bytes[80..]);
        require(
            size <= i64::MAX as u64,
            "HAMMER object exceeds signed file offset range",
        )?;
        let time = |at| {
            let micros = order.u64(&bytes[at..]);
            (micros != 0).then_some((
                (micros / 1_000_000) as i64,
                ((micros % 1_000_000) * 1000) as u32,
            ))
        };
        let link = (kind == FileKind::Symlink && size <= 24)
            .then(|| bytes[88..88 + size as usize].to_vec());
        Ok(Self {
            kind,
            raw_kind,
            local_dirents: bytes[65] & 4 != 0,
            size,
            mode: u32::from(order.u16(&bytes[2..])),
            times: FileTimes {
                changed: time(16),
                modified: time(112),
                accessed: time(120),
                created: None,
            },
            link,
        })
    }
}

pub(crate) fn display_name(raw: &[u8]) -> String {
    if raw == b"." || raw == b".." {
        return "%2E".repeat(raw.len());
    }
    if raw == b"@snapshots" {
        return "%40snapshots".into();
    }
    let mut name = String::new();
    for part in raw.utf8_chunks() {
        for character in part.valid().chars() {
            if matches!(character, '%' | '/' | '\\') || character.is_ascii_control() {
                let _ = write!(name, "%{:02X}", u32::from(character));
            } else {
                name.push(character);
            }
        }
        for &byte in part.invalid() {
            let _ = write!(name, "%{byte:02X}");
        }
    }
    name
}
