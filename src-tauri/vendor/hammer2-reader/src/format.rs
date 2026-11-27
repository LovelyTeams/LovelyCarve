use std::fmt::Write;
use std::ops::RangeInclusive;

use lamfold::{FileKind, FileTimes, FoldError, Result};

use crate::require;

pub const BLOCK_BYTES: usize = 65_536;
pub const ZONE_BYTES: u64 = 2 << 30;
const SIGNATURE: u64 = 0x4841_4d32_0517_2011;
const ALLOCATION_ALIGNMENT: u64 = 1 << 23;
const SIGNATURE_REVERSED: u64 = SIGNATURE.swap_bytes();
pub(crate) const DIRENT_VISIBLE: u64 = 1 << 63;
pub(crate) const BLOCKREF_BYTES: usize = 128;
pub(crate) const DIRECT_DATA: u8 = 1;
pub(crate) const PFS_ROOT: u8 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Order {
    Little,
    Big,
}

impl Order {
    pub(crate) fn magic(bytes: &[u8]) -> Option<Self> {
        match Self::Little.u64(bytes.get(..8)?) {
            SIGNATURE => Some(Self::Little),
            SIGNATURE_REVERSED => Some(Self::Big),
            _ => None,
        }
    }

    pub(crate) fn u16(self, bytes: &[u8]) -> u16 {
        let bytes = [bytes[0], bytes[1]];
        match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        }
    }

    pub(crate) fn u32(self, bytes: &[u8]) -> u32 {
        let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        }
    }

    pub(crate) fn u64(self, bytes: &[u8]) -> u64 {
        let bytes: [u8; 8] = bytes[..8].try_into().expect("fixed width");
        match self {
            Self::Little => u64::from_le_bytes(bytes),
            Self::Big => u64::from_be_bytes(bytes),
        }
    }
}

pub struct Header {
    pub order: Order,
    pub version: u32,
    pub mirror_tid: u64,
    pub offset: u64,
    pub volume_size: u64,
    pub volume_id: u8,
    pub volume_count: u8,
    pub(crate) filesystem: [u8; 16],
    pub(crate) roots: Vec<BlockRef>,
}

impl Header {
    pub fn locate(
        head: &[u8],
        length: u64,
        mut read: impl FnMut(u64, &mut [u8]) -> Result<()>,
    ) -> Result<Option<Self>> {
        let mut best: Option<Self> = None;
        let mut failure = None;
        let mut current = vec![0; BLOCK_BYTES];
        for zone in 0..4 {
            let offset = zone * ZONE_BYTES;
            if offset + BLOCK_BYTES as u64 > length {
                break;
            }
            let order = if zone == 0 {
                Order::magic(head)
            } else {
                let mut magic = [0; 8];
                if read(offset, &mut magic).is_err() {
                    continue;
                }
                Order::magic(&magic)
            };
            let Some(order) = order else { continue };
            let bytes = if zone == 0 && head.len() >= BLOCK_BYTES {
                &head[..BLOCK_BYTES]
            } else {
                if let Err(error) = read(offset, &mut current) {
                    failure = Some(error);
                    continue;
                }
                &current
            };
            match Self::decode(bytes, order, offset, length) {
                Ok(candidate) => {
                    if let Some(previous) = &best {
                        require(
                            previous.filesystem == candidate.filesystem
                                && previous.volume_size == candidate.volume_size
                                && previous.volume_id == candidate.volume_id
                                && previous.volume_count == candidate.volume_count
                                && previous.order == candidate.order,
                            "HAMMER2 volume header copies have different identities",
                        )?;
                    }
                    if best
                        .as_ref()
                        .is_none_or(|previous| previous.mirror_tid < candidate.mirror_tid)
                    {
                        best = Some(candidate);
                    }
                }
                Err(error) => failure = Some(error),
            }
        }
        if best.is_none()
            && let Some(error) = failure
        {
            return Err(error);
        }
        Ok(best)
    }

    fn decode(bytes: &[u8], order: Order, offset: u64, length: u64) -> Result<Self> {
        let crc =
            |start, end, field| crc32c::crc32c(&bytes[start..end]) == order.u32(&bytes[field..]);
        require(
            crc(0, 508, 508) && crc(512, 1024, 504) && crc(0, 65532, 65532),
            "HAMMER2 volume header CRC32C mismatch",
        )?;
        let volume_size = order.u64(&bytes[40..]);
        let version = order.u32(&bytes[48..]);
        let volume_id = bytes[59];
        let volume_count = if version == 1 { 1 } else { bytes[60] };
        require(
            volume_size.is_multiple_of(ALLOCATION_ALIGNMENT)
                && volume_size <= length
                && offset + BLOCK_BYTES as u64 <= volume_size
                && (1..=64).contains(&volume_count)
                && volume_id < volume_count,
            "invalid HAMMER2 volume geometry",
        )?;
        if !(1..=2).contains(&version) {
            return Err(FoldError::Unsupported("HAMMER2 volume version"));
        }
        if version >= 2 {
            let total = order.u64(&bytes[192..]);
            require(
                total >= volume_size && (volume_count > 1 || total == volume_size),
                "invalid HAMMER2 filesystem size",
            )?;
            require(
                volume_id != 0 || order.u64(&bytes[0xe00..]) == 0,
                "invalid HAMMER2 root member offset",
            )?;
        }
        Ok(Self {
            order,
            version,
            mirror_tid: order.u64(&bytes[120..]),
            offset,
            volume_size,
            volume_id,
            volume_count,
            filesystem: bytes[64..80].try_into().expect("fixed UUID width"),
            roots: blockset(&bytes[512..1024], order)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    Inode,
    Indirect,
    Data,
    Dirent,
}

#[derive(Clone, Debug)]
pub(crate) struct BlockRef {
    pub kind: Kind,
    pub methods: u8,
    pub key: u64,
    pub key_bits: u8,
    pub data_offset: u64,
    pub embed: [u8; 16],
    pub check: [u8; 64],
}

impl BlockRef {
    pub fn range(&self) -> Result<RangeInclusive<u64>> {
        let mask = match self.key_bits {
            0..64 => (1 << self.key_bits) - 1,
            64 => u64::MAX,
            _ => return Err(FoldError::Corrupt("invalid HAMMER2 blockref key width")),
        };
        require(self.key & mask == 0, "unaligned HAMMER2 blockref key")?;
        Ok(self.key..=(self.key | mask))
    }

    pub fn physical(&self) -> Result<(u64, usize)> {
        let radix = self.data_offset & 63;
        require(
            (10..=16).contains(&radix),
            "invalid HAMMER2 allocation size",
        )?;
        let size = 1 << radix;
        let offset = self.data_offset & !63;
        require(
            offset != 0 && offset.is_multiple_of(1024),
            "unaligned HAMMER2 allocation offset",
        )?;
        Ok((offset, size as usize))
    }
}

pub(crate) fn blockset(bytes: &[u8], order: Order) -> Result<Vec<BlockRef>> {
    require(
        bytes.len().is_multiple_of(BLOCKREF_BYTES),
        "mis-sized HAMMER2 blockset",
    )?;
    let mut entries = Vec::new();
    for bytes in bytes.as_chunks::<BLOCKREF_BYTES>().0 {
        let kind = match bytes[0] {
            0 => continue,
            1 => Kind::Inode,
            2 => Kind::Indirect,
            3 => Kind::Data,
            4 => Kind::Dirent,
            _ => return Err(FoldError::Unsupported("HAMMER2 topology block type")),
        };
        entries.push(BlockRef {
            kind,
            methods: bytes[1],
            key: order.u64(&bytes[8..]),
            key_bits: bytes[3],
            data_offset: order.u64(&bytes[32..]),
            embed: bytes[48..64].try_into().expect("fixed embed width"),
            check: bytes[64..].try_into().expect("fixed checksum width"),
        });
    }
    Ok(entries)
}

pub(crate) struct Inode {
    pub kind: FileKind,
    pub raw_kind: u8,
    pub flags: u8,
    pub mode: u32,
    pub number: u64,
    pub size: u64,
    pub times: FileTimes,
    pub name: String,
    pub blockset: Vec<BlockRef>,
    pub inline: Vec<u8>,
}

impl Inode {
    pub fn decode(bytes: &[u8], order: Order) -> Result<Self> {
        require(
            bytes.len() == 1024 && order.u16(bytes) == 1,
            "invalid HAMMER2 inode size or version",
        )?;
        let raw_kind = bytes[0x50];
        let kind = match raw_kind {
            1 => FileKind::Directory,
            2 => FileKind::Regular,
            7 => FileKind::Symlink,
            _ => FileKind::Other,
        };
        let flags = bytes[0x51];
        let size = order.u64(&bytes[0x60..]);
        let name_length = usize::from(order.u16(&bytes[0x80..]));
        require(name_length <= 256, "invalid HAMMER2 inode name length")?;
        let time = |at| {
            let micros = order.u64(&bytes[at..]);
            (micros != 0).then_some((
                (micros / 1_000_000) as i64,
                ((micros % 1_000_000) * 1000) as u32,
            ))
        };
        let (inline, blockset) = if flags & DIRECT_DATA != 0 {
            require(
                size <= 512 && kind != FileKind::Directory,
                "invalid HAMMER2 embedded data length",
            )?;
            (bytes[512..512 + size as usize].to_vec(), Vec::new())
        } else {
            (Vec::new(), blockset(&bytes[512..], order)?)
        };
        Ok(Self {
            kind,
            raw_kind,
            flags,
            mode: order.u32(&bytes[0x54..]),
            number: order.u64(&bytes[0x58..]),
            size,
            times: FileTimes {
                modified: time(0x18),
                accessed: time(0x20),
                created: time(0x28),
                changed: time(0x10),
            },
            name: display_name(&bytes[256..256 + name_length]),
            blockset,
            inline,
        })
    }
}

pub(crate) fn display_name(raw: &[u8]) -> String {
    if raw == b"." || raw == b".." {
        return "%2E".repeat(raw.len());
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
