//! Prefix structures adapted from binwalk-ng's Rust formats/yaffs.rs at
//! 845806c163747eb524d66b5fe3de2ed86ea3aaad (MIT); see LICENSE-BINWALK-MIT.
//! Remaining YAFFS2 wire fields are independently decoded from format facts.
use anyhow::{Context, Result, ensure};
use zerocopy::{FromBytes, Immutable, KnownLayout, Unaligned};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    Little,
    Big,
}
impl ByteOrder {
    pub(crate) fn word(self, bytes: &[u8], offset: usize) -> Result<u32> {
        let bytes = bytes
            .get(offset..offset + 4)
            .context("truncated YAFFS2 field")?
            .try_into()?;
        Ok(match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        })
    }
}
#[derive(FromBytes, KnownLayout, Unaligned, Immutable)]
#[repr(C)]
struct ObjectPrefix {
    obj_type: [u8; 4],
    parent_id: [u8; 4],
    name_checksum: [u8; 2],
}
#[derive(FromBytes, KnownLayout, Unaligned, Immutable)]
#[repr(C)]
struct FilePrefix {
    mode: [u8; 4],
    uid: [u8; 4],
    gid: [u8; 4],
    atime: [u8; 4],
    mtime: [u8; 4],
    ctime: [u8; 4],
    file_size: [u8; 4],
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileType {
    File,
    Symlink,
    Directory,
    HardLink,
    Special,
}
#[derive(Debug, Clone)]
pub struct Node {
    pub object_id: u32,
    pub parent_id: u32,
    pub name: String,
    pub kind: FileType,
    pub size: u64,
    pub mode: Option<u32>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub accessed: Option<i64>,
    pub modified: Option<i64>,
    pub changed: Option<i64>,
    pub raw_extended_times: Option<[u32; 6]>,
    pub equivalent: u32,
    pub link_target: String,
    pub device: u32,
    pub(crate) shrink: bool,
    pub(crate) shadow: u32,
}
impl Node {
    pub(crate) fn pseudo(object_id: u32, name: &str) -> Self {
        Self {
            object_id,
            parent_id: 1,
            name: name.into(),
            kind: FileType::Directory,
            size: 0,
            mode: None,
            uid: None,
            gid: None,
            accessed: None,
            modified: None,
            changed: None,
            raw_extended_times: None,
            equivalent: 0,
            link_target: String::new(),
            device: 0,
            shrink: false,
            shadow: 0,
        }
    }
    pub fn hidden(&self) -> bool {
        matches!(self.object_id, 3 | 4) || matches!(self.parent_id, 3 | 4)
    }
}
fn string(bytes: &[u8]) -> Result<String> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .context("unterminated YAFFS2 name/alias")?;
    ensure!(
        bytes[end..].iter().all(|byte| matches!(byte, 0 | 255)),
        "unsupported YAFFS2 name padding/Unicode configuration"
    );
    Ok(std::str::from_utf8(&bytes[..end])
        .context("unsupported non-UTF8 YAFFS2 name/alias")?
        .to_owned())
}
fn timestamp(order: ByteOrder, bytes: &[u8], legacy: usize, extended: usize) -> Result<i64> {
    let legacy = order.word(bytes, legacy)?;
    let low = order.word(bytes, extended)?;
    let high = order.word(bytes, extended + 4)?;
    if high == u32::MAX {
        return Ok(i64::from(legacy));
    }
    ensure!(
        low == legacy,
        "unsupported YAFFS2 WinCE/inconsistent extended timestamp"
    );
    Ok(((u64::from(high) << 32) | u64::from(low)) as i64)
}
pub(crate) fn parse_header(bytes: &[u8], order: ByteOrder, object_id: u32) -> Result<Node> {
    ensure!(bytes.len() >= 512, "truncated YAFFS2 object header");
    let (prefix, _) = ObjectPrefix::ref_from_prefix(bytes)
        .map_err(|_| anyhow::anyhow!("truncated YAFFS2 prefix"))?;
    ensure!(
        prefix.name_checksum == [255, 255],
        "YAFFS2 legacy name-check field is invalid"
    );
    let kind = match order.word(&prefix.obj_type, 0)? {
        1 => FileType::File,
        2 => FileType::Symlink,
        3 => FileType::Directory,
        4 => FileType::HardLink,
        5 => FileType::Special,
        value => anyhow::bail!("unsupported YAFFS2 object type {value}"),
    };
    let parent_id = order.word(&prefix.parent_id, 0)?;
    ensure!(
        object_id > 4 || kind == FileType::Directory,
        "YAFFS2 reserved object is not a directory"
    );
    ensure!(
        parent_id < 0x40000 && (parent_id > 0 || object_id == 1),
        "YAFFS2 invalid parent identity"
    );
    let name = string(&bytes[10..266])?;
    ensure!(
        matches!(object_id, 1 | 3 | 4)
            || (!name.is_empty()
                && !matches!(name.as_str(), "." | "..")
                && !name.contains(['/', '\\'])),
        "YAFFS2 invalid file name"
    );
    ensure!(
        object_id != 1 || (kind == FileType::Directory && matches!(parent_id, 0 | 1)),
        "invalid YAFFS2 root header"
    );
    let (file, _) = FilePrefix::ref_from_prefix(&bytes[268..])
        .map_err(|_| anyhow::anyhow!("truncated YAFFS2 file header"))?;
    let high = order.word(bytes, 496)?;
    let size = if kind == FileType::File {
        u64::from(order.word(&file.file_size, 0)?)
            | if high == u32::MAX {
                0
            } else {
                u64::from(high) << 32
            }
    } else {
        0
    };
    ensure!(
        size <= i64::MAX as u64,
        "YAFFS2 file size exceeds signed format range"
    );
    let link_target = if kind == FileType::Symlink {
        string(&bytes[300..460])?
    } else {
        String::new()
    };
    let shrink = order.word(bytes, 508)?;
    let shadow = order.word(bytes, 504)?;
    ensure!(
        matches!(shrink, 0 | 1 | u32::MAX),
        "invalid YAFFS2 shrink flag"
    );
    ensure!(
        shadow == u32::MAX
            || shadow == 0
            || (shadow > 4 && shadow < 0x40000 && shadow != object_id),
        "invalid YAFFS2 shadow identity"
    );
    let attributes = kind != FileType::HardLink;
    ensure!(
        !attributes || order.word(&file.mode, 0)? <= u16::MAX as u32,
        "unsupported YAFFS2 non-POSIX mode"
    );
    if attributes {
        let mode = order.word(&file.mode, 0)? & 0xf000;
        let valid = mode == 0
            || match kind {
                FileType::File => mode == 0x8000,
                FileType::Directory => mode == 0x4000,
                FileType::Symlink => mode == 0xa000,
                FileType::Special => matches!(mode, 0x1000 | 0x2000 | 0x6000 | 0xc000),
                FileType::HardLink => false,
            };
        ensure!(valid, "YAFFS2 mode disagrees with object type");
    }
    let raw_extended_times = if attributes {
        Some([
            order.word(bytes, 464)?,
            order.word(bytes, 468)?,
            order.word(bytes, 472)?,
            order.word(bytes, 476)?,
            order.word(bytes, 480)?,
            order.word(bytes, 484)?,
        ])
    } else {
        None
    };
    Ok(Node {
        object_id,
        parent_id,
        name,
        kind,
        size: if kind == FileType::Symlink {
            link_target.len() as u64
        } else {
            size
        },
        mode: attributes.then(|| order.word(&file.mode, 0)).transpose()?,
        uid: attributes.then(|| order.word(&file.uid, 0)).transpose()?,
        gid: attributes.then(|| order.word(&file.gid, 0)).transpose()?,
        accessed: attributes
            .then(|| timestamp(order, bytes, 280, 472))
            .transpose()?,
        modified: attributes
            .then(|| timestamp(order, bytes, 284, 480))
            .transpose()?,
        changed: attributes
            .then(|| timestamp(order, bytes, 288, 464))
            .transpose()?,
        raw_extended_times,
        equivalent: if kind == FileType::HardLink {
            order.word(bytes, 296)?
        } else {
            0
        },
        link_target,
        device: order.word(bytes, 460)?,
        shrink: shrink == 1,
        shadow: if shadow == u32::MAX { 0 } else { shadow },
    })
}
