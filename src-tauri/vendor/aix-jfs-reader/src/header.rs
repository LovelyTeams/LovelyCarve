use lamfold::{BlockSource, FoldError, Result};

use crate::{FIRST_INODES, INODE_SIZE, PAGE, be16, be32, require};

pub const SUPERBLOCK_OFFSETS: [u64; 2] = [4096, 31 * 4096];
pub const HEADER_SIZE: usize = 64;
const FSV3_MAGIC: u32 = 0x43218765;
const FSV3P_MAGIC: u32 = 0x65872143;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    Fsv3,
    Fsv3p,
}

#[derive(Clone, Debug)]
pub struct Header {
    pub layout: Layout,
    pub offset: u64,
    pub version: u32,
    pub volume_size: u64,
    pub fragment_size: u64,
    pub fragments_per_group: u64,
    pub inodes_per_group: u64,
    pub label: String,
    pub state: u8,
    pub filesystem_type: u8,
    pub compression: i32,
}

impl Header {
    pub fn inspect(bytes: &[u8], offset: u64, source_size: u64) -> Result<Option<Self>> {
        let layout = match bytes
            .get(..4)
            .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
        {
            Some(FSV3_MAGIC) => Layout::Fsv3,
            Some(FSV3P_MAGIC) => Layout::Fsv3p,
            _ => return Ok(None),
        };
        require(bytes.len() >= HEADER_SIZE, "AIX JFS truncated superblock")?;
        let version = be32(bytes, 48);
        let volume_size = u64::from(be32(bytes, 16)) * 512;
        let fragments_per_group = u64::from(be32(bytes, 8));
        let (fragment_size, inodes_per_group) = match layout {
            Layout::Fsv3 => (PAGE, fragments_per_group),
            Layout::Fsv3p => (u64::from(be32(bytes, 52)), u64::from(be32(bytes, 56))),
        };
        require(
            matches!(fragment_size, 512 | 1024 | 2048 | 4096)
                && u64::from(be16(bytes, 20)) == PAGE
                && volume_size <= source_size
                && volume_size.is_multiple_of(fragment_size)
                && (3..=u64::from(u32::MAX)).contains(&inodes_per_group)
                && FIRST_INODES + inodes_per_group * INODE_SIZE
                    <= volume_size.min(fragments_per_group * fragment_size),
            "AIX JFS bad volume or allocation group geometry",
        )?;
        if layout == Layout::Fsv3p && version != 1 {
            return Err(FoldError::Unsupported("AIX JFS fsv3p superblock version"));
        }
        let label = bytes[24..30]
            .split(|&byte| byte == 0)
            .next()
            .unwrap_or_default();
        Ok(Some(Self {
            layout,
            offset,
            version,
            volume_size,
            fragment_size,
            fragments_per_group,
            inodes_per_group,
            label: String::from_utf8_lossy(label).trim().to_owned(),
            state: bytes[40],
            filesystem_type: bytes[7],
            compression: be32(bytes, 60) as i32,
        }))
    }

    pub(crate) fn read(source: &mut impl BlockSource) -> Result<Option<Self>> {
        let length = source.len();
        Self::select(length, |offset, bytes| source.read_at(offset, bytes))
    }

    pub fn select(
        source_size: u64,
        mut read: impl FnMut(u64, &mut [u8]) -> Result<()>,
    ) -> Result<Option<Self>> {
        let mut best: Option<Self> = None;
        let mut error = None;
        for offset in SUPERBLOCK_OFFSETS {
            if offset + HEADER_SIZE as u64 > source_size {
                continue;
            }
            let mut bytes = [0; HEADER_SIZE];
            match read(offset, &mut bytes).and_then(|_| Self::inspect(&bytes, offset, source_size))
            {
                Ok(Some(header)) => {
                    if let Some(primary) = &best {
                        require(
                            primary.matches_copy(&header),
                            "AIX JFS inconsistent superblock copies",
                        )?;
                    } else {
                        best = Some(header);
                    }
                }
                Ok(None) => {}
                Err(cause) => {
                    if error.is_none() {
                        error = Some(cause);
                    }
                }
            }
        }
        match (best, error) {
            (None, Some(cause)) => Err(cause),
            (best, _) => Ok(best),
        }
    }

    fn matches_copy(&self, other: &Self) -> bool {
        self.layout == other.layout
            && self.version == other.version
            && self.volume_size == other.volume_size
            && self.fragment_size == other.fragment_size
            && self.fragments_per_group == other.fragments_per_group
            && self.inodes_per_group == other.inodes_per_group
            && self.filesystem_type == other.filesystem_type
            && self.compression == other.compression
    }
}
