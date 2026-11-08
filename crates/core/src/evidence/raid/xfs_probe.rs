//! Panic-free, bounds-checked XFS directory-descent validator for the
//! metadata-less RAID heuristic.
//!
//! The RAID candidate scorer needs a way to tell a correct stripe layout from
//! a wrong one when the only filesystem on the array is XFS (RHEL/CentOS/Rocky
//! default) — directly on a partition, or as the root LV inside LVM. The
//! vendored `xfs-fuse` crate cannot be used for scoring: it `panic!`s on the
//! garbage a wrong reconstruction produces and keeps the active superblock in a
//! process-global `OnceLock`, so it cannot be driven across many candidates.
//!
//! This module is the XFS counterpart to `Ext4Probe` / `NtfsProbe` in
//! `heuristic.rs`. It walks the directory tree from the root inode and scores a
//! candidate by **consistency**, not breadth:
//!
//! - every child inode named by a directory must resolve (valid `IN` magic;
//!   for v5 inodes the self-back-pointer `di_ino` and filesystem UUID must
//!   match — forgery-resistant fields a wrong layout cannot fake), and
//! - the directory-entry file type must agree with the resolved inode's mode.
//!
//! A correct layout descends hundreds of directories with zero unresolved
//! children and zero type contradictions; a wrong layout corrupts inodes at the
//! first stripe boundary it crosses and accrues unresolved children within a
//! couple of directories, so the walk early-aborts. Breadth-counting alone is
//! NOT a safe signal (a wrong layout can stay superficially readable across the
//! densely-written front of the volume and out-count the right one) — the
//! unresolved/mismatch gate is what makes this forensic-grade.
//!
//! Every read is bounds-checked against the region length, so the worst a wrong
//! candidate does is score 0.

use std::collections::{HashSet, VecDeque};

use crate::evidence::partitions::SectorSource;

/// XFS superblock magic, "XFSB" big-endian.
const XFS_SB_MAGIC: u32 = 0x5846_5342;
/// XFS on-disk inode magic, "IN" big-endian.
const XFS_DINODE_MAGIC: u16 = 0x494E;
/// v5 directory data/block headers carry a 64-byte CRC header (`XDB3`/`XDD3`);
/// v4 carry a 16-byte header (`XD2B`/`XD2D`).
const XDB3: u32 = 0x5844_4233;
const XDD3: u32 = 0x5844_4433;
const XD2B: u32 = 0x5844_3242;
const XD2D: u32 = 0x5844_3244;

/// Cap on directories descended before declaring success — enough to be
/// decisive while bounding cost. The correct layout reaches this; wrong ones
/// early-abort far sooner.
const MAX_DIRS_PROBED: u32 = 256;
/// Minimum cleanly-descended directories for an XFS-verified accept. A bare
/// `/boot` XFS (a handful of directories) stays below this so it never decides
/// a layout on its own; a real populated volume (a RAID's root filesystem)
/// clears it easily. Set well above the few directories a wrong layout can fake
/// before hitting a corrupted inode.
const MIN_DIRS_FOR_PASS: u32 = 16;
/// Directory floor for the cheap "promising" gate that decides whether a
/// candidate is worth the (4 MiB) LVM open. Lower than the accept floor: a
/// small `/boot` clears it, a wrong layout does not.
const MIN_DIRS_FOR_GATE: u32 = 3;
/// Stop descending once this many contradictions accrue. A correct layout
/// produces zero; the small slack absorbs the occasional directory format this
/// minimal walker does not fully parse without penalising the true layout.
const ABORT_AFTER_CONTRADICTIONS: u32 = 2;

fn be16(buf: &[u8], off: usize) -> Option<u16> {
    buf.get(off..off + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
}

fn be32(buf: &[u8], off: usize) -> Option<u32> {
    buf.get(off..off + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be64(buf: &[u8], off: usize) -> Option<u64> {
    buf.get(off..off + 8)
        .map(|s| u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

/// Bounds-checked relative read: `Some(bytes)` only when the whole window lies
/// inside `[0, len)` and the source returned the full request.
fn read_rel(
    source: &(dyn SectorSource + Send + Sync),
    base: u64,
    len: u64,
    rel: u64,
    n: usize,
) -> Option<Vec<u8>> {
    let end = rel.checked_add(n as u64)?;
    if end > len {
        return None;
    }
    let bytes = source.read(base + rel, n).ok()?;
    if bytes.len() != n {
        return None;
    }
    Some(bytes)
}

/// dirent file-type byte → expected inode `di_mode` top nibble (`S_IFMT`).
fn ftype_to_mode(ft: u8) -> Option<u16> {
    Some(match ft {
        1 => 0x8000, // regular
        2 => 0x4000, // directory
        3 => 0x2000, // char device
        4 => 0x6000, // block device
        5 => 0x1000, // fifo
        6 => 0xC000, // socket
        7 => 0xA000, // symlink
        _ => return None,
    })
}

/// Minimal read-only XFS view over a region `[base, base+len)` of a
/// `SectorSource`. All field offsets are the canonical XFS on-disk layout
/// (big-endian); every read is bounds-checked against `len`.
struct XfsProbe<'a> {
    source: &'a (dyn SectorSource + Send + Sync),
    base: u64,
    len: u64,
    blocksize: u64,
    inodesize: u64,
    agblocks: u64,
    agblklog: u32,
    inopblog: u32,
    rootino: u64,
    has_ftype: bool,
    uuid: Vec<u8>,
}

impl<'a> XfsProbe<'a> {
    fn open(source: &'a (dyn SectorSource + Send + Sync), base: u64, len: u64) -> Option<Self> {
        let sb = read_rel(source, base, len, 0, 512)?;
        if be32(&sb, 0)? != XFS_SB_MAGIC {
            return None;
        }
        let blocksize = be32(&sb, 0x04)? as u64;
        let uuid = sb.get(0x20..0x30)?.to_vec();
        let rootino = be64(&sb, 0x38)?;
        let agblocks = be32(&sb, 0x54)? as u64;
        let versionnum = be16(&sb, 0x64)?;
        let inodesize = be16(&sb, 0x68)? as u64;
        let inopblog = *sb.get(0x7B)? as u32;
        let agblklog = *sb.get(0x7C)? as u32;
        // Reject implausible geometry (also guards the shift math below).
        if !blocksize.is_power_of_two()
            || blocksize < 512
            || inodesize < 256
            || agblocks == 0
            || agblklog >= 64
            || inopblog >= 64
        {
            return None;
        }
        match versionnum & 0x000F {
            4 | 5 => {}
            _ => return None,
        }
        // ftype is recorded either in sb_features2 (v4 era) or, on v5, the
        // incompat feature set. Getting this wrong shifts every dirent field,
        // so derive it the same way the kernel does.
        let features2 = be32(&sb, 0xC8).unwrap_or(0);
        let incompat = be32(&sb, 0xD8).unwrap_or(0);
        let has_ftype = (features2 & 0x0000_0200) != 0 || (incompat & 0x0000_0001) != 0;
        Some(Self {
            source,
            base,
            len,
            blocksize,
            inodesize,
            agblocks,
            agblklog,
            inopblog,
            rootino,
            has_ftype,
            uuid,
        })
    }

    /// Byte offset of inode `ino` within the region, from its packed
    /// AG-number / AG-block / in-block-index encoding.
    fn inode_offset(&self, ino: u64) -> Option<u64> {
        let agno = ino >> (self.agblklog + self.inopblog);
        let agbno = (ino >> self.inopblog) & ((1u64 << self.agblklog) - 1);
        let inoidx = ino & ((1u64 << self.inopblog) - 1);
        let fsblock = agno.checked_mul(self.agblocks)?.checked_add(agbno)?;
        fsblock
            .checked_mul(self.blocksize)?
            .checked_add(inoidx.checked_mul(self.inodesize)?)
    }

    /// AG number an inode lives in (diagnostic / coverage only).
    fn ino_ag(&self, ino: u64) -> u64 {
        ino >> (self.agblklog + self.inopblog)
    }

    /// Read inode `ino`, returning `(di_version, di_format, di_mode,
    /// di_forkoff, raw)`. `None` when the inode does not carry the `IN` magic
    /// or, for v5 inodes, fails its self-back-pointer / UUID check — the
    /// forgery-resistant fields a wrong RAID layout cannot reproduce.
    fn read_inode(&self, ino: u64) -> Option<(i8, u8, u16, u8, Vec<u8>)> {
        let off = self.inode_offset(ino)?;
        let raw = read_rel(
            self.source,
            self.base,
            self.len,
            off,
            self.inodesize as usize,
        )?;
        if be16(&raw, 0)? != XFS_DINODE_MAGIC {
            return None;
        }
        let di_mode = be16(&raw, 2)?;
        let di_version = *raw.get(4)? as i8;
        let di_format = *raw.get(5)?;
        let di_forkoff = *raw.get(0x52)?;
        if di_version >= 3 {
            // v5: di_ino @0x98 stores the inode's own number, di_uuid @0xA0 the
            // filesystem UUID. Both must match or this is not really this inode.
            if be64(&raw, 0x98)? != ino {
                return None;
            }
            if raw.get(0xA0..0xB0)? != self.uuid.as_slice() {
                return None;
            }
        }
        Some((di_version, di_format, di_mode, di_forkoff, raw))
    }

    /// Top nibble (`S_IFMT`) of an inode's mode, or `None` if it does not
    /// resolve.
    fn inode_mode_kind(&self, ino: u64) -> Option<u16> {
        let (_v, _f, mode, _fk, _raw) = self.read_inode(ino)?;
        Some(mode & 0xF000)
    }

    fn literal_area(di_version: i8) -> usize {
        if di_version >= 3 {
            0xB0
        } else {
            0x64
        }
    }

    /// List a directory inode as `(name, child_inode, dirent_ftype)`. Empty
    /// when the inode is not a directory, malformed, or stored in a format this
    /// minimal walker does not parse (btree dirs) — never a panic.
    fn list_dir(&self, ino: u64) -> Vec<(Vec<u8>, u64, Option<u8>)> {
        let Some((ver, fmt, mode, forkoff, raw)) = self.read_inode(ino) else {
            return Vec::new();
        };
        if mode & 0xF000 != 0x4000 {
            return Vec::new();
        }
        let lit = Self::literal_area(ver);
        match fmt {
            1 => self.list_shortform(&raw, lit),
            2 => self.list_extents(&raw, lit, forkoff, ino),
            // 0 = dev, 3 = btree, others: not parsed for scoring.
            _ => Vec::new(),
        }
    }

    /// Shortform (inline) directory: entries packed directly in the inode
    /// literal area. `.`/`..` are not stored on disk and are not emitted.
    fn list_shortform(&self, raw: &[u8], lit: usize) -> Vec<(Vec<u8>, u64, Option<u8>)> {
        let mut out = Vec::new();
        let Some(count) = raw.get(lit).copied() else {
            return out;
        };
        let Some(i8count) = raw.get(lit + 1).copied() else {
            return out;
        };
        let big = i8count > 0;
        // hdr: count(1) + i8count(1) + parent inode (8 if i8count>0 else 4).
        let mut p = lit + 2 + if big { 8 } else { 4 };
        let ino_size = if big { 8 } else { 4 };
        let ftype_size = if self.has_ftype { 1 } else { 0 };
        for _ in 0..count {
            let Some(namelen) = raw.get(p).copied() else {
                break;
            };
            let namelen = namelen as usize;
            // entry: namelen(1) + offset(2) + name + ftype? + inumber(4/8).
            let name_start = p + 3;
            let rec = 3 + namelen + ftype_size + ino_size;
            if p + rec > raw.len() {
                break;
            }
            let Some(name) = raw.get(name_start..name_start + namelen) else {
                break;
            };
            let ftype = if self.has_ftype {
                raw.get(name_start + namelen).copied()
            } else {
                None
            };
            let ino_off = name_start + namelen + ftype_size;
            let child = if big {
                be64(raw, ino_off).unwrap_or(0)
            } else {
                be32(raw, ino_off).unwrap_or(0) as u64
            };
            if child != 0 {
                out.push((name.to_vec(), child, ftype));
            }
            p += rec;
        }
        out
    }

    /// Extents-format directory: parse the packed 16-byte `BmbtRec` extent list
    /// in the data fork and scan the first several data blocks for entries.
    /// Leaf / free-index blocks (at the magic high file offsets) are skipped.
    fn list_extents(
        &self,
        raw: &[u8],
        lit: usize,
        forkoff: u8,
        owner: u64,
    ) -> Vec<(Vec<u8>, u64, Option<u8>)> {
        let fork_end = if forkoff != 0 {
            lit + forkoff as usize * 8
        } else {
            self.inodesize as usize
        };
        if fork_end > raw.len() || fork_end <= lit {
            return Vec::new();
        }
        // Directory data lives at low file offsets; leaf/free-index blocks sit
        // at file offset 32 GiB / 64 GiB. In filesystem blocks that threshold
        // is 2^35 / blocksize.
        let leaf_fileoff = 1u64 << (35 - self.blocksize.trailing_zeros());
        let mut out = Vec::new();
        let mut p = lit;
        let mut blocks_read = 0u32;
        const MAX_BLOCKS: u32 = 8;
        while p + 16 <= fork_end && blocks_read < MAX_BLOCKS {
            let hi = be64(raw, p).unwrap_or(0) as u128;
            let lo = be64(raw, p + 8).unwrap_or(0) as u128;
            let rec = (hi << 64) | lo;
            let blockcount = (rec & ((1u128 << 21) - 1)) as u64;
            let startblock = ((rec >> 21) & ((1u128 << 52) - 1)) as u64;
            let startoff = ((rec >> 73) & ((1u128 << 54) - 1)) as u64;
            p += 16;
            if blockcount == 0 || startoff >= leaf_fileoff {
                continue;
            }
            for i in 0..blockcount {
                if blocks_read >= MAX_BLOCKS {
                    break;
                }
                let fsb = startblock + i;
                let agno = fsb >> self.agblklog;
                let agbno = fsb & ((1u64 << self.agblklog) - 1);
                let Some(byte) = agno
                    .checked_mul(self.agblocks)
                    .and_then(|x| x.checked_add(agbno))
                    .and_then(|x| x.checked_mul(self.blocksize))
                else {
                    continue;
                };
                self.scan_data_block(byte, owner, &mut out);
                blocks_read += 1;
            }
        }
        out
    }

    /// Parse one directory data block at byte offset `byte`, appending
    /// `(name, inode, ftype)` for each real entry. For v5 blocks the
    /// forgery-resistant header back-pointers (owner inode + filesystem UUID)
    /// are verified first, so a wrong layout landing on a stale block is
    /// rejected instead of yielding garbage children.
    fn scan_data_block(&self, byte: u64, owner: u64, out: &mut Vec<(Vec<u8>, u64, Option<u8>)>) {
        let Some(block) = read_rel(
            self.source,
            self.base,
            self.len,
            byte,
            self.blocksize as usize,
        ) else {
            return;
        };
        let Some(magic) = be32(&block, 0) else {
            return;
        };
        let hdr = match magic {
            XDB3 | XDD3 => {
                // v5 header: blkno/lsn/uuid/owner. uuid @24, owner @40.
                if block.get(24..40) != Some(self.uuid.as_slice()) {
                    return;
                }
                if be64(&block, 40) != Some(owner) {
                    return;
                }
                64
            }
            XD2B | XD2D => 16,
            _ => return,
        };
        let ftype_size = if self.has_ftype { 1 } else { 0 };
        let mut p = hdr;
        let mut guard = 0u32;
        while p + 11 <= block.len() && guard < 8192 {
            guard += 1;
            // An unused entry starts with the freetag 0xFFFF, then its length.
            if be16(&block, p) == Some(0xFFFF) {
                let freelen = be16(&block, p + 2).unwrap_or(0) as usize;
                if freelen < 8 {
                    break;
                }
                p += freelen;
                continue;
            }
            let inumber = be64(&block, p).unwrap_or(0);
            let namelen = block.get(p + 8).copied().unwrap_or(0) as usize;
            if namelen == 0 || namelen > 255 {
                break;
            }
            let name_start = p + 9;
            let Some(name) = block.get(name_start..name_start + namelen) else {
                break;
            };
            let ftype = if self.has_ftype {
                block.get(name_start + namelen).copied()
            } else {
                None
            };
            // entry: inumber(8) + namelen(1) + name + ftype? + tag(2), padded
            // up to an 8-byte boundary.
            let rec = (8 + 1 + namelen + ftype_size + 2).div_ceil(8) * 8;
            if inumber != 0 && name != b"." && name != b".." {
                out.push((name.to_vec(), inumber, ftype));
            }
            p += rec;
        }
    }
}

/// Outcome of an XFS directory-descent scan.
pub(crate) struct XfsDescend {
    /// Directories that listed ≥1 child — the deep-validation score.
    pub clean_dirs: u32,
    /// Directories visited.
    pub probed: usize,
    /// Unresolved children + dirent-type/inode-mode contradictions. Zero for a
    /// correct layout.
    pub contradictions: u32,
}

impl XfsDescend {
    /// A confident match: enough directories descended with no contradiction.
    /// A bare `/boot` (few directories) stays below the directory floor so it
    /// never decides a layout alone; a populated root filesystem clears it.
    pub fn passed(&self) -> bool {
        self.clean_dirs >= MIN_DIRS_FOR_PASS && self.contradictions == 0
    }

    /// Cheap "looks like the right layout so far" signal, used to gate the
    /// (4 MiB) LVM open: a few directories descended with no contradiction.
    /// True even for a small `/boot`, false for a wrong layout (which accrues
    /// contradictions within a couple of directories).
    pub fn promising(&self) -> bool {
        self.clean_dirs >= MIN_DIRS_FOR_GATE && self.contradictions == 0
    }
}

/// Score an XFS region by directory-descent consistency. The walk early-aborts
/// once a couple of contradictions accrue, so a wrong RAID layout costs only a
/// handful of reads. Returns `(0, 0, 0)` when the region is not XFS.
pub(crate) fn xfs_descend_score(
    source: &(dyn SectorSource + Send + Sync),
    base: u64,
    len: u64,
) -> XfsDescend {
    let Some(probe) = XfsProbe::open(source, base, len) else {
        return XfsDescend {
            clean_dirs: 0,
            probed: 0,
            contradictions: 0,
        };
    };
    let mut clean_dirs = 0u32;
    let mut probed = 0usize;
    let mut contradictions = 0u32;
    let mut visited: HashSet<u64> = HashSet::new();
    let mut queue: VecDeque<u64> = VecDeque::new();
    queue.push_back(probe.rootino);
    visited.insert(probe.rootino);
    // Touch the root AG so the field counts as read even for tiny trees.
    let _ = probe.ino_ag(probe.rootino);

    while let Some(ino) = queue.pop_front() {
        if clean_dirs >= MAX_DIRS_PROBED || contradictions > ABORT_AFTER_CONTRADICTIONS {
            break;
        }
        probed += 1;
        let children = probe.list_dir(ino);
        if children.is_empty() {
            continue;
        }
        clean_dirs += 1;
        for (_, child, ftype) in children {
            match probe.inode_mode_kind(child) {
                Some(mode) => {
                    if let Some(expect) = ftype.and_then(ftype_to_mode) {
                        if mode != expect {
                            contradictions += 1;
                        }
                    }
                    if mode == 0x4000
                        && visited.insert(child)
                        && visited.len() < MAX_DIRS_PROBED as usize * 8
                    {
                        queue.push_back(child);
                    }
                }
                None => contradictions += 1,
            }
        }
    }
    XfsDescend {
        clean_dirs,
        probed,
        contradictions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory source over a byte buffer (offset-clamped, like the other
    /// probe tests).
    struct BufSource(Vec<u8>);
    impl SectorSource for BufSource {
        fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
            let start = (offset as usize).min(self.0.len());
            let end = (start + len).min(self.0.len());
            Ok(self.0[start..end].to_vec())
        }
        fn size(&self) -> u64 {
            self.0.len() as u64
        }
    }

    fn put_be32(buf: &mut [u8], off: usize, v: u32) {
        buf[off..off + 4].copy_from_slice(&v.to_be_bytes());
    }
    fn put_be16(buf: &mut [u8], off: usize, v: u16) {
        buf[off..off + 2].copy_from_slice(&v.to_be_bytes());
    }
    fn put_be64(buf: &mut [u8], off: usize, v: u64) {
        buf[off..off + 8].copy_from_slice(&v.to_be_bytes());
    }

    /// Minimal valid v5 XFS superblock (geometry only — no inodes/dirs).
    fn minimal_sb() -> Vec<u8> {
        let mut sb = vec![0u8; 512];
        put_be32(&mut sb, 0x00, XFS_SB_MAGIC);
        put_be32(&mut sb, 0x04, 4096); // blocksize
        put_be64(&mut sb, 0x08, 262_144); // dblocks
        for (i, b) in sb.iter_mut().enumerate().take(0x30).skip(0x20) {
            *b = (i as u8).wrapping_mul(3).wrapping_add(1); // arbitrary uuid
        }
        put_be64(&mut sb, 0x38, 128); // rootino
        put_be32(&mut sb, 0x54, 65_536); // agblocks
        put_be32(&mut sb, 0x58, 4); // agcount
        put_be16(&mut sb, 0x64, 5); // versionnum (v5)
        put_be16(&mut sb, 0x68, 512); // inodesize
        sb[0x7B] = 3; // inopblog (512-byte inodes, 4096 block → 8/block → log2=3)
        sb[0x7C] = 16; // agblklog
        sb
    }

    #[test]
    fn open_parses_v5_geometry() {
        let mut buf = minimal_sb();
        buf.resize(1 << 20, 0); // give the region some room
        let src = BufSource(buf);
        let probe = XfsProbe::open(&src, 0, src.size()).expect("opens minimal v5 sb");
        assert_eq!(probe.blocksize, 4096);
        assert_eq!(probe.inodesize, 512);
        assert_eq!(probe.agblocks, 65_536);
        assert_eq!(probe.rootino, 128);
        assert_eq!(probe.agblklog, 16);
        assert_eq!(probe.inopblog, 3);
    }

    #[test]
    fn open_rejects_non_xfs_and_bad_geometry() {
        // No magic.
        let src = BufSource(vec![0u8; 4096]);
        assert!(XfsProbe::open(&src, 0, 4096).is_none());

        // Magic but zero blocksize.
        let mut buf = minimal_sb();
        put_be32(&mut buf, 0x04, 0);
        let src = BufSource(buf);
        assert!(XfsProbe::open(&src, 0, 512).is_none());

        // Magic but unsupported version.
        let mut buf = minimal_sb();
        put_be16(&mut buf, 0x64, 3);
        let src = BufSource(buf);
        assert!(XfsProbe::open(&src, 0, 512).is_none());
    }

    #[test]
    fn descend_on_garbage_scores_zero_without_panic() {
        // A region with a valid-looking SB but garbage inode area must not
        // panic and must not pass — the root inode never resolves.
        let mut buf = minimal_sb();
        buf.resize(1 << 20, 0xAB);
        // restore the SB header bytes the resize fill clobbered past 512 (none
        // — resize only appends), keep SB intact.
        let src = BufSource(buf);
        let d = xfs_descend_score(&src, 0, src.size());
        assert_eq!(d.clean_dirs, 0);
        assert!(!d.passed());
        assert!(!d.promising());
    }

    #[test]
    fn descend_on_pure_noise_scores_zero() {
        let buf: Vec<u8> = (0..(256 * 1024u32)).map(|i| (i % 251) as u8).collect();
        let src = BufSource(buf);
        let d = xfs_descend_score(&src, 0, src.size());
        assert_eq!(d.clean_dirs, 0);
        assert!(!d.passed());
    }

    #[test]
    fn out_of_bounds_reads_return_none() {
        let src = BufSource(vec![0u8; 100]);
        // Window past the end is rejected.
        assert!(read_rel(&src, 0, 100, 50, 60).is_none());
        // Window fully inside is returned.
        assert_eq!(read_rel(&src, 0, 100, 10, 20).map(|v| v.len()), Some(20));
    }

    #[test]
    fn ftype_mode_mapping_covers_known_types() {
        assert_eq!(ftype_to_mode(1), Some(0x8000)); // reg
        assert_eq!(ftype_to_mode(2), Some(0x4000)); // dir
        assert_eq!(ftype_to_mode(7), Some(0xA000)); // symlink
        assert_eq!(ftype_to_mode(9), None); // unknown
    }
}
