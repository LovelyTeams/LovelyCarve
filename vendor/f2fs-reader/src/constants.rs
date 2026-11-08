// Adapted from KarpelesLab/fstool 0.5.0 (MIT); see ORIGIN.md.
//! On-disk constants shared across the F2FS reader modules.
//!
//! Sources: kernel docs (`docs.kernel.org/filesystems/f2fs.html`) and the
//! FAST '15 paper. We deliberately do not import any kernel header.

/// F2FS block size in bytes. Fixed at 4 KiB.
pub const F2FS_BLKSIZE: usize = 4096;

/// Offset (within a 4 KiB CP / NAT / SIT / dnode block) of the trailing
/// CRC32 footer that protects the block. For CP blocks this is what
/// mkfs.f2fs writes into the `checksum_offset` field.
pub const F2FS_BLK_CSUM_OFFSET: usize = F2FS_BLKSIZE - 4;

/// F2FS magic; doubles as the initial CRC seed.
pub const F2FS_SUPER_MAGIC: u32 = 0xF2F5_2010;

/// Compute the F2FS CRC32 over `buf`. F2FS uses Linux's raw `crc32_le`
/// — IEEE 802.3 polynomial 0xEDB88320 but with NO initial XOR and NO
/// final XOR, seeded with `F2FS_SUPER_MAGIC` instead of the usual
/// 0xFFFFFFFF. That is [`crate::crc32_raw`]'s shape; the
/// finalised flavour every general-purpose CRC crate hands out would
/// produce values `fsck.f2fs` / `mkfs.f2fs` reject.
pub fn f2fs_crc32(buf: &[u8]) -> u32 {
    crate::crc32_raw(F2FS_SUPER_MAGIC, buf)
}

/// Inode block — number of direct data-block pointers.
///
/// FAST '15 / kernel docs both quote 923; this is `i_addr` after the
/// 360-byte inode header and the inline xattr reservation. The on-disk
/// `i_inline` bitfield can shrink the array (inline data / inline dentry)
/// but those modes are detected at parse time, not here.
pub const ADDRS_PER_INODE: usize = 923;

/// Number of node-id slots in an inode block (2 direct, 2 indirect, 1
/// triple-indirect).
pub const NIDS_PER_INODE: usize = 5;

/// Indices into `i_nid[5]`.
pub const NID_DIRECT_1: usize = 0;
pub const NID_DIRECT_2: usize = 1;
pub const NID_INDIRECT_1: usize = 2;
pub const NID_INDIRECT_2: usize = 3;
pub const NID_TRIPLE_INDIRECT: usize = 4;

/// Direct / indirect node blocks each hold 1018 u32 entries.
pub const ADDRS_PER_BLOCK: usize = 1018;
pub const NIDS_PER_BLOCK: usize = 1018;

/// NAT entry size on disk: version(1) + ino(4) + block_addr(4) = 9 bytes.
pub const NAT_ENTRY_SIZE: usize = 9;
/// NAT entries per NAT page: floor(4096 / 9) = 455.
pub const NAT_ENTRY_PER_BLOCK: usize = F2FS_BLKSIZE / NAT_ENTRY_SIZE;

/// Summary-block layout (`struct f2fs_summary_block`). 512 seven-byte
/// `f2fs_summary` entries, then the journal, then a five-byte footer.
pub const SUMMARY_SIZE: usize = 7;
pub const ENTRIES_IN_SUM: usize = F2FS_BLKSIZE / 8;
pub const SUM_ENTRY_SIZE: usize = SUMMARY_SIZE * ENTRIES_IN_SUM;
pub const SUM_FOOTER_SIZE: usize = 5;
/// Bytes a `struct f2fs_journal` occupies inside a summary block.
pub const SUM_JOURNAL_SIZE: usize = F2FS_BLKSIZE - SUM_FOOTER_SIZE - SUM_ENTRY_SIZE;
/// `struct nat_journal_entry` = `__le32 nid` + `struct f2fs_nat_entry`.
pub const NAT_JOURNAL_ENTRY_SIZE: usize = 4 + NAT_ENTRY_SIZE;
/// How many NAT entries the journal can hold (`NAT_JOURNAL_ENTRIES`).
/// The leading `__le16 n_nats` eats the first two bytes.
pub const NAT_JOURNAL_ENTRIES: usize = (SUM_JOURNAL_SIZE - 2) / NAT_JOURNAL_ENTRY_SIZE;

/// Lowest / highest `checksum_offset` a CP head may declare
/// (`CP_MIN_CHKSUM_OFFSET` = `offsetof(f2fs_checkpoint,
/// sit_nat_version_bitmap)`, `CP_CHKSUM_OFFSET`).
pub const CP_MIN_CHKSUM_OFFSET: usize = 0xC0;
pub const CP_CHKSUM_OFFSET: usize = F2FS_BLK_CSUM_OFFSET;

/// Dentry block layout — 27-byte bitmap + 3 reserved + 11 × 214 dentries +
/// 8 × 214 filenames = 4096 bytes total.
pub const NR_DENTRY_IN_BLOCK: usize = 214;
pub const SIZE_OF_DIR_ENTRY: usize = 11;
pub const SIZE_OF_DENTRY_BITMAP: usize = 27;
pub const SIZE_OF_RESERVED: usize = 3;
pub const F2FS_SLOT_LEN: usize = 8;

/// Bits in `F2fsInode::i_inline`.
pub const F2FS_INLINE_DATA: u8 = 0x02;
pub const F2FS_INLINE_DENTRY: u8 = 0x04;
pub const F2FS_INLINE_XATTR: u8 = 0x01;
pub const F2FS_DATA_EXIST: u8 = 0x08;

/// Checkpoint flag bits. Values per `include/linux/f2fs_fs.h` in the
/// kernel master tree (every flag here matches the canonical value).
pub const CP_UMOUNT_FLAG: u32 = 0x0000_0001;
pub const CP_ORPHAN_PRESENT_FLAG: u32 = 0x0000_0002;
pub const CP_COMPACT_SUM_FLAG: u32 = 0x0000_0004;
pub const CP_ERROR_FLAG: u32 = 0x0000_0008;
pub const CP_FSCK_FLAG: u32 = 0x0000_0010;
pub const CP_FASTBOOT_FLAG: u32 = 0x0000_0020;
pub const CP_CRC_RECOVERY_FLAG: u32 = 0x0000_0040;
pub const CP_NAT_BITS_FLAG: u32 = 0x0000_0080;
pub const CP_TRIMMED_FLAG: u32 = 0x0000_0100;
pub const CP_NOCRC_RECOVERY_FLAG: u32 = 0x0000_0200;
pub const CP_LARGE_NAT_BITMAP_FLAG: u32 = 0x0000_0400;

/// Reserved/special block addresses. Anything `>= 1` and `< NEW_ADDR`
/// is a real allocation; the two specials below mark "not yet on disk"
/// (data is in the inode's inline area) and "explicit hole".
pub const NULL_ADDR: u32 = 0;
pub const NEW_ADDR: u32 = u32::MAX - 1;

/// Standard POSIX type bits in `i_mode`.
pub const S_IFMT: u16 = 0xF000;
pub const S_IFREG: u16 = 0x8000;
pub const S_IFDIR: u16 = 0x4000;
pub const S_IFLNK: u16 = 0xA000;
pub const S_IFCHR: u16 = 0x2000;
pub const S_IFBLK: u16 = 0x6000;
pub const S_IFIFO: u16 = 0x1000;
pub const S_IFSOCK: u16 = 0xC000;

/// On-disk file types stored in the 1-byte `file_type` field of a
/// `f2fs_dir_entry`. Numbering matches the standard d_type set so it
/// reads naturally.
pub const F2FS_FT_UNKNOWN: u8 = 0;
pub const F2FS_FT_REG_FILE: u8 = 1;
pub const F2FS_FT_DIR: u8 = 2;
pub const F2FS_FT_CHRDEV: u8 = 3;
pub const F2FS_FT_BLKDEV: u8 = 4;
pub const F2FS_FT_FIFO: u8 = 5;
pub const F2FS_FT_SOCK: u8 = 6;
pub const F2FS_FT_SYMLINK: u8 = 7;
