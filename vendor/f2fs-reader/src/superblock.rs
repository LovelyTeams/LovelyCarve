// Adapted from KarpelesLab/fstool 0.5.0 (MIT); see ORIGIN.md.
//! F2FS superblock parsing.
//!
//! Layout reference: [kernel docs](https://docs.kernel.org/filesystems/f2fs.html)
//! and the FAST '15 paper "F2FS: A New File System for Flash Storage".
//! Two physical copies live one F2FS block apart, at byte offsets
//! `1024` and `1024 + 0x1000`. Both copies share the same fields.
//!
//! Only the fields the read driver actually uses are decoded here.

use crate::block::BlockDevice;
use crate::Result;

/// F2FS superblock magic: bytes `0x10 0x20 0xF5 0xF2` on disk
/// (little-endian `u32`).
pub const F2FS_MAGIC: u32 = 0xF2F5_2010;
/// Byte offset of the primary superblock copy.
pub const SB_OFFSET_PRIMARY: u64 = 1024;

/// Byte offset of `cp_payload` inside a `struct f2fs_super_block`.
/// Everything before it is fixed-size: the header through `meta_ino`
/// (0x6C), a 16-byte uuid, `volume_name[512]` UTF-16 code units
/// (0x7C..0x47C), `extension_count` (0x47C) and `extension_list[64][8]`
/// (0x480..0x680).
pub const CP_PAYLOAD_OFFSET: usize = 0x680;

/// Bytes of the superblock we read and decode. Enough to reach
/// [`CP_PAYLOAD_OFFSET`]; the struct itself runs to 0xC00.
pub const SB_DECODE_LEN: usize = CP_PAYLOAD_OFFSET + 4;
/// Byte offset of the backup superblock copy.
pub const SB_OFFSET_BACKUP: u64 = 1024 + 0x1000;

/// Reserved inode numbers — small values are not real on-disk nodes.
/// Conventionally NAT entries `0`, `1`, `2` are the "meta" pseudo-inode,
/// the node-tracking pseudo-inode, and the root directory.
pub const F2FS_ROOT_INO_DEFAULT: u32 = 3;

/// Decoded superblock fields relevant to a read driver.
///
/// All field offsets used by [`Superblock::decode`] follow the publicly
/// documented F2FS on-disk format (kernel docs + FAST '15 paper).
#[derive(Debug, Clone)]
pub struct Superblock {
    pub magic: u32,
    pub major_ver: u16,
    pub minor_ver: u16,
    /// log2 of sector size in bytes (always `9` = 512 B).
    pub log_sectorsize: u32,
    /// log2 of FS block size in bytes (always `12` = 4 KiB).
    pub log_blocksize: u32,
    /// log2 of blocks per segment (typically `9` → 512 blocks → 2 MiB segments).
    pub log_blocks_per_seg: u32,
    /// Segments per section (typically `1`).
    pub segs_per_sec: u32,
    /// Sections per zone (typically `1`).
    pub secs_per_zone: u32,
    /// Total blocks in the volume.
    pub block_count: u64,
    /// Total segments in the volume.
    pub segment_count: u32,
    /// Segment count reserved for each meta region.
    pub segment_count_ckpt: u32,
    pub segment_count_sit: u32,
    pub segment_count_nat: u32,
    pub segment_count_ssa: u32,
    pub segment_count_main: u32,
    /// Start segment / block addresses (block-addressed within the volume).
    pub segment0_blkaddr: u32,
    pub cp_blkaddr: u32,
    pub sit_blkaddr: u32,
    pub nat_blkaddr: u32,
    pub ssa_blkaddr: u32,
    pub main_blkaddr: u32,
    /// Reserved inode numbers.
    pub root_ino: u32,
    pub node_ino: u32,
    pub meta_ino: u32,
    /// Number of `cp_payload` extra blocks following the CP header
    /// (carries the SIT/NAT bitmap overflow when sets are large).
    pub cp_payload: u32,
    /// 16-bit UTF-16LE volume label, NUL-terminated.
    pub volume_name: String,
}

impl Superblock {
    /// Decode at least the first `0x400` bytes of an SB copy. Returns
    /// `None` when the magic doesn't match.
    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < 0x400 {
            return None;
        }
        let r32 = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().ok().unwrap());
        let r16 = |o: usize| u16::from_le_bytes(buf[o..o + 2].try_into().ok().unwrap());
        let r64 = |o: usize| u64::from_le_bytes(buf[o..o + 8].try_into().ok().unwrap());

        let magic = r32(0x00);
        if magic != F2FS_MAGIC {
            return None;
        }
        let major_ver = r16(0x04);
        let minor_ver = r16(0x06);
        // Field offsets per `include/linux/f2fs_fs.h` / kernel docs.
        // Every offset from log_blocksize onward was previously off by
        // +4 bytes; mkfs.f2fs / fsck.f2fs reject those images even
        // though our reader could round-trip them via the same wrong
        // offsets. Aligned to the canonical layout now.
        let log_sectorsize = r32(0x08);
        // 0x0C log_sectors_per_block (unused — we only support 4 KiB blocks)
        let log_blocksize = r32(0x10);
        let log_blocks_per_seg = r32(0x14);

        // Validate the log-shift geometry before any field derived from it
        // is used (block_size = 1 << log_blocksize feeds NAT/SIT geometry
        // and every shift below). An out-of-range shift would overflow or
        // produce nonsensical addresses; F2FS fixes these values, so an
        // image with anything else is malformed. Reject early.
        if log_blocksize != 12 {
            return None;
        }
        if log_sectorsize != 9 {
            return None;
        }
        if !(1..=10).contains(&log_blocks_per_seg) {
            return None;
        }
        let segs_per_sec = r32(0x18);
        let secs_per_zone = r32(0x1C);
        // 0x20 checksum_offset
        let block_count = r64(0x24);
        let _section_count = r32(0x2C);
        let segment_count = r32(0x30);
        let segment_count_ckpt = r32(0x34);
        let segment_count_sit = r32(0x38);
        let segment_count_nat = r32(0x3C);
        let segment_count_ssa = r32(0x40);
        let segment_count_main = r32(0x44);
        let segment0_blkaddr = r32(0x48);
        let cp_blkaddr = r32(0x4C);
        let sit_blkaddr = r32(0x50);
        let nat_blkaddr = r32(0x54);
        let ssa_blkaddr = r32(0x58);
        let main_blkaddr = r32(0x5C);
        let root_ino = r32(0x60);
        let node_ino = r32(0x64);
        let meta_ino = r32(0x68);

        // The 16-byte uuid sits at 0x6C..0x7C; volume_name at 0x7C spans
        // 512 UTF-16LE code units (1024 bytes). We read at most 64 bytes
        // for the human-readable label.
        let name_off = 0x7C;
        let volume_name = if buf.len() >= name_off + 64 {
            utf16_lossy_until_nul(&buf[name_off..name_off + 64])
        } else {
            String::new()
        };

        // `cp_payload` follows the volume name (0x7C, 512 UTF-16 code
        // units → ends 0x47C), `extension_count` (0x47C) and
        // `extension_list[64][8]` (0x480 → ends 0x680), so it sits at
        // 0x680 — not inside the volume name where we used to read it.
        // A caller that handed us only the first 0x400 bytes gets 0,
        // the value every image this crate writes carries anyway.
        let cp_payload = if buf.len() >= CP_PAYLOAD_OFFSET + 4 {
            r32(CP_PAYLOAD_OFFSET)
        } else {
            0
        };

        Some(Self {
            magic,
            major_ver,
            minor_ver,
            log_sectorsize,
            log_blocksize,
            log_blocks_per_seg,
            segs_per_sec,
            secs_per_zone,
            block_count,
            segment_count,
            segment_count_ckpt,
            segment_count_sit,
            segment_count_nat,
            segment_count_ssa,
            segment_count_main,
            segment0_blkaddr,
            cp_blkaddr,
            sit_blkaddr,
            nat_blkaddr,
            ssa_blkaddr,
            main_blkaddr,
            root_ino,
            node_ino,
            meta_ino,
            cp_payload,
            volume_name,
        })
    }

    /// FS block size in bytes (always 4096 on F2FS).
    #[inline]
    pub fn block_size(&self) -> u32 {
        1u32 << self.log_blocksize
    }

    /// Blocks per segment.
    #[inline]
    pub fn blocks_per_seg(&self) -> u32 {
        1u32 << self.log_blocks_per_seg
    }
}

fn utf16_lossy_until_nul(bytes: &[u8]) -> String {
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len() / 2);
    for &chunk in bytes.as_chunks::<2>().0 {
        let unit = u16::from_le_bytes(chunk);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16_lossy(&units)
}

/// Load whichever superblock copy validates. Errors `InvalidImage` if
/// neither passes.
pub fn load(dev: &mut dyn BlockDevice) -> Result<Superblock> {
    if dev.total_size() < SB_OFFSET_BACKUP + 0x400 {
        return Err(crate::invalid(
            "f2fs: device too small to hold a superblock".into(),
        ));
    }
    let total_size = dev.total_size();
    // Read far enough to reach `cp_payload`, but never past the end of
    // the device: the minimum size check above only guarantees the two
    // 0x400-byte heads, and `decode` copes with a short tail.
    let want = SB_DECODE_LEN.min((total_size - SB_OFFSET_BACKUP) as usize);
    let mut buf = vec![0u8; want];
    dev.read_at(SB_OFFSET_PRIMARY, &mut buf)?;
    if let Some(sb) = Superblock::decode(&buf) {
        return validate_geometry(sb, total_size);
    }
    dev.read_at(SB_OFFSET_BACKUP, &mut buf)?;
    if let Some(sb) = Superblock::decode(&buf) {
        return validate_geometry(sb, total_size);
    }
    Err(crate::invalid(
        "f2fs: superblock magic not found in either primary or backup slot".into(),
    ))
}

/// Reject a superblock whose `block_count` is larger than the device could
/// physically hold. `block_count` is otherwise attacker-controlled and feeds
/// directory-walk loop bounds (see `mod.rs::list_inode`); an inflated value
/// would let a tiny image drive a ~1e9-iteration loop.
fn validate_geometry(sb: Superblock, total_size: u64) -> Result<Superblock> {
    let max_blocks = total_size / sb.block_size() as u64;
    if sb.block_count > max_blocks {
        return Err(crate::invalid(format!(
            "f2fs: superblock block_count {} exceeds device capacity {max_blocks} blocks",
            sb.block_count
        )));
    }
    Ok(sb)
}
