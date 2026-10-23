//! Wire fields adapted from jffs2-rs; byte order and CRC checks apply to every node.
use anyhow::{Context, Result, bail, ensure};

pub(crate) const ACCURATE: u16 = 0x2000;
pub(crate) const MAX_DATA: usize = 1024 * 1024;
const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::Algorithm {
    width: 32,
    poly: 0x04c11db7,
    init: 0,
    refin: true,
    refout: true,
    xorout: 0,
    check: 0x2dfd2d88,
    residue: 0,
});
pub(crate) fn checksum(bytes: &[u8]) -> u32 {
    CRC.checksum(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    Little,
    Big,
}
impl ByteOrder {
    pub(crate) fn u16(self, bytes: &[u8], offset: usize) -> Result<u16> {
        let value = bytes
            .get(offset..offset + 2)
            .context("JFFS2 truncated u16")?;
        let value: [u8; 2] = value.try_into()?;
        Ok(match self {
            Self::Little => u16::from_le_bytes(value),
            Self::Big => u16::from_be_bytes(value),
        })
    }
    pub(crate) fn u32(self, bytes: &[u8], offset: usize) -> Result<u32> {
        let value = bytes
            .get(offset..offset + 4)
            .context("JFFS2 truncated u32")?;
        let value: [u8; 4] = value.try_into()?;
        Ok(match self {
            Self::Little => u32::from_le_bytes(value),
            Self::Big => u32::from_be_bytes(value),
        })
    }
}

/// Validate a complete common header, including obsolete nodes' original CRC.
pub fn validate_header(bytes: &[u8]) -> Result<(ByteOrder, u16, u32)> {
    ensure!(bytes.len() >= 12, "JFFS2 truncated common header");
    let order = match bytes.get(..2) {
        Some([0x85, 0x19]) => ByteOrder::Little,
        Some([0x19, 0x85]) => ByteOrder::Big,
        _ => bail!("JFFS2 magic mismatch"),
    };
    let kind = order.u16(bytes, 2)?;
    let length = order.u32(bytes, 4)?;
    ensure!(length >= 12, "JFFS2 node shorter than common header");
    let mut original = bytes[..8].to_vec();
    let accurate = kind | ACCURATE;
    original[2..4].copy_from_slice(&match order {
        ByteOrder::Little => accurate.to_le_bytes(),
        ByteOrder::Big => accurate.to_be_bytes(),
    });
    ensure!(
        checksum(&original) == order.u32(bytes, 8)?,
        "JFFS2 header CRC mismatch"
    );
    Ok((order, kind, length))
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
    pub(crate) fn from_mode(mode: u32) -> Result<Self> {
        Self::from_dir_type(((mode & 0xf000) >> 12) as u8)
    }
    pub(crate) fn from_dir_type(kind: u8) -> Result<Self> {
        Ok(match kind {
            1 => Self::Fifo,
            2 => Self::CharacterDevice,
            4 => Self::Directory,
            6 => Self::BlockDevice,
            8 => Self::File,
            10 => Self::Symlink,
            12 => Self::Socket,
            _ => bail!("unsupported JFFS2 inode type {kind}"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inode {
    pub number: u32,
    pub version: u32,
    pub kind: FileType,
    pub mode: u32,
    pub uid: u16,
    pub gid: u16,
    pub size: u64,
    pub accessed: u32,
    pub modified: u32,
    pub changed: u32,
    /// JFFS2 permits an implicit root inode when only its dirents are present.
    pub implicit_root: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataNode {
    pub inode: Inode,
    pub offset: u32,
    pub compressed: u32,
    pub size: u32,
    pub compression: u8,
    pub data_crc: u32,
    pub source: u64,
}
pub(crate) fn inode(bytes: &[u8], order: ByteOrder, length: u32, source: u64) -> Result<DataNode> {
    ensure!(bytes.len() >= 68, "JFFS2 truncated inode");
    ensure!(
        checksum(&bytes[..60]) == order.u32(bytes, 64)?,
        "JFFS2 inode CRC mismatch"
    );
    let number = order.u32(bytes, 12)?;
    let mode = order.u32(bytes, 20)?;
    let size = order.u32(bytes, 52)?;
    let compressed = order.u32(bytes, 48)?;
    ensure!(
        number != 0 && compressed.checked_add(68) == Some(length),
        "JFFS2 inode length/number mismatch"
    );
    ensure!(
        size as usize <= MAX_DATA && compressed as usize <= MAX_DATA,
        "JFFS2 data node exceeds limit"
    );
    let offset = order.u32(bytes, 44)?;
    let file_size = order.u32(bytes, 28)?;
    let kind = FileType::from_mode(mode)?;
    ensure!(
        kind != FileType::Symlink || file_size != 0 || size == 0,
        "unsupported old JFFS2 symlink with zero isize"
    );
    ensure!(
        offset <= file_size && offset.checked_add(size).is_some(),
        "JFFS2 file offset overflow"
    );
    ensure!(
        order.u16(bytes, 58)? & !3 == 0,
        "unsupported JFFS2 inode flags"
    );
    if bytes[56] == 1 {
        ensure!(
            compressed == 0,
            "unsupported old JFFS2 swapped zero-node sizes"
        );
    }
    let data_crc = order.u32(bytes, 60)?;
    ensure!(
        compressed != 0 || data_crc == 0,
        "JFFS2 empty data CRC mismatch"
    );
    if matches!(bytes[56], 0 | 4) {
        ensure!(compressed == size, "JFFS2 stored size mismatch");
    }
    Ok(DataNode {
        inode: Inode {
            number,
            version: order.u32(bytes, 16)?,
            kind,
            mode,
            uid: order.u16(bytes, 24)?,
            gid: order.u16(bytes, 26)?,
            size: u64::from(file_size),
            accessed: order.u32(bytes, 32)?,
            modified: order.u32(bytes, 36)?,
            changed: order.u32(bytes, 40)?,
            implicit_root: false,
        },
        offset,
        compressed,
        size,
        compression: bytes[56],
        data_crc,
        source: source + 68,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub parent: u32,
    pub version: u32,
    pub inode: u32,
    pub modified: u32,
    pub name: String,
    pub kind: Option<FileType>,
}
pub(crate) fn dirent(bytes: &[u8], order: ByteOrder) -> Result<DirectoryEntry> {
    ensure!(bytes.len() >= 40, "JFFS2 truncated dirent");
    ensure!(
        checksum(&bytes[..32]) == order.u32(bytes, 32)?,
        "JFFS2 dirent CRC mismatch"
    );
    let length = bytes[28] as usize;
    ensure!(
        (1..=254).contains(&length) && bytes.len() == 40 + length,
        "JFFS2 dirent name length mismatch"
    );
    ensure!(
        checksum(&bytes[40..]) == order.u32(bytes, 36)?,
        "JFFS2 name CRC mismatch"
    );
    let name = std::str::from_utf8(&bytes[40..]).context("unsupported non-UTF8 JFFS2 name")?;
    ensure!(
        !matches!(name, "." | "..") && !name.contains(['/', '\\', '\0']),
        "JFFS2 invalid path component"
    );
    let parent = order.u32(bytes, 12)?;
    ensure!(parent != 0, "JFFS2 zero parent inode");
    Ok(DirectoryEntry {
        parent,
        version: order.u32(bytes, 16)?,
        inode: order.u32(bytes, 20)?,
        modified: order.u32(bytes, 24)?,
        name: name.to_owned(),
        kind: if bytes[29] == 0 {
            None
        } else {
            Some(FileType::from_dir_type(bytes[29])?)
        },
    })
}
