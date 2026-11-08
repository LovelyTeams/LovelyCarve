//! ext4 jbd2 journal mining for forensic recovery.
//!
//! The jbd2 journal is a circular log of metadata (and, in `data=journal`
//! mode, data) blocks written ahead of their final location. ext4-view replays
//! it only to present the *latest committed* state; for forensics the journal
//! is the opposite — it holds *prior* versions of inode-table, directory, and
//! bitmap blocks that the live filesystem has since overwritten. Recovering
//! those historical blocks can resurrect a deleted file's block pointers,
//! size, and `i_dtime`, or a directory entry whose name the live block no
//! longer carries.
//!
//! The vendored crate's jbd2 parser is `pub(crate)`, so this module reads the
//! journal inode's bytes through the public volume API and decodes the jbd2
//! structures (big-endian, stable on-disk format) independently. It records
//! every `(commit sequence, filesystem block, journal offset)` mapping —
//! *appended*, never collapsed — so callers can dump any historical revision
//! of a given filesystem block.

use anyhow::Result;

use crate::filesystem::volumes::ext4::Ext4Volume;

/// jbd2 block-header magic (`JBD2_MAGIC_NUMBER`), big-endian on disk.
const JBD2_MAGIC: u32 = 0xC03B_3998;

// jbd2 block types (`h_blocktype`).
const BTYPE_DESCRIPTOR: u32 = 1;
const BTYPE_COMMIT: u32 = 2;
const BTYPE_SUPERBLOCK_V1: u32 = 3;
const BTYPE_SUPERBLOCK_V2: u32 = 4;
const BTYPE_REVOKE: u32 = 5;

// jbd2 `s_feature_incompat` bits that affect descriptor-tag layout.
const FEAT_64BIT: u32 = 0x0000_0010;
const FEAT_CSUM_V3: u32 = 0x0000_0020;

// Descriptor tag flags.
const TAG_FLAG_SAME_UUID: u16 = 0x02;
const TAG_FLAG_LAST_TAG: u16 = 0x08;

/// One historical filesystem block captured in the journal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JournalBlockRef {
    /// Commit sequence (transaction id) this revision belongs to. Higher =
    /// more recent.
    pub sequence: u32,
    /// Target filesystem block number this journal entry will be written to.
    pub fs_block: u64,
    /// Offset (in bytes, journal-relative) of the data block in the journal
    /// inode's stream — read `block_size` bytes here for the historical
    /// content.
    pub journal_offset: u64,
}

/// Parsed jbd2 journal: the appended list of historical block revisions plus
/// the set of revoked blocks.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Ext4Journal {
    pub block_size: u32,
    /// Number of distinct commit transactions seen.
    pub transactions: u32,
    /// Every captured block revision, in journal order (append-only).
    pub blocks: Vec<JournalBlockRef>,
    /// Filesystem blocks marked revoked (their journalled copies are stale).
    pub revoked: Vec<u64>,
}

fn be32(buf: &[u8], off: usize) -> Option<u32> {
    buf.get(off..off + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

/// jbd2 descriptor tag size for the negotiated feature set. CSUM_V3 uses the
/// 16-byte `journal_block_tag3_t`; otherwise the classic `journal_block_tag_t`
/// is 8 bytes, +4 when 64-bit block numbers are enabled.
fn tag_size(incompat: u32) -> usize {
    if incompat & FEAT_CSUM_V3 != 0 {
        16
    } else if incompat & FEAT_64BIT != 0 {
        12
    } else {
        8
    }
}

/// Decodes one descriptor tag at `buf`, returning `(fs_block, flags, has_uuid)`
/// and the tag's on-disk size. `None` if the slice is too short.
fn parse_tag(buf: &[u8], incompat: u32) -> Option<(u64, u16, bool)> {
    let csum_v3 = incompat & FEAT_CSUM_V3 != 0;
    let is_64bit = incompat & FEAT_64BIT != 0;
    let (blocknr_lo, flags, blocknr_hi) = if csum_v3 {
        // journal_block_tag3_t: blocknr(0), flags(4), blocknr_high(8), checksum(12)
        let lo = be32(buf, 0)?;
        let flags = be32(buf, 4)? as u16;
        let hi = if is_64bit { be32(buf, 8)? } else { 0 };
        (lo, flags, hi)
    } else {
        // journal_block_tag_t: blocknr(0), checksum(4, u16), flags(6, u16),
        // [blocknr_high(8) when 64bit]
        let lo = be32(buf, 0)?;
        let flags = u16::from_be_bytes([*buf.get(6)?, *buf.get(7)?]);
        let hi = if is_64bit { be32(buf, 8)? } else { 0 };
        (lo, flags, hi)
    };
    let fs_block = ((blocknr_hi as u64) << 32) | blocknr_lo as u64;
    let has_uuid = flags & TAG_FLAG_SAME_UUID == 0;
    Some((fs_block, flags, has_uuid))
}

impl Ext4Journal {
    /// Reads and parses the journal inode (`inode_number`, normally 8 / the
    /// superblock's `s_journal_inum`). `max_bytes` caps the journal read.
    pub fn parse(volume: &Ext4Volume, inode_number: u64, max_bytes: usize) -> Result<Self> {
        let inode = volume.read_inode(inode_number)?;
        let raw = volume.read_inode_data(&inode, max_bytes)?;
        Self::parse_bytes(&raw)
    }

    /// Parses an in-memory jbd2 journal image (journal-inode logical bytes).
    pub fn parse_bytes(raw: &[u8]) -> Result<Self> {
        // --- Journal superblock (jbd2 block 0). ---
        if raw.len() < 64 {
            anyhow::bail!("jbd2 journal too small");
        }
        if be32(raw, 0) != Some(JBD2_MAGIC) {
            anyhow::bail!("not a jbd2 journal (bad magic)");
        }
        let blocktype = be32(raw, 4).unwrap_or(0);
        if blocktype != BTYPE_SUPERBLOCK_V1 && blocktype != BTYPE_SUPERBLOCK_V2 {
            anyhow::bail!("jbd2 block 0 is not a superblock (type {blocktype})");
        }
        let block_size = be32(raw, 12).unwrap_or(0);
        let maxlen = be32(raw, 16).unwrap_or(0);
        let first = be32(raw, 20).unwrap_or(1);
        let start = be32(raw, 28).unwrap_or(0);
        let incompat = be32(raw, 0x28).unwrap_or(0);
        if block_size == 0 || (block_size as usize) > raw.len() {
            anyhow::bail!("jbd2 superblock block size invalid: {block_size}");
        }
        let bs = block_size as usize;
        let total_blocks = (raw.len() / bs) as u32;
        let maxlen = if maxlen == 0 || maxlen > total_blocks {
            total_blocks
        } else {
            maxlen
        };

        let mut journal = Ext4Journal {
            block_size,
            transactions: 0,
            blocks: Vec::new(),
            revoked: Vec::new(),
        };

        // A clean journal records s_start = 0 (nothing to replay). The on-disk
        // log still physically contains the last transactions, so scan from
        // s_first regardless — that's exactly the historical data we want.
        let mut block_idx = if start == 0 { first.max(1) } else { start };
        let tagsz = tag_size(incompat);
        let mut scanned = 0u32;
        // Journal blocks already consumed (as control or data blocks). The log
        // is circular, so stop as soon as we'd revisit a block — otherwise a
        // wrap re-reads earlier transactions and double-counts them.
        let mut visited = std::collections::HashSet::<u32>::new();

        // Walk journal blocks. A descriptor block is followed by one data
        // block per tag; a commit block ends a transaction; a revoke block
        // lists stale fs blocks.
        while scanned < maxlen {
            if !visited.insert(block_idx) {
                break;
            }
            let off = block_idx as usize * bs;
            let Some(block) = raw.get(off..off + bs) else {
                break;
            };
            scanned += 1;
            block_idx += 1;
            if block_idx >= maxlen {
                block_idx = first.max(1); // wrap
            }

            if be32(block, 0) != Some(JBD2_MAGIC) {
                continue; // not a journal control block — skip
            }
            let btype = be32(block, 4).unwrap_or(0);
            let sequence = be32(block, 8).unwrap_or(0);

            match btype {
                BTYPE_DESCRIPTOR => {
                    // Tags start after the 12-byte block header.
                    let mut p = 12usize;
                    while let Some(tag) = block.get(p..p + tagsz) {
                        let Some((fs_block, flags, has_uuid)) = parse_tag(tag, incompat) else {
                            break;
                        };
                        // The data block for this tag is the next journal block.
                        if !visited.insert(block_idx) {
                            break;
                        }
                        let data_off = block_idx as usize * bs;
                        journal.blocks.push(JournalBlockRef {
                            sequence,
                            fs_block,
                            journal_offset: data_off as u64,
                        });
                        scanned += 1;
                        block_idx += 1;
                        if block_idx >= maxlen {
                            block_idx = first.max(1);
                        }
                        p += tagsz;
                        // A non-SAME_UUID tag carries a 16-byte UUID inline.
                        if has_uuid {
                            p += 16;
                        }
                        if flags & TAG_FLAG_LAST_TAG != 0 {
                            break;
                        }
                    }
                }
                BTYPE_COMMIT => {
                    journal.transactions += 1;
                }
                BTYPE_REVOKE => {
                    // Revoke block: count(u32 BE @12) total bytes of the block
                    // used; entries are fs block numbers (u32, or u64 with
                    // 64bit feature) starting at +16.
                    let count = be32(block, 12).unwrap_or(0) as usize;
                    let entry = if incompat & FEAT_64BIT != 0 { 8 } else { 4 };
                    let mut p = 16usize;
                    while p + entry <= count.min(bs) {
                        let b = if entry == 8 {
                            let hi = be32(block, p).unwrap_or(0) as u64;
                            let lo = be32(block, p + 4).unwrap_or(0) as u64;
                            (hi << 32) | lo
                        } else {
                            be32(block, p).unwrap_or(0) as u64
                        };
                        journal.revoked.push(b);
                        p += entry;
                    }
                }
                BTYPE_SUPERBLOCK_V1 | BTYPE_SUPERBLOCK_V2 => {
                    // Secondary superblock copy — ignore.
                }
                _ => {}
            }
        }

        Ok(journal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put_be32(buf: &mut [u8], off: usize, v: u32) {
        buf[off..off + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// Builds a tiny jbd2 image: superblock + one descriptor (1 tag) + its data
    /// block + a commit block. Classic 8-byte tags, no 64bit/csum features.
    fn make_journal(bs: usize) -> Vec<u8> {
        let mut raw = vec![0u8; bs * 8];
        // Superblock (block 0).
        put_be32(&mut raw, 0, JBD2_MAGIC);
        put_be32(&mut raw, 4, BTYPE_SUPERBLOCK_V2);
        put_be32(&mut raw, 12, bs as u32); // s_blocksize
        put_be32(&mut raw, 16, 8); // s_maxlen
        put_be32(&mut raw, 20, 1); // s_first
        put_be32(&mut raw, 28, 1); // s_start
                                   // Descriptor block (block 1).
        let d = bs;
        put_be32(&mut raw, d, JBD2_MAGIC);
        put_be32(&mut raw, d + 4, BTYPE_DESCRIPTOR);
        put_be32(&mut raw, d + 8, 42); // sequence
                                       // One tag at +12: blocknr=100, flags = LAST_TAG | SAME_UUID.
        put_be32(&mut raw, d + 12, 100); // t_blocknr
        let flags = (TAG_FLAG_LAST_TAG | TAG_FLAG_SAME_UUID) as u32;
        raw[d + 18..d + 20].copy_from_slice(&(flags as u16).to_be_bytes()); // t_flags @ +6
                                                                            // Data block is block 2 (no magic, arbitrary content).
        raw[2 * bs] = 0xAB;
        // Commit block (block 3).
        let c = 3 * bs;
        put_be32(&mut raw, c, JBD2_MAGIC);
        put_be32(&mut raw, c + 4, BTYPE_COMMIT);
        put_be32(&mut raw, c + 8, 42);
        raw
    }

    #[test]
    fn parses_descriptor_and_commit() {
        let bs = 1024;
        let raw = make_journal(bs);
        let journal = Ext4Journal::parse_bytes(&raw).unwrap();
        assert_eq!(journal.block_size, bs as u32);
        assert_eq!(journal.transactions, 1);
        assert_eq!(journal.blocks.len(), 1);
        assert_eq!(journal.blocks[0].fs_block, 100);
        assert_eq!(journal.blocks[0].sequence, 42);
        // The captured data block is block 2.
        assert_eq!(journal.blocks[0].journal_offset, 2 * bs as u64);
        assert_eq!(raw[journal.blocks[0].journal_offset as usize], 0xAB);
    }

    #[test]
    fn rejects_non_journal() {
        let raw = vec![0u8; 1024];
        assert!(Ext4Journal::parse_bytes(&raw).is_err());
    }
}
