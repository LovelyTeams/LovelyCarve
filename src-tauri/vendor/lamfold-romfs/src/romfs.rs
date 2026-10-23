//! Linux romfs reader, clean-roomed from the public `-rom1fs-` format.
//!
//! Layout: an 8-byte magic, big-endian 32-bit size + checksum, a NUL-terminated
//! volume name padded to 16 bytes; then a chain of file headers, each 16-byte
//! aligned: `next` (low 4 bits = type/exec, high bits = next header offset),
//! `info` (first child for a directory), `size`, checksum, a NUL-terminated name
//! padded to 16, then the file data. Everything is big-endian and uncompressed.

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use lamfold::{
    checked_full_read_len, BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, Metadata,
    NodeId, Result, SubstrateCtx,
};

const MAGIC: &[u8; 8] = b"-rom1fs-";
const MAX_NAME: usize = 256;
/// genromfs writes a hard link to a non-link header, so one hop is normal;
/// a longer chain is corrupt (and might be a loop).
const MAX_LINK_HOPS: usize = 8;

// file types (low 3 bits of `next`); devices/fifo/socket (4..=7) fall
// through to FileKind::Other. A hard link (0) names its target's header in
// `info` and is resolved to it.
const FT_HARDLINK: u32 = 0;
const FT_DIRECTORY: u32 = 1;
const FT_REGULAR: u32 = 2;
const FT_SYMLINK: u32 = 3;

struct Header {
    next_off: u64,
    ftype: u32,
    info: u64,
    size: u64,
    name: String,
    data_off: u64,
    executable: bool,
}

/// A mounted romfs volume.
pub struct Romfs<S: BlockSource> {
    src: S,
    first_hdr: u64,
    size: u64,
}

impl<S: BlockSource> Romfs<S> {
    fn read_header(&mut self, off: u64) -> Result<Header> {
        if off < self.first_hdr || off % 16 != 0 {
            return Err(FoldError::Corrupt("romfs: invalid header offset"));
        }
        let avail = core::cmp::min((16 + MAX_NAME) as u64, self.size.saturating_sub(off));
        if avail < 16 {
            return Err(FoldError::Corrupt("romfs: truncated header"));
        }
        let mut win = vec![0u8; avail as usize];
        self.src.read_at(off, &mut win)?;
        let next_raw = be_u32(&win, 0)?;
        let info = u64::from(be_u32(&win, 4)?);
        let size = u64::from(be_u32(&win, 8)?);
        let name_bytes = &win[16..];
        let nul = name_bytes
            .iter()
            .position(|&b| b == 0)
            .ok_or(FoldError::Corrupt("romfs: unterminated name"))?;
        let name = core::str::from_utf8(&name_bytes[..nul])
            .map_err(|_| FoldError::Unsupported("romfs: non-UTF-8 name"))?;
        if name.is_empty() || name.contains('/') || name.contains('\\') {
            return Err(FoldError::Corrupt("romfs: invalid entry name"));
        }
        let metadata_length = 16 + align16(nul + 1);
        if metadata_length > win.len() || checksum(&win[..metadata_length]) != 0 {
            return Err(FoldError::Corrupt("romfs: header checksum mismatch"));
        }
        let data_off = off + metadata_length as u64;
        let ftype = next_raw & 7;
        let next_off = u64::from(next_raw & 0xFFFF_FFF0);
        if (next_off != 0 && (next_off < self.first_hdr || next_off + 16 > self.size))
            || (matches!(ftype, FT_REGULAR | FT_SYMLINK) && data_off + size > self.size)
            || (!matches!(ftype, FT_REGULAR | FT_SYMLINK) && size != 0)
        {
            return Err(FoldError::Corrupt("romfs: entry exceeds filesystem"));
        }
        Ok(Header {
            next_off,
            ftype,
            info,
            size,
            data_off,
            name: String::from(name),
            executable: next_raw & 8 != 0,
        })
    }

    /// The header at `off`, with a hard link resolved to its target. Returns
    /// the target's offset too, which is the node a link stands for.
    fn resolved_header(&mut self, mut off: u64) -> Result<(u64, Header)> {
        for _ in 0..=MAX_LINK_HOPS {
            let h = self.read_header(off)?;
            if h.ftype != FT_HARDLINK {
                return Ok((off, h));
            }
            off = h.info;
        }
        Err(FoldError::Corrupt("romfs: hard link chain too long"))
    }

    fn kind(ftype: u32) -> FileKind {
        match ftype {
            FT_DIRECTORY => FileKind::Directory,
            FT_REGULAR => FileKind::Regular,
            FT_SYMLINK => FileKind::Symlink,
            _ => FileKind::Other,
        }
    }
}

impl<S: BlockSource> FoldFrontend<S> for Romfs<S> {
    const TAG: &'static str = "romfs";

    fn probe(src: &mut S) -> Result<bool> {
        if src.len() < 16 {
            return Ok(false);
        }
        let mut m = [0u8; 8];
        src.read_at(0, &mut m)?;
        Ok(&m == MAGIC)
    }

    fn open(src: S, _cx: &mut SubstrateCtx<'_>) -> Result<Self> {
        let mut src = src;
        if src.len() < 16 {
            return Err(FoldError::Corrupt("romfs: shorter than header"));
        }
        let mut head = [0u8; 16 + MAX_NAME];
        let avail = core::cmp::min((16 + MAX_NAME) as u64, src.len()) as usize;
        src.read_at(0, &mut head[..avail])?;
        if &head[..8] != MAGIC {
            return Err(FoldError::Corrupt("romfs: bad magic"));
        }
        let size = u64::from(be_u32(&head, 8)?);
        if size < 32 || size > src.len() || size % 16 != 0 {
            return Err(FoldError::Corrupt("romfs: invalid declared size"));
        }
        let mut sum_bytes = vec![0; core::cmp::min(512, size) as usize];
        src.read_at(0, &mut sum_bytes)?;
        if checksum(&sum_bytes) != 0 {
            return Err(FoldError::Corrupt("romfs: superblock checksum mismatch"));
        }
        // volume name: NUL-terminated from byte 16, padded to 16.
        let nul = head[16..core::cmp::min(avail, size as usize)]
            .iter()
            .position(|&b| b == 0)
            .ok_or(FoldError::Corrupt("romfs: unterminated volume name"))?;
        let first_hdr = 16 + align16(nul + 1) as u64;
        let mut fs = Romfs {
            src,
            first_hdr,
            size,
        };
        let (_, root) = fs.resolved_header(first_hdr)?;
        if root.ftype != FT_DIRECTORY {
            return Err(FoldError::Corrupt("romfs: root is not a directory"));
        }
        Ok(fs)
    }

    fn root(&self) -> NodeId {
        // The first file header is the root directory's first entry ("."), and
        // its own `info` points back here — so it serves as the root node.
        self.first_hdr
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
        let (_, head) = self.resolved_header(dir)?;
        if head.ftype != FT_DIRECTORY {
            return Err(FoldError::NotDirectory);
        }
        let mut out = Vec::new();
        let mut visited = BTreeSet::new();
        let mut names = BTreeSet::new();
        let mut o = head.info; // first child header
                               // bound the walk so a cyclic `next` can't spin forever
        while o != 0 {
            if !visited.insert(o) || visited.len() > 100_000 {
                return Err(FoldError::Corrupt("romfs: directory cycle or entry limit"));
            }
            let h = self.read_header(o)?;
            if h.name != "." && h.name != ".." {
                if !names.insert(h.name.clone()) {
                    return Err(FoldError::Corrupt("romfs: duplicate directory name"));
                }
                // A hard link stands for its target: Same node, same data.
                let (node, target) = self.resolved_header(o)?;
                out.push(DirEntry {
                    name: h.name,
                    node,
                    kind: Self::kind(target.ftype),
                });
            }
            o = h.next_off;
        }
        Ok(out)
    }

    fn metadata(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let (_, h) = self.resolved_header(node)?;
        Ok(Metadata {
            kind: Self::kind(h.ftype),
            size: h.size,
            mode: if h.executable { 0o555 } else { 0o444 },
        })
    }

    fn read_at(
        &mut self,
        node: NodeId,
        off: u64,
        buf: &mut [u8],
        _cx: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let (_, h) = self.resolved_header(node)?;
        if !matches!(h.ftype, FT_REGULAR | FT_SYMLINK) {
            return Err(FoldError::Unsupported("romfs: node has no file data"));
        }
        if off >= h.size {
            return Ok(0);
        }
        let n = core::cmp::min(buf.len() as u64, h.size - off) as usize;
        self.src.read_at(h.data_off + off, &mut buf[..n])?;
        Ok(n)
    }

    fn read_link(&mut self, node: NodeId, _cx: &mut SubstrateCtx<'_>) -> Result<Option<Vec<u8>>> {
        let (_, h) = self.resolved_header(node)?;
        if h.ftype != FT_SYMLINK {
            return Ok(None);
        }
        let len = checked_full_read_len(h.size)?;
        let mut target = vec![0u8; len];
        self.src.read_at(h.data_off, &mut target)?;
        Ok(Some(target))
    }
}

fn align16(n: usize) -> usize {
    (n + 15) & !15
}

fn checksum(bytes: &[u8]) -> u32 {
    bytes.chunks_exact(4).fold(0u32, |sum, word| {
        sum.wrapping_add(u32::from_be_bytes([word[0], word[1], word[2], word[3]]))
    })
}

fn be_u32(b: &[u8], o: usize) -> Result<u32> {
    b.get(o..o + 4)
        .and_then(|s| s.try_into().ok())
        .map(u32::from_be_bytes)
        .ok_or(FoldError::Corrupt("romfs: truncated u32"))
}
