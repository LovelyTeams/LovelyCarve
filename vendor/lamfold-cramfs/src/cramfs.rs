//! cramfs reader, clean-roomed from the public format (`cramfs_fs.h`).
//!
//! Layout: a 76-byte superblock whose tail is the 12-byte root inode; inodes are
//! bitfield-packed (`mode`/`uid` in word 0, `size`/`gid` in word 1, `namelen`
//! and `offset` — both in 4-byte units — in word 2); a name follows each inode.
//! A directory's data is a run of `inode + name` records; a file's data is a
//! per-page block index (one u32 end-offset per 4 KiB page) followed by the
//! zlib-compressed pages.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use lamfold::{
    checked_full_read_len, decode, BlockSource, Codec, DirEntry, FileKind, FoldError, FoldFrontend,
    Metadata, NodeId, Result, SubstrateCtx,
};

const MAGIC: u32 = 0x28CD_3D45;
const ROOT_INODE_OFF: u64 = 64;
const PAGE: u64 = 4096;
const FLAG_EXT_BLOCK_POINTERS: u32 = 0x0000_0800;

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

    /// Decode only the 4 KiB pages intersecting the logical range.
    fn read_bytes(&mut self, inode: &CrInode, offset: u64, length: usize) -> Result<Vec<u8>> {
        let total = checked_full_read_len(inode.size.saturating_sub(offset).min(length as u64))?;
        if total == 0 {
            return Ok(Vec::new());
        }
        let nblocks = inode.size.div_ceil(PAGE) as usize;
        // block index: one u32 end-offset per page
        let table_end = inode.data_off + nblocks as u64 * 4;
        if inode.data_off < 76 || table_end > self.size {
            return Err(FoldError::Corrupt("cramfs: invalid block table"));
        }
        let first = offset / PAGE;
        let last = (offset + total as u64 - 1) / PAGE;
        let mut out = Vec::with_capacity(total);
        for page in first..=last {
            let mut pointer = [0; 4];
            let start = if page == 0 {
                table_end
            } else {
                self.read_checked(inode.data_off + (page - 1) * 4, &mut pointer)?;
                u64::from(self.word(pointer))
            };
            self.read_checked(inode.data_off + page * 4, &mut pointer)?;
            let end = u64::from(self.word(pointer));
            let want = core::cmp::min(PAGE, inode.size - page * PAGE) as usize;
            if start < table_end || end < start || end > self.size || end - start > PAGE * 2 {
                return Err(FoldError::Corrupt("cramfs: invalid block pointer"));
            }
            let decoded = if end == start {
                if self.flags & 0x100 == 0 {
                    return Err(FoldError::Corrupt("cramfs: hole without HOLES flag"));
                }
                // a hole: the page is all zeros
                vec![0; want]
            } else {
                let mut comp = vec![0u8; (end - start) as usize];
                self.read_checked(start, &mut comp)?;
                let decoded = decode(Codec::Zlib, &comp, want)?;
                if decoded.len() != want {
                    return Err(FoldError::Corrupt("cramfs: short decoded page"));
                }
                decoded
            };
            let skip = offset.saturating_sub(page * PAGE) as usize;
            let count = core::cmp::min(want - skip, total - out.len());
            out.extend_from_slice(&decoded[skip..skip + count]);
        }
        out.truncate(total);
        Ok(out)
    }
}

impl<S: BlockSource> FoldFrontend<S> for Cramfs<S> {
    const TAG: &'static str = "cramfs";

    fn probe(src: &mut S) -> Result<bool> {
        if src.len() < ROOT_INODE_OFF + 12 {
            return Ok(false);
        }
        let mut m = [0u8; 4];
        src.read_at(0, &mut m)?;
        Ok(u32::from_le_bytes(m) == MAGIC || u32::from_be_bytes(m) == MAGIC)
    }

    fn open(src: S, _cx: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut src = src;
        if src.len() < ROOT_INODE_OFF + 12 {
            return Err(FoldError::Corrupt("cramfs: shorter than superblock"));
        }
        let mut head = [0u8; 76];
        src.read_at(0, &mut head)?;
        let magic = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        if magic != MAGIC && magic.swap_bytes() != MAGIC {
            return Err(FoldError::Corrupt("cramfs: bad magic"));
        }
        let mut fs = Cramfs {
            src,
            size: 76,
            big_endian: magic != MAGIC,
            flags: 0,
        };
        let flags = fs.word([head[8], head[9], head[10], head[11]]);
        if flags & FLAG_EXT_BLOCK_POINTERS != 0 || flags & !0x503 != 0 {
            return Err(FoldError::Unsupported(
                "cramfs: unsupported filesystem flags (including EXT_BLOCK_POINTERS)",
            ));
        }
        fs.flags = flags;
        if flags & 1 == 0 {
            return Err(FoldError::Unsupported("cramfs: FSID v1 without CRC"));
        }
        fs.size = u64::from(fs.word([head[4], head[5], head[6], head[7]]));
        if fs.size < 76 || fs.size > fs.src.len() || &head[16..32] != b"Compressed ROMFS" {
            return Err(FoldError::Corrupt(
                "cramfs: invalid superblock size or signature",
            ));
        }
        let expected = fs.word([head[32], head[33], head[34], head[35]]);
        let mut hasher = crc32fast::Hasher::new();
        let mut offset = 0;
        let mut buffer = vec![0; 64 * 1024];
        while offset < fs.size {
            let count = core::cmp::min(buffer.len() as u64, fs.size - offset) as usize;
            fs.read_checked(offset, &mut buffer[..count])?;
            if offset == 0 {
                buffer[32..36].fill(0);
            }
            hasher.update(&buffer[..count]);
            offset += count as u64;
        }
        if hasher.finalize() != expected {
            return Err(FoldError::Corrupt("cramfs: CRC mismatch"));
        }
        let root = fs.parse_inode(ROOT_INODE_OFF)?;
        if Self::kind(root.mode) != FileKind::Directory
            || (root.size != 0
                && (root.data_off < 76 || (flags & 0x400 == 0 && root.data_off != 76)))
        {
            return Err(FoldError::Corrupt("cramfs: invalid root directory"));
        }
        Ok(fs)
    }

    fn root(&self) -> NodeId {
        ROOT_INODE_OFF
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
        if end > self.size || inode.data_off % 4 != 0 {
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
