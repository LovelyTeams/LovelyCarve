//! UBIFS node layouts adapted from rust_ubidump's MIT node/key/inode readers.
//! Corrected against Linux v6.12 ubifs-media.h; see LOVELYCARVE.md.
use crate::{ReadAt, utils::crc::crc32_jamcrc, volume::read_exact};
use anyhow::{Context, Result, bail, ensure};

pub const MAGIC: [u8; 4] = [0x31, 0x18, 0x10, 0x06];
pub const BLOCK_SIZE: usize = 4096;

pub fn number<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N]> {
    bytes
        .get(offset..offset.checked_add(N).context("UBIFS field overflow")?)
        .context("truncated UBIFS field")?
        .try_into()
        .context("UBIFS field size")
}
pub fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(number(bytes, offset)?))
}
pub fn wide(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(number(bytes, offset)?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Key {
    pub inode: u32,
    pub kind: u8,
    pub value: u32,
}
impl Key {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let inode = word(bytes, 0)?;
        let value = word(bytes, 4)?;
        let kind = (value >> 29) as u8;
        ensure!(inode > 0 && kind <= 3, "unsupported UBIFS key");
        Ok(Self {
            inode,
            kind,
            value: value & 0x1fffffff,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub block: u32,
    pub offset: u32,
    pub length: u32,
}
impl Location {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            block: word(bytes, 0)?,
            offset: word(bytes, 4)?,
            length: word(bytes, 8)?,
        })
    }
}

pub struct Geometry {
    pub leb_size: u32,
    pub leb_count: u32,
    pub minimum_io: u32,
    pub main_first: u32,
    pub fanout: u32,
    pub key_hash: u8,
    pub version: u32,
    pub time_granularity: u32,
}
impl Geometry {
    pub fn open(source: &dyn ReadAt) -> Result<Self> {
        let bytes = read_exact(source, 0, 4096)?;
        validate_node(&bytes, 6)?;
        ensure!(
            bytes.len() == word(&bytes, 16)? as usize,
            "invalid UBIFS superblock length"
        );
        ensure!(
            bytes[27] == 0 && bytes[26] <= 1,
            "unsupported UBIFS key format/hash"
        );
        let flags = word(&bytes, 28)?;
        ensure!(
            flags & !0x0e == 0,
            "unsupported encrypted/authenticated UBIFS or flags {flags:#x}"
        );
        let minimum_io = word(&bytes, 32)?;
        let leb_size = word(&bytes, 36)?;
        let leb_count = word(&bytes, 40)?;
        ensure!(
            minimum_io.is_power_of_two() && (8..=65536).contains(&minimum_io),
            "invalid UBIFS minimum I/O"
        );
        ensure!(
            (4096..=16 * 1024 * 1024).contains(&leb_size) && leb_size % minimum_io == 0,
            "invalid UBIFS LEB size"
        );
        ensure!(
            leb_count <= word(&bytes, 44)?,
            "UBIFS LEB count exceeds maximum"
        );
        ensure!(
            u64::from(leb_size) * u64::from(leb_count) <= source.size(),
            "truncated UBIFS logical volume"
        );
        let main_first = 3u32
            .checked_add(word(&bytes, 56)?)
            .and_then(|n| n.checked_add(word(&bytes, 60).ok()?))
            .and_then(|n| n.checked_add(word(&bytes, 64).ok()?))
            .context("UBIFS area overflow")?;
        ensure!(main_first < leb_count, "UBIFS has no main area");
        let fanout = word(&bytes, 72)?;
        ensure!((3..=65535).contains(&fanout), "invalid UBIFS index fanout");
        let version = word(&bytes, 80)?;
        ensure!(
            (4..=5).contains(&version) && word(&bytes, 124)? == 0,
            "unsupported UBIFS format/RO compatibility"
        );
        let time_granularity = word(&bytes, 104)?;
        ensure!(
            (1..=1_000_000_000).contains(&time_granularity),
            "invalid UBIFS time granularity"
        );
        Ok(Self {
            leb_size,
            leb_count,
            minimum_io,
            main_first,
            fanout,
            key_hash: bytes[26],
            version,
            time_granularity,
        })
    }
    pub fn address(&self, location: Location) -> Result<u64> {
        ensure!(
            location.block < self.leb_count
                && location.offset.is_multiple_of(8)
                && location.length >= 24
                && location
                    .offset
                    .checked_add(location.length)
                    .is_some_and(|end| end <= self.leb_size),
            "UBIFS node outside LEB"
        );
        Ok(u64::from(location.block) * u64::from(self.leb_size) + u64::from(location.offset))
    }
    pub fn read_node(&self, source: &dyn ReadAt, location: Location, kind: u8) -> Result<Vec<u8>> {
        let address = self.address(location)?;
        let bytes = read_exact(source, address, location.length as usize)?;
        validate_node(&bytes, kind)?;
        Ok(bytes)
    }
}

pub fn validate_node(bytes: &[u8], kind: u8) -> Result<()> {
    ensure!(
        bytes.len() >= 24 && bytes[..4] == MAGIC,
        "invalid UBIFS node magic/header"
    );
    ensure!(
        word(bytes, 16)? as usize == bytes.len() && bytes[20] == kind,
        "UBIFS node length/type mismatch"
    );
    ensure!(
        bytes[21] <= 2 && bytes[22..24] == [0, 0],
        "invalid UBIFS node group/padding"
    );
    ensure!(
        crc32_jamcrc(&bytes[8..]) == word(bytes, 4)?,
        "UBIFS node CRC mismatch"
    );
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File,
    Directory,
    Symlink,
    BlockDevice,
    CharacterDevice,
    Fifo,
    Socket,
}
impl FileType {
    pub fn from_mode(mode: u32) -> Result<Self> {
        match mode & 0xf000 {
            0x8000 => Ok(Self::File),
            0x4000 => Ok(Self::Directory),
            0xa000 => Ok(Self::Symlink),
            0x6000 => Ok(Self::BlockDevice),
            0x2000 => Ok(Self::CharacterDevice),
            0x1000 => Ok(Self::Fifo),
            0xc000 => Ok(Self::Socket),
            _ => bail!("invalid UBIFS inode mode"),
        }
    }
    pub fn directory_type(self) -> u8 {
        match self {
            Self::File => 0,
            Self::Directory => 1,
            Self::Symlink => 2,
            Self::BlockDevice => 3,
            Self::CharacterDevice => 4,
            Self::Fifo => 5,
            Self::Socket => 6,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Inode {
    pub number: u32,
    pub size: u64,
    pub kind: FileType,
    pub mode: u32,
    pub uid: u32,
    pub gid: u32,
    pub links: u32,
    pub flags: u32,
    pub accessed: (i64, u32),
    pub changed: (i64, u32),
    pub modified: (i64, u32),
    pub inline: Vec<u8>,
}
impl Inode {
    pub fn parse(bytes: &[u8], key: Key) -> Result<Self> {
        ensure!(
            bytes.len() >= 160 && key.kind == 0 && key.value == 0,
            "invalid UBIFS inode key/size"
        );
        let size = wide(bytes, 48)?;
        ensure!(
            size <= ((1u64 << 29) * 4096),
            "UBIFS inode exceeds simple-key file size limit"
        );
        let mode = word(bytes, 104)?;
        let flags = word(bytes, 108)?;
        ensure!(
            flags & !0x3f == 0,
            "unsupported UBIFS inode flags/encryption"
        );
        let kind = FileType::from_mode(mode)?;
        let data_length = word(bytes, 112)? as usize;
        ensure!(
            data_length <= BLOCK_SIZE && bytes.len() == 160 + data_length,
            "invalid UBIFS inline length"
        );
        let inline = bytes[160..].to_vec();
        ensure!(
            flags & 0x20 == 0 || (kind == FileType::File && size == data_length as u64),
            "invalid UBIFS xattr inode"
        );
        let links = word(bytes, 92)?;
        ensure!(
            links > 0 && (kind != FileType::Symlink || size == data_length as u64),
            "invalid UBIFS links/symlink size"
        );
        ensure!(
            kind != FileType::File || flags & 0x20 != 0 || data_length == 0,
            "unsupported UBIFS inline regular file"
        );
        ensure!(
            kind != FileType::Directory || data_length == 0,
            "UBIFS directory has inline content"
        );
        let timestamp = |seconds, nanos| -> Result<(i64, u32)> {
            let nanos = word(bytes, nanos)?;
            ensure!(nanos < 1_000_000_000, "invalid UBIFS timestamp nanoseconds");
            Ok((i64::from_le_bytes(number(bytes, seconds)?), nanos))
        };
        Ok(Self {
            number: key.inode,
            size,
            kind,
            mode,
            uid: word(bytes, 96)?,
            gid: word(bytes, 100)?,
            links,
            flags,
            accessed: timestamp(56, 80)?,
            changed: timestamp(64, 84)?,
            modified: timestamp(72, 88)?,
            inline,
        })
    }
}

#[derive(Debug, Clone)]
pub struct DirectoryEntry {
    pub parent: u32,
    pub inode: u32,
    pub name: String,
    pub kind: u8,
}
pub fn parse_entry(bytes: &[u8], key: Key, hash_kind: u8) -> Result<DirectoryEntry> {
    ensure!(
        bytes.len() >= 57 && (key.kind == 2 || key.kind == 3),
        "invalid UBIFS directory entry"
    );
    let inode =
        u32::try_from(wide(bytes, 40)?).context("UBIFS directory inode exceeds simple key")?;
    let length = u16::from_le_bytes(number(bytes, 50)?) as usize;
    ensure!(
        (1..=255).contains(&length) && bytes.len() == 57 + length && bytes[56 + length] == 0,
        "invalid UBIFS name length/termination"
    );
    let name_bytes = &bytes[56..56 + length];
    ensure!(
        !name_bytes.contains(&0) && (key.kind == 3 || !name_bytes.contains(&b'/')),
        "invalid UBIFS entry name"
    );
    let name = std::str::from_utf8(name_bytes)
        .context("unsupported non-UTF8 UBIFS name")?
        .to_owned();
    ensure!(
        key.kind == 3 || (name != "." && name != ".." && !name.contains('\\')),
        "unsupported UBIFS path component"
    );
    let hash = if hash_kind == 0 {
        let mut value = 0u32;
        for byte in name_bytes {
            let signed = i32::from(*byte as i8);
            value = value
                .wrapping_add((signed << 4) as u32)
                .wrapping_add((signed >> 4) as u32)
                .wrapping_mul(11);
        }
        value & 0x1fffffff
    } else {
        let mut value = 0u32;
        for (index, byte) in name_bytes.iter().take(4).enumerate() {
            value |= u32::from(*byte) << (index * 8);
        }
        value & 0x1fffffff
    };
    let hash = if hash <= 2 { hash + 3 } else { hash };
    ensure!(
        key.value == hash && inode != 0 && bytes[49] <= 6,
        "UBIFS directory hash/target/type mismatch"
    );
    Ok(DirectoryEntry {
        parent: key.inode,
        inode,
        name,
        kind: bytes[49],
    })
}
