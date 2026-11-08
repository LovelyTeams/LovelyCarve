//! ext4 heuristic probes and directory plausibility scoring.
use super::*;
use std::collections::VecDeque;

/// Minimal read-only ext4 view over a partition slice of a `SectorSource`. All
/// offsets are relative to the partition start (`base`); every read is bounds
/// checked against `len` so untrusted reconstructions never panic.
pub(super) struct Ext4Probe<'a> {
    pub(super) source: &'a (dyn SectorSource + Send + Sync),
    pub(super) base: u64,
    pub(super) len: u64,
    pub(super) block_size: u64,
    pub(super) inodes_per_group: u64,
    pub(super) inode_size: u64,
    pub(super) desc_size: u64,
    pub(super) first_data_block: u64,
}

impl<'a> Ext4Probe<'a> {
    pub(super) fn open(
        source: &'a (dyn SectorSource + Send + Sync),
        base: u64,
        len: u64,
    ) -> Option<Self> {
        let sb = read_rel(source, base, len, 1024, 1024)?;
        if sb.len() < 0x100 || sb[0x38] != 0x53 || sb[0x39] != 0xEF {
            return None;
        }
        let log_block_size = le32(&sb, 0x18)?;
        if log_block_size > 6 {
            return None;
        }
        let block_size = 1024u64 << log_block_size;
        let inodes_per_group = le32(&sb, 0x28)? as u64;
        if inodes_per_group == 0 {
            return None;
        }
        let inode_size = match le16(&sb, 0x58)? {
            0 => 128,
            n => n as u64,
        };
        let feature_incompat = le32(&sb, 0x60)?;
        let sixtyfour = feature_incompat & 0x80 != 0;
        let desc_size = if sixtyfour {
            match le16(&sb, 0xFE)? {
                0 => 64,
                n => n as u64,
            }
        } else {
            32
        };
        let first_data_block = if block_size == 1024 { 1 } else { 0 };
        Some(Self {
            source,
            base,
            len,
            block_size,
            inodes_per_group,
            inode_size,
            desc_size,
            first_data_block,
        })
    }

    pub(super) fn block(&self, n: u64) -> Option<Vec<u8>> {
        let off = n.checked_mul(self.block_size)?;
        read_rel(
            self.source,
            self.base,
            self.len,
            off,
            self.block_size as usize,
        )
    }

    /// Inode-table starting block of group `g` from the group-descriptor table.
    pub(super) fn inode_table_block(&self, g: u64) -> Option<u64> {
        let gdt_off = (self.first_data_block + 1)
            .checked_mul(self.block_size)?
            .checked_add(g.checked_mul(self.desc_size)?)?;
        let e = read_rel(
            self.source,
            self.base,
            self.len,
            gdt_off,
            self.desc_size as usize,
        )?;
        let lo = le32(&e, 0x8)? as u64;
        let hi = if self.desc_size >= 64 {
            le32(&e, 0x28).unwrap_or(0) as u64
        } else {
            0
        };
        Some(lo | (hi << 32))
    }

    pub(super) fn read_inode(&self, ino: u64) -> Option<Vec<u8>> {
        if ino == 0 {
            return None;
        }
        let g = (ino - 1) / self.inodes_per_group;
        let idx = (ino - 1) % self.inodes_per_group;
        let itbl = self.inode_table_block(g)?;
        let off = itbl
            .checked_mul(self.block_size)?
            .checked_add(idx.checked_mul(self.inode_size)?)?;
        read_rel(
            self.source,
            self.base,
            self.len,
            off,
            self.inode_size as usize,
        )
    }

    /// Data blocks of an inode: depth-0 extent leaves or the 12 direct
    /// pointers. Deeper extent trees / htree are skipped (return empty) —
    /// scoring only needs the small directories that fit inline.
    pub(super) fn inode_blocks(&self, inode: &[u8]) -> Vec<u64> {
        if inode.len() < 0x28 + 60 {
            return Vec::new();
        }
        let flags = match le32(inode, 0x20) {
            Some(f) => f,
            None => return Vec::new(),
        };
        let iblock = &inode[0x28..0x28 + 60];
        let mut out = Vec::new();
        if flags & 0x80000 != 0 {
            if iblock[0] != 0x0A || iblock[1] != 0xF3 {
                return out;
            }
            let entries = le16(iblock, 2).unwrap_or(0).min(4);
            let depth = le16(iblock, 6).unwrap_or(1);
            if depth != 0 {
                return out;
            }
            for i in 0..entries as usize {
                let e = 12 + i * 12;
                let ee_len = le16(iblock, e + 4).unwrap_or(0).min(4096);
                let start_lo = le32(iblock, e + 8).unwrap_or(0) as u64;
                let start_hi = le16(iblock, e + 2).unwrap_or(0) as u64;
                let start = start_lo | (start_hi << 32);
                for k in 0..ee_len as u64 {
                    out.push(start + k);
                    if out.len() >= 16 {
                        return out;
                    }
                }
            }
        } else {
            for i in 0..12 {
                if let Some(b) = le32(iblock, i * 4) {
                    if b != 0 {
                        out.push(b as u64);
                    }
                }
            }
        }
        out
    }

    /// List a directory inode as `(name, child_inode)`. Empty when the inode
    /// is not a directory or any read is malformed.
    pub(super) fn list_dir(&self, ino: u64) -> Vec<(String, u64)> {
        let inode = match self.read_inode(ino) {
            Some(i) => i,
            None => return Vec::new(),
        };
        let mode = match le16(&inode, 0) {
            Some(m) => m,
            None => return Vec::new(),
        };
        if mode & 0xF000 != 0x4000 {
            return Vec::new();
        }
        let mut out = Vec::new();
        for b in self.inode_blocks(&inode).into_iter().take(8) {
            let data = match self.block(b) {
                Some(d) if d.len() == self.block_size as usize => d,
                _ => continue,
            };
            let mut p = 0usize;
            while p + 8 <= data.len() {
                let e_ino = match le32(&data, p) {
                    Some(v) => v as u64,
                    None => break,
                };
                let rec = le16(&data, p + 4).unwrap_or(0) as usize;
                let nl = data[p + 6] as usize;
                if rec < 8 || !rec.is_multiple_of(4) || p + rec > data.len() || 8 + nl > rec {
                    return Vec::new();
                }
                if e_ino != 0 && nl > 0 && p + 8 + nl <= data.len() {
                    let name = String::from_utf8_lossy(&data[p + 8..p + 8 + nl]).into_owned();
                    out.push((name, e_ino));
                }
                p += rec;
            }
        }
        out
    }
}

/// Bounds-checked relative read: `Some(bytes)` only when the whole window lies
/// inside `[0, len)` and the source returned the full request.
pub(super) fn read_rel(
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

pub(super) fn le16(buf: &[u8], off: usize) -> Option<u16> {
    buf.get(off..off + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
}

pub(super) fn le32(buf: &[u8], off: usize) -> Option<u32> {
    buf.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

// ---------------------------------------------------------------------------
// NTFS $MFT continuity validation (panic-free, bounds-checked)
// ---------------------------------------------------------------------------
//

// A wrong RAID reconstruction produces garbage that makes a general ext4
// parser slice out of bounds and panic, so candidate scoring cannot reuse the
// vendored `ext4-view` (it is for browsing *valid* volumes, after the layout
// is known). This minimal walker only does what scoring needs — list the root
// directory and descend a bounded tree — and every read is length-checked so the
// worst a wrong candidate does is score 0.

/// Score a candidate by how many directories of an ext4 partition descend
/// cleanly, verifying their self and parent entries. A NAS often has only one
/// share under the root, so validation must reach that share's descendants.
/// Returns `(successful_descents, inodes_probed, matching_file_headers)`.
pub(super) fn ext4_descend_score(
    source: Arc<dyn SectorSource + Send + Sync>,
    base: u64,
    len: u64,
) -> (u32, usize, u32) {
    let probe = match Ext4Probe::open(source.as_ref(), base, len) {
        Some(p) => p,
        None => return (0, 0, 0),
    };
    let root = probe.list_dir(2);
    if !root.iter().any(|(name, inode)| name == "." && *inode == 2)
        || !root.iter().any(|(name, inode)| name == ".." && *inode == 2)
    {
        return (0, 0, 0);
    }
    const MAX_INODES_PROBED: usize = 256;
    let mut pending: VecDeque<_> = root
        .into_iter()
        .filter(|(name, _)| name != "." && name != "..")
        .map(|(name, inode)| (inode, 2, name))
        .take(MAX_INODES_PROBED)
        .collect();
    let mut visited = HashSet::from([2]);
    let mut probed = 0;
    let mut ok = 0u32;
    let mut file_headers = 0;
    while let Some((inode, parent, name)) = pending.pop_front() {
        if probed >= MAX_INODES_PROBED || ok as usize >= MAX_SUBDIRS_PROBED {
            break;
        }
        if !visited.insert(inode) {
            continue;
        }
        probed += 1;
        if let Some(metadata) = probe.read_inode(inode) {
            if le16(&metadata, 0).is_some_and(|mode| mode & 0xf000 == 0x8000) {
                let first_block = probe.inode_blocks(&metadata).first().copied();
                if let Some(data) = first_block.and_then(|block| probe.block(block)) {
                    file_headers += u32::from(file_header_matches(&name, &data));
                }
                continue;
            }
        }
        // `list_dir` returns empty for files (mode check) and for any
        // malformed read, so a non-empty result means a real, walkable dir.
        let entries = probe.list_dir(inode);
        if !entries
            .iter()
            .any(|(name, child)| name == "." && *child == inode)
            || !entries
                .iter()
                .any(|(name, child)| name == ".." && *child == parent)
        {
            continue;
        }
        ok += 1;
        for (name, child) in entries
            .into_iter()
            .filter(|(name, _)| name != "." && name != "..")
        {
            if pending.len() + probed >= MAX_INODES_PROBED {
                break;
            }
            pending.push_back((child, inode, name));
        }
    }
    (ok, probed, file_headers)
}

/// Positive signatures only: an unusual or intentionally renamed evidence
/// file contributes no vote rather than disqualifying the filesystem.
fn file_header_matches(name: &str, data: &[u8]) -> bool {
    let Some((_, extension)) = name.rsplit_once('.') else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    match extension.as_str() {
        "png" => data.starts_with(b"\x89PNG\r\n\x1a\n"),
        "jpg" | "jpeg" => data.starts_with(&[0xff, 0xd8, 0xff]),
        "gif" => data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a"),
        "pdf" => data.starts_with(b"%PDF-"),
        "zip" | "docx" | "xlsx" | "pptx" | "odt" | "ods" | "odp" => data.starts_with(b"PK\x03\x04"),
        "doc" | "xls" | "ppt" => {
            data.starts_with(&[0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1])
        }
        "mp4" | "mov" | "m4v" => data.get(4..8) == Some(b"ftyp"),
        "7z" => data.starts_with(b"7z\xbc\xaf\x27\x1c"),
        "rar" => data.starts_with(b"Rar!\x1a\x07"),
        _ => false,
    }
}
