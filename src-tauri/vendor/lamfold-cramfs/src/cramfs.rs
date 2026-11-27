//! cramfs reader, clean-roomed from the public format (`cramfs_fs.h`).
//!
//! Layout: a 76-byte superblock whose tail is the 12-byte root inode; inodes are
//! bitfield-packed (`mode`/`uid` in word 0, `size`/`gid` in word 1, `namelen`
//! and `offset` — both in 4-byte units — in word 2); a name follows each inode.
//! A directory's data is a run of `inode + name` records; a file's data is a
//! per-page block index (one u32 per 4 KiB page) and the data it references.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use lamfold::{
    checked_full_read_len, decode, BlockSource, Codec, DirEntry, FileKind, FoldError, FoldFrontend,
    Metadata, NodeId, Result, SubstrateCtx,
};

const MAGIC: u32 = 0x28CD_3D45;
const ROOT_INODE_OFF: usize = 64;
const SUPER_LEN: usize = 76;
const BOOT_PAD: usize = 512;
const SIGNATURE: &[u8; 16] = b"Compressed ROMFS";
const PAGE: u64 = 4096;
const FLAG_FSID_V2: u32 = 1;
const FLAG_SORTED_DIRS: u32 = 2;
const FLAG_HOLES: u32 = 0x100;
const FLAG_SHIFTED_ROOT: u32 = 0x400;
const FLAG_EXT_BLOCK_POINTERS: u32 = 0x0000_0800;
const SUPPORTED_FLAGS: u32 =
    FLAG_FSID_V2 | FLAG_SORTED_DIRS | FLAG_HOLES | FLAG_SHIFTED_ROOT | FLAG_EXT_BLOCK_POINTERS;
const BLOCK_UNCOMPRESSED: u32 = 1 << 31;
const BLOCK_DIRECT: u32 = 1 << 30;
const BLOCK_FLAGS: u32 = BLOCK_UNCOMPRESSED | BLOCK_DIRECT;

/// Locate the superblock in a prefix of the original filesystem bytes.
pub fn superblock_offset(prefix: &[u8]) -> Option<usize> {
    [0, BOOT_PAD].into_iter().find(|&offset| {
        prefix
            .get(offset..offset + 4)
            .is_some_and(|magic| magic == MAGIC.to_le_bytes() || magic == MAGIC.to_be_bytes())
            && (offset == 0 || prefix.get(offset + 16..offset + 32) == Some(SIGNATURE))
    })
}

struct BlockPointer(u32);

impl BlockPointer {
    fn value(&self) -> u64 {
        u64::from(self.0 & !BLOCK_FLAGS)
    }

    fn direct(&self) -> bool {
        self.0 & BLOCK_DIRECT != 0
    }

    fn uncompressed(&self) -> bool {
        self.0 & BLOCK_UNCOMPRESSED != 0
    }
}

struct CrInode {
    mode: u16,
    size: u64,
    namelen: usize,
    data_off: u64,
}

/// A mounted cramfs volume.
pub struct Cramfs<S: BlockSource> {
    src: S,
    size: u64,
    super_offset: u64,
    big_endian: bool,
    flags: u32,
}

impl<S: BlockSource> Cramfs<S> {
    fn read_checked(&mut self, off: u64, bytes: &mut [u8]) -> Result<()> {
        if off
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.size)
        {
            return Err(FoldError::Corrupt("cramfs: read outside filesystem"));
        }
        self.src.read_at(off, bytes)
    }

    fn word(&self, bytes: [u8; 4]) -> u32 {
        if self.big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        }
    }

    fn parse_inode(&mut self, off: u64) -> Result<CrInode> {
        let mut b = [0u8; 12];
        self.read_checked(off, &mut b)?;
        let w0 = self.word([b[0], b[1], b[2], b[3]]);
        let w1 = self.word([b[4], b[5], b[6], b[7]]);
        let w2 = self.word([b[8], b[9], b[10], b[11]]);
        Ok(CrInode {
            mode: if self.big_endian {
                (w0 >> 16) as u16
            } else {
                w0 as u16
            },
            size: u64::from(if self.big_endian {
                w1 >> 8
            } else {
                w1 & 0x00FF_FFFF
            }),
            namelen: (if self.big_endian { w2 >> 26 } else { w2 & 0x3F }) as usize * 4,
            data_off: u64::from(if self.big_endian {
                w2 & 0x03FF_FFFF
            } else {
                w2 >> 6
            }) * 4,
        })
    }

    fn kind(mode: u16) -> FileKind {
        match mode & 0xF000 {
            0x8000 => FileKind::Regular,
            0x4000 => FileKind::Directory,
            0xA000 => FileKind::Symlink,
            _ => FileKind::Other,
        }
    }

    fn block_pointer(&mut self, offset: u64) -> Result<BlockPointer> {
        let mut word = [0; 4];
        self.read_checked(offset, &mut word)?;
        let value = self.word(word);
        if value & BLOCK_FLAGS != 0 && self.flags & FLAG_EXT_BLOCK_POINTERS == 0 {
            return Err(FoldError::Corrupt("cramfs: missing extended pointer flag"));
        }
        Ok(BlockPointer(value))
    }

    fn validate_block(&self, start: u64, end: u64, table: (u64, u64)) -> Result<()> {
        if start < self.super_offset + SUPER_LEN as u64
            || end < start
            || end > self.size
            || end - start > PAGE * 2
            || (start < table.1 && end > table.0)
        {
            return Err(FoldError::Corrupt("cramfs: invalid block pointer"));
        }
        Ok(())
    }

    fn direct_block(
        &mut self,
        pointer: &BlockPointer,
        want: u64,
        table: (u64, u64),
    ) -> Result<(u64, u64)> {
        let start = pointer.value() << 2;
        if pointer.uncompressed() {
            self.validate_block(start, start + want, table)?;
            return Ok((start, start + want));
        }
        let mut length = [0; 2];
        self.validate_block(start, start + 2, table)?;
        self.read_checked(start, &mut length)?;
        let length = u64::from(if self.big_endian {
            u16::from_be_bytes(length)
        } else {
            u16::from_le_bytes(length)
        });
        if length == 0 {
            return Err(FoldError::Corrupt("cramfs: empty direct compressed block"));
        }
        self.validate_block(start + 2, start + 2 + length, table)?;
        Ok((start + 2, start + 2 + length))
    }

    /// Decode only the 4 KiB pages intersecting the logical range.
    fn read_bytes(&mut self, inode: &CrInode, offset: u64, length: usize) -> Result<Vec<u8>> {
        let total = checked_full_read_len(inode.size.saturating_sub(offset).min(length as u64))?;
        if total == 0 {
            return Ok(Vec::new());
        }
        let table_end = inode.data_off + inode.size.div_ceil(PAGE) * 4;
        if inode.data_off < self.super_offset + SUPER_LEN as u64 || table_end > self.size {
            return Err(FoldError::Corrupt("cramfs: invalid block table"));
        }
        let table = (inode.data_off, table_end);
        let first = offset / PAGE;
        let last = (offset + total as u64 - 1) / PAGE;
        let mut out = Vec::with_capacity(total);
        let mut previous_end = table_end;
        for page in first..=last {
            let want = PAGE.min(inode.size - page * PAGE);
            let pointer = self.block_pointer(inode.data_off + page * 4)?;
            let (start, end) = if pointer.direct() {
                self.direct_block(&pointer, want, table)?
            } else {
                if page == first && first != 0 {
                    let previous = self.block_pointer(inode.data_off + (page - 1) * 4)?;
                    previous_end = if previous.direct() {
                        self.direct_block(&previous, PAGE, table)?.1
                    } else {
                        previous.value()
                    };
                }
                if self.flags & FLAG_EXT_BLOCK_POINTERS == 0
                    && (previous_end < table_end
                        || pointer.value() > table_end + (page + 1) * PAGE * 2)
                {
                    return Err(FoldError::Corrupt("cramfs: page outside sequential data"));
                }
                self.validate_block(previous_end, pointer.value(), table)?;
                (previous_end, pointer.value())
            };
            previous_end = end;
            let want = want as usize;
            let skip = offset.saturating_sub(page * PAGE) as usize;
            let count = (want - skip).min(total - out.len());
            if pointer.uncompressed() {
                if end - start != want as u64 {
                    return Err(FoldError::Corrupt("cramfs: invalid raw page length"));
                }
                let written = out.len();
                out.resize(written + count, 0);
                self.read_checked(start + skip as u64, &mut out[written..])?;
            } else if end == start {
                if self.flags & FLAG_FSID_V2 != 0 && self.flags & FLAG_HOLES == 0 {
                    return Err(FoldError::Corrupt("cramfs: hole without HOLES flag"));
                }
                out.resize(out.len() + count, 0);
            } else {
                let mut comp = vec![0; (end - start) as usize];
                self.read_checked(start, &mut comp)?;
                let decoded = decode(Codec::Zlib, &comp, want)?;
                if decoded.len() != want {
                    return Err(FoldError::Corrupt("cramfs: short decoded page"));
                }
                out.extend_from_slice(&decoded[skip..skip + count]);
            }
        }
        Ok(out)
    }

    fn verify_crc(&mut self, expected: u32) -> Result<()> {
        let mut hasher = crc32fast::Hasher::new();
        let mut offset = self.super_offset;
        let mut buffer = vec![0; 64 * 1024];
        while offset < self.size {
            let count = (buffer.len() as u64).min(self.size - offset) as usize;
            self.read_checked(offset, &mut buffer[..count])?;
            if offset == self.super_offset {
                buffer[32..36].fill(0);
            }
            hasher.update(&buffer[..count]);
            offset += count as u64;
        }
        if hasher.finalize() != expected {
            return Err(FoldError::Corrupt("cramfs: CRC mismatch"));
        }
        Ok(())
    }
}

impl<S: BlockSource> FoldFrontend<S> for Cramfs<S> {
    const TAG: &'static str = "cramfs";

    fn probe(src: &mut S) -> Result<bool> {
        if src.len() < SUPER_LEN as u64 {
            return Ok(false);
        }
        let mut prefix = [0; BOOT_PAD + SUPER_LEN];
        let len = src.len().min(prefix.len() as u64) as usize;
        src.read_at(0, &mut prefix[..len])?;
        Ok(superblock_offset(&prefix[..len]).is_some_and(|offset| offset + SUPER_LEN <= len))
    }

    fn open(src: S, _cx: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut src = src;
        let mut prefix = [0; BOOT_PAD + SUPER_LEN];
        let len = src.len().min(prefix.len() as u64) as usize;
        src.read_at(0, &mut prefix[..len])?;
        let super_offset =
            superblock_offset(&prefix[..len]).ok_or(FoldError::Corrupt("cramfs: bad magic"))?;
        let head = prefix[..len]
            .get(super_offset..super_offset + SUPER_LEN)
            .ok_or(FoldError::Corrupt("cramfs: shorter than superblock"))?;
        let magic = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        let mut fs = Cramfs {
            src,
            size: 0,
            super_offset: super_offset as u64,
            big_endian: magic != MAGIC,
            flags: 0,
        };
        let flags = fs.word([head[8], head[9], head[10], head[11]]);
        if flags & !SUPPORTED_FLAGS != 0 {
            return Err(FoldError::Unsupported(
                "cramfs: unsupported filesystem flags",
            ));
        }
        fs.flags = flags;
        let has_crc = flags & FLAG_FSID_V2 != 0;
        fs.size = if has_crc {
            u64::from(fs.word([head[4], head[5], head[6], head[7]]))
        } else {
            // FSID v1 mkcramfs writes a placeholder instead of the volume size.
            fs.src.len()
        };
        let data_start = fs.super_offset + SUPER_LEN as u64;
        if fs.size < data_start || fs.size > fs.src.len() || &head[16..32] != SIGNATURE {
            return Err(FoldError::Corrupt(
                "cramfs: invalid superblock size or signature",
            ));
        }
        if has_crc {
            fs.verify_crc(fs.word([head[32], head[33], head[34], head[35]]))?;
        }
        let root = fs.parse_inode(fs.root())?;
        if Self::kind(root.mode) != FileKind::Directory
            || (root.size != 0
                && (root.data_off < data_start
                    || (flags & FLAG_SHIFTED_ROOT == 0 && root.data_off != data_start)))
        {
            return Err(FoldError::Corrupt("cramfs: invalid root directory"));
        }
        Ok(fs)
    }

    fn root(&self) -> NodeId {
        self.super_offset + ROOT_INODE_OFF as u64
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        cx: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self
            .read_dir(dir, cx)?
            .into_iter()
            .find(|e| e.name == name)
            .map(|e| e.node))
    }

    fn read_dir(&mut self, dir: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        let inode = self.parse_inode(dir)?;
        if Self::kind(inode.mode) != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        let end = inode.data_off + inode.size;
        if end > self.size
            || (inode.size != 0 && inode.data_off < self.super_offset + SUPER_LEN as u64)
        {
            return Err(FoldError::Corrupt("cramfs: directory outside filesystem"));
        }
        let mut out = Vec::new();
        let mut names = BTreeSet::new();
        let mut p = inode.data_off;
        while p < end {
            let child = self.parse_inode(p)?;
            if child.namelen == 0 || p + 12 + child.namelen as u64 > end || out.len() >= 100_000 {
                return Err(FoldError::Corrupt(
                    "cramfs: invalid directory entry or entry limit",
                ));
            }
            let mut name = vec![0u8; child.namelen];
            self.read_checked(p + 12, &mut name)?;
            let name = name.split(|&b| b == 0).next().unwrap_or(&name);
            let name = core::str::from_utf8(name)
                .map_err(|_| FoldError::Unsupported("cramfs: non-UTF-8 name"))?;
            if name.is_empty()
                || name == "."
                || name == ".."
                || name.contains('/')
                || name.contains('\\')
                || !names.insert(String::from(name))
            {
                return Err(FoldError::Corrupt("cramfs: invalid or duplicate name"));
            }
            out.push(DirEntry {
                name: String::from(name),
                node: p,
                kind: Self::kind(child.mode),
            });
            p += 12 + child.namelen as u64;
        }
        Ok(out)
    }

    fn metadata(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let inode = self.parse_inode(node)?;
        Ok(Metadata {
            kind: Self::kind(inode.mode),
            size: inode.size,
            mode: u32::from(inode.mode) & 0o7777,
        })
    }

    fn read_at(
        &mut self,
        node: NodeId,
        off: u64,
        buf: &mut [u8],
        _cx: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let inode = self.parse_inode(node)?;
        if !matches!(
            Self::kind(inode.mode),
            FileKind::Regular | FileKind::Symlink
        ) {
            return Err(FoldError::Unsupported("cramfs: node has no file data"));
        }
        if off >= inode.size {
            return Ok(0);
        }
        let data = self.read_bytes(&inode, off, buf.len())?;
        let n = data.len();
        buf[..n].copy_from_slice(&data);
        Ok(n)
    }

    fn read_link(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        let inode = self.parse_inode(node)?;
        if Self::kind(inode.mode) != FileKind::Symlink {
            return Ok(None);
        }
        Ok(Some(self.read_bytes(
            &inode,
            0,
            checked_full_read_len(inode.size)?,
        )?))
    }
}
