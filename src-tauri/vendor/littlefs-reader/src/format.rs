use crate::le32;

pub const MAGIC: &[u8; 8] = b"littlefs";
pub const DISK_VERSION: u32 = 0x0002_0001;
pub const NAME_MAX: usize = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagType {
    RegularFile,
    Directory,
    Superblock,
    DirStruct,
    InlineStruct,
    CtzStruct,
    Create,
    Delete,
    SoftTail,
    HardTail,
    MoveState,
    Other,
}

#[derive(Clone, Copy)]
pub struct Tag(pub u32);

impl Tag {
    pub fn is_valid(self) -> bool {
        self.0 != 0 && self.0 >> 31 == 0
    }

    pub fn tag_type(self) -> TagType {
        match (self.0 >> 20) & 0x7ff {
            0x001 => TagType::RegularFile,
            0x002 => TagType::Directory,
            0x0ff => TagType::Superblock,
            0x200 => TagType::DirStruct,
            0x201 => TagType::InlineStruct,
            0x202 => TagType::CtzStruct,
            0x401 => TagType::Create,
            0x4ff => TagType::Delete,
            0x600 => TagType::SoftTail,
            0x601 => TagType::HardTail,
            0x7ff => TagType::MoveState,
            _ => TagType::Other,
        }
    }

    pub fn id(self) -> u16 {
        ((self.0 >> 10) & 1023) as u16
    }

    pub fn is_special_length(self) -> bool {
        self.0 & 1023 == 1023
    }

    pub fn dsize(self) -> usize {
        4 + if self.is_special_length() {
            0
        } else {
            (self.0 & 1023) as usize
        }
    }

    pub fn ccrc_chunk(self) -> Option<u8> {
        ((self.0 >> 21) & 0x3ff == 0x280).then_some(((self.0 >> 20) & 1) as u8)
    }
}

#[derive(Clone, Copy)]
pub struct Superblock {
    pub version: u32,
    pub block_size: u32,
    pub block_count: u32,
    pub name_max: u32,
    pub file_max: u32,
    pub attr_max: u32,
}

impl Superblock {
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        (bytes.len() == 24).then(|| Self {
            version: le32(bytes),
            block_size: le32(&bytes[4..]),
            block_count: le32(&bytes[8..]),
            name_max: le32(&bytes[12..]),
            file_max: le32(&bytes[16..]),
            attr_max: le32(&bytes[20..]),
        })
    }
}

#[derive(Clone, Copy)]
pub(crate) struct CtzStruct {
    pub head_block: u32,
    pub size: u32,
}

impl CtzStruct {
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        (bytes.len() == 8).then(|| Self {
            head_block: le32(bytes),
            size: le32(&bytes[4..]),
        })
    }

    pub fn block_count(self, block_size: u32) -> u32 {
        if self.size == 0 {
            0
        } else {
            Self::position(self.size - 1, block_size).0 + 1
        }
    }

    /// Maps a logical byte to its CTZ index and physical offset within that block.
    pub fn position(offset: u32, block_size: u32) -> (u32, u32) {
        let average = block_size - 8;
        let estimate = offset / average;
        if estimate == 0 {
            return (0, offset);
        }
        let index = (offset - 4 * ((estimate - 1).count_ones() + 2)) / average;
        let within = offset - average * index - 4 * index.count_ones();
        (index, within)
    }
}
