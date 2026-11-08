//! Amiga Rigid Disk Block (RDB) partition tables.
//!
//! The RDB is the Amiga's partition-table format: a `RDSK` block in the
//! first 16 blocks of a disk, chaining to `PART` blocks (one per
//! partition, each carrying a `DosEnvec` describing geometry and mount
//! parameters), `FSHD`/`LSEG` blocks (loadable filesystem drivers), and
//! `BADB` bad-block lists. Layouts follow the AmigaOS NDK's
//! `devices/hardblocks.h` and `dos/filehandler.h`.
//!
//! This crate is pure format logic. I/O comes in through one trait —
//! [`BlockSource`], "read me block N" — so the same code serves an
//! emulator holding an image file, a tool holding a raw device, and a
//! test holding a `Vec<u8>`. [`BlockSink`] is its write-side mirror,
//! kept separate so that "this code only reads" is a fact the type
//! system enforces. [`RdbBuilder`] writes a fresh table through it — the
//! whole layout computed and checked before its first block reaches the
//! disk — and [`RdbEditor`] mutates an existing one in place, preserving
//! every byte it does not model and ordering its writes so that an
//! interrupted commit leaves the old table or the new one, never a
//! splice of the two. What is *inside* a partition is out of
//! scope by design: one filesystem family per crate, composed through
//! an adapter that offsets a partition's LBAs into the parent device.
//!
//! Every LBA in this crate's API — chain pointers, [`Partition`]
//! extents, [`PartitionSource`] addressing — is a *device* block of the
//! source's [`block_size`](BlockSource::block_size), which the RDB's
//! `rdb_BlockBytes` must match. That is deliberately not the same thing
//! as a partition's *filesystem* block size (`de_SizeBlock`), which is
//! per-partition and frequently larger; conflating the two is exactly
//! where silent corruption comes from, so this crate never speaks
//! filesystem blocks.
//!
//! Everything on disk is big-endian; all multi-byte reads go through
//! [`be32`]/[`be16`] rather than any `#[repr(C)]` overlay, so the crate
//! is byte-order- and alignment-safe on any host.
//!
//! # Example
//!
//! The whole read path: a disk, [`Rdb::parse`], the partitions it found,
//! and a [`PartitionSource`] handed to whatever mounts the filesystem.
//! The source here is a `Vec<u8>` so the example is self-contained; a
//! real one is a file (with the `std` feature, [`SeekBlockSource`] wraps
//! any `Read + Seek`) or a raw device. The write side has worked
//! examples of its own, on [`RdbBuilder`] (create a table from nothing)
//! and [`RdbEditor`] (edit one in place).
//!
//! ```
//! use amiga_rdb::{BlockSource, PartitionSource, Rdb};
//!
//! // The one seam: "read me block N". 512-byte blocks here; a 4 KB-sector
//! // disk says 4096 and every LBA below counts in *those* blocks.
//! struct MemDisk(Vec<u8>);
//!
//! impl BlockSource for MemDisk {
//!     type Error = std::io::Error;
//!
//!     fn block_size(&self) -> usize {
//!         512
//!     }
//!
//!     fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
//!         let off = lba as usize * 512;
//!         let block = self.0.get(off..off + 512).ok_or_else(|| {
//!             std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "past the end of the disk")
//!         })?;
//!         buf.copy_from_slice(block);
//!         Ok(())
//!     }
//!
//!     fn block_count(&self) -> Option<u64> {
//!         Some(self.0.len() as u64 / 512)
//!     }
//! }
//!
//! # // A one-partition image, built here so the example runs as a test.
//! # fn put32(d: &mut [u8], block: usize, off: usize, v: u32) {
//! #     let o = block * 512 + off;
//! #     d[o..o + 4].copy_from_slice(&v.to_be_bytes());
//! # }
//! # fn seal(d: &mut [u8], block: usize) {
//! #     let base = block * 512;
//! #     amiga_rdb::seal_checksum(&mut d[base..base + 512], 64).unwrap();
//! # }
//! # fn image() -> Vec<u8> {
//! #     let mut d = vec![0u8; 320 * 512];
//! #     put32(&mut d, 0, 0, 0x5244_534B);            // RDSK
//! #     put32(&mut d, 0, 16, 512);                   // rdb_BlockBytes
//! #     put32(&mut d, 0, 24, 0xFFFF_FFFF);           // rdb_BadBlockList
//! #     put32(&mut d, 0, 28, 1);                     // rdb_PartitionList
//! #     put32(&mut d, 0, 32, 0xFFFF_FFFF);           // rdb_FileSysHeaderList
//! #     put32(&mut d, 0, 132, 15);                   // rdb_RDBBlocksHi
//! #     seal(&mut d, 0);
//! #     put32(&mut d, 1, 0, 0x5041_5254);            // PART
//! #     put32(&mut d, 1, 16, 0xFFFF_FFFF);           // pb_Next
//! #     d[512 + 36] = 3;                             // pb_DriveName, BCPL
//! #     d[512 + 37..512 + 40].copy_from_slice(b"DH0");
//! #     put32(&mut d, 1, 128, 16);                   // de_TableSize
//! #     put32(&mut d, 1, 128 + 4, 128);              // de_SizeBlock
//! #     put32(&mut d, 1, 128 + 12, 1);               // de_Surfaces
//! #     put32(&mut d, 1, 128 + 20, 32);              // de_BlocksPerTrack
//! #     put32(&mut d, 1, 128 + 36, 2);               // de_LowCyl
//! #     put32(&mut d, 1, 128 + 40, 9);               // de_HighCyl
//! #     put32(&mut d, 1, 128 + 64, 0x444F_5303);     // de_DosType
//! #     seal(&mut d, 1);
//! #     d
//! # }
//! #
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! # let mut disk = MemDisk(image());
//! // let mut disk = MemDisk(std::fs::read("disk.hdf")?);
//! let rdb = Rdb::parse(&mut disk)?;
//!
//! // Two owners of the same blocks is a layout that parses perfectly and
//! // destroys itself on the first write, so ask before trusting it.
//! for issue in rdb.validate() {
//!     eprintln!("layout issue: {issue}");
//! }
//!
//! for partition in &rdb.partitions {
//!     println!(
//!         "{} dostype {:08X} blocks {}..{}",
//!         partition.name,
//!         partition.dos_type,
//!         partition.start_lba,
//!         partition.start_lba + partition.block_len,
//!     );
//!
//!     // The composition seam: LBA 0 of this source is the partition's
//!     // first block on the disk. A filesystem crate mounts one of these
//!     // and never sees the partition table.
//!     let mut source = PartitionSource::new(&mut disk, partition);
//!     let mut boot = vec![0u8; source.block_size()];
//!     source.read_block(0, &mut boot)?;
//!     println!("  first block starts {:02X?}", &boot[..4]);
//! }
//! # assert_eq!(rdb.partitions.len(), 1);
//! # Ok(())
//! # }
//! ```

#![cfg_attr(not(feature = "std"), no_std)]
#![deny(missing_docs)]

extern crate alloc;

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::String;
use alloc::vec::Vec;

/// Anything that can produce fixed-size blocks by LBA.
///
/// The one seam between this crate and the world. Implementations are
/// expected to be cheap to call repeatedly with the same LBA; the crate
/// does not cache.
///
/// The block size is a runtime property of the source — a real device
/// knows its sector size, an image container knows (or is told) what it
/// holds. 512 bytes is the classic value, but the format's 32-bit block
/// and cylinder fields cap a 512-byte-block disk at 2 TB; larger
/// `rdb_BlockBytes` is how RDB reaches modern media (4 KB → 16 TB), and
/// such disks are in live use. Supported sizes are powers of two in
/// [`MIN_BLOCK_SIZE`]`..=`[`MAX_BLOCK_SIZE`].
pub trait BlockSource {
    /// How this source reports a failed read. No bound is imposed here —
    /// a test's source may fail with `()` — but a source whose error is
    /// [`Display`](core::fmt::Display) makes [`RdbError`] displayable too.
    type Error;

    /// Bytes per device block. Must be constant for the source's
    /// lifetime and a power of two in
    /// [`MIN_BLOCK_SIZE`]`..=`[`MAX_BLOCK_SIZE`]; [`Rdb::parse`]
    /// rejects anything else rather than misreading geometry.
    fn block_size(&self) -> usize;

    /// Read block `lba` into `buf`, whose length is exactly
    /// [`block_size`](Self::block_size).
    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error>;

    /// Total number of blocks, if known. `None` is legitimate (a raw
    /// character device may not know); only operations that need the
    /// disk's end require it.
    fn block_count(&self) -> Option<u64> {
        None
    }
}

/// Anything that can accept fixed-size blocks by LBA — the write seam.
///
/// Deliberately a *second* trait rather than `write_block` bolted onto
/// [`BlockSource`], because read-only sources are the common case and
/// the majority of this crate's work: a `File` opened for reading, a
/// memory-mapped image, a `&[u8]`, an emulator's read-only medium. A
/// single trait would force every one of them to supply a `write_block`
/// that can only fail at runtime, which is a compile-time truth thrown
/// away. Splitting them means "this code writes" is visible in the
/// bound: anything that reads *and* writes says `S: BlockSource +
/// BlockSink`, and anything that only reads cannot be handed a sink by
/// accident. The cost is [`block_size`](Self::block_size) and
/// [`block_count`](Self::block_count) appearing on both traits — a
/// deliberate duplication, since a type implementing both will have
/// them agree trivially, and making `BlockSink: BlockSource` instead
/// would rule out a write-only target (a fresh image being streamed
/// out) for no gain.
///
/// The block size is a runtime property, on exactly the terms
/// [`BlockSource`] describes: every LBA is a *device* block of
/// [`block_size`](Self::block_size) bytes, and the RDB's
/// `rdb_BlockBytes` must agree with it.
pub trait BlockSink {
    /// How this sink reports a failed write. No bound is imposed here,
    /// as on [`BlockSource::Error`].
    type Error;

    /// Bytes per device block. Must be constant for the sink's
    /// lifetime, a power of two in
    /// [`MIN_BLOCK_SIZE`]`..=`[`MAX_BLOCK_SIZE`], and — for a type that
    /// is also a [`BlockSource`] — equal to what that trait reports.
    fn block_size(&self) -> usize;

    /// Write `buf` to block `lba`. `buf`'s length is exactly
    /// [`block_size`](Self::block_size); a sink may treat a different
    /// length as a caller bug (this crate never passes one).
    ///
    /// Whether the write has reached stable storage when this returns
    /// is the implementation's business — this crate never assumes it,
    /// and a caller that needs durability flushes the underlying object
    /// itself. Ordering, however, *is* this crate's business: the write
    /// paths are ordered so an interruption leaves the previous
    /// structure intact, which only holds if a sink does not reorder
    /// writes behind the caller's back.
    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error>;

    /// Total number of blocks, if known — `None` on the same terms as
    /// [`BlockSource::block_count`]. A sink that knows its size lets a
    /// writer refuse a layout that runs off the end *before* writing
    /// the first block rather than halfway through.
    ///
    /// Unlike [`block_size`](Self::block_size), this carries no
    /// documented equality with [`BlockSource::block_count`] on a type
    /// that is both: a sink's writable capacity and a source's readable
    /// one are allowed to differ (a backing store that can grow to
    /// accept a write past what has been read so far, for instance).
    /// Code bounding a *write* should ask this trait, not
    /// [`BlockSource::block_count`].
    fn block_count(&self) -> Option<u64> {
        None
    }
}

/// Smallest supported device block size (and the classic Amiga value).
pub const MIN_BLOCK_SIZE: usize = 512;

/// Largest supported device block size. 32 KB blocks put the format's
/// 32-bit block addressing at 256 TB, comfortably past current media.
pub const MAX_BLOCK_SIZE: usize = 32 * 1024;

/// How many blocks from the start of the disk the `RDSK` block may
/// legally sit in (`RDB_LOCATION_LIMIT`, NDK `devices/hardblocks.h`).
pub const RDB_LOCATION_LIMIT: u64 = 16;

/// How many blocks one chain (`PART`, `FSHD`, `LSEG`, `BADB`) may hold
/// before the walk gives up with [`RdbError::ChainTooLong`].
///
/// Not a format limit — the format has none beyond the 32-bit block
/// pointers, and a chain of distinct blocks is bounded by the disk. It
/// exists for the one case where nothing else bounds the walk: a
/// [`BlockSource`] that reports no [`block_count`](BlockSource::block_count)
/// cannot have its pointers range-checked, so a hostile image can hand
/// the parser an arbitrarily long acyclic-so-far chain and be answered
/// with an arbitrarily large visited set. With a block count the visited
/// set already ends the walk after at most one hop per block on the
/// disk, and this constant is never reached.
///
/// 2²⁰ blocks is 512 MB of `LSEG` payload at the classic 512-byte
/// block: three orders of magnitude past the largest filesystem driver
/// anyone has shipped in an RDB, past any plausible reserved *area*, and
/// past most whole disks of the era — so no legitimate image is refused
/// — while keeping the visited set a crafted one can force to tens of
/// megabytes rather than gigabytes. A cap high enough to be
/// unreachable-in-practice and low enough to be reachable-in-a-test is
/// the point: an unbounded walk is bounded by the attacker's patience,
/// and any bound at all is better than that.
pub const MAX_CHAIN_BLOCKS: usize = 1 << 20;

/// Block identifiers, as big-endian magic numbers.
pub mod id {
    /// `RDSK` — the rigid disk block itself.
    pub const RDSK: u32 = 0x5244_534B;
    /// `PART` — one partition.
    pub const PART: u32 = 0x5041_5254;
    /// `FSHD` — filesystem header (a loadable filesystem driver).
    pub const FSHD: u32 = 0x4653_4844;
    /// `LSEG` — one chunk of a filesystem driver's load segments.
    pub const LSEG: u32 = 0x4C53_4547;
    /// `BADB` — bad-block list.
    pub const BADB: u32 = 0x4241_4442;
}

/// `rdb_Flags` bits (`RDBFF_*`, NDK `devices/hardblocks.h`).
///
/// The field stays a plain [`u32`] on [`Rdb`] — the format allows any
/// bit pattern and a reader must preserve what it did not understand —
/// but the bits that *are* defined get names here rather than being
/// spelled `1 << 4` at every call site. Most of them are SCSI-bus
/// scanning hints written by the controller's setup tool, meaningful
/// only to the driver that scans the bus; two of them
/// ([`DISK_ID`](rdb_flags::DISK_ID)/[`CTRLR_ID`](rdb_flags::CTRLR_ID)) gate
/// whether the RDSK's identification strings hold anything real, so a
/// consumer printing those must check.
pub mod rdb_flags {
    /// No disks exist after this one on this controller — the scan may
    /// stop here.
    pub const LAST: u32 = 1 << 0;
    /// No LUNs exist after this one on this target — stop probing LUNs.
    pub const LAST_LUN: u32 = 1 << 1;
    /// No target IDs exist after this one — stop probing targets.
    pub const LAST_TID: u32 = 1 << 2;
    /// The drive may not be told to reselect; the driver must keep the
    /// bus for the whole transfer.
    pub const NO_RESELECT: u32 = 1 << 3;
    /// The `rdb_DiskVendor`/`Product`/`Revision` strings are valid.
    /// Without this bit their bytes mean nothing and must not be shown.
    pub const DISK_ID: u32 = 1 << 4;
    /// The `rdb_ControllerVendor`/`Product`/`Revision` strings are
    /// valid, on the same terms.
    pub const CTRLR_ID: u32 = 1 << 5;
    /// The drive supports synchronous SCSI transfers.
    pub const SYNCH: u32 = 1 << 6;
}

/// `fhb_PatchFlags` bits (NDK `devices/hardblocks.h`, and identically
/// `fse_PatchFlags` in `dos/filehandler.h`).
///
/// The bit mask says which of the `FileSysHeaderBlock`'s tail fields are
/// meaningful and should be substituted into the device node when the
/// filesystem is mounted. Bit *n* covers the *n*th longword after
/// `PatchFlags` itself, so the mapping is positional: unset means "this
/// filesystem does not override that field", which is emphatically not
/// the same as "override it with zero" — hence the [`Option`]s on
/// [`FileSysHeader`].
///
/// Note the ordering trap: [`SEG_LIST`](fshd_patch::SEG_LIST) is bit 7
/// and [`GLOBAL_VEC`](fshd_patch::GLOBAL_VEC) bit
/// 8, because `fhb_SegListBlocks` physically precedes `fhb_GlobalVec` in
/// the block. Sources that describe "eight patched fields" and put
/// `GlobalVec` at bit 7 have silently dropped `SegList` from the count.
pub mod fshd_patch {
    /// `fhb_Type` — the device node type.
    pub const TYPE: u32 = 1 << 0;
    /// `fhb_Task` — handler task pointer (0 for a seglist-loaded handler).
    pub const TASK: u32 = 1 << 1;
    /// `fhb_Lock` — a lock to pass to the handler.
    pub const LOCK: u32 = 1 << 2;
    /// `fhb_Handler` — BSTR name of the handler to load.
    pub const HANDLER: u32 = 1 << 3;
    /// `fhb_StackSize` — handler process stack.
    pub const STACK_SIZE: u32 = 1 << 4;
    /// `fhb_Priority` — handler process priority.
    pub const PRIORITY: u32 = 1 << 5;
    /// `fhb_Startup` — startup value passed to the handler.
    pub const STARTUP: u32 = 1 << 6;
    /// `fhb_SegListBlocks` — the `LSEG` chain head. Surfaced
    /// unconditionally as
    /// [`FileSysHeader::seg_list_blocks`](super::FileSysHeader::seg_list_blocks)
    /// because the chain has to be walkable either way; this bit only
    /// records whether the FSHD asked for it to be patched in.
    pub const SEG_LIST: u32 = 1 << 7;
    /// `fhb_GlobalVec` — BCPL global vector (-1 for a non-BCPL handler).
    pub const GLOBAL_VEC: u32 = 1 << 8;
}

/// The chain terminator used by every block-pointer field
/// (`rdb_PartitionList`, `pb_Next`, ...): `0xFFFFFFFF`, i.e. `-1`, not
/// `0` — block 0 is a valid block address on a disk whose RDSK sits
/// later in the first sixteen.
pub const CHAIN_END: u32 = 0xFFFF_FFFF;

/// Is `size` a device block size this crate accepts?
#[inline]
pub fn block_size_ok(size: usize) -> bool {
    size.is_power_of_two() && (MIN_BLOCK_SIZE..=MAX_BLOCK_SIZE).contains(&size)
}

/// Read a big-endian u32 at byte offset `off`.
#[inline]
pub fn be32(block: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([block[off], block[off + 1], block[off + 2], block[off + 3]])
}

/// Read a big-endian u16 at byte offset `off`.
#[inline]
pub fn be16(block: &[u8], off: usize) -> u16 {
    u16::from_be_bytes([block[off], block[off + 1]])
}

/// Write a big-endian u32 at byte offset `off` — the inverse of
/// [`be32`], and the only way this crate puts a longword into a block.
///
/// Public for the same reason [`be32`] is: a consumer assembling or
/// patching a block field this crate does not model needs the format's
/// byte order without reaching for a `#[repr(C)]` overlay that would be
/// wrong on a little-endian host.
#[inline]
pub fn put_be32(block: &mut [u8], off: usize, v: u32) {
    block[off..off + 4].copy_from_slice(&v.to_be_bytes());
}

/// Byte offsets into the five-longword header every RDB-family block
/// begins with — `RDSK`, `PART`, `FSHD`, `LSEG` and `BADB` alike.
///
/// The first three are what [`checksum_ok`] and [`seal_checksum`] share;
/// `HostID` and `Next` follow (the latter named in [`chain`]).
mod hdr {
    pub const ID: usize = 0;
    pub const SUMMED_LONGS: usize = 4;
    pub const CHK_SUM: usize = 8;
    /// Shared by every block type — the `RDSK`'s `rdb_HostID`, the
    /// `PART`'s `pb_HostID`, and so on: one longword, one offset.
    pub const HOST_ID: usize = 12;
}

/// The smallest `SummedLongs` that can produce a passing checksum: the
/// sum must cover `ChkSum` itself, which is the third longword.
///
/// A count below this is not merely unusual, it is unsatisfiable —
/// storing a value at `ChkSum` that the sum does not include cannot
/// change the sum — so [`checksum_ok`] rejects such a block and
/// [`seal_checksum`] refuses to write one.
pub const MIN_SUMMED_LONGS: u32 = (hdr::CHK_SUM / 4) as u32 + 1;

/// Verify an RDB-family block checksum.
///
/// Every RDB-family block carries `SummedLongs` (longword count, at
/// byte 4) and `ChkSum` (at byte 8) such that the first `SummedLongs`
/// big-endian longwords of the block sum to zero with 32-bit wrapping
/// arithmetic. Returns `false` for a `SummedLongs` that doesn't fit the
/// block — a malformed count must fail the check, not panic the host.
///
/// A slice too short to hold the header longwords the check itself
/// reads (`ID`, `SummedLongs`, `ChkSum` — twelve bytes) is `false`
/// rather than a panic: the function is handed whatever a caller has,
/// and "this is not a valid block" is the honest answer for a fragment,
/// not a reason to take the host down.
///
/// A `SummedLongs` below [`MIN_SUMMED_LONGS`] is `false` too, matching
/// what that constant documents and what [`seal_checksum`] refuses to
/// write: a sum that does not cover `ChkSum` cannot be made zero by any
/// value stored there, so a block claiming one is unsatisfiable however
/// its longwords happen to add up. Real blocks are far above the floor —
/// `RDSK`, `PART` and `FSHD` say 64, the shortest `BADB` says 6 — so
/// nothing legitimate is excluded.
pub fn checksum_ok(block: &[u8]) -> bool {
    if block.len() < hdr::CHK_SUM + 4 {
        return false;
    }
    let longs = be32(block, hdr::SUMMED_LONGS) as usize;
    if longs < MIN_SUMMED_LONGS as usize || longs > block.len() / 4 {
        return false;
    }
    let mut sum: u32 = 0;
    for i in 0..longs {
        sum = sum.wrapping_add(be32(block, i * 4));
    }
    sum == 0
}

/// Why [`seal_checksum`] refused to seal a block.
///
/// Both variants describe a `summed_longs` that no block could ever
/// satisfy, so sealing anyway would produce a block [`checksum_ok`]
/// rejects — the one outcome a *writer* must never have. Refusing is
/// deliberately not clamping: clamping would silently seal a different
/// number of longwords than the caller asked for, and since the caller
/// derived that number from the structure it is writing (`SummedLongs`
/// is 64 for `RDSK`/`PART`/`FSHD`, the whole block for `LSEG`, a
/// function of the entry count for `BADB`), a clamp would mean the
/// block's own header disagrees with its layout. Better to fail where
/// the arithmetic was done than to write a self-consistent lie.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealError {
    /// `summed_longs` is below [`MIN_SUMMED_LONGS`], so the sum would
    /// not cover `ChkSum` and no stored value could make it zero.
    SummedLongsTooShort {
        /// The count that was asked for.
        summed_longs: u32,
    },
    /// `summed_longs` longwords do not fit in the block — including the
    /// case of a block too small to hold the header at all.
    SummedLongsTooLong {
        /// The count that was asked for.
        summed_longs: u32,
        /// How many longwords the block actually holds.
        capacity: usize,
    },
}

impl core::fmt::Display for SealError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SealError::SummedLongsTooShort { summed_longs } => write!(
                f,
                "SummedLongs {summed_longs} is below the {MIN_SUMMED_LONGS} needed \
                 to cover ChkSum itself"
            ),
            SealError::SummedLongsTooLong {
                summed_longs,
                capacity,
            } => write!(
                f,
                "SummedLongs {summed_longs} exceeds the {capacity} longwords the block holds"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for SealError {}

/// Seal an RDB-family block: store `summed_longs` and the `ChkSum` that
/// makes the block check out — the exact inverse of [`checksum_ok`].
///
/// Writes `summed_longs` at byte 4, zeroes `ChkSum` at byte 8, sums the
/// first `summed_longs` big-endian longwords with 32-bit wrapping
/// arithmetic, and stores the negation of that sum at `ChkSum`. After
/// this returns `Ok`, `checksum_ok(block)` is `true` — that round trip
/// is the whole contract, and it is what every block writer in this
/// crate depends on.
///
/// `summed_longs` is the caller's, not derived from the block, because
/// the count is part of what the *structure* says about itself and
/// differs per block type: 64 for `RDSK`, `PART` and `FSHD` (which sum
/// their first 256 bytes whatever the device block size), `block_size /
/// 4` for `LSEG` (the whole block is payload), and header-plus-entries
/// for `BADB`. There is nothing in a half-built block to infer it from.
///
/// Fails rather than panics or clamps on a count that cannot work — see
/// [`SealError`]. A block shorter than three longwords is
/// [`SealError::SummedLongsTooLong`] by the same check, so no length is
/// a panic.
pub fn seal_checksum(block: &mut [u8], summed_longs: u32) -> Result<(), SealError> {
    if summed_longs < MIN_SUMMED_LONGS {
        return Err(SealError::SummedLongsTooShort { summed_longs });
    }
    let capacity = block.len() / 4;
    let longs = summed_longs as usize;
    if longs > capacity {
        return Err(SealError::SummedLongsTooLong {
            summed_longs,
            capacity,
        });
    }

    put_be32(block, hdr::SUMMED_LONGS, summed_longs);
    put_be32(block, hdr::CHK_SUM, 0);
    let mut sum: u32 = 0;
    for i in 0..longs {
        sum = sum.wrapping_add(be32(block, i * 4));
    }
    put_be32(block, hdr::CHK_SUM, sum.wrapping_neg());
    Ok(())
}

/// A drive geometry: the cylinders/heads/sectors triple an `RDSK` block
/// stores in `rdb_Cylinders`/`rdb_Heads`/`rdb_Sectors`, and which every
/// partition's `de_LowCyl`/`de_HighCyl` is expressed in.
///
/// Geometry is a fiction on any drive made since the 1990s — the disk
/// reports LBAs and invents whatever CHS the host asks for — but the RDB
/// format has no other way to say where a partition starts, so *some*
/// triple has to be chosen and every tool has to choose the same way for
/// its images to be interchangeable. [`synthesize_geometry`] is where
/// that choice is made and documented.
///
/// The block size rides along because a triple means nothing without it:
/// the same cylinders/heads/sectors describe eight times the disk at
/// 4096-byte blocks that they do at 512.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// `rdb_Cylinders`.
    pub cylinders: u32,
    /// `rdb_Heads` — surfaces per cylinder.
    pub heads: u32,
    /// `rdb_Sectors` — device blocks per track.
    pub sectors: u32,
    /// Bytes per device block, matching the source or sink the geometry
    /// is for and what `rdb_BlockBytes` will say.
    pub block_size: usize,
}

impl Geometry {
    /// Device blocks per cylinder — `heads * sectors`, which is what
    /// `rdb_CylBlocks` and a partition's `de_Surfaces *
    /// de_BlocksPerTrack` both hold.
    ///
    /// Saturating, because the fields are public and a hand-built
    /// `Geometry` may hold anything; one from
    /// [`synthesize_geometry`] never comes close.
    pub fn cylinder_blocks(&self) -> u64 {
        (self.heads as u64).saturating_mul(self.sectors as u64)
    }

    /// Device blocks the whole geometry describes, saturating on the
    /// same terms.
    pub fn total_blocks(&self) -> u64 {
        (self.cylinders as u64).saturating_mul(self.cylinder_blocks())
    }

    /// Bytes the whole geometry describes, saturating on the same terms.
    /// For a synthesized geometry this is at most the size asked for,
    /// and short of it by less than one cylinder.
    pub fn total_bytes(&self) -> u64 {
        self.total_blocks().saturating_mul(self.block_size as u64)
    }
}

/// Why [`synthesize_geometry`] could not produce a geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryError {
    /// `block_size` is not a power of two in
    /// [`MIN_BLOCK_SIZE`]`..=`[`MAX_BLOCK_SIZE`].
    UnsupportedBlockSize {
        /// The block size asked for.
        block_size: usize,
    },
    /// The disk is smaller than one cylinder of the smallest geometry
    /// the convention will produce, so there is nothing to describe.
    TooSmall {
        /// The size asked for.
        total_bytes: u64,
        /// The block size asked for.
        block_size: usize,
        /// The smallest size that does yield a geometry, in bytes.
        minimum_bytes: u64,
    },
    /// The disk is so large that no candidate geometry's fields fit the
    /// format's 32-bit `rdb_Cylinders`/`rdb_Heads`/`rdb_CylBlocks`.
    ///
    /// An error rather than a wrap on purpose: a wrapped cylinder count
    /// describes a disk that is not there, and every partition placed
    /// against it would point somewhere real and wrong. (The threshold
    /// is far past any medium — petabytes at 512-byte blocks — but it is
    /// reachable from a `u64` byte count, so it is answered rather than
    /// assumed away.)
    TooLarge {
        /// The size asked for.
        total_bytes: u64,
        /// The block size asked for.
        block_size: usize,
    },
}

impl core::fmt::Display for GeometryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GeometryError::UnsupportedBlockSize { block_size } => write!(
                f,
                "unsupported device block size {block_size}: \
                 must be a power of two in {MIN_BLOCK_SIZE}..={MAX_BLOCK_SIZE}"
            ),
            GeometryError::TooSmall {
                total_bytes,
                block_size,
                minimum_bytes,
            } => write!(
                f,
                "{total_bytes} bytes is too small for a geometry in {block_size}-byte blocks: \
                 at least {minimum_bytes} bytes are needed for one cylinder"
            ),
            GeometryError::TooLarge {
                total_bytes,
                block_size,
            } => write!(
                f,
                "{total_bytes} bytes in {block_size}-byte blocks exceeds what the RDB's \
                 32-bit geometry fields can describe"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for GeometryError {}

/// Constants of the geometry convention this crate follows — amitools'
/// `rdbtool`, whose images are this crate's fixtures and its
/// differential oracle.
///
/// Two candidate geometries are generated and the one wasting fewer
/// bytes wins; see [`synthesize_geometry`] for the whole story.
mod geo {
    /// The "PC-ish" candidate's fixed sector count: 63, the largest a
    /// PC BIOS's six-bit sector field could hold, and so the number
    /// every PC-derived geometry has used since.
    pub const PC_SECTORS: u64 = 63;

    /// `(inclusive upper bound in KiB, heads)`, in order. A size at
    /// exactly a bound takes that row's head count; anything past the
    /// last row takes [`PC_HEADS_ABOVE`].
    ///
    /// The bounds are the classic BIOS translation breakpoints — 504 MB,
    /// then doubling — expressed in KiB because that is the unit the
    /// comparison is made in, and the comparison is against the
    /// *requested byte size*, not the block count, so it does not move
    /// with the block size.
    pub const PC_HEADS: [(u64, u64); 4] = [
        (504 * 1024, 16),
        (1008 * 1024, 32),
        (2016 * 1024, 64),
        (4032 * 1024, 128),
    ];

    /// Heads for anything past the last [`PC_HEADS`] row.
    pub const PC_HEADS_ABOVE: u64 = 256;

    /// The "Amiga-ish" candidate's fixed sector count: 32 blocks per
    /// track, the value AmigaOS partitioning tools have always used.
    pub const AMIGA_SECTORS: u64 = 32;

    /// The cylinder ceiling the Amiga-ish candidate halves down to.
    /// 65535, not 65536: an artefact of the 16-bit cylinder counts real
    /// controllers had, kept because deviating from it would put this
    /// crate's images a cylinder away from every other tool's.
    pub const MAX_CYLINDERS: u64 = 65535;
}

/// Choose a cylinders/heads/sectors geometry for a disk of
/// `total_bytes` in `block_size`-byte device blocks.
///
/// # Which convention, and why
///
/// **amitools' `rdbtool`, pinned against version 0.8.1.** Tools differ
/// here and there is no right answer — geometry is invented on any
/// modern medium — so the tie-breaker is interoperability: amitools'
/// images are this crate's test fixtures and `xdftool` is the
/// differential oracle milestone 2 diffs against, so producing a
/// *different* geometry for the same size would make every such
/// comparison a false positive. The behaviour below was pinned
/// empirically, by creating images at a spread of sizes and reading back
/// what `rdbtool` chose; the observed triples are unit tests.
///
/// # The convention
///
/// Two candidates are generated and the one wasting fewer bytes wins,
/// with the first winning an exact tie:
///
/// 1. **PC-ish**: 63 sectors, heads from a table of the classic BIOS
///    translation breakpoints applied to the *requested byte size*
///    (≤ 504 MiB → 16 heads, then 32, 64, 128 at each doubling, 256
///    above 4032 MiB).
/// 2. **Amiga-ish**: 32 sectors, 1 head — then, while the cylinder count
///    exceeds 65535, halve the cylinders and double the heads.
///
/// Both compute `cylinders = (total_bytes / block_size) / (heads *
/// sectors)`, **rounding the cylinder count down**. A geometry therefore
/// describes at most the size asked for and never more: the last partial
/// cylinder of a disk whose size is not a whole number of them is simply
/// unaddressable, which is what every real tool does and the only safe
/// direction to round — rounding up would place a partition's last
/// cylinder past the end of the medium.
///
/// In practice candidate 2 wins almost everywhere, its cylinders being
/// 16 KiB apart at 512-byte blocks against candidate 1's ~504 KiB. The
/// exceptions are sizes that are an exact multiple of candidate 1's
/// cylinder size, where both waste nothing and the tie hands it to
/// candidate 1 — which is why `rdbtool` emits a 63-sector geometry for
/// (say) exactly 51 609 600 bytes and a 32-sector one for 10 MiB.
///
/// # Deviations, deliberately
///
/// A candidate whose fields would not fit the format's 32-bit
/// `rdb_Cylinders`/`rdb_Heads`/`rdb_CylBlocks` is discarded rather than
/// truncated, and [`GeometryError::TooLarge`] is returned if that leaves
/// none. `rdbtool` is written in Python, whose integers do not wrap, and
/// so has no answer here at all; a wrapped geometry describing a disk
/// that is not there is the one outcome this crate will not produce. The
/// threshold is petabytes away from any real medium, so this never
/// changes the answer for a size anyone will ask for.
///
/// # Block size
///
/// The head and sector choices do *not* depend on `block_size` — the
/// candidate-1 table is byte-based and candidate 2's start values are
/// fixed — so only the cylinder count scales with it, which is what was
/// observed. `block_size` still matters to the *result*, because it
/// decides where the 65535-cylinder halving kicks in.
pub fn synthesize_geometry(total_bytes: u64, block_size: usize) -> Result<Geometry, GeometryError> {
    if !block_size_ok(block_size) {
        return Err(GeometryError::UnsupportedBlockSize { block_size });
    }
    let bs = block_size as u64;
    let total_blocks = total_bytes / bs;

    // Candidate 2 with one head is the smallest geometry the convention
    // can produce, so a disk short of one of its cylinders has no
    // geometry at all — and candidate 1's cylinder is 31.5 times larger,
    // so there is no size where it rescues one that fails here.
    let minimum_bytes = geo::AMIGA_SECTORS * bs;
    if total_blocks < geo::AMIGA_SECTORS {
        return Err(GeometryError::TooSmall {
            total_bytes,
            block_size,
            minimum_bytes,
        });
    }

    // Candidate 1 first, because the waste comparison below keeps the
    // incumbent on a tie and an exact tie must go to candidate 1.
    let candidates = [
        pc_geometry(total_bytes, total_blocks, block_size),
        amiga_geometry(total_blocks, block_size),
    ];
    let mut best: Option<(Geometry, u64)> = None;
    for g in candidates.into_iter().flatten() {
        let waste = total_bytes - g.total_bytes();
        let better = match best {
            Some((_, incumbent)) => waste < incumbent,
            None => true,
        };
        if better {
            best = Some((g, waste));
        }
    }

    match best {
        Some((g, _)) => Ok(g),
        None => Err(GeometryError::TooLarge {
            total_bytes,
            block_size,
        }),
    }
}

/// Candidate 1: 63 sectors, heads from the BIOS-breakpoint table.
///
/// The table is consulted with the *requested byte size*, not the block
/// count — so a 4 KB-block disk of a given size picks the same head
/// count as a 512-byte-block one of that size.
fn pc_geometry(total_bytes: u64, total_blocks: u64, block_size: usize) -> Option<Geometry> {
    let kib = total_bytes / 1024;
    let mut heads = geo::PC_HEADS_ABOVE;
    for &(limit_kib, h) in geo::PC_HEADS.iter() {
        if kib <= limit_kib {
            heads = h;
            break;
        }
    }
    let cylinders = total_blocks / (heads * geo::PC_SECTORS);
    representable(cylinders, heads, geo::PC_SECTORS, block_size)
}

/// Candidate 2: 32 sectors, one head, halving cylinders and doubling
/// heads until the cylinder count fits the 65535 ceiling.
///
/// The halving is applied to the *already floored* cylinder count, so an
/// odd count loses a whole cylinder rather than half of one — matching
/// what was observed, and the reason a 65537-cylinder disk ends up 16 KiB
/// short where a 65536-cylinder one is exact.
fn amiga_geometry(total_blocks: u64, block_size: usize) -> Option<Geometry> {
    let mut heads: u64 = 1;
    let mut cylinders = total_blocks / geo::AMIGA_SECTORS;
    while cylinders > geo::MAX_CYLINDERS {
        cylinders /= 2;
        // Terminates: cylinders strictly decreases while above 65535,
        // and `heads` is only checked against the u32 ceiling at the end
        // because doubling it in a u64 cannot overflow first (the loop
        // runs at most 64 times).
        heads = heads.saturating_mul(2);
    }
    representable(cylinders, heads, geo::AMIGA_SECTORS, block_size)
}

/// A candidate geometry, or `None` if it describes nothing or does not
/// fit the format's 32-bit fields.
///
/// `rdb_CylBlocks` is a single longword as much as `rdb_Heads` is, so
/// `heads * sectors` is checked too and not just the factors.
fn representable(cylinders: u64, heads: u64, sectors: u64, block_size: usize) -> Option<Geometry> {
    let max = u32::MAX as u64;
    if cylinders == 0 || cylinders > max || heads > max || sectors > max {
        return None;
    }
    heads.checked_mul(sectors).filter(|&cb| cb <= max)?;
    Some(Geometry {
        cylinders: cylinders as u32,
        heads: heads as u32,
        sectors: sectors as u32,
        block_size,
    })
}

/// Errors from parsing an RDB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RdbError<E> {
    /// The underlying [`BlockSource`] failed.
    Io(E),
    /// The source's [`block_size`](BlockSource::block_size) is not a
    /// power of two in [`MIN_BLOCK_SIZE`]`..=`[`MAX_BLOCK_SIZE`].
    UnsupportedBlockSize {
        /// What the source said its block size was.
        block_size: usize,
    },
    /// No valid `RDSK` block in the first [`RDB_LOCATION_LIMIT`] blocks.
    ///
    /// Not necessarily damage: RDB-less images (a bare filesystem from
    /// block 0) are a real, if less common, layout — this is how a
    /// caller detects one.
    ///
    /// Also the answer when the scan cannot read that far: a source
    /// that declines to report a [`block_count`](BlockSource::block_count)
    /// signals its end of medium by failing a read, and a medium too
    /// short to hold an RDB has no RDB on it. A device failing for some
    /// other reason therefore reports this too — the probe is a search,
    /// not a health check — but only during the search: a read that
    /// fails once an `RDSK` has been found is [`Io`](Self::Io).
    NoRdsk,
    /// A block in a chain had the wrong ID. `expected`/`found` are the
    /// magic numbers; `lba` is where.
    WrongId {
        /// The block the chain pointed at.
        lba: u64,
        /// The magic number the chain's type requires (see [`id`]).
        expected: u32,
        /// The magic number actually found there.
        found: u32,
    },
    /// A block's checksum failed. The chain is reported broken rather
    /// than the block trusted: a bad checksum on this platform usually
    /// means a bug wrote it, and silently accepting it is how images
    /// get corrupted further.
    BadChecksum {
        /// The block whose `ChkSum` did not add up.
        lba: u64,
    },
    /// A chain pointer walked past the end of the disk (only detectable
    /// when [`BlockSource::block_count`] is `Some`).
    ChainOutOfRange {
        /// The off-disk block the chain pointed at.
        lba: u64,
    },
    /// A chain revisited a block — a cycle. Without this check a
    /// crafted or corrupted image loops the parser forever.
    ChainCycle {
        /// The block the chain came back to.
        lba: u64,
    },
    /// A chain ran past [`MAX_CHAIN_BLOCKS`] blocks without repeating
    /// one. Only reachable from a [`BlockSource`] that reports no
    /// [`block_count`](BlockSource::block_count): with a count, an
    /// acyclic chain cannot outlast the disk. See [`MAX_CHAIN_BLOCKS`]
    /// for why the limit is where it is.
    ChainTooLong {
        /// The limit that was exceeded, i.e. [`MAX_CHAIN_BLOCKS`].
        limit: usize,
    },
    /// Two `FSHD` blocks' `LSEG` chains share a block, so one driver's
    /// payload is spliced into another's.
    ///
    /// Damage, not a layout the format supports: each `FSHD` owns its
    /// chain, and an editor that rewrote one would silently rewrite the
    /// other. Reported by [`RdbEditor::open`], which has to hold every
    /// block of every chain and cannot faithfully edit aliased ones.
    /// [`Rdb::parse`] does not raise it — it walks one chain at a time
    /// and reads each faithfully — and
    /// [`Rdb::validate_seg_lists`](Rdb::validate_seg_lists) *reports* it
    /// as [`ValidationIssue::SharedLsegChain`] instead, validation being
    /// a report rather than a refusal.
    SharedChain {
        /// The first block found on two chains.
        lba: u64,
    },
    /// `rdb_BlockBytes` disagrees with the source's
    /// [`block_size`](BlockSource::block_size). Every LBA in the RDB is
    /// in `rdb_BlockBytes` units; reading them through a differently
    /// sized source would silently address the wrong bytes, so the
    /// mismatch is an error, not a guess. (An image of a 4 KB-sector
    /// disk must be presented by a source that says 4096.)
    BlockBytesMismatch {
        /// `rdb_BlockBytes`, as the `RDSK` block declares it.
        block_bytes: u32,
        /// What the source says it reads in.
        block_size: usize,
    },
}

impl<E: core::fmt::Display> core::fmt::Display for RdbError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RdbError::Io(e) => write!(f, "reading a block failed: {e}"),
            RdbError::UnsupportedBlockSize { block_size } => write!(
                f,
                "unsupported device block size {block_size}: \
                 must be a power of two in {MIN_BLOCK_SIZE}..={MAX_BLOCK_SIZE}"
            ),
            RdbError::NoRdsk => write!(
                f,
                "no valid RDSK block in the first {RDB_LOCATION_LIMIT} blocks"
            ),
            RdbError::WrongId {
                lba,
                expected,
                found,
            } => write!(
                f,
                "block {lba} has ID {} where {} was expected",
                Fourcc(*found),
                Fourcc(*expected)
            ),
            RdbError::BadChecksum { lba } => {
                write!(f, "block {lba} has a bad checksum")
            }
            RdbError::ChainOutOfRange { lba } => {
                write!(
                    f,
                    "a chain pointed at block {lba}, past the end of the disk"
                )
            }
            RdbError::ChainCycle { lba } => {
                write!(f, "a chain loops back to block {lba}")
            }
            RdbError::ChainTooLong { limit } => {
                write!(f, "a chain is longer than the {limit}-block limit")
            }
            RdbError::SharedChain { lba } => write!(
                f,
                "two filesystems' LSEG chains share block {lba}, so neither can be edited"
            ),
            RdbError::BlockBytesMismatch {
                block_bytes,
                block_size,
            } => write!(
                f,
                "the RDB declares {block_bytes}-byte blocks but the source reads \
                 {block_size}-byte blocks"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl<E: std::error::Error + 'static> std::error::Error for RdbError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RdbError::Io(e) => Some(e),
            _ => None,
        }
    }
}

/// A block ID rendered the way the format writes it — four ASCII
/// characters, e.g. `PART` — falling back to hex for the corrupt case
/// that produced the error in the first place.
struct Fourcc(u32);

impl core::fmt::Display for Fourcc {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let b = self.0.to_be_bytes();
        if b.iter().all(|c| (0x20..0x7F).contains(c)) {
            for c in b {
                write!(f, "{}", c as char)?;
            }
            Ok(())
        } else {
            write!(f, "{:#010x}", self.0)
        }
    }
}

/// One partition, as read from a `PART` block.
///
/// All block quantities (`start_lba`, `block_len`, `cylinder_blocks`)
/// are *device* blocks of the parent source's size — never
/// `de_SizeBlock` filesystem blocks, which may be larger and vary per
/// partition on one disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    /// LBA of the `PART` block this came from.
    pub part_block: u64,
    /// The drive name (`pb_DriveName`, BCPL string) — e.g. `DH0`.
    pub name: String,
    /// `pb_Flags` bit 0: bootable.
    pub bootable: bool,
    /// `pb_Flags` bit 1: present but not to be mounted automatically.
    pub no_automount: bool,
    /// First block of the partition, in disk device-block LBAs.
    /// (`de_LowCyl * cylinder_blocks`, saturating: all three factors are
    /// attacker-controlled u32s and their product need not fit a u64.)
    pub start_lba: u64,
    /// Number of device blocks in the partition:
    /// `(high_cyl - low_cyl + 1) * cylinder_blocks`, saturating.
    ///
    /// **Zero when `high_cyl < low_cyl`** — an inverted cylinder range
    /// describes no blocks, so that is its length, and
    /// [`ValidationIssue::PartitionCylindersInverted`] is how a consumer
    /// hears about it. The raw [`low_cyl`](Self::low_cyl) and
    /// [`high_cyl`](Self::high_cyl) are still exactly what was on disk.
    pub block_len: u64,
    /// `de_DosType` — e.g. `0x444F5303` (`DOS\x03`).
    pub dos_type: u32,
    /// `de_BootPri`.
    pub boot_pri: i32,
    /// `de_MaxTransfer`.
    pub max_transfer: u32,
    /// `de_Mask`.
    pub mask: u32,
    /// Device blocks per cylinder (`de_Surfaces * de_BlocksPerTrack`),
    /// kept because filesystems and repartitioners both need it.
    pub cylinder_blocks: u64,
    /// `de_LowCyl` — the partition's first cylinder.
    pub low_cyl: u32,
    /// `de_HighCyl` — its last cylinder, *inclusive*.
    pub high_cyl: u32,
    /// `de_NumBuffers` — a mount parameter the handler needs, saying
    /// nothing about the disk itself.
    pub num_buffers: u32,
    /// `de_BufMemType` — which memory those buffers want, likewise.
    pub buf_mem_type: u32,
    /// `de_SizeBlock` in longwords (128 == 512-byte filesystem blocks).
    /// Per partition: one disk can carry differently sized filesystem
    /// blocks side by side.
    pub size_block_longs: u32,
    /// `de_Baud` (envec longword 17) — serial rate for a
    /// serial-attached handler. `None` when `de_TableSize` stops short
    /// of it: absent and zero are different things, and a writer that
    /// rounds-trips must not invent a field the original did not have.
    pub baud: Option<u32>,
    /// `de_Control` (18) — handler-defined control word.
    pub control: Option<u32>,
    /// `de_BootBlocks` (19) — number of blocks reserved for boot code.
    pub boot_blocks: Option<u32>,
    /// The `DosEnvec` as raw longwords, `de_TableSize` included, so the
    /// vector holds `table_size + 1` entries — everything the envec
    /// claims, whether or not this crate models it. A consumer can
    /// round-trip an envec byte-for-byte from this.
    ///
    /// Clamped to what actually fits between the envec's offset (128)
    /// and the end of the block: `de_TableSize` is attacker-controlled
    /// and a hostile value must truncate, never read past the block.
    /// So `envec_raw.len()` is `min(table_size + 1, (block_size - 128) /
    /// 4)` — compare it against `table_size` to detect the truncation.
    pub envec_raw: Vec<u32>,
}

/// One loadable filesystem driver, as read from a `FileSysHeaderBlock`.
///
/// An RDB may carry the filesystem handlers its partitions need, so a
/// ROM that has never heard of (say) `DOS\x07` can still mount it: the
/// boot code finds the FSHD whose `dos_type` matches the partition's,
/// reassembles the driver binary from the `LSEG` chain
/// ([`Rdb::load_filesystem`]), and patches the fields this struct gates
/// into the device node.
///
/// The eight [`Option`] fields are gated by [`patch_flags`](Self::patch_flags)
/// — see [`fshd_patch`]. `None` means the FSHD does not override that
/// device-node field at all; it does *not* mean zero, and a writer that
/// round-trips this must not turn one into the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSysHeader {
    /// LBA of the `FSHD` block this came from.
    pub fshd_block: u64,
    /// `fhb_HostID` — SCSI ID of the owning controller, as on the RDSK.
    pub host_id: u32,
    /// `fhb_Flags`. No bits are defined by the NDK; carried so a
    /// rewrite does not drop whatever a tool put there.
    pub flags: u32,
    /// `fhb_DosType` — which partitions this driver serves, matched
    /// against a partition's [`Partition::dos_type`].
    pub dos_type: u32,
    /// `fhb_Version`, raw: major in the high 16 bits, minor in the low.
    /// Kept as the packed longword because that is what the format
    /// stores and what a version comparison actually wants (a plain
    /// `>` on the whole word orders releases correctly); split it with
    /// [`version_major`](Self::version_major) /
    /// [`version_minor`](Self::version_minor).
    pub version: u32,
    /// `fhb_PatchFlags` — which of the fields below are significant.
    /// Preserved raw, bits this crate does not model included.
    pub patch_flags: u32,
    /// `fhb_Type`, gated by [`fshd_patch::TYPE`].
    pub node_type: Option<u32>,
    /// `fhb_Task`, gated by [`fshd_patch::TASK`].
    pub task: Option<u32>,
    /// `fhb_Lock`, gated by [`fshd_patch::LOCK`].
    pub lock: Option<u32>,
    /// `fhb_Handler`, gated by [`fshd_patch::HANDLER`].
    pub handler: Option<u32>,
    /// `fhb_StackSize`, gated by [`fshd_patch::STACK_SIZE`].
    pub stack_size: Option<u32>,
    /// `fhb_Priority` (signed), gated by [`fshd_patch::PRIORITY`].
    pub priority: Option<i32>,
    /// `fhb_Startup` (signed), gated by [`fshd_patch::STARTUP`].
    pub startup: Option<i32>,
    /// `fhb_GlobalVec` (signed; -1 means "not BCPL"), gated by
    /// [`fshd_patch::GLOBAL_VEC`].
    pub global_vec: Option<i32>,
    /// `fhb_SegListBlocks` — head of this driver's `LSEG` chain, or
    /// [`CHAIN_END`] when the FSHD carries no binary (a header that
    /// only patches device-node fields for a filesystem already in ROM).
    ///
    /// Not an `Option` despite [`fshd_patch::SEG_LIST`] existing: the
    /// chain must be walkable to load the driver regardless of whether
    /// the FSHD asked for the pointer to be patched into the node, and
    /// `CHAIN_END` already expresses "no chain" unambiguously. The bit
    /// itself survives in [`patch_flags`](Self::patch_flags).
    pub seg_list_blocks: u32,
}

impl FileSysHeader {
    /// Major version — `fhb_Version >> 16`.
    pub fn version_major(&self) -> u16 {
        (self.version >> 16) as u16
    }

    /// Minor version — the low 16 bits of `fhb_Version`.
    pub fn version_minor(&self) -> u16 {
        self.version as u16
    }
}

/// One bad-block remapping from a `BadBlockBlock`: the drive block that
/// went bad, and the spare that stands in for it. Both are device-block
/// LBAs on the parent disk.
///
/// Effectively extinct — drives have remapped their own defects
/// internally since well before the format stopped being used — but the
/// entries exist on old images and a repartitioner that silently dropped
/// them would hand the filesystem blocks that do not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadBlockEntry {
    /// The failed block.
    pub bad: u32,
    /// The block substituted for it.
    pub good: u32,
}

/// A parsed RDB: the disk-level header, its partitions, its loadable
/// filesystems and its bad-block list.
///
/// The `FSHD` and `BADB` chains are parsed eagerly during
/// [`Rdb::parse`] — they are a handful of blocks and the alternative
/// would be methods taking `&mut S`, which would tie the returned value
/// to the source's lifetime. `LSEG` payloads are the exception and stay
/// lazy behind [`Rdb::load_filesystem`]: a driver binary runs to
/// hundreds of kilobytes and most consumers only want the partition
/// table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rdb {
    /// LBA the `RDSK` block was found at (0..16).
    pub rdsk_block: u64,
    /// `rdb_BlockBytes` — bytes per device block. Always equal to the
    /// source's [`block_size`](BlockSource::block_size) after a
    /// successful parse; carried so consumers can do byte math without
    /// the source in hand.
    pub block_bytes: u32,
    /// `rdb_Flags`. Left as a bare `u32` because the format permits
    /// bits this crate does not know and a reader must preserve them;
    /// the defined bits have names in [`rdb_flags`].
    pub flags: u32,
    /// `rdb_HostID` — the SCSI ID of the controller that owns this
    /// disk. Meaningless on non-SCSI media, preserved regardless.
    pub host_id: u32,
    /// `rdb_DriveInit` — an optional seglist pointer for drive-specific
    /// init code. On disk it is a block address; this crate does not
    /// follow it (nothing in the wild uses it), it just carries it so a
    /// rewrite does not drop it.
    pub drive_init: u32,
    /// `rdb_Cylinders` — cylinders on the drive, as the RDB declares it.
    pub cylinders: u32,
    /// `rdb_Heads` — surfaces per cylinder.
    pub heads: u32,
    /// `rdb_Sectors` — blocks per track.
    pub sectors: u32,
    /// `rdb_Interleave` — physical sector interleave. Historical: a
    /// value tuned to a controller too slow to read consecutive
    /// sectors. Kept because it is part of what the drive was formatted
    /// with, not because anything modern honours it.
    pub interleave: u32,
    /// `rdb_Park` — the cylinder to park the heads on. Dead on any
    /// drive made since parking became automatic.
    pub park: u32,
    /// `rdb_WritePreComp` — first cylinder needing write precompensation.
    pub write_pre_comp: u32,
    /// `rdb_ReducedWrite` — first cylinder needing reduced write current.
    pub reduced_write: u32,
    /// `rdb_StepRate` — head step rate in the drive's own units.
    pub step_rate: u32,
    /// `rdb_RDBBlocksLo` — first block of the area the RDB structures
    /// themselves occupy: the area a repartitioner may rewrite and a
    /// filesystem must never touch.
    pub rdb_blocks_lo: u32,
    /// `rdb_RDBBlocksHi` — the last block of that area, *inclusive*.
    pub rdb_blocks_hi: u32,
    /// `rdb_LoCylinder` — first cylinder available to partitions.
    /// Distinct from `cylinders`: the RDB area itself normally sits
    /// below this, so this is the range a partitioner may hand out.
    pub lo_cylinder: u32,
    /// `rdb_HiCylinder` — the last such cylinder, *inclusive*.
    pub hi_cylinder: u32,
    /// `rdb_CylBlocks` — device blocks per cylinder as the *drive*
    /// declares it. Each partition states its own (`de_Surfaces *
    /// de_BlocksPerTrack`) and the two are allowed to disagree, which is
    /// why both are surfaced rather than one derived from the other.
    pub cyl_blocks: u32,
    /// `rdb_AutoParkSeconds` — idle seconds before an auto-park, 0 for
    /// never. As dead as [`park`](Self::park).
    pub auto_park_seconds: u32,
    /// `rdb_HighRDSKBlock` — the highest block any RDB structure
    /// currently occupies. `rdb_blocks_hi` is the *reserved* ceiling;
    /// this is the high-water mark actually used, so a writer knows
    /// where free space in the RDB area begins.
    pub high_rdsk_block: u32,
    /// `rdb_DiskVendor` — SCSI INQUIRY identity of the drive,
    /// space-padded ASCII (*not* BCPL, unlike `pb_DriveName`). Only
    /// meaningful when `flags` has [`rdb_flags::DISK_ID`]; otherwise
    /// this and its two neighbours are whatever bytes happened to be
    /// there, so they are parsed unconditionally but must not be
    /// displayed without checking the bit.
    pub disk_vendor: String,
    /// `rdb_DiskProduct`, gated by [`rdb_flags::DISK_ID`] as above.
    pub disk_product: String,
    /// `rdb_DiskRevision`, gated by [`rdb_flags::DISK_ID`] as above.
    pub disk_revision: String,
    /// `rdb_ControllerVendor` — the same INQUIRY identity for the
    /// controller, gated the same way by [`rdb_flags::CTRLR_ID`].
    pub controller_vendor: String,
    /// `rdb_ControllerProduct`, gated by [`rdb_flags::CTRLR_ID`].
    pub controller_product: String,
    /// `rdb_ControllerRevision`, gated by [`rdb_flags::CTRLR_ID`].
    pub controller_revision: String,
    /// Head of the `FSHD` chain ([`CHAIN_END`] if none).
    pub filesys_header_list: u32,
    /// Head of the `BADB` chain ([`CHAIN_END`] if none).
    pub bad_block_list: u32,
    /// Partitions in on-disk chain order.
    pub partitions: Vec<Partition>,
    /// Loadable filesystems in `FSHD` chain order. Empty when
    /// [`filesys_header_list`](Self::filesys_header_list) is
    /// [`CHAIN_END`].
    pub filesystems: Vec<FileSysHeader>,
    /// Bad-block remappings, flattened across every `BADB` block in the
    /// chain and kept in on-disk order.
    ///
    /// Flattened rather than grouped per block because the grouping
    /// carries no information: which entries share a block is an
    /// allocation artefact of whichever tool wrote the list, and a
    /// rewrite repacks them anyway. The chain head survives in
    /// [`bad_block_list`](Self::bad_block_list) for round-tripping.
    pub bad_blocks: Vec<BadBlockEntry>,
    /// LBAs of the `BADB` blocks the parse visited, in chain order.
    ///
    /// [`bad_blocks`](Self::bad_blocks) deliberately forgets which block
    /// each entry came from, but [`validate`](Self::validate) has to know
    /// *where the blocks were* to say whether the chain strayed outside
    /// the RDB area — and unlike `PART` and `FSHD`, whose LBAs ride along
    /// on [`Partition::part_block`] and [`FileSysHeader::fshd_block`],
    /// a `BADB` block has no per-block struct to carry it. Hence a list
    /// here rather than a field that does not exist anywhere else.
    pub badb_blocks: Vec<u64>,
}

/// Which kind of RDB structure a [`ValidationIssue`] is about.
///
/// Only the four chained block types appear: the `RDSK` block is not
/// part of any chain and its legal location is bounded by
/// [`RDB_LOCATION_LIMIT`], not by `rdb_RDBBlocksLo..=Hi`, so an `RDSK`
/// outside the RDB area is normal rather than an issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    /// A `PART` partition block.
    Part,
    /// A `FSHD` filesystem-header block.
    Fshd,
    /// A `LSEG` filesystem-driver payload block.
    Lseg,
    /// A `BADB` bad-block-list block.
    Badb,
}

impl core::fmt::Display for BlockKind {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            BlockKind::Part => "PART",
            BlockKind::Fshd => "FSHD",
            BlockKind::Lseg => "LSEG",
            BlockKind::Badb => "BADB",
        })
    }
}

/// One way an RDB's *layout* is self-destructive, as reported by
/// [`Rdb::validate`].
///
/// Not an error: every one of these describes an image that parses
/// perfectly and reads back exactly what is on it. They describe two
/// owners of the same blocks — an RDB structure sitting where a
/// filesystem believes it owns the space, or two partitions claiming the
/// same extent — which is a live grenade rather than a parse failure.
/// Partitioning tools have written RDB blocks past a too-small reserved
/// area into the first partition; the image is readable right up until
/// either side writes, after which both are wrong. A recovery tool needs
/// the data, so the parser hands it over; this type is how a consumer
/// learns not to *write*.
///
/// All block quantities are *device* blocks, like everything else in
/// this crate's API. The [`Display`](core::fmt::Display) impl renders a
/// single line fit to show a user, in `no_std` as much as `std` — as do
/// [`RdbError`] and [`PartitionSourceError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationIssue {
    /// `rdb_RDBBlocksLo` is above `rdb_RDBBlocksHi`: the reserved area is
    /// empty or inverted, so *nothing* can be said about what lies
    /// inside it. Reported once, and the checks that depend on the area
    /// ([`BlockOutsideRdbArea`](Self::BlockOutsideRdbArea),
    /// [`PartitionOverlapsRdbArea`](Self::PartitionOverlapsRdbArea)) are
    /// skipped rather than producing an issue per block against a range
    /// that means nothing. Partition-versus-partition checking is
    /// unaffected — it never consults the area.
    RdbAreaInvalid {
        /// `rdb_RDBBlocksLo` as it was read.
        lo: u32,
        /// `rdb_RDBBlocksHi` as it was read — below `lo`, which is the issue.
        hi: u32,
    },
    /// A chained block sits outside `rdb_RDBBlocksLo..=rdb_RDBBlocksHi` —
    /// i.e. in space the RDB itself says is not reserved, and which a
    /// partition or a repartitioner is therefore entitled to reuse.
    BlockOutsideRdbArea {
        /// Which chain the block belongs to.
        kind: BlockKind,
        /// Where it actually is.
        lba: u64,
        /// First block of the reserved area it should have been inside.
        lo: u64,
        /// Last block of that area, inclusive.
        hi: u64,
    },
    /// A partition's extent covers part of the RDB area: the filesystem
    /// and the partition table own the same blocks. The classic
    /// damaged-by-construction image.
    PartitionOverlapsRdbArea {
        /// Index into [`Rdb::partitions`].
        index: usize,
        /// The partition's `pb_DriveName`, so a report can name it.
        name: String,
        /// First block of the partition's extent.
        start_lba: u64,
        /// Its length, so the extent is `start_lba..start_lba + block_len`.
        block_len: u64,
        /// First block of the reserved area it collides with.
        lo: u64,
        /// Last block of that area, inclusive.
        hi: u64,
    },
    /// A partition's `de_HighCyl` is below its `de_LowCyl`: the extent
    /// runs backwards and so describes no blocks at all.
    /// [`Partition::block_len`] is zero for such a partition, which
    /// keeps it out of the overlap checks — this is the issue that says
    /// why, and it is not the same as a legitimately empty partition
    /// (there is no such thing: `de_HighCyl` is inclusive, so the
    /// smallest honest partition is one cylinder).
    PartitionCylindersInverted {
        /// Index into [`Rdb::partitions`].
        index: usize,
        /// The partition's `pb_DriveName`, so a report can name it.
        name: String,
        /// `de_LowCyl` as it was read.
        low_cyl: u32,
        /// `de_HighCyl` as it was read — below `low_cyl`, which is the issue.
        high_cyl: u32,
    },
    /// Two partitions' extents intersect. Beyond the letter of the
    /// "overlap validation" plan item, but the same failure family and
    /// the same consequence — two filesystems mounting the same blocks,
    /// each destroying the other — for one extra comparison.
    PartitionsOverlap {
        /// Index into [`Rdb::partitions`] of the first partition, always
        /// the lower of the two.
        a: usize,
        /// Index of the second, always above `a`.
        b: usize,
        /// `a`'s `pb_DriveName`, so a report can name it.
        a_name: String,
        /// `b`'s `pb_DriveName`.
        b_name: String,
        /// First block both claim.
        start: u64,
        /// How many, so the shared extent is `start..start + len`.
        len: u64,
    },
    /// A partition's `DosEnvec` is too short to reach `de_DosType`, so
    /// the fields below it — the dostype, and at a short enough
    /// `de_TableSize` the `de_LowCyl`/`de_HighCyl` extent and the
    /// `de_Surfaces`/`de_BlocksPerTrack` geometry too — are not on the
    /// block at all.
    ///
    /// The partition is still parsed and still in
    /// [`Rdb::partitions`](Rdb::partitions): a damaged entry must not
    /// cost a recovery tool the other entries on the chain, and the
    /// `PART` block round-trips through an edit unchanged because
    /// nothing takes its envec apart. What the absent fields read as is
    /// zero, which for the extent pair means a zero-length extent —
    /// excluded from every overlap check here, exactly like an inverted
    /// range.
    EnvecTooShort {
        /// Index into [`Rdb::partitions`].
        index: usize,
        /// The partition's `pb_DriveName`, so a report can name it.
        name: String,
        /// `de_TableSize` as the block declares it — below
        /// `de_DosType`'s index, which is the issue.
        table_size: u32,
    },
    /// Two filesystems' `LSEG` chains run through the same block: one
    /// driver's payload is spliced into another's, and rewriting either
    /// filesystem rewrites both.
    ///
    /// Reported by
    /// [`validate_seg_lists`](Rdb::validate_seg_lists) — the only half
    /// of validation that has the chains' LBAs — once per filesystem
    /// that collides with an earlier one, naming the first shared block
    /// rather than every one of them. [`RdbEditor::open`] refuses such
    /// an image outright ([`RdbError::SharedChain`]); this is the
    /// read-side report of the same damage.
    SharedLsegChain {
        /// Index into [`Rdb::filesystems`] of the filesystem whose chain
        /// ran into a block an earlier chain already used.
        index: usize,
        /// Index of that earlier filesystem, always below `index`.
        other: usize,
        /// The first block the two chains share.
        lba: u64,
    },
}

impl core::fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ValidationIssue::RdbAreaInvalid { lo, hi } => write!(
                f,
                "RDB area is empty or inverted: RDBBlocksLo {lo} is above RDBBlocksHi {hi}"
            ),
            ValidationIssue::BlockOutsideRdbArea { kind, lba, lo, hi } => write!(
                f,
                "{kind} block at {lba} lies outside the RDB area {lo}..={hi}"
            ),
            ValidationIssue::PartitionOverlapsRdbArea {
                index,
                name,
                start_lba,
                block_len,
                lo,
                hi,
            } => write!(
                f,
                "partition {index} ({name}) covers blocks {start_lba}..{} \
                 and overlaps the RDB area {lo}..={hi}",
                start_lba.saturating_add(*block_len)
            ),
            ValidationIssue::PartitionCylindersInverted {
                index,
                name,
                low_cyl,
                high_cyl,
            } => write!(
                f,
                "partition {index} ({name}) has an inverted cylinder range: \
                 LowCyl {low_cyl} is above HighCyl {high_cyl}"
            ),
            ValidationIssue::PartitionsOverlap {
                a,
                b,
                a_name,
                b_name,
                start,
                len,
            } => write!(
                f,
                "partitions {a} ({a_name}) and {b} ({b_name}) both claim blocks {start}..{}",
                start.saturating_add(*len)
            ),
            ValidationIssue::EnvecTooShort {
                index,
                name,
                table_size,
            } => write!(
                f,
                "partition {index} ({name}) declares de_TableSize {table_size}, \
                 short of the {} needed to reach de_DosType",
                de::DOS_TYPE
            ),
            ValidationIssue::SharedLsegChain { index, other, lba } => {
                write!(f, "filesystems {other} and {index} share LSEG block {lba}")
            }
        }
    }
}

/// Byte offsets into a `RDSK` block (NDK `RigidDiskBlock`).
mod rdsk {
    pub const HOST_ID: usize = 12;
    pub const BLOCK_BYTES: usize = 16;
    pub const FLAGS: usize = 20;
    pub const BAD_BLOCK_LIST: usize = 24;
    pub const PARTITION_LIST: usize = 28;
    pub const FILESYS_HEADER_LIST: usize = 32;
    pub const DRIVE_INIT: usize = 36;
    // 40..64: rdb_Reserved1[6]
    pub const CYLINDERS: usize = 64;
    pub const SECTORS: usize = 68;
    pub const HEADS: usize = 72;
    pub const INTERLEAVE: usize = 76;
    pub const PARK: usize = 80;
    // 84..96: rdb_Reserved2[3]
    pub const WRITE_PRE_COMP: usize = 96;
    pub const REDUCED_WRITE: usize = 100;
    pub const STEP_RATE: usize = 104;
    // 108..128: rdb_Reserved3[5]
    pub const RDB_BLOCKS_LO: usize = 128;
    pub const RDB_BLOCKS_HI: usize = 132;
    pub const LO_CYLINDER: usize = 136;
    pub const HI_CYLINDER: usize = 140;
    pub const CYL_BLOCKS: usize = 144;
    pub const AUTO_PARK_SECONDS: usize = 148;
    pub const HIGH_RDSK_BLOCK: usize = 152;
    // 156: rdb_Reserved4

    /// The identification strings: `(byte offset, byte length)`. Not
    /// BCPL — plain space-padded ASCII, unlike `pb_DriveName` four
    /// structures away.
    pub const DISK_VENDOR: (usize, usize) = (160, 8);
    pub const DISK_PRODUCT: (usize, usize) = (168, 16);
    pub const DISK_REVISION: (usize, usize) = (184, 4);
    pub const CONTROLLER_VENDOR: (usize, usize) = (188, 8);
    pub const CONTROLLER_PRODUCT: (usize, usize) = (196, 16);
    pub const CONTROLLER_REVISION: (usize, usize) = (212, 4);
}

/// Byte offsets shared by every chained RDB block.
///
/// `PART`, `FSHD`, `LSEG` and `BADB` all begin with the same five
/// longwords — ID, SummedLongs, ChkSum, HostID, Next — which is what
/// makes one [`walk_chain`] able to serve all four.
mod chain {
    pub const NEXT: usize = 16;
}

/// Byte offsets into a `PART` block (NDK `PartitionBlock`).
mod part {
    pub const FLAGS: usize = 20;
    pub const DRIVE_NAME: usize = 36; // BCPL: length byte, then chars (32 bytes total)
    pub const ENVIRONMENT: usize = 128; // DosEnvec, longwords
}

/// `DosEnvec` field indices, in longwords from its start
/// (NDK `dos/filehandler.h`).
mod de {
    pub const TABLE_SIZE: usize = 0;
    pub const SIZE_BLOCK: usize = 1;
    pub const SEC_ORG: usize = 2;
    pub const SURFACES: usize = 3;
    pub const SECTORS_PER_BLOCK: usize = 4;
    pub const BLOCKS_PER_TRACK: usize = 5;
    pub const RESERVED: usize = 6;
    pub const PRE_ALLOC: usize = 7;
    pub const INTERLEAVE: usize = 8;
    pub const LOW_CYL: usize = 9;
    pub const HIGH_CYL: usize = 10;
    pub const NUM_BUFFERS: usize = 11;
    pub const BUF_MEM_TYPE: usize = 12;
    pub const MAX_TRANSFER: usize = 13;
    pub const MASK: usize = 14;
    pub const BOOT_PRI: usize = 15;
    pub const DOS_TYPE: usize = 16;
    /// The three optional tail fields. `de_TableSize` counts longwords
    /// *after itself*, so a field at index `i` is present exactly when
    /// `table_size >= i`.
    pub const BAUD: usize = 17;
    pub const CONTROL: usize = 18;
    pub const BOOT_BLOCKS: usize = 19;
}

/// Byte offsets into a `FSHD` block (NDK `FileSysHeaderBlock`).
mod fshd {
    pub const HOST_ID: usize = 12;
    pub const FLAGS: usize = 20;
    // 24..32: fhb_Reserved1[2]
    pub const DOS_TYPE: usize = 32;
    pub const VERSION: usize = 36;
    pub const PATCH_FLAGS: usize = 40;
    /// First of the nine patched longwords. Everything gated by
    /// [`super::fshd_patch`] is at `PATCHED + n * 4`, which is exactly
    /// why the bit numbering is positional.
    pub const PATCHED: usize = 44;
    /// Index into the patched longwords, not a byte offset — the one
    /// field read unconditionally.
    pub const SEG_LIST_INDEX: usize = 7;
}

/// Byte offsets into a `LSEG` block (NDK `LoadSegBlock`).
mod lseg {
    /// Start of `lsb_LoadData`; everything from here to the end of the
    /// block is driver payload.
    pub const LOAD_DATA: usize = 20;
}

/// Byte offsets into a `BADB` block (NDK `BadBlockBlock`).
mod badb {
    // 20: bbb_Reserved
    /// Start of `bbb_BlockPairs` — pairs of (bad, good) longwords.
    pub const ENTRIES: usize = 24;
    /// Longwords before the entries. `SummedLongs` counts the whole
    /// block header plus the entries, so the entry count is
    /// `SummedLongs - HEADER_LONGS`.
    pub const HEADER_LONGS: usize = ENTRIES / 4;
}

/// Walk one RDB block chain, verifying every block and calling `visit`.
///
/// The four chains (`PART`, `FSHD`, `LSEG`, `BADB`) differ only in the
/// ID they expect and what they do with each block, so the discipline
/// that matters — off-disk pointers, cycles, wrong IDs, bad checksums —
/// lives here once rather than four times. `buf` is the caller's scratch
/// block and holds the last-read block on return.
///
/// Bounded two ways. The visited set is the real bound and the one that
/// catches the failure mode a crafted or corrupted image produces — a
/// cycle — and, when the source reports a
/// [`block_count`](BlockSource::block_count), it is sufficient on its
/// own: every hop is at a distinct in-range block, so the chain cannot
/// outlast the disk. [`MAX_CHAIN_BLOCKS`] is the backstop for a source
/// that reports no count, where nothing else limits how long an
/// acyclic-so-far chain can run.
///
/// The set is a [`BTreeSet`], not a list: an *O(L²)* membership scan
/// over a chain the image chooses the length of is work an attacker
/// picks, and a driver chain of tens of thousands of `LSEG` blocks is
/// perfectly legitimate.
fn walk_chain<S, F>(
    disk: &mut S,
    head: u32,
    expected: u32,
    buf: &mut [u8],
    mut visit: F,
) -> Result<(), RdbError<S::Error>>
where
    S: BlockSource,
    F: FnMut(&[u8], u64) -> Result<(), RdbError<S::Error>>,
{
    let mut next = head;
    let mut visited: BTreeSet<u32> = BTreeSet::new();
    while next != CHAIN_END {
        let lba = next as u64;
        if let Some(n) = disk.block_count() {
            if lba >= n {
                return Err(RdbError::ChainOutOfRange { lba });
            }
        }
        if !visited.insert(next) {
            return Err(RdbError::ChainCycle { lba });
        }
        if visited.len() > MAX_CHAIN_BLOCKS {
            return Err(RdbError::ChainTooLong {
                limit: MAX_CHAIN_BLOCKS,
            });
        }

        disk.read_block(lba, buf).map_err(RdbError::Io)?;
        let found = be32(buf, hdr::ID);
        if found != expected {
            return Err(RdbError::WrongId {
                lba,
                expected,
                found,
            });
        }
        if !checksum_ok(buf) {
            return Err(RdbError::BadChecksum { lba });
        }

        visit(buf, lba)?;
        next = be32(buf, chain::NEXT);
    }
    Ok(())
}

impl Rdb {
    /// Find and parse the RDB on `disk`.
    ///
    /// Scans the first [`RDB_LOCATION_LIMIT`] blocks for a `RDSK` block
    /// whose checksum passes (both conditions: an `RDSK` ID with a bad
    /// sum is skipped, matching what the ROM does, so a stale copy at a
    /// lower LBA cannot shadow the live RDB), then walks the `PART`
    /// chain. A read that fails *during the scan* ends it rather than
    /// failing the parse — see [`RdbError::NoRdsk`] for why, and for
    /// what it costs. `rdb_BlockBytes` must match the source's
    /// [`block_size`](BlockSource::block_size) — see
    /// [`RdbError::BlockBytesMismatch`].
    pub fn parse<S: BlockSource>(disk: &mut S) -> Result<Self, RdbError<S::Error>> {
        let block_size = disk.block_size();
        if !block_size_ok(block_size) {
            return Err(RdbError::UnsupportedBlockSize { block_size });
        }
        let mut buf = alloc::vec![0u8; block_size];

        let mut rdsk_at = None;
        let scan_end = match disk.block_count() {
            Some(n) => n.min(RDB_LOCATION_LIMIT),
            None => RDB_LOCATION_LIMIT,
        };
        for lba in 0..scan_end {
            // A read that fails ends the *scan*, it does not fail the
            // parse: a source that declines to give a block count
            // (`block_count() == None`) can only signal its end of
            // medium by failing a read, and a disk of four blocks with
            // no RDB on it must answer `NoRdsk` — the documented
            // outcome — rather than an `Io` about block 4. The cost is
            // that a genuinely failing device gets `NoRdsk` from the
            // probe too, which is the right trade: the probe is a
            // search, not a health check, and every read *after* an
            // RDSK is found still reports its error faithfully.
            if disk.read_block(lba, &mut buf).is_err() {
                break;
            }
            if be32(&buf, hdr::ID) == id::RDSK && checksum_ok(&buf) {
                rdsk_at = Some(lba);
                break;
            }
        }
        let rdsk_at = rdsk_at.ok_or(RdbError::NoRdsk)?;

        let block_bytes = be32(&buf, rdsk::BLOCK_BYTES);
        if block_bytes as usize != block_size {
            return Err(RdbError::BlockBytesMismatch {
                block_bytes,
                block_size,
            });
        }

        let mut rdb = Rdb {
            rdsk_block: rdsk_at,
            block_bytes,
            flags: be32(&buf, rdsk::FLAGS),
            host_id: be32(&buf, rdsk::HOST_ID),
            drive_init: be32(&buf, rdsk::DRIVE_INIT),
            cylinders: be32(&buf, rdsk::CYLINDERS),
            heads: be32(&buf, rdsk::HEADS),
            sectors: be32(&buf, rdsk::SECTORS),
            interleave: be32(&buf, rdsk::INTERLEAVE),
            park: be32(&buf, rdsk::PARK),
            write_pre_comp: be32(&buf, rdsk::WRITE_PRE_COMP),
            reduced_write: be32(&buf, rdsk::REDUCED_WRITE),
            step_rate: be32(&buf, rdsk::STEP_RATE),
            rdb_blocks_lo: be32(&buf, rdsk::RDB_BLOCKS_LO),
            rdb_blocks_hi: be32(&buf, rdsk::RDB_BLOCKS_HI),
            lo_cylinder: be32(&buf, rdsk::LO_CYLINDER),
            hi_cylinder: be32(&buf, rdsk::HI_CYLINDER),
            cyl_blocks: be32(&buf, rdsk::CYL_BLOCKS),
            auto_park_seconds: be32(&buf, rdsk::AUTO_PARK_SECONDS),
            high_rdsk_block: be32(&buf, rdsk::HIGH_RDSK_BLOCK),
            disk_vendor: padded_ascii(&buf, rdsk::DISK_VENDOR),
            disk_product: padded_ascii(&buf, rdsk::DISK_PRODUCT),
            disk_revision: padded_ascii(&buf, rdsk::DISK_REVISION),
            controller_vendor: padded_ascii(&buf, rdsk::CONTROLLER_VENDOR),
            controller_product: padded_ascii(&buf, rdsk::CONTROLLER_PRODUCT),
            controller_revision: padded_ascii(&buf, rdsk::CONTROLLER_REVISION),
            filesys_header_list: be32(&buf, rdsk::FILESYS_HEADER_LIST),
            bad_block_list: be32(&buf, rdsk::BAD_BLOCK_LIST),
            partitions: Vec::new(),
            filesystems: Vec::new(),
            bad_blocks: Vec::new(),
            badb_blocks: Vec::new(),
        };
        let partition_list = be32(&buf, rdsk::PARTITION_LIST);

        let mut partitions = Vec::new();
        walk_chain(disk, partition_list, id::PART, &mut buf, |b, lba| {
            partitions.push(parse_part(b, lba));
            Ok(())
        })?;
        rdb.partitions = partitions;

        let mut filesystems = Vec::new();
        walk_chain(
            disk,
            rdb.filesys_header_list,
            id::FSHD,
            &mut buf,
            |b, lba| {
                filesystems.push(parse_fshd(b, lba));
                Ok(())
            },
        )?;
        rdb.filesystems = filesystems;

        let mut bad_blocks = Vec::new();
        let mut badb_blocks = Vec::new();
        walk_chain(disk, rdb.bad_block_list, id::BADB, &mut buf, |b, lba| {
            badb_blocks.push(lba);
            parse_badb(b, &mut bad_blocks);
            Ok(())
        })?;
        rdb.bad_blocks = bad_blocks;
        rdb.badb_blocks = badb_blocks;

        Ok(rdb)
    }

    /// Reassemble one filesystem driver's binary from its `LSEG` chain.
    ///
    /// Lazy rather than a field on [`Rdb`]: driver binaries run to
    /// hundreds of kilobytes and most consumers of a partition table
    /// never want them, so the cost is paid only when asked for. `disk`
    /// must be the source the RDB was parsed from — the chain pointers
    /// are LBAs on it — which is checked via its block size.
    ///
    /// The result is the driver in AmigaDOS hunk format, exactly as a
    /// `LoadSeg` would consume it. This crate does not parse or relocate
    /// hunks; that is the loader's job wherever the driver ends up
    /// running.
    ///
    /// **Length has block granularity.** `LSEG` records no exact byte
    /// count anywhere — each block contributes its whole `lsb_LoadData`
    /// area (`block_bytes - 20` bytes) — so the returned `Vec` is the
    /// binary followed by up to that much slack from the final block.
    /// This is not a defect in the reassembly: the hunk structure inside
    /// knows where it ends, and every real consumer finds the end by
    /// parsing hunks rather than by trusting a length.
    ///
    /// An FSHD with no chain ([`CHAIN_END`]) yields an empty `Vec`, not
    /// an error — a header that only patches device-node fields for a
    /// ROM filesystem is legitimate.
    pub fn load_filesystem<S: BlockSource>(
        &self,
        fshd: &FileSysHeader,
        disk: &mut S,
    ) -> Result<Vec<u8>, RdbError<S::Error>> {
        let block_size = disk.block_size();
        if block_size != self.block_bytes as usize {
            return Err(RdbError::BlockBytesMismatch {
                block_bytes: self.block_bytes,
                block_size,
            });
        }
        let mut buf = alloc::vec![0u8; block_size];
        let mut out = Vec::new();
        walk_chain(disk, fshd.seg_list_blocks, id::LSEG, &mut buf, |b, _lba| {
            out.extend_from_slice(&b[lseg::LOAD_DATA..]);
            Ok(())
        })?;
        Ok(out)
    }

    /// Check the *layout* for blocks with two owners.
    ///
    /// Separate from [`parse`](Self::parse) on purpose, and returning a
    /// list rather than an error: an image whose RDB structures have
    /// spilled into a partition still parses, still reads back exactly
    /// what is on it, and a recovery tool needs precisely that. Refusing
    /// to parse it would destroy the only path to the data. What a
    /// consumer needs to know before *writing* is a different question,
    /// and this is where it is answered.
    ///
    /// Reports, in a stable order:
    ///
    /// 1. every chained block the parse visited — `PART`, `FSHD`, `BADB` —
    ///    lying outside `rdb_RDBBlocksLo..=rdb_RDBBlocksHi`;
    /// 2. every partition extent (`start_lba..start_lba + block_len`,
    ///    device blocks) overlapping that same area;
    /// 3. every partition whose `de_HighCyl` is below its `de_LowCyl`;
    /// 4. every pair of partitions whose extents intersect;
    /// 5. every partition whose `de_TableSize` is too short to reach
    ///    `de_DosType`, whose fields below it are therefore absent
    ///    rather than zero (see [`ValidationIssue::EnvecTooShort`]).
    ///
    /// Check 4 goes beyond the RDB-versus-partition case, but it is the
    /// same failure — two owners, both writing — and costs a sort plus
    /// one comparison per pair that actually overlaps, rather than one
    /// per pair. Checks 3 and 4 never consult the RDB area,
    /// so they run even when it is unusable.
    ///
    /// `LSEG` blocks are *not* covered here: they are lazy by design (a
    /// driver binary is hundreds of kilobytes and their LBAs are never
    /// held in memory), so checking them needs the disk back.
    /// [`validate_seg_lists`](Self::validate_seg_lists) does that, and a
    /// consumer that cares about the whole layout runs both.
    ///
    /// An empty result means the layout is self-consistent. It does not
    /// mean the image is undamaged — checksums and chain discipline are
    /// [`parse`](Self::parse)'s business, and passed already.
    pub fn validate(&self) -> Vec<ValidationIssue> {
        let mut issues = Vec::new();

        if self.rdb_blocks_lo > self.rdb_blocks_hi {
            issues.push(ValidationIssue::RdbAreaInvalid {
                lo: self.rdb_blocks_lo,
                hi: self.rdb_blocks_hi,
            });
        } else {
            let lo = self.rdb_blocks_lo as u64;
            let hi = self.rdb_blocks_hi as u64;

            let outside = |kind: BlockKind, lba: u64, issues: &mut Vec<ValidationIssue>| {
                if lba < lo || lba > hi {
                    issues.push(ValidationIssue::BlockOutsideRdbArea { kind, lba, lo, hi });
                }
            };
            for p in &self.partitions {
                outside(BlockKind::Part, p.part_block, &mut issues);
            }
            for f in &self.filesystems {
                outside(BlockKind::Fshd, f.fshd_block, &mut issues);
            }
            for &lba in &self.badb_blocks {
                outside(BlockKind::Badb, lba, &mut issues);
            }

            for (index, p) in self.partitions.iter().enumerate() {
                if p.block_len != 0
                    && p.start_lba <= hi
                    && p.start_lba.saturating_add(p.block_len) > lo
                {
                    issues.push(ValidationIssue::PartitionOverlapsRdbArea {
                        index,
                        name: p.name.clone(),
                        start_lba: p.start_lba,
                        block_len: p.block_len,
                        lo,
                        hi,
                    });
                }
            }
        }

        // Inverted cylinder ranges, which need no RDB area either — and
        // which run first of the area-independent checks because an
        // inverted extent is *why* a partition is missing from the
        // overlap results below.
        for (index, p) in self.partitions.iter().enumerate() {
            if p.high_cyl < p.low_cyl {
                issues.push(ValidationIssue::PartitionCylindersInverted {
                    index,
                    name: p.name.clone(),
                    low_cyl: p.low_cyl,
                    high_cyl: p.high_cyl,
                });
            }
        }

        // Partition-versus-partition, which needs no RDB area and so
        // runs even when the area is unusable.
        //
        // Swept in start order rather than compared pairwise: the number
        // of partitions is whatever the `PART` chain says, and an image
        // is free to say thousands. Sorting first costs O(P log P) and
        // then each partition is compared only against the ones that
        // actually start before it ends, so the quadratic term is paid
        // only for pairs that genuinely overlap — which are pairs the
        // caller asked to be told about.
        let mut order: Vec<usize> = (0..self.partitions.len()).collect();
        let extent = |i: usize| {
            let p = &self.partitions[i];
            (p.start_lba, p.start_lba.saturating_add(p.block_len))
        };
        order.sort_by_key(|&i| extent(i));
        for (rank, &i) in order.iter().enumerate() {
            let (i_start, i_end) = extent(i);
            if i_start >= i_end {
                continue;
            }
            for &j in &order[rank + 1..] {
                let (j_start, j_end) = extent(j);
                // Sorted by start, so once one candidate starts at or
                // after `i`'s end, every later one does too.
                if j_start >= i_end {
                    break;
                }
                if j_start >= j_end {
                    continue;
                }
                // `a` is always the lower index, as documented, which
                // the sort order does not preserve.
                let (a, b) = (i.min(j), i.max(j));
                let start = i_start.max(j_start);
                let end = i_end.min(j_end);
                issues.push(ValidationIssue::PartitionsOverlap {
                    a,
                    b,
                    a_name: self.partitions[a].name.clone(),
                    b_name: self.partitions[b].name.clone(),
                    start,
                    len: end - start,
                });
            }
        }

        // Short envecs last: a partition whose `de_TableSize` does not
        // reach `de_DosType` parsed anyway (see `parse_part`), and this
        // is where a consumer is told that some of what it is reading
        // was not on the block. Like the inverted case, it explains a
        // partition's absence from the extent checks above.
        for (index, p) in self.partitions.iter().enumerate() {
            let table_size = p.envec_raw.first().copied().unwrap_or(0);
            if (table_size as usize) < de::DOS_TYPE {
                issues.push(ValidationIssue::EnvecTooShort {
                    index,
                    name: p.name.clone(),
                    table_size,
                });
            }
        }

        issues
    }

    /// The `LSEG` half of [`validate`](Self::validate): walk every
    /// filesystem's driver chain and report blocks outside the RDB area,
    /// and chains that run through each other
    /// ([`ValidationIssue::SharedLsegChain`]).
    ///
    /// Takes the disk because `LSEG` chains are only ever walked on
    /// demand — [`load_filesystem`](Self::load_filesystem) is where their
    /// LBAs exist at all — so unlike the other three chains there is
    /// nothing in memory to check. The blocks are read for their chain
    /// pointers and their payload discarded, which is cheap next to
    /// reassembling the binaries.
    ///
    /// Fails only the way [`load_filesystem`](Self::load_filesystem)
    /// does: a broken chain (wrong ID, bad checksum, cycle, off-disk
    /// pointer) is a parse error, not a layout issue. When the RDB area
    /// itself is unusable ([`ValidationIssue::RdbAreaInvalid`], which
    /// [`validate`](Self::validate) reports) this returns no issues,
    /// there being no range to compare against.
    pub fn validate_seg_lists<S: BlockSource>(
        &self,
        disk: &mut S,
    ) -> Result<Vec<ValidationIssue>, RdbError<S::Error>> {
        if self.rdb_blocks_lo > self.rdb_blocks_hi {
            return Ok(Vec::new());
        }
        let block_size = disk.block_size();
        if block_size != self.block_bytes as usize {
            return Err(RdbError::BlockBytesMismatch {
                block_bytes: self.block_bytes,
                block_size,
            });
        }
        let (lo, hi) = (self.rdb_blocks_lo as u64, self.rdb_blocks_hi as u64);
        let mut buf = alloc::vec![0u8; block_size];
        let mut issues = Vec::new();
        // Which filesystem claimed each LSEG block, so a block on two
        // chains is caught. A shared chain is damage — see
        // [`ValidationIssue::SharedLsegChain`] — and it is only visible
        // from here, where every chain is walked in one pass.
        let mut owner: BTreeMap<u64, usize> = BTreeMap::new();
        for (index, f) in self.filesystems.iter().enumerate() {
            let mut shared: Option<(usize, u64)> = None;
            walk_chain(disk, f.seg_list_blocks, id::LSEG, &mut buf, |_b, lba| {
                if lba < lo || lba > hi {
                    issues.push(ValidationIssue::BlockOutsideRdbArea {
                        kind: BlockKind::Lseg,
                        lba,
                        lo,
                        hi,
                    });
                }
                // One issue per colliding filesystem, not one per shared
                // block: two chains that are the same chain share every
                // block of it, and an image gets to choose how many that
                // is.
                match owner.get(&lba) {
                    Some(&other) if shared.is_none() => shared = Some((other, lba)),
                    Some(_) => {}
                    None => {
                        owner.insert(lba, index);
                    }
                }
                Ok(())
            })?;
            if let Some((other, lba)) = shared {
                issues.push(ValidationIssue::SharedLsegChain { index, other, lba });
            }
        }
        Ok(issues)
    }
}

/// Parse a verified `FSHD` block.
///
/// Infallible: every field is at a fixed offset well inside the
/// smallest legal block, and `PatchFlags` cannot make a read go out of
/// bounds — it only decides whether an already-in-bounds longword is
/// meaningful.
fn parse_fshd(buf: &[u8], lba: u64) -> FileSysHeader {
    let patch_flags = be32(buf, fshd::PATCH_FLAGS);
    let patched = |i: usize| be32(buf, fshd::PATCHED + i * 4);
    let gated = |i: usize| {
        if patch_flags & (1 << i) != 0 {
            Some(patched(i))
        } else {
            None
        }
    };

    FileSysHeader {
        fshd_block: lba,
        host_id: be32(buf, fshd::HOST_ID),
        flags: be32(buf, fshd::FLAGS),
        dos_type: be32(buf, fshd::DOS_TYPE),
        version: be32(buf, fshd::VERSION),
        patch_flags,
        node_type: gated(0),
        task: gated(1),
        lock: gated(2),
        handler: gated(3),
        stack_size: gated(4),
        priority: gated(5).map(|v| v as i32),
        startup: gated(6).map(|v| v as i32),
        // Bit 7 is SegList, read unconditionally below.
        global_vec: gated(8).map(|v| v as i32),
        seg_list_blocks: patched(fshd::SEG_LIST_INDEX),
    }
}

/// Append a verified `BADB` block's entries to `out`.
///
/// `SummedLongs` counts the header plus the entry longwords, so the
/// entry count is `(SummedLongs - 6) / 2`. That value is
/// attacker-controlled, and although [`checksum_ok`] has already
/// rejected a `SummedLongs` larger than the block, the arithmetic is
/// clamped to what the block physically holds anyway: a bounds check
/// that depends on a checksum passing is one refactor away from not
/// being a bounds check.
fn parse_badb(buf: &[u8], out: &mut Vec<BadBlockEntry>) {
    let summed_longs = be32(buf, hdr::SUMMED_LONGS) as usize;
    let entry_longs = summed_longs.saturating_sub(badb::HEADER_LONGS);
    let capacity_longs = (buf.len() - badb::ENTRIES) / 4;
    let pairs = entry_longs.min(capacity_longs) / 2;
    for i in 0..pairs {
        let off = badb::ENTRIES + i * 8;
        out.push(BadBlockEntry {
            bad: be32(buf, off),
            good: be32(buf, off + 4),
        });
    }
}

/// Read a fixed-width, space-padded ASCII field as a `String`.
///
/// The RDSK identification fields are SCSI INQUIRY data copied
/// verbatim: fixed width, padded with spaces — not BCPL, and not
/// NUL-terminated by specification, though real controllers pad with
/// NULs often enough that both are trimmed. Bytes become chars
/// one-for-one (latin-1-ish) rather than being trusted as UTF-8, the
/// same treatment `pb_DriveName` gets.
fn padded_ascii(buf: &[u8], (off, len): (usize, usize)) -> String {
    let field = &buf[off..off + len];
    let end = field
        .iter()
        .rposition(|&b| b != b' ' && b != 0)
        .map_or(0, |i| i + 1);
    field[..end].iter().map(|&b| b as char).collect()
}

/// Parse one `PART` block. Infallible by design: every field is either
/// present, or absent and reported as such.
///
/// **A short `DosEnvec` is tolerated, not refused.** `de_TableSize`
/// counts the longwords after itself, and a block declaring fewer than
/// `de_DosType`'s index has no dostype and — below that — no
/// `de_LowCyl`/`de_HighCyl` either, so it describes no extent. Failing
/// the whole parse on it (which this crate used to do) contradicts the
/// read-everything philosophy in the most expensive way: one damaged
/// `PART` block would deny a recovery tool every *other* partition on
/// the disk. So the partition is parsed with what is there, fields
/// beyond `de_TableSize` read as zero — giving a zero-length extent,
/// the same treatment an inverted range gets — and
/// [`ValidationIssue::EnvecTooShort`] reports it where the rest of the
/// layout damage is reported. `envec_raw` still holds exactly the
/// longwords the block carried, so an editor round-trips such a
/// partition byte for byte.
fn parse_part(buf: &[u8], lba: u64) -> Partition {
    let envec = |i: usize| be32(buf, part::ENVIRONMENT + i * 4);

    let table_size = envec(de::TABLE_SIZE);
    // How many envec longwords the block physically holds. de_TableSize
    // is attacker-controlled, so every read — the optional fields and
    // envec_raw alike — is clamped to this, and a hostile 0xFFFFFFFF
    // truncates instead of running off the block.
    let envec_capacity = (buf.len() - part::ENVIRONMENT) / 4;
    let present = |i: usize| {
        if table_size as usize >= i && i < envec_capacity {
            Some(envec(i))
        } else {
            None
        }
    };
    // A field the declared envec does not reach is *not on the block*,
    // whatever bytes happen to lie at its offset: `de_TableSize` is how
    // long the structure a mount copies out is, so reading past it would
    // be inventing a value. Zero is what an absent numeric field reads
    // as, which for the extent pair means no extent at all.
    let field = |i: usize| present(i).unwrap_or(0);
    // table_size + 1 longwords, TableSize itself included; saturating
    // so table_size == u32::MAX cannot wrap to zero.
    let raw_len = (table_size as usize).saturating_add(1).min(envec_capacity);
    let envec_raw: Vec<u32> = (0..raw_len).map(envec).collect();

    // BCPL string: length byte then bytes, no terminator. The name is
    // ASCII in every image ever seen, but bytes are passed through
    // as-is (lossy) rather than trusted to be UTF-8.
    let name_len = (buf[part::DRIVE_NAME] as usize).min(31);
    let name_bytes = &buf[part::DRIVE_NAME + 1..part::DRIVE_NAME + 1 + name_len];
    let name: String = name_bytes.iter().map(|&b| b as char).collect();

    let surfaces = field(de::SURFACES) as u64;
    let blocks_per_track = field(de::BLOCKS_PER_TRACK) as u64;
    // Two u32s widened first, so this product alone cannot exceed u64.
    // Everything downstream multiplies it by a *third* u32 and so can,
    // which is why the extent arithmetic below saturates.
    let cylinder_blocks = surfaces * blocks_per_track;
    let low_cyl = field(de::LOW_CYL);
    let high_cyl = field(de::HIGH_CYL);
    let flags = be32(buf, part::FLAGS);

    // `de_HighCyl` is inclusive, so the span is `high - low + 1` — but
    // nothing on disk makes `high >= low` true, and an inverted pair is
    // exactly what a corrupt (or hostile) PART block carries. Zero is
    // the honest length: an inverted range contains no blocks, and a
    // zero-length extent is already excluded from every overlap check.
    // The inversion itself is reported by
    // [`Rdb::validate`](Rdb::validate), where layout nonsense belongs —
    // refusing the parse would deny a recovery tool the only view of
    // the damage. Found by the fuzzer: subtracting unchecked panicked
    // in a debug build and, worse, wrapped in a release one, conjuring
    // a multi-exabyte partition out of two plausible u32s.
    let block_len = match high_cyl.checked_sub(low_cyl) {
        Some(span) => (span as u64 + 1).saturating_mul(cylinder_blocks),
        None => 0,
    };

    Partition {
        part_block: lba,
        name,
        bootable: flags & 1 != 0,
        no_automount: flags & 2 != 0,
        start_lba: (low_cyl as u64).saturating_mul(cylinder_blocks),
        block_len,
        dos_type: field(de::DOS_TYPE),
        boot_pri: field(de::BOOT_PRI) as i32,
        max_transfer: field(de::MAX_TRANSFER),
        mask: field(de::MASK),
        cylinder_blocks,
        low_cyl,
        high_cyl,
        num_buffers: field(de::NUM_BUFFERS),
        buf_mem_type: field(de::BUF_MEM_TYPE),
        size_block_longs: field(de::SIZE_BLOCK),
        baud: present(de::BAUD),
        control: present(de::CONTROL),
        boot_blocks: present(de::BOOT_BLOCKS),
        envec_raw,
    }
}

/// The values this crate writes into a new `PART` block's `DosEnvec`
/// when the caller does not override them, and where each came from.
///
/// **Provenance: every number here was read back out of an image made by
/// amitools' `rdbtool` 0.8.1**, by creating a disk, adding partitions and
/// parsing the raw `PART` blocks — not from the NDK's suggested values
/// and not from folklore. The reason is the one
/// [`synthesize_geometry`] gives for the geometry convention:
/// `rdbtool`'s images are this crate's fixtures and its differential
/// oracle, so a default that differs from `rdbtool`'s would make every
/// such comparison a false positive, and would hand the Amiga a mount
/// parameter subtly unlike the one every other image on the machine
/// carries.
///
/// A caller who wants different values sets them on the
/// [`PartitionSpec`]; nothing here is enforced, only defaulted.
pub mod envec_defaults {
    /// `de_TableSize` — 16, the index of `de_DosType`, so the envec runs
    /// exactly as far as the last field the format's mount path needs.
    /// The three tail fields (`de_Baud`, `de_Control`, `de_BootBlocks`)
    /// are *absent* rather than zero, which is what `rdbtool` writes and
    /// what [`Partition::baud`](super::Partition::baud) and friends report as `None`.
    pub const TABLE_SIZE: u32 = 16;

    /// `de_SecOrg` — sector origin. Always zero; the field has never had
    /// another meaning.
    pub const SEC_ORG: u32 = 0;

    /// `de_SectorPerBlock` — device sectors per filesystem block. One,
    /// with `de_SizeBlock` carrying the size instead; every image in the
    /// wild says one.
    pub const SECTORS_PER_BLOCK: u32 = 1;

    /// `de_Reserved` — blocks reserved at the start of the partition for
    /// the boot block. Two, which is what a DOS-family filesystem needs
    /// and what every tool writes.
    pub const RESERVED: u32 = 2;

    /// `de_PreAlloc` — blocks reserved at the *end* of the partition.
    /// Zero: a DOS filesystem needs none.
    pub const PRE_ALLOC: u32 = 0;

    /// `de_Interleave` — filesystem-level interleave. Zero.
    pub const INTERLEAVE: u32 = 0;

    /// `de_NumBuffers` — cache buffers the handler allocates at mount.
    /// A mount parameter that says nothing about the disk; 30 is
    /// `rdbtool`'s default.
    pub const NUM_BUFFERS: u32 = 30;

    /// `de_BufMemType` — which memory those buffers want. Zero means
    /// "any", which is right for everything except a DMA controller that
    /// cannot reach fast RAM.
    pub const BUF_MEM_TYPE: u32 = 0;

    /// `de_MaxTransfer` — the largest transfer, in bytes, the driver may
    /// issue in one go.
    pub const MAX_TRANSFER: u32 = 0x00FF_FFFF;

    /// `de_Mask` — address mask for DMA-reachable memory. `0x7FFFFFFE`
    /// is the conventional "anything word-aligned in the low 2 GB".
    pub const MASK: u32 = 0x7FFF_FFFE;

    /// `de_DosType` — `DOS\x03` (FFS with international caching),
    /// `rdbtool`'s default for a new partition. Callers who want
    /// `DOS\x00` or `DOS\x07` say so.
    pub const DOS_TYPE: u32 = 0x444F_5303;

    /// `de_SizeBlock`, the filesystem block size in longwords, is *not*
    /// a constant: `rdbtool` writes `rdb_BlockBytes / 4` — 128 on a
    /// 512-byte-block disk, 1024 on a 4 KB one — so the filesystem block
    /// and the device block start out the same size. This is the one
    /// default that depends on the geometry, which is why
    /// [`PartitionSpec::size_block_longs`](super::PartitionSpec::size_block_longs) is an [`Option`] rather than
    /// carrying a number the constructor cannot know.
    ///
    /// The two remain independent knobs after that: a caller may ask for
    /// a 32 KB filesystem block on a 512-byte-block disk, which is a real
    /// and working configuration.
    pub const fn size_block_longs(block_size: usize) -> u32 {
        (block_size / 4) as u32
    }
}

/// The values this crate writes into a new `RDSK` block when the caller
/// does not override them — same provenance as [`envec_defaults`]:
/// observed in `rdbtool` 0.8.1's output, not assumed.
pub mod rdsk_defaults {
    /// `rdb_Flags` — `LAST | LASTLUN | LASTTID`
    /// ([`rdb_flags`](super::rdb_flags)): there is no disk after this
    /// one, no LUN after this one, no target after this one. The honest
    /// answer for an image file, which is not on a bus at all, and what
    /// `rdbtool` writes.
    pub const FLAGS: u32 =
        super::rdb_flags::LAST | super::rdb_flags::LAST_LUN | super::rdb_flags::LAST_TID;

    /// `rdb_HostID` — the controller's own SCSI ID. Seven by convention
    /// (the host adapter traditionally takes the highest priority ID),
    /// and meaningless on an image.
    pub const HOST_ID: u32 = 7;

    /// `rdb_Interleave` — physical sector interleave. One means "none".
    pub const INTERLEAVE: u32 = 1;

    /// `rdb_StepRate` — head step rate in the drive's own units.
    pub const STEP_RATE: u32 = 3;

    /// `rdb_AutoParkSeconds` — idle seconds before an auto-park. Zero is
    /// never, which is the only sane value for anything modern.
    pub const AUTO_PARK_SECONDS: u32 = 0;
}

/// The values this crate writes into a new `FSHD` block when the caller
/// does not override them — same provenance as [`envec_defaults`] and
/// [`rdsk_defaults`]: observed in `rdbtool` 0.8.1's output, by creating
/// an image, running `rdbtool <img> fsadd <driver>` and reading the raw
/// `FSHD` block back, not assumed from the NDK.
///
/// What `rdbtool` writes, in full: `fhb_HostID` **0**, `fhb_Flags` 0,
/// `fhb_Version` 0 unless asked (`version=43.4` packs to `0x002B0004`),
/// `fhb_PatchFlags` **0x180** — and *only* 0x180, i.e.
/// [`SEG_LIST`](fshd_patch::SEG_LIST) and
/// [`GLOBAL_VEC`](fshd_patch::GLOBAL_VEC) — with `fhb_GlobalVec`
/// **-1** ("not BCPL") and every other patched longword left zero *and
/// unpatched*. `fhb_Type`, `Task`, `Lock`, `Handler`, `StackSize`,
/// `Priority` and `Startup` are therefore absent rather than zero, which
/// is exactly the distinction [`FileSysHeader`]'s [`Option`]s preserve on
/// the read side, so the defaults here are `None` and setting one is what
/// turns its bit on.
pub mod fshd_defaults {
    /// `fhb_HostID` — **0**, which is what `rdbtool` writes into an
    /// `FSHD` even while writing 7 into the `RDSK` and every `PART` on
    /// the same disk. Matched rather than "corrected" to 7: the field is
    /// as meaningless on an image as the other two, and byte-identity
    /// with the differential oracle is worth more than tidiness.
    /// [`FileSystemSpec::host_id`](super::FileSystemSpec::host_id) is the
    /// override for a caller reproducing a real controller's image.
    pub const HOST_ID: u32 = 0;

    /// `fhb_Flags` — zero. The NDK defines no bits in it.
    pub const FLAGS: u32 = 0;

    /// `fhb_GlobalVec` — **-1**, the value that means "this handler is
    /// not BCPL and wants no global vector". Written by `rdbtool` for
    /// every filesystem it adds, and gated on by
    /// [`fshd_patch::GLOBAL_VEC`](super::fshd_patch::GLOBAL_VEC), which
    /// is why it is one of only two bits `rdbtool` sets.
    pub const GLOBAL_VEC: i32 = -1;
}

/// One loadable filesystem driver to write: which partitions it serves,
/// what version it is, its binary, and whichever device-node fields it
/// wants patched.
///
/// This is the `DOS\x07` shipping story — put the filesystem *inside*
/// the image and a ROM that never heard of the dostype can still mount
/// the partition, because the RDB carries the handler it needs.
///
/// **The binary is not parsed.** It is AmigaDOS hunk-format data as
/// `LoadSeg` would consume it, and this crate splits it into `LSEG`
/// blocks and chains them without looking inside — hunk parsing and
/// relocation are a founding non-goal, the loader's job wherever the
/// driver ends up running. Any bytes are accepted, including bytes that
/// are not hunks at all; `rdbtool` accepts them too, which is how the
/// defaults here were observed.
///
/// The eight patchable fields are [`Option`]s for the reason
/// [`FileSysHeader`]'s are: unset means "this filesystem does not
/// override that device-node field", which is not the same as
/// overriding it with zero. Setting one sets its
/// [`fshd_patch`] bit; leaving it `None` leaves the longword zero and
/// the bit clear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSystemSpec {
    /// `fhb_DosType` — the dostype whose partitions this driver serves,
    /// matched against a [`PartitionSpec::dos_type`].
    pub dos_type: u32,
    /// The driver binary, hunk-format, unparsed — see the type's docs.
    pub binary: Vec<u8>,
    /// `fhb_Version`'s high half.
    pub version_major: u16,
    /// `fhb_Version`'s low half.
    pub version_minor: u16,
    /// `fhb_HostID`, defaulting to [`fshd_defaults::HOST_ID`].
    pub host_id: u32,
    /// `fhb_Flags`, defaulting to [`fshd_defaults::FLAGS`].
    pub flags: u32,
    /// `fhb_PatchFlags` written verbatim, or `None` to derive it from
    /// which fields below are `Some` — plus
    /// [`fshd_patch::SEG_LIST`] whenever there is an `LSEG` chain to
    /// point at, which is what `rdbtool` does.
    ///
    /// The override exists for a caller reproducing an existing image
    /// bit-for-bit, including bits this crate does not model. It changes
    /// only the mask: the longwords themselves are still written from
    /// the fields, so a bit set here over a `None` field patches a zero.
    pub patch_flags: Option<u32>,
    /// `fhb_Type`, gated by [`fshd_patch::TYPE`].
    pub node_type: Option<u32>,
    /// `fhb_Task`, gated by [`fshd_patch::TASK`].
    pub task: Option<u32>,
    /// `fhb_Lock`, gated by [`fshd_patch::LOCK`].
    pub lock: Option<u32>,
    /// `fhb_Handler`, gated by [`fshd_patch::HANDLER`].
    pub handler: Option<u32>,
    /// `fhb_StackSize`, gated by [`fshd_patch::STACK_SIZE`].
    pub stack_size: Option<u32>,
    /// `fhb_Priority`, gated by [`fshd_patch::PRIORITY`].
    pub priority: Option<i32>,
    /// `fhb_Startup`, gated by [`fshd_patch::STARTUP`].
    pub startup: Option<i32>,
    /// `fhb_GlobalVec`, gated by [`fshd_patch::GLOBAL_VEC`]. Defaults to
    /// `Some(`[`fshd_defaults::GLOBAL_VEC`]`)` — the one patched field
    /// `rdbtool` fills in — rather than `None`, since a non-BCPL handler
    /// is what every driver written since the 1980s is.
    pub global_vec: Option<i32>,
}

impl FileSystemSpec {
    /// A driver for `dos_type` carrying `binary`, with every patchable
    /// field at its [`fshd_defaults`] value.
    pub fn new(dos_type: u32, binary: Vec<u8>) -> Self {
        Self {
            dos_type,
            binary,
            version_major: 0,
            version_minor: 0,
            host_id: fshd_defaults::HOST_ID,
            flags: fshd_defaults::FLAGS,
            patch_flags: None,
            node_type: None,
            task: None,
            lock: None,
            handler: None,
            stack_size: None,
            priority: None,
            startup: None,
            global_vec: Some(fshd_defaults::GLOBAL_VEC),
        }
    }

    /// Set `fhb_Version` from its two halves.
    pub fn version(mut self, major: u16, minor: u16) -> Self {
        self.version_major = major;
        self.version_minor = minor;
        self
    }

    /// Patch `fhb_StackSize` into the device node.
    pub fn stack_size(mut self, bytes: u32) -> Self {
        self.stack_size = Some(bytes);
        self
    }

    /// Patch `fhb_Priority` into the device node.
    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = Some(priority);
        self
    }

    /// Patch `fhb_GlobalVec`, overriding the default of
    /// [`fshd_defaults::GLOBAL_VEC`].
    pub fn global_vec(mut self, global_vec: i32) -> Self {
        self.global_vec = Some(global_vec);
        self
    }

    /// `fhb_Version` as the format packs it: major in the high half.
    fn packed_version(&self) -> u32 {
        ((self.version_major as u32) << 16) | self.version_minor as u32
    }

    /// The nine patched longwords, in block order, as
    /// `(value, patched)` — `patched` deciding the
    /// [`fshd_patch`] bit. Index 7 is `fhb_SegListBlocks`, which the
    /// caller fills in from the layout because only the layout knows
    /// where the chain starts.
    fn patched_fields(&self) -> [(u32, bool); 9] {
        let opt32 = |v: Option<u32>| (v.unwrap_or(0), v.is_some());
        let opti32 = |v: Option<i32>| (v.unwrap_or(0) as u32, v.is_some());
        [
            opt32(self.node_type),
            opt32(self.task),
            opt32(self.lock),
            opt32(self.handler),
            opt32(self.stack_size),
            opti32(self.priority),
            opti32(self.startup),
            (CHAIN_END, false), // SegListBlocks: the layout's business
            opti32(self.global_vec),
        ]
    }
}

/// Where a partition goes: a size the builder turns into cylinders, or
/// the cylinders themselves.
///
/// Two entry points because there are two kinds of caller. One has a
/// size — "give me a 500 MB `DH0`" — and wants the cylinder arithmetic
/// done for it. The other is reproducing a layout it already knows (a
/// clone of an existing disk, a recipe in a build file) and must be able
/// to say the exact cylinders, because rounding a size back into
/// cylinders is not guaranteed to land on the same ones.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement {
    /// At most this many bytes, placed after the previous partition.
    ///
    /// **Rounds the cylinder count down** — `bytes / cylinder_bytes`,
    /// floored — so a partition never claims more space than was asked
    /// for, and falls short of it by less than one cylinder. That is what
    /// `rdbtool` 0.8.1 does (`cyls = num_bytes //
    /// rdisk.get_cylinder_bytes()`, verified against the images it
    /// writes: 10 MiB on a 129 024-byte cylinder becomes 81 cylinders,
    /// 10 450 944 bytes, not 82), and it is the same direction
    /// [`synthesize_geometry`] rounds, for the same reason — a size is a
    /// ceiling, and the tool that hands out more than it was asked for is
    /// the one that walks off the end of something.
    ///
    /// A size below one whole cylinder therefore describes no partition
    /// at all and is [`BuildError::PartitionTooSmall`], not a silently
    /// rounded-up one: `de_HighCyl` is inclusive, so there is no
    /// zero-cylinder partition to write. `rdbtool` refuses the same case
    /// ("invalid partition range given!").
    ///
    /// A caller who needs "at least *n* bytes" rounds up itself, or uses
    /// [`Cylinders`](Self::Cylinders) — the point of the pair is that the
    /// exact answer is always available.
    Size(u64),
    /// Exactly these cylinders, `high` *inclusive*, as `de_LowCyl` and
    /// `de_HighCyl` will say. Placed where it says, not after the
    /// previous partition — a builder given only explicit ranges places
    /// nothing implicitly, and overlaps are refused rather than shuffled.
    Cylinders {
        /// `de_LowCyl`, the first cylinder.
        low: u32,
        /// `de_HighCyl`, the last — inclusive, so `low == high` is a
        /// legal one-cylinder partition.
        high: u32,
    },
}

/// One partition to create: where it goes and what its `PART` block will
/// say.
///
/// Construct with [`by_size`](Self::by_size) or
/// [`by_cylinders`](Self::by_cylinders), which fill every mount
/// parameter from [`envec_defaults`]; change what you need through the
/// chainable setters or by assigning the public fields directly, since a
/// caller cloning an existing disk needs to reproduce values this crate
/// has no opinion about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionSpec {
    /// Where the partition goes.
    pub placement: Placement,
    /// `pb_DriveName`, or `None` to have the builder assign the first
    /// free `DH`*n* — see [`RdbBuilder::build`] for exactly which.
    pub name: Option<String>,
    /// `pb_Flags` bit 0: the ROM may boot from this partition.
    pub bootable: bool,
    /// `pb_Flags` bit 1: mount it, but not automatically at boot.
    pub no_automount: bool,
    /// `de_BootPri` — boot priority, higher wins. Ignored unless
    /// [`bootable`](Self::bootable).
    pub boot_pri: i32,
    /// `de_DosType`.
    pub dos_type: u32,
    /// `de_SizeBlock`, in longwords — the *filesystem* block size, which
    /// is per partition and independent of `rdb_BlockBytes`.
    ///
    /// `None` takes [`envec_defaults::size_block_longs`] of the
    /// geometry's block size, which is what `rdbtool` writes and the one
    /// default the [`PartitionSpec`] constructors cannot fill in, not
    /// knowing the disk they will be built against.
    pub size_block_longs: Option<u32>,
    /// `de_SecOrg`.
    pub sec_org: u32,
    /// `de_SectorPerBlock`.
    pub sectors_per_block: u32,
    /// `de_Reserved` — boot blocks at the start of the partition.
    pub reserved: u32,
    /// `de_PreAlloc` — blocks held back at the end.
    pub pre_alloc: u32,
    /// `de_Interleave`.
    pub interleave: u32,
    /// `de_NumBuffers`.
    pub num_buffers: u32,
    /// `de_BufMemType`.
    pub buf_mem_type: u32,
    /// `de_MaxTransfer`.
    pub max_transfer: u32,
    /// `de_Mask`.
    pub mask: u32,
}

impl PartitionSpec {
    /// A partition of at least `bytes`, placed after the previous one —
    /// see [`Placement::Size`] for the rounding.
    pub fn by_size(bytes: u64) -> Self {
        Self::with_placement(Placement::Size(bytes))
    }

    /// A partition on exactly cylinders `low..=high` (inclusive).
    pub fn by_cylinders(low: u32, high: u32) -> Self {
        Self::with_placement(Placement::Cylinders { low, high })
    }

    fn with_placement(placement: Placement) -> Self {
        Self {
            placement,
            name: None,
            bootable: false,
            no_automount: false,
            boot_pri: 0,
            dos_type: envec_defaults::DOS_TYPE,
            size_block_longs: None,
            sec_org: envec_defaults::SEC_ORG,
            sectors_per_block: envec_defaults::SECTORS_PER_BLOCK,
            reserved: envec_defaults::RESERVED,
            pre_alloc: envec_defaults::PRE_ALLOC,
            interleave: envec_defaults::INTERLEAVE,
            num_buffers: envec_defaults::NUM_BUFFERS,
            buf_mem_type: envec_defaults::BUF_MEM_TYPE,
            max_transfer: envec_defaults::MAX_TRANSFER,
            mask: envec_defaults::MASK,
        }
    }

    /// Set `pb_DriveName` explicitly instead of taking an assigned one.
    pub fn named(mut self, name: &str) -> Self {
        self.name = Some(String::from(name));
        self
    }

    /// Set `de_DosType`.
    pub fn dos_type(mut self, dos_type: u32) -> Self {
        self.dos_type = dos_type;
        self
    }

    /// Mark the partition bootable (`pb_Flags` bit 0) with the given
    /// `de_BootPri`.
    pub fn bootable(mut self, boot_pri: i32) -> Self {
        self.bootable = true;
        self.boot_pri = boot_pri;
        self
    }

    /// Set `de_SizeBlock` in longwords — the filesystem block size, not
    /// the device's.
    pub fn size_block_longs(mut self, longs: u32) -> Self {
        self.size_block_longs = Some(longs);
        self
    }
}

/// Why [`RdbBuilder::build`] refused to write.
///
/// Every variant except [`Io`](Self::Io) is raised **before the first
/// block is written**: the builder computes the whole layout, checks it,
/// and only then writes, so a rejected build leaves the target exactly as
/// it found it. That is the point of the type — the failure mode this
/// crate exists to prevent is a partitioner discovering halfway through
/// that its structures do not fit and writing them into partition space
/// anyway.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError<E> {
    /// The [`BlockSink`] failed on a write. The only variant that can
    /// leave the target half-written — the layout was valid and the
    /// device said no.
    Io(E),
    /// The sink's [`block_size`](BlockSink::block_size) is not a power of
    /// two in [`MIN_BLOCK_SIZE`]`..=`[`MAX_BLOCK_SIZE`].
    UnsupportedBlockSize {
        /// What the sink said its block size was.
        block_size: usize,
    },
    /// The [`Geometry`]'s block size and the sink's disagree. Every LBA
    /// the builder computes is in geometry blocks; writing them through a
    /// differently sized sink would address the wrong bytes.
    BlockSizeMismatch {
        /// [`Geometry::block_size`].
        geometry: usize,
        /// [`BlockSink::block_size`].
        sink: usize,
    },
    /// The geometry describes no blocks — a zero in `cylinders`, `heads`
    /// or `sectors`. Nothing can be placed against it.
    EmptyGeometry {
        /// The geometry as given.
        geometry: Geometry,
    },
    /// A `pb_DriveName` does not fit the 32-byte BCPL field, or is empty.
    InvalidName {
        /// The name asked for.
        name: String,
        /// The longest name the field holds.
        max: usize,
    },
    /// Two partitions were given the same `pb_DriveName`. Refused rather
    /// than silently renamed: two `DH0`s is a layout whose mounts fight
    /// each other, and the caller asked for it explicitly.
    DuplicateName {
        /// The name asked for twice.
        name: String,
    },
    /// A [`Placement::Cylinders`] range runs backwards.
    CylindersInverted {
        /// The partition's name.
        name: String,
        /// `de_LowCyl` as asked for.
        low_cyl: u32,
        /// `de_HighCyl` as asked for — below `low_cyl`, which is the issue.
        high_cyl: u32,
    },
    /// A [`Placement::Size`] is below one cylinder, so it describes no
    /// partition at all — see that variant for why this is refused
    /// rather than rounded up to one.
    PartitionTooSmall {
        /// The partition's name.
        name: String,
        /// The size asked for.
        bytes: u64,
        /// One cylinder, in bytes — the smallest partition there is.
        cylinder_bytes: u64,
    },
    /// A partition's last cylinder is past the last cylinder the geometry
    /// has. Covers both a [`Placement::Cylinders`] range that overshoots
    /// and a [`Placement::Size`] larger than the space left.
    PartitionPastEndOfDisk {
        /// The partition's name.
        name: String,
        /// The last cylinder it wanted.
        high_cyl: u32,
        /// The last cylinder the disk has (`rdb_Cylinders - 1`).
        last_cylinder: u32,
    },
    /// A partition starts below `rdb_LoCylinder`, i.e. inside the
    /// reserved RDB area. The overlap this crate exists to refuse.
    PartitionOverlapsRdbArea {
        /// The partition's name.
        name: String,
        /// The cylinder it wanted to start on.
        low_cyl: u32,
        /// The first cylinder available to partitions.
        lo_cylinder: u32,
    },
    /// Two partitions claim the same cylinders.
    PartitionsOverlap {
        /// The first partition's name, in the order they were added.
        a_name: String,
        /// The second's.
        b_name: String,
        /// The first cylinder both claim.
        low_cyl: u32,
        /// The last, inclusive.
        high_cyl: u32,
    },
    /// The reserved RDB area cannot hold the blocks the layout needs —
    /// one `RDSK`, one `PART` per partition, and one `FSHD` plus its
    /// `LSEG` chain per filesystem. Either too many of them, or a
    /// [`RdbBuilder::reserved_blocks`] override too small; a driver
    /// binary is by far the most likely cause, being hundreds of blocks
    /// where a partition is one.
    RdbAreaTooSmall {
        /// Blocks the layout needs, `RDSK` included.
        needed: u32,
        /// Blocks the area has (`rdb_RDBBlocksHi - rdb_RDBBlocksLo + 1`).
        available: u32,
    },
    /// The sink is smaller than the layout: its
    /// [`block_count`](BlockSink::block_count) is below the last block
    /// the layout would occupy. Only detectable when the sink knows its
    /// size; a sink reporting `None` is taken at its word.
    SinkTooSmall {
        /// Blocks the layout needs to exist.
        needed: u64,
        /// Blocks the sink says it has.
        available: u64,
    },
    /// Sealing a block's checksum failed.
    ///
    /// Unreachable in practice — the builder seals 64 longwords into
    /// blocks of at least [`MIN_BLOCK_SIZE`], which hold 128 — but the
    /// alternative to a variant here is an `unwrap`, and a writer that
    /// can panic on a caller's arithmetic is exactly what
    /// [`seal_checksum`] returns a `Result` to avoid.
    Seal(SealError),
}

impl<E: core::fmt::Display> core::fmt::Display for BuildError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            BuildError::Io(e) => write!(f, "writing a block failed: {e}"),
            BuildError::UnsupportedBlockSize { block_size } => write!(
                f,
                "unsupported device block size {block_size}: \
                 must be a power of two in {MIN_BLOCK_SIZE}..={MAX_BLOCK_SIZE}"
            ),
            BuildError::BlockSizeMismatch { geometry, sink } => write!(
                f,
                "the geometry is in {geometry}-byte blocks but the sink writes \
                 {sink}-byte blocks"
            ),
            BuildError::EmptyGeometry { geometry } => write!(
                f,
                "the geometry {}/{}/{} describes no blocks",
                geometry.cylinders, geometry.heads, geometry.sectors
            ),
            BuildError::InvalidName { name, max } => write!(
                f,
                "drive name {name:?} does not fit pb_DriveName: \
                 1..={max} characters are available"
            ),
            BuildError::DuplicateName { name } => {
                write!(f, "two partitions are both named {name:?}")
            }
            BuildError::CylindersInverted {
                name,
                low_cyl,
                high_cyl,
            } => write!(
                f,
                "partition {name:?} has an inverted cylinder range: \
                 LowCyl {low_cyl} is above HighCyl {high_cyl}"
            ),
            BuildError::PartitionTooSmall {
                name,
                bytes,
                cylinder_bytes,
            } => write!(
                f,
                "partition {name:?} asks for {bytes} bytes, less than the \
                 {cylinder_bytes}-byte cylinder that is the smallest partition"
            ),
            BuildError::PartitionPastEndOfDisk {
                name,
                high_cyl,
                last_cylinder,
            } => write!(
                f,
                "partition {name:?} ends on cylinder {high_cyl}, past the disk's last \
                 cylinder {last_cylinder}"
            ),
            BuildError::PartitionOverlapsRdbArea {
                name,
                low_cyl,
                lo_cylinder,
            } => write!(
                f,
                "partition {name:?} starts on cylinder {low_cyl}, inside the RDB area \
                 that ends at cylinder {}",
                lo_cylinder.saturating_sub(1)
            ),
            BuildError::PartitionsOverlap {
                a_name,
                b_name,
                low_cyl,
                high_cyl,
            } => write!(
                f,
                "partitions {a_name:?} and {b_name:?} both claim cylinders \
                 {low_cyl}..={high_cyl}"
            ),
            BuildError::RdbAreaTooSmall { needed, available } => write!(
                f,
                "the RDB area holds {available} blocks but the layout needs {needed}"
            ),
            BuildError::SinkTooSmall { needed, available } => write!(
                f,
                "the layout needs {needed} blocks but the target has {available}"
            ),
            BuildError::Seal(e) => write!(f, "sealing a block failed: {e}"),
        }
    }
}

#[cfg(feature = "std")]
impl<E: std::error::Error + 'static> std::error::Error for BuildError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            BuildError::Io(e) => Some(e),
            BuildError::Seal(e) => Some(e),
            _ => None,
        }
    }
}

/// Where one partition landed: the answer to "what did you write, and
/// where", for a caller that wants to act on the result without
/// re-parsing the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedPartition {
    /// LBA of the `PART` block written for it.
    pub part_block: u64,
    /// The `pb_DriveName` written — the assigned one if the spec left it
    /// to the builder.
    pub name: String,
    /// `de_LowCyl` as written.
    pub low_cyl: u32,
    /// `de_HighCyl` as written, *inclusive*.
    pub high_cyl: u32,
    /// First device block of the partition's data, as
    /// [`Partition::start_lba`] will report it.
    pub start_lba: u64,
    /// How many device blocks it covers, as [`Partition::block_len`]
    /// will report it.
    pub block_len: u64,
}

/// Where one loadable filesystem landed: its `FSHD` block and the
/// `LSEG` chain carrying its binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedFileSystem {
    /// LBA of the `FSHD` block written for it.
    pub fshd_block: u64,
    /// `fhb_DosType` as written, so a caller can match it to a partition
    /// without re-parsing.
    pub dos_type: u32,
    /// `fhb_SegListBlocks` as written — the first `LSEG` block, or
    /// [`CHAIN_END`] for a header carrying no binary.
    pub seg_list_blocks: u32,
    /// How many `LSEG` blocks the chain has. Zero exactly when
    /// [`seg_list_blocks`](Self::seg_list_blocks) is [`CHAIN_END`]; the
    /// blocks are consecutive from it, which is how this crate (and
    /// `rdbtool`) allocate them.
    pub lseg_block_count: u32,
}

/// The complete block layout [`RdbBuilder::build`] computed and wrote.
///
/// Returned rather than nothing so a caller need not re-parse the disk
/// to learn where its partitions ended up — though re-parsing is exactly
/// what this crate's own tests do, on the principle that the disk is the
/// only authority on what is on the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RdbLayout {
    /// LBA the `RDSK` block was written to.
    pub rdsk_block: u64,
    /// `rdb_RDBBlocksLo` as written.
    pub rdb_blocks_lo: u32,
    /// `rdb_RDBBlocksHi` as written — the reserved ceiling, *inclusive*.
    pub rdb_blocks_hi: u32,
    /// `rdb_HighRDSKBlock` as written — the highest block actually used,
    /// so the gap up to [`rdb_blocks_hi`](Self::rdb_blocks_hi) is the
    /// headroom a later edit has to work in.
    pub high_rdsk_block: u32,
    /// `rdb_LoCylinder` — the first cylinder available to partitions.
    pub lo_cylinder: u32,
    /// `rdb_HiCylinder` — the last, *inclusive*.
    pub hi_cylinder: u32,
    /// The geometry written into the `RDSK` block.
    pub geometry: Geometry,
    /// The partitions, in the order they were added and chained.
    pub partitions: Vec<PlacedPartition>,
    /// The loadable filesystems, in the order they were added and
    /// chained — the `FSHD` chain `rdb_FileSysHeaderList` heads.
    pub filesystems: Vec<PlacedFileSystem>,
}

/// Build a fresh RDB — an `RDSK` block and its `PART` chain — on an
/// empty target.
///
/// # The order of operations, which is the whole design
///
/// [`build`](Self::build) computes the **complete** block layout,
/// validates every part of it, and only then writes. There is no code
/// path that writes block *N+1* after discovering that block *N* was the
/// last one that fit: "does not fit" is a [`BuildError`] returned before
/// the sink is touched. This is not defensiveness for its own sake — RDB
/// images damaged by exactly that failure exist in the wild, partition
/// tables written past a too-small reserved area into the first
/// partition, after which the filesystem and the partition table each
/// destroy the other. [`Rdb::validate`] is how a reader finds such an
/// image; this is how a writer never makes one.
///
/// Blocks are written `PART` chain first and `RDSK` last, so an
/// interrupted build leaves no valid `RDSK` — an unpartitioned disk
/// rather than a partition table pointing at blocks that were never
/// written.
///
/// # Example
///
/// ```
/// use amiga_rdb::{PartitionSpec, RdbBuilder};
/// # use amiga_rdb::{BlockSink, BlockSource, Rdb};
/// # struct MemDisk(Vec<u8>);
/// # fn eof() -> std::io::Error {
/// #     std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "past the end of the disk")
/// # }
/// # impl BlockSource for MemDisk {
/// #     type Error = std::io::Error;
/// #     fn block_size(&self) -> usize { 512 }
/// #     fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
/// #         let off = lba as usize * 512;
/// #         buf.copy_from_slice(self.0.get(off..off + 512).ok_or_else(eof)?);
/// #         Ok(())
/// #     }
/// #     fn block_count(&self) -> Option<u64> { Some(self.0.len() as u64 / 512) }
/// # }
/// # impl BlockSink for MemDisk {
/// #     type Error = std::io::Error;
/// #     fn block_size(&self) -> usize { 512 }
/// #     fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error> {
/// #         let off = lba as usize * 512;
/// #         self.0.get_mut(off..off + 512).ok_or_else(eof)?.copy_from_slice(buf);
/// #         Ok(())
/// #     }
/// #     fn block_count(&self) -> Option<u64> { Some(self.0.len() as u64 / 512) }
/// # }
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut disk = MemDisk(vec![0u8; 64 * 1024 * 1024]);
///
/// let layout = RdbBuilder::for_size(64 * 1024 * 1024, 512)?
///     .partition(PartitionSpec::by_size(16 * 1024 * 1024).bootable(0))
///     .partition(PartitionSpec::by_size(16 * 1024 * 1024).named("WORK"))
///     .build(&mut disk)?;
///
/// assert_eq!(layout.partitions[0].name, "DH0");
/// assert_eq!(layout.partitions[1].name, "WORK");
///
/// // The disk is the authority on what is on the disk.
/// let rdb = Rdb::parse(&mut disk)?;
/// assert_eq!(rdb.partitions.len(), 2);
/// assert!(rdb.validate().is_empty());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RdbBuilder {
    geometry: Geometry,
    rdsk_block: u32,
    reserved_blocks: Option<u32>,
    flags: u32,
    host_id: u32,
    specs: Vec<PartitionSpec>,
    filesystems: Vec<FileSystemSpec>,
}

/// Longest `pb_DriveName` the 32-byte BCPL field holds: one length byte
/// and 31 characters.
const MAX_DRIVE_NAME: usize = 31;

/// How many longwords an `RDSK`, `PART` or `FSHD` block sums over — 64,
/// i.e. the first 256 bytes, whatever the device block size.
const HEADER_SUMMED_LONGS: u32 = 64;

/// Longwords of `LSEG` header before `lsb_LoadData` — five: ID,
/// SummedLongs, ChkSum, HostID, Next.
const LSEG_HEADER_LONGS: u32 = (lseg::LOAD_DATA / 4) as u32;

/// Driver payload one `LSEG` block of `block_size` bytes carries: the
/// whole block past `lsb_LoadData`.
///
/// The format records no byte count anywhere, which is why
/// [`Rdb::load_filesystem`] returns a block-granular length — and why
/// this is the only number the split needs.
const fn lseg_payload_bytes(block_size: usize) -> usize {
    block_size - lseg::LOAD_DATA
}

/// The disk-level values one `PART` block copies out of the `RDSK`: the
/// owning controller's ID, the geometry the envec repeats, and the
/// device block size the default `de_SizeBlock` follows.
///
/// Bundled rather than passed one at a time because the two writers take
/// them from different places — [`RdbBuilder`] from its [`Geometry`],
/// [`RdbEditor`] from the `RDSK` it parsed — and everything downstream
/// wants all four together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PartContext {
    /// `pb_HostID`.
    host_id: u32,
    /// `de_Surfaces`.
    heads: u32,
    /// `de_BlocksPerTrack`.
    sectors: u32,
    /// The device block size, for [`envec_defaults::size_block_longs`].
    block_size: usize,
}

/// Fill a zeroed buffer with one `PART` block's fields — everything
/// including `SummedLongs`, everything except the `ChkSum`.
///
/// Shared by [`RdbBuilder::fill_part`], which seals it on the spot, and
/// by [`RdbEditor::add_partition`], which leaves the seal to the reseal
/// every block gets on the way out of `prepare`. One function so the two
/// paths cannot drift about what a new partition looks like — a created
/// `DH0` and an added one are the same 512 bytes.
fn fill_part_fields(
    buf: &mut [u8],
    spec: &PartitionSpec,
    name: &str,
    (low_cyl, high_cyl): (u32, u32),
    next: u32,
    ctx: PartContext,
) {
    put_be32(buf, hdr::ID, id::PART);
    put_be32(buf, hdr::SUMMED_LONGS, HEADER_SUMMED_LONGS);
    put_be32(buf, hdr::HOST_ID, ctx.host_id);
    put_be32(buf, chain::NEXT, next);
    let flags = (spec.bootable as u32) | ((spec.no_automount as u32) << 1);
    put_be32(buf, part::FLAGS, flags);

    // pb_DriveName is BCPL — a length byte then the characters, no
    // terminator — unlike the RDSK's identification strings four
    // structures away, which are space-padded ASCII.
    let name = name.as_bytes();
    buf[part::DRIVE_NAME] = name.len() as u8;
    buf[part::DRIVE_NAME + 1..part::DRIVE_NAME + 1 + name.len()].copy_from_slice(name);

    let mut env = |i: usize, v: u32| put_be32(buf, part::ENVIRONMENT + i * 4, v);
    env(de::TABLE_SIZE, envec_defaults::TABLE_SIZE);
    env(
        de::SIZE_BLOCK,
        spec.size_block_longs
            .unwrap_or_else(|| envec_defaults::size_block_longs(ctx.block_size)),
    );
    env(de::SEC_ORG, spec.sec_org);
    env(de::SURFACES, ctx.heads);
    env(de::SECTORS_PER_BLOCK, spec.sectors_per_block);
    env(de::BLOCKS_PER_TRACK, ctx.sectors);
    env(de::RESERVED, spec.reserved);
    env(de::PRE_ALLOC, spec.pre_alloc);
    env(de::INTERLEAVE, spec.interleave);
    env(de::LOW_CYL, low_cyl);
    env(de::HIGH_CYL, high_cyl);
    env(de::NUM_BUFFERS, spec.num_buffers);
    env(de::BUF_MEM_TYPE, spec.buf_mem_type);
    env(de::MAX_TRANSFER, spec.max_transfer);
    env(de::MASK, spec.mask);
    env(de::BOOT_PRI, spec.boot_pri as u32);
    env(de::DOS_TYPE, spec.dos_type);
}

/// Fill a zeroed buffer with one `FSHD` block's fields, checksum aside.
///
/// `fhb_PatchFlags` is derived from which of the spec's optional fields
/// are set — plus [`fshd_patch::SEG_LIST`] whenever `seg_list_blocks` is
/// not [`CHAIN_END`], which is what `rdbtool` does — unless the spec
/// overrides the mask outright. Either way every one of the nine
/// longwords is written: an unpatched field is a zero behind a clear
/// bit, which is exactly how the read side distinguishes "absent" from
/// "zero".
///
/// A writer that does not yet know where the chain will land passes any
/// placed value and lets the layout patch the longword afterwards — only
/// the *value* is unknown at that point, the bit being decided by
/// whether there is a chain at all.
fn fill_fshd_fields(buf: &mut [u8], spec: &FileSystemSpec, seg_list_blocks: u32, next: u32) {
    put_be32(buf, hdr::ID, id::FSHD);
    put_be32(buf, hdr::SUMMED_LONGS, HEADER_SUMMED_LONGS);
    put_be32(buf, fshd::HOST_ID, spec.host_id);
    put_be32(buf, chain::NEXT, next);
    put_be32(buf, fshd::FLAGS, spec.flags);
    put_be32(buf, fshd::DOS_TYPE, spec.dos_type);
    put_be32(buf, fshd::VERSION, spec.packed_version());

    let mut fields = spec.patched_fields();
    fields[fshd::SEG_LIST_INDEX] = (seg_list_blocks, seg_list_blocks != CHAIN_END);

    let mut derived = 0u32;
    for (i, &(value, patched)) in fields.iter().enumerate() {
        put_be32(buf, fshd::PATCHED + i * 4, value);
        if patched {
            derived |= 1 << i;
        }
    }
    put_be32(buf, fshd::PATCH_FLAGS, spec.patch_flags.unwrap_or(derived));
}

/// Fill a zeroed buffer with one `LSEG` block carrying `data`, and
/// return the `SummedLongs` it declares.
///
/// **`SummedLongs` is the number of longwords actually summed**, not the
/// whole block: five header longwords plus `data.len() / 4`, *floored*.
/// A full block therefore sums `block_size / 4` and the final partial
/// one sums only as far as its payload reaches, with any trailing one to
/// three bytes outside the sum. That is what `rdbtool` 0.8.1 writes —
/// observed across payload lengths either side of every boundary (a
/// 493-byte driver at 512-byte blocks gives `[128, 5]`, 496 bytes gives
/// `[128, 6]`) — and it is not merely cosmetic: `rdbtool`'s `fsget`
/// recovers the driver's byte length from these counts, so a
/// block-sized `SummedLongs` on the last block would hand a reader a
/// driver padded out with slack.
///
/// **One deliberate deviation, and it is a bug on the other side.**
/// `rdbtool` 0.8.1 writes that reduced count but computes `ChkSum` over
/// the *whole block* regardless. The two agree only while the bytes past
/// the declared count are zero — which they are for a driver whose
/// length is a multiple of four, and are not for any other, whose
/// trailing one to three bytes then sit outside the sum `rdbtool`
/// actually took. Such a block does not check out over the longwords it
/// says it summed — [`checksum_ok`] rejects it, as would any reader that
/// follows `SummedLongs`, this crate's parser and a 68k ROM alike.
/// Matching the count is interoperability; matching the checksum would
/// be writing a block that fails its own header, which is the one thing
/// a writer must never do. The count is matched, the sum is correct, and
/// the two agree.
fn fill_lseg_fields(buf: &mut [u8], host_id: u32, data: &[u8], next: u32) -> u32 {
    put_be32(buf, hdr::ID, id::LSEG);
    put_be32(buf, hdr::HOST_ID, host_id);
    put_be32(buf, chain::NEXT, next);
    buf[lseg::LOAD_DATA..lseg::LOAD_DATA + data.len()].copy_from_slice(data);
    let summed = LSEG_HEADER_LONGS + (data.len() / 4) as u32;
    put_be32(buf, hdr::SUMMED_LONGS, summed);
    summed
}

impl RdbBuilder {
    /// A builder for a disk of exactly this [`Geometry`] — the entry
    /// point for a caller that already has one, whether from
    /// [`synthesize_geometry`] or from an existing disk it is cloning.
    pub fn new(geometry: Geometry) -> Self {
        Self {
            geometry,
            rdsk_block: 0,
            reserved_blocks: None,
            flags: rdsk_defaults::FLAGS,
            host_id: rdsk_defaults::HOST_ID,
            specs: Vec::new(),
            filesystems: Vec::new(),
        }
    }

    /// A builder for a disk of `total_bytes` in `block_size`-byte blocks,
    /// with the geometry [`synthesize_geometry`] chooses — the entry
    /// point for a caller that has a size and no opinion about cylinders,
    /// which is most of them.
    pub fn for_size(total_bytes: u64, block_size: usize) -> Result<Self, GeometryError> {
        Ok(Self::new(synthesize_geometry(total_bytes, block_size)?))
    }

    /// Add a partition. Order matters: [`Placement::Size`] partitions are
    /// laid out in the order added, and the `PART` chain follows it.
    pub fn partition(mut self, spec: PartitionSpec) -> Self {
        self.specs.push(spec);
        self
    }

    /// Add a loadable filesystem driver. Order matters the same way
    /// [`partition`](Self::partition)'s does: the `FSHD` chain follows
    /// the order added, and each driver's `LSEG` blocks are allocated
    /// straight after its own `FSHD`.
    ///
    /// The blocks live in the RDB area alongside the `RDSK` and `PART`
    /// blocks, and a driver is *large* next to them — hundreds of
    /// blocks where a partition takes one — so this is the feature that
    /// makes the area-sizing policy earn its keep: the default area
    /// grows past `rdbtool`'s first cylinder to fit the payload, and an
    /// explicit [`reserved_blocks`](Self::reserved_blocks) too small for
    /// it is [`BuildError::RdbAreaTooSmall`] before a byte is written.
    /// (`rdbtool` 0.8.1 refuses outright here — "no space in RDB left" —
    /// having fixed the area at one cylinder when the disk was created.)
    pub fn filesystem(mut self, spec: FileSystemSpec) -> Self {
        self.filesystems.push(spec);
        self
    }

    /// Put the `RDSK` block somewhere other than block 0.
    ///
    /// Block 0 is where `rdbtool` writes it and where every image this
    /// crate has seen carries it; the format allows anywhere in the first
    /// [`RDB_LOCATION_LIMIT`] blocks, which exists so a disk can carry a
    /// foreign boot sector at block 0 and an RDB behind it. Values at or
    /// above the limit are not refused here — the RDSK simply would not
    /// be found, which [`build`](Self::build) leaves to the caller who
    /// deliberately asked for it.
    pub fn rdsk_block(mut self, lba: u32) -> Self {
        self.rdsk_block = lba;
        self
    }

    /// Reserve exactly this many blocks for the RDB area
    /// (`rdb_RDBBlocksLo..=rdb_RDBBlocksHi`), overriding the default
    /// policy [`build`](Self::build) documents.
    ///
    /// The count is from block 0, so it is also `rdb_RDBBlocksHi + 1`. A
    /// value too small for the `RDSK` and `PART` blocks the layout needs
    /// is [`BuildError::RdbAreaTooSmall`], not a silent overflow into the
    /// first partition.
    pub fn reserved_blocks(mut self, blocks: u32) -> Self {
        self.reserved_blocks = Some(blocks);
        self
    }

    /// Set `rdb_Flags`, overriding [`rdsk_defaults::FLAGS`].
    pub fn flags(mut self, flags: u32) -> Self {
        self.flags = flags;
        self
    }

    /// Set `rdb_HostID`, overriding [`rdsk_defaults::HOST_ID`].
    pub fn host_id(mut self, host_id: u32) -> Self {
        self.host_id = host_id;
        self
    }

    /// Compute the layout, validate it, and write it to `sink`.
    ///
    /// # What gets written where
    ///
    /// The `RDSK` block goes at [`rdsk_block`](Self::rdsk_block)
    /// (default 0, `rdbtool`'s choice), and the `PART` blocks follow it
    /// consecutively, chained in the order the partitions were added and
    /// terminated with [`CHAIN_END`]. `rdb_HighRDSKBlock` records the
    /// last block used; `rdb_RDBBlocksHi` records the reserved ceiling,
    /// which is normally higher — see below.
    ///
    /// Any [`filesystem`](Self::filesystem) follows the `PART` blocks:
    /// one `FSHD` block, then its driver split across `ceil(len /
    /// (block_size - 20))` consecutive `LSEG` blocks, then the next
    /// filesystem's pair, chained through `rdb_FileSysHeaderList` in the
    /// order added. The `BADB` chain head is [`CHAIN_END`] — a fresh RDB
    /// has no bad blocks, and inventing a list for a medium that has not
    /// failed yet would be a lie.
    ///
    /// # Names
    ///
    /// A [`PartitionSpec`] without a name gets the first `DH`*n* not
    /// already claimed by an *explicit* name anywhere in the build — so a
    /// mixed build of `[unnamed, "DH0", unnamed]` yields `DH1`, `DH0`,
    /// `DH2`, and never a duplicate. Two explicit names that collide are
    /// [`BuildError::DuplicateName`]; the builder renames nothing it was
    /// told.
    ///
    /// # Cylinders
    ///
    /// `rdb_LoCylinder` is the first cylinder past the reserved area and
    /// `rdb_HiCylinder` the geometry's last, both inclusive.
    /// [`Placement::Size`] partitions are packed from `rdb_LoCylinder`
    /// upward in the order added, each starting on the cylinder after the
    /// previous one's last; [`Placement::Cylinders`] partitions go
    /// exactly where they say and do not move the packing cursor past
    /// themselves except when they end above it. Every partition is then
    /// checked against the disk's end, against the RDB area, and against
    /// every other partition.
    ///
    /// # Zero partitions
    ///
    /// Legal, and supported: `rdb_PartitionList` is [`CHAIN_END`] and the
    /// result is an initialised disk with no partitions, which is what
    /// `rdbtool`'s `create` + `init` produces and what a caller
    /// partitioning in a later step wants.
    pub fn build<S: BlockSink>(&self, sink: &mut S) -> Result<RdbLayout, BuildError<S::Error>> {
        let block_size = sink.block_size();
        if !block_size_ok(block_size) {
            return Err(BuildError::UnsupportedBlockSize { block_size });
        }
        if self.geometry.block_size != block_size {
            return Err(BuildError::BlockSizeMismatch {
                geometry: self.geometry.block_size,
                sink: block_size,
            });
        }

        let layout = self.layout(sink.block_count())?;

        // Everything below is a write. Nothing above one wrote a byte,
        // which is the invariant the whole type exists for.
        let mut buf = alloc::vec![0u8; block_size];
        for (i, placed) in layout.partitions.iter().enumerate() {
            let next = match layout.partitions.get(i + 1) {
                Some(p) => p.part_block as u32,
                None => CHAIN_END,
            };
            buf.iter_mut().for_each(|b| *b = 0);
            self.fill_part(&mut buf, &self.specs[i], placed, next)?;
            sink.write_block(placed.part_block, &buf)
                .map_err(BuildError::Io)?;
        }

        // Each filesystem's LSEG chain before its FSHD, and — like the
        // PART blocks above — every one of them before the RDSK. Within
        // a chain the blocks go out head first, so a prefix of this
        // write really can leave a chain pointing at a block that has
        // not been written yet; what makes that harmless is the RDSK
        // landing last, so nothing *reaches* those chains until all of
        // them are down. (`RdbEditor::commit` writes each chain from its
        // tail, because there the target is not empty and an old RDSK is
        // still published. Here the target is empty by contract, and an
        // interrupted build leaves blocks nothing references.)
        for (i, placed) in layout.filesystems.iter().enumerate() {
            let spec = &self.filesystems[i];
            let payload = lseg_payload_bytes(block_size);
            for chunk in 0..placed.lseg_block_count as usize {
                let lba = placed.seg_list_blocks as u64 + chunk as u64;
                let next = if chunk + 1 == placed.lseg_block_count as usize {
                    CHAIN_END
                } else {
                    (lba + 1) as u32
                };
                let start = chunk * payload;
                let data = &spec.binary[start..(start + payload).min(spec.binary.len())];
                buf.iter_mut().for_each(|b| *b = 0);
                self.fill_lseg(&mut buf, spec, data, next)?;
                sink.write_block(lba, &buf).map_err(BuildError::Io)?;
            }

            let next = match layout.filesystems.get(i + 1) {
                Some(f) => f.fshd_block as u32,
                None => CHAIN_END,
            };
            buf.iter_mut().for_each(|b| *b = 0);
            self.fill_fshd(&mut buf, spec, placed, next)?;
            sink.write_block(placed.fshd_block, &buf)
                .map_err(BuildError::Io)?;
        }

        // The RDSK last: until it lands the disk has no partition table
        // at all, which is a better outcome for an interrupted build
        // than a table pointing at blocks that were never written.
        buf.iter_mut().for_each(|b| *b = 0);
        self.fill_rdsk(&mut buf, &layout)?;
        sink.write_block(layout.rdsk_block, &buf)
            .map_err(BuildError::Io)?;

        Ok(layout)
    }

    /// The whole layout, or the first reason it cannot exist. Split out
    /// of [`build`](Self::build) so that "compute everything, then write"
    /// is structural rather than a discipline: this function has no sink
    /// and so cannot write.
    fn layout<E>(&self, sink_blocks: Option<u64>) -> Result<RdbLayout, BuildError<E>> {
        let g = self.geometry;
        let cyl_blocks = g.cylinder_blocks();
        if g.cylinders == 0 || cyl_blocks == 0 {
            return Err(BuildError::EmptyGeometry { geometry: g });
        }
        let last_cylinder = g.cylinders - 1;

        // Names first: the layout errors below name the partition they
        // are about, so the names have to exist before the placement.
        let names = self.assign_names()?;

        // The RDB area. `rdb_RDBBlocksLo` is 0 rather than the RDSK's own
        // block: the area is what a repartitioner owns, and that includes
        // any block before the RDSK it might move the RDSK into.
        //
        // The budget is known up front for every chain the builder
        // writes: one RDSK, one PART per partition, and — the item this
        // arithmetic was waiting for — one FSHD plus its LSEG blocks per
        // filesystem, the LSEG count being `ceil(len / (block_size -
        // 20))` because the payload area is all of the block past
        // `lsb_LoadData`.
        let payload_bytes = lseg_payload_bytes(g.block_size) as u64;
        let lseg_counts: Vec<u64> = self
            .filesystems
            .iter()
            .map(|f| {
                let len = f.binary.len() as u64;
                (len + payload_bytes - 1) / payload_bytes
            })
            .collect();
        let fs_blocks: u64 = lseg_counts.iter().map(|n| n + 1).sum();
        let needed = 1u64 + self.specs.len() as u64 + fs_blocks;
        let reserved = match self.reserved_blocks {
            Some(n) => n as u64,
            None => self.default_reserved_blocks(cyl_blocks, needed),
        };
        if reserved < needed || reserved > u32::MAX as u64 {
            return Err(BuildError::RdbAreaTooSmall {
                needed: needed.min(u32::MAX as u64) as u32,
                available: reserved.min(u32::MAX as u64) as u32,
            });
        }
        // Every RDB block must sit inside the area, the RDSK included.
        let last_used = self.rdsk_block as u64 + needed - 1;
        if last_used >= reserved {
            return Err(BuildError::RdbAreaTooSmall {
                needed: (last_used + 1).min(u32::MAX as u64) as u32,
                available: reserved as u32,
            });
        }

        // The area is rounded up to a whole cylinder because a partition
        // can only start on one: a reserved area ending mid-cylinder
        // would leave the rest of that cylinder owned by nobody, or —
        // worse — by the first partition, which is the overlap.
        // (`div_ceil` would say this, but it is newer than this crate's
        // MSRV; `reserved >= 1` so the addition cannot be the problem.)
        let lo_cylinder = (reserved + cyl_blocks - 1) / cyl_blocks;
        if lo_cylinder > last_cylinder as u64 {
            return Err(BuildError::PartitionOverlapsRdbArea {
                name: String::from("<the disk>"),
                low_cyl: last_cylinder,
                lo_cylinder: lo_cylinder.min(u32::MAX as u64) as u32,
            });
        }
        let lo_cylinder = lo_cylinder as u32;

        // Placement, in the order added. `next_cyl` is where the next
        // sized partition starts; an explicit range pushes it past
        // itself, so mixing the two packs rather than colliding.
        let mut next_cyl = lo_cylinder;
        let mut partitions = Vec::with_capacity(self.specs.len());
        for (i, spec) in self.specs.iter().enumerate() {
            let name = names[i].clone();
            let (low_cyl, high_cyl) = match spec.placement {
                Placement::Cylinders { low, high } => {
                    if high < low {
                        return Err(BuildError::CylindersInverted {
                            name,
                            low_cyl: low,
                            high_cyl: high,
                        });
                    }
                    (low, high)
                }
                Placement::Size(bytes) => {
                    // Floored, matching rdbtool: a partition claims at
                    // most what was asked for. Below one cylinder there
                    // is nothing to claim — de_HighCyl is inclusive, so a
                    // zero-cylinder partition cannot be written — and
                    // that is an error rather than a rounded-up cylinder.
                    let cylinder_bytes = cyl_blocks * g.block_size as u64;
                    let want = bytes / cylinder_bytes;
                    if want == 0 {
                        return Err(BuildError::PartitionTooSmall {
                            name,
                            bytes,
                            cylinder_bytes,
                        });
                    }
                    let low = next_cyl as u64;
                    let high = low.saturating_add(want - 1);
                    if high > last_cylinder as u64 {
                        return Err(BuildError::PartitionPastEndOfDisk {
                            name,
                            high_cyl: high.min(u32::MAX as u64) as u32,
                            last_cylinder,
                        });
                    }
                    (low as u32, high as u32)
                }
            };

            if high_cyl > last_cylinder {
                return Err(BuildError::PartitionPastEndOfDisk {
                    name,
                    high_cyl,
                    last_cylinder,
                });
            }
            if low_cyl < lo_cylinder {
                return Err(BuildError::PartitionOverlapsRdbArea {
                    name,
                    low_cyl,
                    lo_cylinder,
                });
            }
            next_cyl = next_cyl.max(high_cyl.saturating_add(1));

            partitions.push(PlacedPartition {
                part_block: self.rdsk_block as u64 + 1 + i as u64,
                name,
                low_cyl,
                high_cyl,
                start_lba: low_cyl as u64 * cyl_blocks,
                block_len: (high_cyl as u64 - low_cyl as u64 + 1) * cyl_blocks,
            });
        }

        // Pairwise overlap. Quadratic on a list that is single digits in
        // every real layout and capped by the RDB area's block count in
        // any case; the same check [`Rdb::validate`] makes, made before
        // the image exists rather than after.
        for a in 0..partitions.len() {
            for b in a + 1..partitions.len() {
                let (pa, pb) = (&partitions[a], &partitions[b]);
                let low = pa.low_cyl.max(pb.low_cyl);
                let high = pa.high_cyl.min(pb.high_cyl);
                if low <= high {
                    return Err(BuildError::PartitionsOverlap {
                        a_name: pa.name.clone(),
                        b_name: pb.name.clone(),
                        low_cyl: low,
                        high_cyl: high,
                    });
                }
            }
        }

        // The FSHD blocks and their LSEG chains, packed consecutively
        // after the PART blocks — each FSHD immediately followed by its
        // own chain, which is both what `rdbtool` writes and what keeps
        // `lseg_block_count` meaningful as a run rather than a set.
        let mut next_block = self.rdsk_block as u64 + 1 + self.specs.len() as u64;
        let mut filesystems = Vec::with_capacity(self.filesystems.len());
        for (spec, &lsegs) in self.filesystems.iter().zip(&lseg_counts) {
            let fshd_block = next_block;
            next_block += 1;
            let seg_list_blocks = if lsegs == 0 {
                CHAIN_END
            } else {
                next_block as u32
            };
            next_block += lsegs;
            filesystems.push(PlacedFileSystem {
                fshd_block,
                dos_type: spec.dos_type,
                seg_list_blocks,
                lseg_block_count: lsegs as u32,
            });
        }

        // The target has to actually hold what the layout describes: the
        // RDB area, and every partition's last block.
        let mut needed_blocks = reserved;
        for p in &partitions {
            needed_blocks = needed_blocks.max(p.start_lba + p.block_len);
        }
        if let Some(available) = sink_blocks {
            if needed_blocks > available {
                return Err(BuildError::SinkTooSmall {
                    needed: needed_blocks,
                    available,
                });
            }
        }

        Ok(RdbLayout {
            rdsk_block: self.rdsk_block as u64,
            rdb_blocks_lo: 0,
            rdb_blocks_hi: (reserved - 1) as u32,
            high_rdsk_block: last_used as u32,
            lo_cylinder,
            hi_cylinder: last_cylinder,
            geometry: g,
            partitions,
            filesystems,
        })
    }

    /// How many blocks to reserve when the caller did not say.
    ///
    /// **Policy: `rdbtool`'s reserved area, or what the layout needs plus
    /// headroom, whichever is larger.** `rdbtool` reserves the disk's
    /// whole first cylinder (`rdb_RDBBlocksHi = rdb_CylBlocks - 1`,
    /// `rdb_LoCylinder = 1`) — a *fixed* policy that does not scale with
    /// the partition count, which is in tension with this crate's plan to
    /// "size from what will actually be stored". The tension is resolved
    /// in favour of interoperability for the common case and correctness
    /// for the uncommon one: a first cylinder is 32 blocks at the
    /// smallest Amiga geometry and 1024 at a PC-ish one, which is roomy
    /// for any realistic partition count, so matching `rdbtool` costs
    /// nothing and keeps images comparable. Where it is *not* enough —
    /// many partitions on a small-cylinder disk, or a future `FSHD`/`LSEG`
    /// payload — the area grows to what is needed plus
    /// [`RDB_HEADROOM_BLOCKS`] of slack for later edits, rather than
    /// overflowing into the first partition.
    ///
    /// [`reserved_blocks`](Self::reserved_blocks) overrides this
    /// entirely, for a caller reproducing an existing image's area or one
    /// who knows what it is about to store.
    fn default_reserved_blocks(&self, cyl_blocks: u64, needed: u64) -> u64 {
        let rdbtool_area = cyl_blocks;
        let with_headroom = (self.rdsk_block as u64 + needed).saturating_add(RDB_HEADROOM_BLOCKS);
        rdbtool_area.max(with_headroom)
    }

    /// The `pb_DriveName` for every spec, explicit ones as given and the
    /// rest assigned.
    fn assign_names<E>(&self) -> Result<Vec<String>, BuildError<E>> {
        let mut names: Vec<Option<String>> = Vec::with_capacity(self.specs.len());
        for spec in &self.specs {
            match &spec.name {
                Some(n) => {
                    if n.is_empty() || n.len() > MAX_DRIVE_NAME {
                        return Err(BuildError::InvalidName {
                            name: n.clone(),
                            max: MAX_DRIVE_NAME,
                        });
                    }
                    if names.iter().flatten().any(|other| other == n) {
                        return Err(BuildError::DuplicateName { name: n.clone() });
                    }
                    names.push(Some(n.clone()));
                }
                None => names.push(None),
            }
        }

        // Assigned names avoid every *explicit* name, not merely the ones
        // already assigned — otherwise a build of [unnamed, "DH0"] would
        // hand out DH0 twice and the collision check above would not see
        // it, the two names having been decided in different places.
        let mut next = 0u32;
        let out = names
            .iter()
            .map(|n| match n {
                Some(n) => n.clone(),
                None => loop {
                    let candidate = alloc::format!("DH{next}");
                    next += 1;
                    if !names.iter().flatten().any(|other| *other == candidate) {
                        break candidate;
                    }
                },
            })
            .collect();
        Ok(out)
    }

    /// Fill a zeroed buffer with the `RDSK` block and seal it.
    fn fill_rdsk<E>(&self, buf: &mut [u8], layout: &RdbLayout) -> Result<(), BuildError<E>> {
        let g = layout.geometry;
        put_be32(buf, hdr::ID, id::RDSK);
        put_be32(buf, rdsk::HOST_ID, self.host_id);
        put_be32(buf, rdsk::BLOCK_BYTES, g.block_size as u32);
        put_be32(buf, rdsk::FLAGS, self.flags);
        put_be32(buf, rdsk::BAD_BLOCK_LIST, CHAIN_END);
        put_be32(
            buf,
            rdsk::PARTITION_LIST,
            match layout.partitions.first() {
                Some(p) => p.part_block as u32,
                None => CHAIN_END,
            },
        );
        put_be32(
            buf,
            rdsk::FILESYS_HEADER_LIST,
            match layout.filesystems.first() {
                Some(f) => f.fshd_block as u32,
                None => CHAIN_END,
            },
        );
        put_be32(buf, rdsk::DRIVE_INIT, CHAIN_END);
        put_be32(buf, rdsk::CYLINDERS, g.cylinders);
        put_be32(buf, rdsk::SECTORS, g.sectors);
        put_be32(buf, rdsk::HEADS, g.heads);
        put_be32(buf, rdsk::INTERLEAVE, rdsk_defaults::INTERLEAVE);
        // Park on the cylinder past the last: the landing zone of a drive
        // whose data ends where the geometry does.
        put_be32(buf, rdsk::PARK, g.cylinders);
        put_be32(buf, rdsk::WRITE_PRE_COMP, g.cylinders);
        put_be32(buf, rdsk::REDUCED_WRITE, g.cylinders);
        put_be32(buf, rdsk::STEP_RATE, rdsk_defaults::STEP_RATE);
        put_be32(buf, rdsk::RDB_BLOCKS_LO, layout.rdb_blocks_lo);
        put_be32(buf, rdsk::RDB_BLOCKS_HI, layout.rdb_blocks_hi);
        put_be32(buf, rdsk::LO_CYLINDER, layout.lo_cylinder);
        put_be32(buf, rdsk::HI_CYLINDER, layout.hi_cylinder);
        put_be32(buf, rdsk::CYL_BLOCKS, g.cylinder_blocks() as u32);
        put_be32(
            buf,
            rdsk::AUTO_PARK_SECONDS,
            rdsk_defaults::AUTO_PARK_SECONDS,
        );
        put_be32(buf, rdsk::HIGH_RDSK_BLOCK, layout.high_rdsk_block);
        // The six identification fields stay zero — a deliberate
        // deviation from rdbtool, which writes "RDBTOOL"/"IMAGE"/"2012"
        // into the disk triple while leaving rdb_Flags at 0x7, i.e.
        // without DISK_ID set. By the format's own rule those bytes then
        // mean nothing, and a consumer that checks the bit (as this
        // crate's docs require) will not show them either way; writing a
        // vendor string for a disk that is not on a bus is an invention,
        // and an unflagged one is an invention a careless reader prints.
        seal_checksum(buf, HEADER_SUMMED_LONGS).map_err(BuildError::Seal)
    }

    /// Fill a zeroed buffer with one `PART` block and seal it — see
    /// [`fill_part_fields`], which the editor's add path shares.
    fn fill_part<E>(
        &self,
        buf: &mut [u8],
        spec: &PartitionSpec,
        placed: &PlacedPartition,
        next: u32,
    ) -> Result<(), BuildError<E>> {
        fill_part_fields(
            buf,
            spec,
            &placed.name,
            (placed.low_cyl, placed.high_cyl),
            next,
            PartContext {
                host_id: self.host_id,
                heads: self.geometry.heads,
                sectors: self.geometry.sectors,
                block_size: self.geometry.block_size,
            },
        );
        seal_checksum(buf, HEADER_SUMMED_LONGS).map_err(BuildError::Seal)
    }

    /// Fill a zeroed buffer with one `FSHD` block and seal it — see
    /// [`fill_fshd_fields`].
    fn fill_fshd<E>(
        &self,
        buf: &mut [u8],
        spec: &FileSystemSpec,
        placed: &PlacedFileSystem,
        next: u32,
    ) -> Result<(), BuildError<E>> {
        fill_fshd_fields(buf, spec, placed.seg_list_blocks, next);
        seal_checksum(buf, HEADER_SUMMED_LONGS).map_err(BuildError::Seal)
    }

    /// Fill a zeroed buffer with one `LSEG` block carrying `data` and
    /// seal it over the count [`fill_lseg_fields`] chose.
    fn fill_lseg<E>(
        &self,
        buf: &mut [u8],
        spec: &FileSystemSpec,
        data: &[u8],
        next: u32,
    ) -> Result<(), BuildError<E>> {
        let summed = fill_lseg_fields(buf, spec.host_id, data, next);
        seal_checksum(buf, summed).map_err(BuildError::Seal)
    }
}

/// Blocks of slack the RDB area gets past what a build actually uses,
/// when the layout needs more than `rdbtool`'s first cylinder.
///
/// Headroom exists so a later edit — one more partition, an `FSHD` and
/// its `LSEG` chain — has somewhere to go that is not the first
/// partition. Sixteen is a cylinder's worth on the smallest geometry this
/// crate produces and cheap on any disk; growing the area after the fact
/// means moving a partition, which is the expensive operation this is
/// buying insurance against.
pub const RDB_HEADROOM_BLOCKS: u64 = 16;

/// The LBA a structure added by an [`RdbEditor`] carries until a commit
/// decides where it goes.
///
/// An added `PART`, `FSHD`, `LSEG` or `BADB` block has no location yet —
/// [`RdbEditor::commit`] allocates one, and its [`CommitReport`] is the
/// answer — so every block-valued field of such a structure reads this
/// until then: [`Partition::part_block`], [`FileSysHeader::fshd_block`],
/// the entries of [`Rdb::badb_blocks`]. The u32 fields
/// ([`FileSysHeader::seg_list_blocks`], [`Rdb::bad_block_list`]) read
/// [`CHAIN_END`], which is what this truncates to and is already the
/// format's own "no block".
pub const UNPLACED_BLOCK: u64 = u64::MAX;

/// One RDB structure block as it was found on disk: its LBA and all of
/// its bytes, whole and unexamined.
///
/// Keeping the bytes — rather than rebuilding a block from the fields
/// [`Rdb`] models — *is* the preserve-unmodelled-fields property. A
/// longword this crate has never heard of survives an edit because
/// nothing ever took it apart; `de_TableSize` stays what it was found
/// as; an unknown `pb_Flags` bit is still set afterwards. The
/// alternative — write the block out of the model — is exactly how
/// AmiPart drops `rdb_DriveInit`, the controller strings and every
/// unmodelled flag bit on each write (`docs/amipart-survey.md` §6), and
/// it is the single most copyable mistake in that survey.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RawBlock {
    /// Where the block was read from. Where it will be *written* is the
    /// commit plan's business, not this struct's.
    lba: u64,
    /// The block, byte for byte.
    bytes: Vec<u8>,
}

/// Where one block of the *published* layout pointed when
/// [`RdbEditor::open`] read it — the shape of the chain that is on the
/// disk right now, and that a commit must leave walkable until the
/// `RDSK` flip replaces it wholesale.
///
/// Recorded from the block's own bytes rather than derived from the
/// order the structures were read in, because that order is exactly
/// what the edits are allowed to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OriginalLinks {
    /// The block this describes.
    lba: u64,
    /// Its `pb_Next`/`fhb_Next`/`bbb_Next`/`lsb_Next` — one field at one
    /// offset for all four chains.
    next: u32,
    /// For an `FSHD`, its `fhb_SegListBlocks`: the head of the driver
    /// chain hanging off it, which is a successor of the same kind.
    /// [`CHAIN_END`] for every other block, which has none.
    seg_list: u32,
}

impl RawBlock {
    /// What this block, as read from the disk, points at.
    fn links(&self, is_fshd: bool) -> OriginalLinks {
        OriginalLinks {
            lba: self.lba,
            next: be32(&self.bytes, chain::NEXT),
            seg_list: if is_fshd {
                be32(&self.bytes, fshd::PATCHED + fshd::SEG_LIST_INDEX * 4)
            } else {
                CHAIN_END
            },
        }
    }

    fn read<S: BlockSource>(
        disk: &mut S,
        lba: u64,
        block_size: usize,
    ) -> Result<Self, RdbError<S::Error>> {
        let mut bytes = alloc::vec![0u8; block_size];
        disk.read_block(lba, &mut bytes).map_err(RdbError::Io)?;
        Ok(Self { lba, bytes })
    }
}

/// Why an [`RdbEditor`] setter refused the edit.
///
/// Every variant describes a request the *format* cannot express, and
/// none of them involves I/O: a setter only patches the editor's
/// in-memory blocks, so a rejected edit leaves both the editor and the
/// disk exactly as they were. The failures that need a disk live in
/// [`CommitError`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// There is no partition at that index.
    NoSuchPartition {
        /// The index asked for.
        index: usize,
        /// How many partitions the RDB has, so `index` had to be below it.
        count: usize,
    },
    /// A `pb_DriveName` does not fit the 32-byte BCPL field, or is empty
    /// — the same rule [`BuildError::InvalidName`] enforces on create.
    InvalidName {
        /// The name asked for.
        name: String,
        /// The longest name the field holds.
        max: usize,
    },
    /// Another partition already carries that `pb_DriveName`. Refused
    /// rather than allowed, for the reason
    /// [`BuildError::DuplicateName`] gives: two `DH0`s is a layout whose
    /// mounts fight each other.
    DuplicateName {
        /// The name asked for.
        name: String,
    },
    /// An identification string is longer than its fixed-width `RDSK`
    /// field. These are SCSI INQUIRY copies of a fixed size — there is
    /// nowhere to put the overflow — and truncating silently would
    /// write an identity the caller did not ask for.
    IdentityTooLong {
        /// Which field, as the NDK names it (e.g. `rdb_DiskProduct`).
        field: &'static str,
        /// The value asked for.
        value: String,
        /// How many bytes the field holds.
        max: usize,
    },
    /// There is no loadable filesystem at that index.
    NoSuchFileSystem {
        /// The index asked for.
        index: usize,
        /// How many the RDB has, so `index` had to be below it.
        count: usize,
    },
    /// A cylinder range runs backwards. `de_HighCyl` is *inclusive*, so
    /// `low == high` is a legal one-cylinder extent and only `high <
    /// low` is this.
    CylindersInverted {
        /// `de_LowCyl` as asked for.
        low_cyl: u32,
        /// `de_HighCyl` as asked for — below `low_cyl`, which is the issue.
        high_cyl: u32,
    },
    /// An extent runs past the end of the medium.
    ///
    /// Decided in device blocks — the extent's last block against the
    /// block count the drive geometry gives (`rdb_Cylinders`,
    /// `rdb_HiCylinder`, `rdb_Heads`, `rdb_Sectors`) — because a
    /// partition's `de_Surfaces`/`de_BlocksPerTrack` may describe a
    /// cylinder of a different size from the drive's, and then the two
    /// cylinder numbers are not comparable at all.
    PastEndOfDisk {
        /// The last cylinder asked for, in the extent's own cylinders.
        high_cyl: u32,
        /// The last cylinder the extent's own geometry can reach on the
        /// medium: the drive's block count divided by the extent's
        /// cylinder, less one. The lower of `rdb_HiCylinder` and
        /// `rdb_Cylinders - 1` whenever the extent's geometry and the
        /// drive's agree, which is the usual case.
        last_cylinder: u32,
    },
    /// An extent reaches into the RDB area — the overlap this crate
    /// exists to refuse, in the direction that would let a filesystem
    /// scribble on the partition table.
    OverlapsRdbArea {
        /// The first cylinder asked for.
        low_cyl: u32,
        /// The first cylinder available to partitions — `rdb_LoCylinder`,
        /// or the cylinder after the one holding `rdb_RDBBlocksHi` if
        /// that is higher — in the *extent's own* cylinders, the floor
        /// itself being decided in device blocks for the reason
        /// [`PastEndOfDisk`](Self::PastEndOfDisk) gives.
        lo_cylinder: u32,
    },
    /// An extent overlaps another partition's. Reported in *device
    /// blocks*, like [`ValidationIssue::PartitionsOverlap`], because
    /// that is the unit two partitions with different `de_Surfaces`
    /// actually collide in.
    PartitionsOverlap {
        /// Index of the partition already holding the blocks.
        index: usize,
        /// Its `pb_DriveName`.
        name: String,
        /// First block both would claim.
        start: u64,
        /// How many blocks both would claim.
        len: u64,
    },
    /// A [`Placement::Size`] is below one cylinder, so it describes no
    /// partition at all — refused rather than rounded up, exactly as
    /// [`BuildError::PartitionTooSmall`] is on create.
    PartitionTooSmall {
        /// The size asked for.
        bytes: u64,
        /// One cylinder, in bytes — the smallest partition there is.
        cylinder_bytes: u64,
    },
    /// No free run of cylinders is long enough for a
    /// [`Placement::Size`] partition. The disk has the space in total,
    /// or it does not; either way there is no single gap that fits, and
    /// this crate does not move partitions to make one (see
    /// [`RdbEditor::set_extent`]).
    NoRoomForPartition {
        /// Cylinders the partition needs.
        cylinders: u64,
        /// The largest free run there is, in cylinders.
        largest_gap: u64,
    },
    /// The RDB's own geometry says a cylinder holds no blocks, so no
    /// extent can be computed against it. A damaged image, not a
    /// rejected request — [`Rdb::validate`] is how a consumer sees the
    /// rest of the damage.
    UnusableGeometry {
        /// `rdb_Heads`, or the partition's `de_Surfaces`.
        heads: u32,
        /// `rdb_Sectors`, or the partition's `de_BlocksPerTrack`.
        sectors: u32,
    },
    /// [`RdbEditor::expand_rdb_area`] was asked for a *lower*
    /// `rdb_RDBBlocksHi` than the RDB already declares.
    ///
    /// The declared area is a lease and this crate never shrinks it: a
    /// block below the old ceiling may hold a structure some other tool
    /// wrote, and handing it back is how an image loses the headroom
    /// that makes the atomic-swap placement possible in the first place
    /// (`docs/amipart-survey.md` §7.1). Shrinking is refused rather than
    /// ignored, because a caller who asked for it wanted *something* to
    /// happen.
    RdbAreaWouldShrink {
        /// `rdb_RDBBlocksHi` as it stands.
        hi: u32,
        /// The value asked for — below `hi`, which is the issue.
        new_hi: u32,
    },
    /// A partition already owns blocks the RDB area is being expanded
    /// into.
    ///
    /// AmiPart's `MSG_PV_OVERFLOW_BLOCKED` is the model for the message:
    /// name the partition in the way and the cylinder it would have to
    /// move to, because "the area is too small" without that is a dead
    /// end for the user. The predicate is stated in *blocks* rather than
    /// AmiPart's cylinders — the same arithmetic
    /// [`ValidationIssue::PartitionOverlapsRdbArea`] reports with, so an
    /// expansion is refused exactly when a parse of the result would
    /// have complained.
    RdbAreaBlocked {
        /// Index of the partition in the way.
        index: usize,
        /// Its `pb_DriveName`.
        name: String,
        /// The `rdb_RDBBlocksHi` asked for.
        new_hi: u32,
        /// The first cylinder clear of the new area, in the *blocking
        /// partition's* own `de_Surfaces`/`de_BlocksPerTrack` cylinders:
        /// move it to at least here and the expansion is permitted.
        move_to_cylinder: u32,
    },
    /// A partition starts below the `rdb_LoCylinder`
    /// [`RdbEditor::set_lo_cylinder`] was asked for, so raising the
    /// boundary would swallow it.
    LoCylinderBlocked {
        /// Index of the partition in the way.
        index: usize,
        /// Its `pb_DriveName`.
        name: String,
        /// Its `de_LowCyl` — below `lo_cylinder`, which is the issue.
        low_cyl: u32,
        /// The `rdb_LoCylinder` asked for.
        lo_cylinder: u32,
    },
    /// [`RdbEditor::set_geometry_cylinders`] would leave a partition's
    /// last cylinder past the end of the disk.
    CylindersBelowPartition {
        /// Index of the partition that would be cut off.
        index: usize,
        /// Its `pb_DriveName`.
        name: String,
        /// Its `de_HighCyl`, which the new cylinder count does not reach.
        high_cyl: u32,
        /// The `rdb_Cylinders` asked for.
        cylinders: u32,
    },
    /// A disk-level edit would claim a block the disk does not have.
    ///
    /// Raised by [`RdbEditor::expand_rdb_area`] for an area reaching
    /// past the medium, and by
    /// [`RdbEditor::set_geometry_cylinders`] both ways — for a geometry
    /// describing more disk than there is, and for one shrunk so far
    /// that the RDB area itself no longer fits inside it.
    ClaimsBlocksPastEndOfDisk {
        /// The last block the edit would claim, inclusive.
        last_block: u64,
        /// How many blocks the disk has: what the [`BlockSource`]
        /// reported to [`RdbEditor::open`], or — when it reported
        /// nothing — the RDB geometry's own
        /// `rdb_Cylinders * rdb_Heads * rdb_Sectors`.
        disk_blocks: u64,
    },
}

impl core::fmt::Display for EditError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EditError::NoSuchPartition { index, count } => write!(
                f,
                "there is no partition {index}: the RDB has {count} of them"
            ),
            EditError::InvalidName { name, max } => write!(
                f,
                "drive name {name:?} does not fit pb_DriveName: \
                 1..={max} characters are available"
            ),
            EditError::DuplicateName { name } => {
                write!(f, "another partition is already named {name:?}")
            }
            EditError::IdentityTooLong { field, value, max } => {
                write!(f, "{field} holds {max} characters, which {value:?} exceeds")
            }
            EditError::NoSuchFileSystem { index, count } => write!(
                f,
                "there is no filesystem {index}: the RDB has {count} of them"
            ),
            EditError::CylindersInverted { low_cyl, high_cyl } => write!(
                f,
                "the cylinder range runs backwards: LowCyl {low_cyl} is above HighCyl {high_cyl}"
            ),
            EditError::PastEndOfDisk {
                high_cyl,
                last_cylinder,
            } => write!(
                f,
                "cylinder {high_cyl} is past {last_cylinder}, the last one the disk has"
            ),
            EditError::OverlapsRdbArea {
                low_cyl,
                lo_cylinder,
            } => write!(
                f,
                "cylinder {low_cyl} is inside the RDB area: the first cylinder \
                 available to partitions is {lo_cylinder}"
            ),
            EditError::PartitionsOverlap {
                index,
                name,
                start,
                len,
            } => write!(
                f,
                "the extent overlaps partition {index} ({name:?}) on {len} blocks from block {start}"
            ),
            EditError::PartitionTooSmall {
                bytes,
                cylinder_bytes,
            } => write!(
                f,
                "{bytes} bytes is less than the {cylinder_bytes}-byte cylinder \
                 that is the smallest partition"
            ),
            EditError::NoRoomForPartition {
                cylinders,
                largest_gap,
            } => write!(
                f,
                "no free run of {cylinders} cylinders: the largest gap is {largest_gap}"
            ),
            EditError::UnusableGeometry { heads, sectors } => write!(
                f,
                "a cylinder of {heads} heads by {sectors} sectors holds no blocks"
            ),
            EditError::RdbAreaWouldShrink { hi, new_hi } => write!(
                f,
                "RDBBlocksHi {hi} is never shrunk: {new_hi} is below it"
            ),
            EditError::RdbAreaBlocked {
                index,
                name,
                new_hi,
                move_to_cylinder,
            } => write!(
                f,
                "partition {index} ({name:?}) is inside the blocks an RDBBlocksHi of \
                 {new_hi} would claim: move it to cylinder {move_to_cylinder} or above first"
            ),
            EditError::LoCylinderBlocked {
                index,
                name,
                low_cyl,
                lo_cylinder,
            } => write!(
                f,
                "partition {index} ({name:?}) starts at cylinder {low_cyl}, \
                 below the LoCylinder {lo_cylinder} asked for"
            ),
            EditError::CylindersBelowPartition {
                index,
                name,
                high_cyl,
                cylinders,
            } => write!(
                f,
                "a disk of {cylinders} cylinders does not reach cylinder {high_cyl}, \
                 where partition {index} ({name:?}) ends"
            ),
            EditError::ClaimsBlocksPastEndOfDisk {
                last_block,
                disk_blocks,
            } => write!(
                f,
                "block {last_block} is past the end of a disk of {disk_blocks} blocks"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for EditError {}

/// Why [`RdbEditor::commit`] refused to write, or failed while writing.
///
/// Every variant except [`Io`](Self::Io) is raised **before the first
/// block is written**, on the same terms as [`BuildError`]: the plan and
/// every block it will write are computed, checked and sealed by
/// functions that have no sink in scope, so a rejected commit leaves the
/// disk exactly as it found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitError<E> {
    /// The [`BlockSink`] failed on a write. The only variant that can
    /// leave the disk part-written — and the case
    /// [`RdbEditor::commit`]'s write ordering exists for.
    Io(E),
    /// The sink's [`block_size`](BlockSink::block_size) is not a power of
    /// two in [`MIN_BLOCK_SIZE`]`..=`[`MAX_BLOCK_SIZE`].
    UnsupportedBlockSize {
        /// What the sink said its block size was.
        block_size: usize,
    },
    /// The sink's block size is not the one the RDB was read in. Every
    /// LBA in the plan is a device block of the *original* size; writing
    /// them through a differently sized sink would address other bytes.
    BlockSizeMismatch {
        /// `rdb_BlockBytes`, as parsed.
        rdb: usize,
        /// [`BlockSink::block_size`].
        sink: usize,
    },
    /// `rdb_RDBBlocksLo` is above `rdb_RDBBlocksHi`: the area the editor
    /// is allowed to write is empty or inverted, so there is nowhere to
    /// put the structures. The read-side counterpart is
    /// [`ValidationIssue::RdbAreaInvalid`].
    RdbAreaInvalid {
        /// `rdb_RDBBlocksLo` as it was read.
        lo: u32,
        /// `rdb_RDBBlocksHi` as it was read — below `lo`, which is the issue.
        hi: u32,
    },
    /// The `RDSK` block sits above `rdb_RDBBlocksHi`, so rewriting it
    /// would mean writing outside the area the editor leases. The
    /// `RDSK` is never moved (see [`RdbEditor::commit`]), so this is a
    /// refusal rather than a relocation.
    RdskOutsideRdbArea {
        /// Where the `RDSK` was found.
        rdsk_block: u64,
        /// `rdb_RDBBlocksHi`, the last block the editor may write.
        hi: u64,
    },
    /// The RDB area cannot hold the blocks the new layout needs.
    ///
    /// The remedy is [`RdbEditor::expand_rdb_area`], which raises
    /// `rdb_RDBBlocksHi` into blocks no partition claims — the usual way
    /// out for an image created with the historically tiny default area.
    /// When a partition is in the way, that call names it and the
    /// cylinder it would have to move to
    /// ([`EditError::RdbAreaBlocked`]). The area is never shrunk, so the
    /// shortfall here is real rather than self-inflicted.
    RdbAreaTooSmall {
        /// Blocks the new layout needs inside the area.
        needed: u64,
        /// Blocks the area has (`rdb_RDBBlocksHi - rdb_RDBBlocksLo + 1`).
        available: u64,
        /// `rdb_RDBBlocksLo`.
        lo: u64,
        /// `rdb_RDBBlocksHi`, inclusive.
        hi: u64,
    },
    /// The RDB area this editor *expanded* reaches past the end of the
    /// sink.
    ///
    /// Raised only when [`RdbEditor::expand_rdb_area`] moved
    /// `rdb_RDBBlocksHi` during this editor's life, and only when the
    /// sink reports a [`block_count`](BlockSink::block_count): the
    /// expansion was already checked against what the *source* said at
    /// [`open`](RdbEditor::open), and this is the same check against the
    /// only authority present at commit time. An editor that did not
    /// touch the ceiling does not perform it — it writes nowhere it was
    /// not already entitled to write, and refusing an image whose stored
    /// `rdb_RDBBlocksHi` was always past the end of its own medium would
    /// break the no-op commit's byte-identity on exactly the damaged
    /// images this crate exists to repair.
    RdbAreaPastEndOfDisk {
        /// `rdb_RDBBlocksHi` as the expansion set it.
        hi: u32,
        /// [`BlockSink::block_count`], so `hi` had to be below it.
        block_count: u64,
    },
    /// A block would have been written outside `0..=rdb_RDBBlocksHi`.
    ///
    /// Unreachable by construction — every write goes through one
    /// private helper that owns the sink and checks the bound, and the
    /// plan only ever assigns blocks inside it — but the variant is what
    /// makes the never-touch guarantee *structural*: there is no code
    /// path that can reach the sink around the check, and if the
    /// arithmetic above it were ever wrong the write would fail here
    /// instead of landing in a partition.
    OutsideRdbArea {
        /// The block that was about to be written.
        lba: u64,
        /// `rdb_RDBBlocksHi`, the last block the editor may write.
        hi: u64,
    },
    /// Sealing a block's checksum failed — see [`SealError`]. Raised
    /// while preparing the blocks, so before any write. Reachable only
    /// for a block whose own `SummedLongs` is below
    /// [`MIN_SUMMED_LONGS`], which the editor preserves rather than
    /// replaces; such a block could not have passed the parse's
    /// checksum check in the first place.
    Seal(SealError),
}

impl<E: core::fmt::Display> core::fmt::Display for CommitError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CommitError::Io(e) => write!(f, "writing a block failed: {e}"),
            CommitError::UnsupportedBlockSize { block_size } => write!(
                f,
                "unsupported device block size {block_size}: \
                 must be a power of two in {MIN_BLOCK_SIZE}..={MAX_BLOCK_SIZE}"
            ),
            CommitError::BlockSizeMismatch { rdb, sink } => write!(
                f,
                "the RDB is in {rdb}-byte blocks but the sink writes {sink}-byte blocks"
            ),
            CommitError::RdbAreaInvalid { lo, hi } => write!(
                f,
                "RDB area is empty or inverted: RDBBlocksLo {lo} is above RDBBlocksHi {hi}"
            ),
            CommitError::RdskOutsideRdbArea { rdsk_block, hi } => write!(
                f,
                "the RDSK block at {rdsk_block} is above RDBBlocksHi {hi}, \
                 outside the area this edit may write"
            ),
            CommitError::RdbAreaTooSmall {
                needed,
                available,
                lo,
                hi,
            } => write!(
                f,
                "the RDB area {lo}..={hi} holds {available} blocks but the new layout \
                 needs {needed}; grow it with expand_rdb_area"
            ),
            CommitError::RdbAreaPastEndOfDisk { hi, block_count } => write!(
                f,
                "the expanded RDB area ends at block {hi}, past the {block_count} \
                 blocks the sink has"
            ),
            CommitError::OutsideRdbArea { lba, hi } => write!(
                f,
                "refusing to write block {lba}, which is outside the RDB area 0..={hi}"
            ),
            CommitError::Seal(e) => write!(f, "sealing a block failed: {e}"),
        }
    }
}

#[cfg(feature = "std")]
impl<E: std::error::Error + 'static> std::error::Error for CommitError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            CommitError::Io(e) => Some(e),
            CommitError::Seal(e) => Some(e),
            _ => None,
        }
    }
}

/// What one [`RdbEditor::commit`] did: where every structure landed and
/// which blocks it touched, in the order it touched them.
///
/// Returned so a caller need not re-parse the disk to learn the result —
/// though re-parsing is what this crate's own tests do, the disk being
/// the only authority on what is on the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitReport {
    /// LBA the `RDSK` block was written to — always where it was found,
    /// since the editor never moves it.
    pub rdsk_block: u64,
    /// `rdb_HighRDSKBlock` as written: the highest block the new layout
    /// occupies, recomputed rather than preserved.
    pub high_rdsk_block: u32,
    /// `rdb_RDBBlocksHi` as written — preserved from the parse, never
    /// shrunk. The area is a lease, and this is its ceiling.
    pub rdb_blocks_hi: u32,
    /// LBAs of the `PART` blocks, in chain order.
    pub part_blocks: Vec<u64>,
    /// LBAs of the `FSHD` blocks, in chain order.
    pub fshd_blocks: Vec<u64>,
    /// Every block written, **in the order it was written** — the chains
    /// first and the `RDSK` last, then the zeroed blocks. A caller
    /// reasoning about an interrupted commit reads this as the prefix
    /// that landed.
    pub blocks_written: Vec<u64>,
    /// The blocks the old layout used and the new one does not, zeroed
    /// after the `RDSK` landed. Always inside the area: a block the old
    /// layout used *outside* it belongs to whatever owns that space now,
    /// and the never-touch guarantee outranks tidiness.
    pub blocks_zeroed: Vec<u64>,
}

/// One block, prepared in full — patched and sealed — and not yet
/// written. The unit the commit computes before it touches the sink.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PreparedBlock {
    lba: u64,
    bytes: Vec<u8>,
}

/// Where each structure will be written, and what the `RDSK` will say
/// about it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CommitPlan {
    part_lbas: Vec<u64>,
    fshd_lbas: Vec<u64>,
    /// Per filesystem, in [`fshd_lbas`](Self::fshd_lbas) order.
    lseg_lbas: Vec<Vec<u64>>,
    badb_lbas: Vec<u64>,
    high_rdsk_block: u32,
    /// Blocks the old layout used, the new one does not, and which lie
    /// inside the area.
    zeroed: Vec<u64>,
}

/// The only thing in this crate that writes through a lease.
///
/// Owns the sink for the whole of a commit, so there is no path to the
/// sink that skips the bound check — the never-touch guarantee is a
/// property of the type rather than of remembering to check. The lower
/// bound is 0 rather than `rdb_RDBBlocksLo` because the `RDSK` may
/// legally sit below the area it declares (a disk carrying a foreign
/// boot sector puts `rdb_RDBBlocksLo` at 1 and the `RDSK` behind it),
/// and rewriting the `RDSK` where it was found is not a violation.
///
/// # The lease follows the *new* ceiling on an expanding commit
///
/// [`RdbEditor::expand_rdb_area`] raises `rdb_RDBBlocksHi` in the
/// editor's model, so the commit that carries the expansion leases the
/// **new**, larger window — and writes into the newly claimed blocks
/// happen *before* the `RDSK` flip publishes the larger area. That
/// inversion is deliberate and it is the whole point of the operation:
/// an expansion exists precisely so that this commit may write above the
/// old ceiling, and the `RDSK`-last rule means the block that announces
/// the new ceiling is the last one out. Leasing the *old* ceiling
/// instead would refuse every write the expansion was performed to
/// enable.
///
/// It is safe because the expansion is validated before any of it: the
/// claimed blocks were proved to intersect no partition's extent
/// ([`EditError::RdbAreaBlocked`]) and to lie inside the disk
/// ([`EditError::ClaimsBlocksPastEndOfDisk`], re-checked against the
/// sink as [`CommitError::RdbAreaPastEndOfDisk`]). So the blocks written
/// above the old ceiling belong to nobody: not to a partition, not to
/// the old chains. If the commit is interrupted before the `RDSK`
/// lands, they are simply unreferenced bytes in space the old table
/// never claimed — see [`RdbEditor::commit`] for what that costs, which
/// is nothing.
struct LeasedSink<'a, S: BlockSink> {
    sink: &'a mut S,
    hi: u64,
}

impl<'a, S: BlockSink> LeasedSink<'a, S> {
    fn write(&mut self, lba: u64, bytes: &[u8]) -> Result<(), CommitError<S::Error>> {
        if lba > self.hi {
            return Err(CommitError::OutsideRdbArea { lba, hi: self.hi });
        }
        self.sink.write_block(lba, bytes).map_err(CommitError::Io)
    }
}

/// Reseal a block over the `SummedLongs` it already declares.
///
/// The count is *preserved*, not recomputed, because it is part of what
/// the block says about itself and tools differ: 64 is this crate's and
/// `rdbtool`'s choice for `RDSK`/`PART`/`FSHD`, AmiPart writes
/// `block_size / 4` for `PART` and `RDSK`, and an `LSEG`'s count encodes
/// its payload length. Replacing it with our own would change a foreign
/// image's blocks in a way no edit asked for — and, for the `LSEG` case,
/// would change what a reader believes the driver's length to be.
fn reseal_preserving<E>(block: &mut [u8]) -> Result<(), CommitError<E>> {
    let summed_longs = be32(block, hdr::SUMMED_LONGS);
    seal_checksum(block, summed_longs).map_err(CommitError::Seal)
}

/// Edit an RDB that already exists, in place.
///
/// # The model: read, edit, write the whole area
///
/// [`open`](Self::open) parses the disk *and keeps every RDB block's
/// bytes* — `RDSK`, every `PART`, every `FSHD`, every `LSEG` of every
/// driver, every `BADB`. An edit patches the field it is about and
/// nothing else; [`commit`](Self::commit) recomputes the layout, patches
/// the chain pointers, reseals, and writes the area back.
///
/// **Raw blocks rather than a field-by-field rebuild** is the decision
/// the type turns on, and it is what makes *preserve unmodelled fields*
/// a property rather than an aspiration: `rdb_DriveInit`, the controller
/// identity strings, an unknown `rdb_Flags` bit, an unknown `pb_Flags`
/// bit, a `de_TableSize` of 19 with tail values, envec longwords past
/// anything this crate models — none of them are reconstructed, because
/// none of them are ever taken apart. Only these change on a commit:
///
/// - the fields an edit explicitly set;
/// - `pb_Next`, `fhb_Next`, `lsb_Next`, `bbb_Next` and
///   `fhb_SegListBlocks` — the chain, which the layout owns;
/// - `rdb_PartitionList`, `rdb_FileSysHeaderList`, `rdb_BadBlockList`
///   and `rdb_HighRDSKBlock` on the `RDSK`, for the same reason;
/// - each block's `ChkSum`, resealed over the `SummedLongs` the block
///   itself declares;
/// - whole blocks a *structural* edit added ([`add_partition`](Self::add_partition),
///   [`add_filesystem`](Self::add_filesystem), [`set_bad_blocks`](Self::set_bad_blocks))
///   and whole blocks one removed, which are zeroed.
///
/// A commit with no edits at all is therefore byte-identical, with one
/// documented exception: `rdb_HighRDSKBlock` is *recomputed* as the
/// high-water mark of the layout, so an image whose stored value
/// disagreed with its own blocks gets the truthful one.
///
/// # What it will not do
///
/// `rdb_RDBBlocksHi` is **never shrunk** — the declared area is a lease,
/// preserved from the parse, and the high-water mark moves instead. The
/// `RDSK` is **never moved** from where the parse found it: relocating
/// it to `rdb_RDBBlocksLo` (as AmiPart does) means a window in which two
/// checksum-valid `RDSK` blocks describe two layouts, and the format's
/// scan takes the lower one. And no write ever lands outside
/// `0..=rdb_RDBBlocksHi`, which is a structural guarantee rather than a
/// documented intention: the commit hands the sink to one private
/// writer that refuses anything else, so partition contents are provably
/// out of reach.
///
/// # Example
///
/// ```
/// use amiga_rdb::{PartitionSpec, Rdb, RdbBuilder, RdbEditor};
/// # use amiga_rdb::{BlockSink, BlockSource};
/// # struct MemDisk(Vec<u8>);
/// # fn eof() -> std::io::Error {
/// #     std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "past the end of the disk")
/// # }
/// # impl BlockSource for MemDisk {
/// #     type Error = std::io::Error;
/// #     fn block_size(&self) -> usize { 512 }
/// #     fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
/// #         let off = lba as usize * 512;
/// #         buf.copy_from_slice(self.0.get(off..off + 512).ok_or_else(eof)?);
/// #         Ok(())
/// #     }
/// #     fn block_count(&self) -> Option<u64> { Some(self.0.len() as u64 / 512) }
/// # }
/// # impl BlockSink for MemDisk {
/// #     type Error = std::io::Error;
/// #     fn block_size(&self) -> usize { 512 }
/// #     fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error> {
/// #         let off = lba as usize * 512;
/// #         self.0.get_mut(off..off + 512).ok_or_else(eof)?.copy_from_slice(buf);
/// #         Ok(())
/// #     }
/// #     fn block_count(&self) -> Option<u64> { Some(self.0.len() as u64 / 512) }
/// # }
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let mut disk = MemDisk(vec![0u8; 16 * 1024 * 1024]);
/// RdbBuilder::for_size(16 * 1024 * 1024, 512)?
///     .partition(PartitionSpec::by_size(4 * 1024 * 1024))
///     .build(&mut disk)?;
///
/// let mut editor = RdbEditor::open(&mut disk)?;
/// editor.set_name(0, "SYS")?;
/// editor.set_bootable(0, true)?;
/// editor.set_boot_priority(0, 5)?;
/// editor.commit(&mut disk)?;
///
/// let rdb = Rdb::parse(&mut disk)?;
/// assert_eq!(rdb.partitions[0].name, "SYS");
/// assert_eq!(rdb.partitions[0].boot_pri, 5);
/// assert!(rdb.validate().is_empty());
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RdbEditor {
    /// The parse model, kept in step with the raw blocks by re-parsing
    /// the block every setter touches — so [`rdb`](Self::rdb) can never
    /// disagree with what a commit would write.
    rdb: Rdb,
    block_size: usize,
    rdsk: Vec<u8>,
    parts: Vec<RawBlock>,
    fshds: Vec<RawBlock>,
    /// Per filesystem, in `fshds` order: its whole `LSEG` chain.
    lsegs: Vec<Vec<RawBlock>>,
    badbs: Vec<RawBlock>,
    /// Every block the layout occupied when [`open`](Self::open) read
    /// it, `RDSK` aside, with the pointers each one carried — kept
    /// because a structure the edits *removed* is gone from the lists
    /// above, and the blocks a commit must zero are exactly the ones
    /// this holds and the new layout does not; and because the old
    /// chain's shape is what [`plan`](Self::plan) must not disturb
    /// before the `RDSK` flip.
    ///
    /// Keyed by LBA: [`plan`](Self::plan) asks "was this block ours, and
    /// what did it point at" once per structure per fixed-point pass,
    /// and a linear scan there is quadratic in an area whose size the
    /// image chooses.
    original: BTreeMap<u64, OriginalLinks>,
    /// [`BlockSource::block_count`] as `open` found it, when the source
    /// said — the disk-size authority [`expand_rdb_area`](Self::expand_rdb_area)
    /// and [`set_geometry_cylinders`](Self::set_geometry_cylinders)
    /// check a claim against.
    disk_blocks: Option<u64>,
    /// `rdb_RDBBlocksHi` as `open` found it, so a commit can tell
    /// whether this editor moved the ceiling — the one case in which the
    /// sink's own block count is checked.
    opened_rdb_blocks_hi: u32,
}

impl RdbEditor {
    /// Parse the RDB on `disk` and read every block of it into memory.
    ///
    /// Strictly more work than [`Rdb::parse`]: the `LSEG` chains, which
    /// the parser leaves lazy behind [`Rdb::load_filesystem`], are walked
    /// here too, because an editor that regenerates the area must know
    /// where every block of it is. The whole area is a few dozen blocks
    /// plus whatever driver binaries it carries, which is the cost of
    /// being able to check a complete layout before writing any of it.
    ///
    /// Fails exactly as [`Rdb::parse`] does, plus the same failures over
    /// the `LSEG` chains: a cycle, an off-disk pointer, a wrong ID or a
    /// bad checksum is an error rather than a truncated read.
    ///
    /// # How much it can be made to hold
    ///
    /// One copy of every block of the area, and no more. Within a chain
    /// that is the chain walk's visited set; *across* chains it is
    /// [`RdbError::SharedChain`], which refuses an image whose `FSHD`s
    /// share `LSEG` blocks — without it, `k` filesystems pointing at one
    /// `L`-block chain would be retained `k` times over, an
    /// amplification the image chooses both factors of. With both, every
    /// retained block is a distinct block of the disk, so a source that
    /// reports a [`block_count`](BlockSource::block_count) bounds the
    /// memory by its own size, and one that does not is bounded by
    /// [`MAX_CHAIN_BLOCKS`] per chain.
    ///
    /// A shared chain is damage rather than a layout to support: each
    /// `FSHD` owns its chain, an edit to one would rewrite the other's
    /// driver, and there is no faithful answer for an editor to give.
    /// [`Rdb::parse`] and [`Rdb::load_filesystem`] still read such an
    /// image — one chain at a time, each read correctly — and
    /// [`Rdb::validate_seg_lists`] reports it as
    /// [`ValidationIssue::SharedLsegChain`].
    pub fn open<S: BlockSource>(disk: &mut S) -> Result<Self, RdbError<S::Error>> {
        let rdb = Rdb::parse(disk)?;
        let block_size = disk.block_size();
        let mut buf = alloc::vec![0u8; block_size];

        disk.read_block(rdb.rdsk_block, &mut buf)
            .map_err(RdbError::Io)?;
        let rdsk = buf.clone();

        let mut parts = Vec::with_capacity(rdb.partitions.len());
        for p in &rdb.partitions {
            parts.push(RawBlock::read(disk, p.part_block, block_size)?);
        }

        let mut fshds = Vec::with_capacity(rdb.filesystems.len());
        let mut lsegs = Vec::with_capacity(rdb.filesystems.len());
        // Every LSEG block already claimed by an earlier filesystem.
        // Two `FSHD`s sharing a chain is damage the parser reads
        // faithfully and the editor cannot: it would hold `k` copies of
        // the same blocks (a `k`-fold amplification an image chooses the
        // size of) and every edit would rewrite all of them. Refused,
        // clearly, rather than accepted and mishandled.
        let mut claimed: BTreeSet<u64> = BTreeSet::new();
        for f in &rdb.filesystems {
            fshds.push(RawBlock::read(disk, f.fshd_block, block_size)?);
            let mut chain = Vec::new();
            let mut shared = None;
            walk_chain(disk, f.seg_list_blocks, id::LSEG, &mut buf, |b, lba| {
                if !claimed.insert(lba) {
                    shared.get_or_insert(lba);
                    // Stop growing the copy the moment it is known to be
                    // one: an aliased chain is refused below either way.
                    return Ok(());
                }
                chain.push(RawBlock {
                    lba,
                    bytes: b.to_vec(),
                });
                Ok(())
            })?;
            if let Some(lba) = shared {
                return Err(RdbError::SharedChain { lba });
            }
            lsegs.push(chain);
        }

        let mut badbs = Vec::with_capacity(rdb.badb_blocks.len());
        for &lba in &rdb.badb_blocks {
            badbs.push(RawBlock::read(disk, lba, block_size)?);
        }

        let original = parts
            .iter()
            .map(|b| b.links(false))
            .chain(fshds.iter().map(|b| b.links(true)))
            .chain(lsegs.iter().flatten().map(|b| b.links(false)))
            .chain(badbs.iter().map(|b| b.links(false)))
            .map(|l| (l.lba, l))
            .collect();

        Ok(Self {
            disk_blocks: disk.block_count(),
            opened_rdb_blocks_hi: rdb.rdb_blocks_hi,
            rdb,
            block_size,
            rdsk,
            parts,
            fshds,
            lsegs,
            badbs,
            original,
        })
    }

    /// The RDB as it stands, edits included.
    ///
    /// The block LBAs in it (`part_block`, `fshd_block`, ...) are where
    /// the structures were *found*; where they will be written is
    /// [`commit`](Self::commit)'s answer, in its [`CommitReport`]. A
    /// structure the editor *added* has not been anywhere yet and reads
    /// [`UNPLACED_BLOCK`] until then — so [`Rdb::validate`] on this
    /// value will report such a block as outside the area, which is a
    /// statement about the commit not having happened rather than about
    /// the layout. Validate the disk after the commit; that is where
    /// this crate's own tests do it, the disk being the only authority
    /// on what is on the disk.
    ///
    /// The same goes for the chain heads — `filesys_header_list`,
    /// `bad_block_list` — which are kept in step with the lists they
    /// head after every structural edit, so the model is never
    /// self-contradictory, but which read [`CHAIN_END`] for a chain
    /// whose first block has not been placed yet. In short: **every
    /// field is the edited value; every block LBA is provisional until
    /// [`commit`](Self::commit)**.
    pub fn rdb(&self) -> &Rdb {
        &self.rdb
    }

    /// The partitions as they stand, edits included — shorthand for
    /// [`rdb`](Self::rdb)`().partitions`.
    pub fn partitions(&self) -> &[Partition] {
        &self.rdb.partitions
    }

    /// Patch partition `index`'s `PART` block and bring the model back
    /// in step with it.
    ///
    /// Re-parsing rather than updating the model field by field, so the
    /// two cannot drift: whatever a commit would write is what
    /// [`rdb`](Self::rdb) reports.
    fn edit_part<F>(&mut self, index: usize, patch: F) -> Result<(), EditError>
    where
        F: FnOnce(&mut [u8]),
    {
        if index >= self.parts.len() {
            return Err(EditError::NoSuchPartition {
                index,
                count: self.parts.len(),
            });
        }
        patch(&mut self.parts[index].bytes);
        self.rdb.partitions[index] = parse_part(&self.parts[index].bytes, self.parts[index].lba);
        Ok(())
    }

    /// Set one `DosEnvec` longword, extending `de_TableSize` if the
    /// field lies past it.
    ///
    /// **Extending exposes longwords that were absent**, and absent is
    /// not zero: `de_Baud`, `de_Control` and `de_BootBlocks` sit above
    /// the `de_TableSize` of 16 that this crate and `rdbtool` write, so
    /// setting `de_BootBlocks` on such a partition makes `de_Baud` and
    /// `de_Control` readable for the first time. Those newly exposed
    /// longwords are **zeroed**, not left as whatever slack the block
    /// carried: a mount that starts reading a field must not be handed a
    /// value nobody wrote. The field being set is written after, so
    /// setting `de_Control` alone leaves `de_Baud` at zero and
    /// `de_TableSize` at 18.
    ///
    /// Round-tripping is unaffected either way — `envec_raw` is re-read
    /// from the patched block, so it grows with `de_TableSize` and still
    /// says exactly what the block says.
    fn set_envec(&mut self, index: usize, field: usize, value: u32) -> Result<(), EditError> {
        self.edit_part(index, |bytes| {
            let table_size = be32(bytes, part::ENVIRONMENT + de::TABLE_SIZE * 4) as usize;
            if field > table_size {
                for i in table_size + 1..field {
                    put_be32(bytes, part::ENVIRONMENT + i * 4, 0);
                }
                put_be32(bytes, part::ENVIRONMENT + de::TABLE_SIZE * 4, field as u32);
            }
            put_be32(bytes, part::ENVIRONMENT + field * 4, value);
        })
    }

    /// Set `pb_DriveName`.
    ///
    /// The 32-byte BCPL field is cleared before the new name goes in, so
    /// a shorter name leaves none of the old one behind — the
    /// field-level case of not leaving what we stopped using on the
    /// disk. A name that collides with another partition's is
    /// [`EditError::DuplicateName`]; one that does not fit is
    /// [`EditError::InvalidName`].
    pub fn set_name(&mut self, index: usize, name: &str) -> Result<(), EditError> {
        if name.is_empty() || name.len() > MAX_DRIVE_NAME {
            return Err(EditError::InvalidName {
                name: String::from(name),
                max: MAX_DRIVE_NAME,
            });
        }
        if self
            .rdb
            .partitions
            .iter()
            .enumerate()
            .any(|(i, p)| i != index && p.name == name)
        {
            return Err(EditError::DuplicateName {
                name: String::from(name),
            });
        }
        let bytes = name.as_bytes();
        self.edit_part(index, |block| {
            let field = &mut block[part::DRIVE_NAME..part::DRIVE_NAME + 32];
            field.fill(0);
            field[0] = bytes.len() as u8;
            field[1..1 + bytes.len()].copy_from_slice(bytes);
        })
    }

    /// Set `de_DosType` — which filesystem is to mount the partition.
    pub fn set_dos_type(&mut self, index: usize, dos_type: u32) -> Result<(), EditError> {
        self.set_envec(index, de::DOS_TYPE, dos_type)
    }

    /// Set `de_BootPri`. Meaningful only on a bootable partition, and
    /// not forced to be one: the two are separate fields on disk and a
    /// tool that changed both would be inventing an intention.
    pub fn set_boot_priority(&mut self, index: usize, boot_pri: i32) -> Result<(), EditError> {
        self.set_envec(index, de::BOOT_PRI, boot_pri as u32)
    }

    /// Set or clear `pb_Flags` bit 0, "the ROM may boot from this
    /// partition". Every other bit is left exactly as it was — which is
    /// the whole point, AmiPart's GUI rebuilding `pb_Flags` from four
    /// checkboxes being how another tool's flag bit gets cleared by an
    /// unrelated edit (`docs/amipart-survey.md` §6).
    pub fn set_bootable(&mut self, index: usize, bootable: bool) -> Result<(), EditError> {
        self.set_part_flag(index, 0, bootable)
    }

    /// Set or clear automatic mounting. On disk the bit is the negative
    /// — `pb_Flags` bit 1 is `PBFF_NOMOUNT` — so `set_automount(i,
    /// false)` is what sets it; the parameter is phrased the way a user
    /// thinks about it rather than the way the format stores it, and
    /// [`Partition::no_automount`] still reports the bit.
    pub fn set_automount(&mut self, index: usize, automount: bool) -> Result<(), EditError> {
        self.set_part_flag(index, 1, !automount)
    }

    fn set_part_flag(&mut self, index: usize, bit: u32, set: bool) -> Result<(), EditError> {
        self.edit_part(index, |bytes| {
            let mut flags = be32(bytes, part::FLAGS);
            if set {
                flags |= 1 << bit;
            } else {
                flags &= !(1 << bit);
            }
            put_be32(bytes, part::FLAGS, flags);
        })
    }

    /// Set the whole of `pb_Flags` — the escape hatch for the bits this
    /// crate does not model.
    ///
    /// Deliberately offered, because the alternative is a caller that
    /// needs a flag we have not named reaching around the crate to poke
    /// the block itself. Note that it overwrites bits 0 and 1 as well,
    /// so a caller using this owns them too.
    pub fn set_flags_raw(&mut self, index: usize, flags: u32) -> Result<(), EditError> {
        self.edit_part(index, |bytes| put_be32(bytes, part::FLAGS, flags))
    }

    /// Set `de_Reserved` — blocks held back at the start of the
    /// partition for the boot block.
    pub fn set_reserved(&mut self, index: usize, blocks: u32) -> Result<(), EditError> {
        self.set_envec(index, de::RESERVED, blocks)
    }

    /// Set `de_PreAlloc` — blocks held back at the end.
    pub fn set_pre_alloc(&mut self, index: usize, blocks: u32) -> Result<(), EditError> {
        self.set_envec(index, de::PRE_ALLOC, blocks)
    }

    /// Set `de_Interleave` — the filesystem's interleave, not the
    /// drive's `rdb_Interleave`.
    pub fn set_interleave(&mut self, index: usize, interleave: u32) -> Result<(), EditError> {
        self.set_envec(index, de::INTERLEAVE, interleave)
    }

    /// Set `de_NumBuffers` — cache buffers the handler allocates at mount.
    pub fn set_num_buffers(&mut self, index: usize, buffers: u32) -> Result<(), EditError> {
        self.set_envec(index, de::NUM_BUFFERS, buffers)
    }

    /// Set `de_BufMemType` — which memory those buffers want.
    pub fn set_buf_mem_type(&mut self, index: usize, mem_type: u32) -> Result<(), EditError> {
        self.set_envec(index, de::BUF_MEM_TYPE, mem_type)
    }

    /// Set `de_MaxTransfer` — the largest transfer the driver may issue
    /// in one go.
    pub fn set_max_transfer(&mut self, index: usize, max_transfer: u32) -> Result<(), EditError> {
        self.set_envec(index, de::MAX_TRANSFER, max_transfer)
    }

    /// Set `de_Mask` — the DMA-reachable address mask.
    pub fn set_mask(&mut self, index: usize, mask: u32) -> Result<(), EditError> {
        self.set_envec(index, de::MASK, mask)
    }

    /// Set `de_Baud`, **extending `de_TableSize` to 17** if the envec
    /// stopped short of it.
    ///
    /// This and the two setters below are the only ones that can move
    /// `de_TableSize`, and extending it *exposes longwords that were
    /// absent* — absent being a different thing from zero, which is why
    /// [`Partition::baud`] and its neighbours are [`Option`]s. Any
    /// longword that becomes readable on the way is **zeroed** rather
    /// than left holding whatever slack the block carried: a handler
    /// that starts reading a field must not be handed a value nobody
    /// wrote. Setting `de_Control` alone therefore leaves `de_Baud` zero
    /// and `de_TableSize` at 18, and `envec_raw` — re-read from the
    /// patched block — grows to say exactly that.
    pub fn set_baud(&mut self, index: usize, baud: u32) -> Result<(), EditError> {
        self.set_envec(index, de::BAUD, baud)
    }

    /// Set `de_Control`, extending `de_TableSize` to 18 if needed.
    pub fn set_control(&mut self, index: usize, control: u32) -> Result<(), EditError> {
        self.set_envec(index, de::CONTROL, control)
    }

    /// Set `de_BootBlocks`, extending `de_TableSize` to 19 if needed.
    pub fn set_boot_blocks(&mut self, index: usize, blocks: u32) -> Result<(), EditError> {
        self.set_envec(index, de::BOOT_BLOCKS, blocks)
    }

    /// Set `rdb_DiskVendor`/`Product`/`Revision` and turn
    /// [`rdb_flags::DISK_ID`] on.
    ///
    /// The flag comes with the strings deliberately: without it those
    /// bytes mean nothing by the format's own rule, so writing them
    /// unflagged — which is what `rdbtool` does — plants an identity a
    /// careful reader will not show and a careless one will.
    /// [`set_rdb_flags`](Self::set_rdb_flags) can clear the bit again for
    /// a caller reproducing exactly that.
    ///
    /// The three fields are 8, 16 and 4 bytes; anything longer is
    /// [`EditError::IdentityTooLong`] rather than a silent truncation.
    pub fn set_disk_identity(
        &mut self,
        vendor: &str,
        product: &str,
        revision: &str,
    ) -> Result<(), EditError> {
        self.set_identity(
            [
                ("rdb_DiskVendor", rdsk::DISK_VENDOR, vendor),
                ("rdb_DiskProduct", rdsk::DISK_PRODUCT, product),
                ("rdb_DiskRevision", rdsk::DISK_REVISION, revision),
            ],
            rdb_flags::DISK_ID,
        )
    }

    /// Set `rdb_ControllerVendor`/`Product`/`Revision` and turn
    /// [`rdb_flags::CTRLR_ID`] on — the controller's identity, on exactly
    /// the terms [`set_disk_identity`](Self::set_disk_identity)
    /// describes.
    pub fn set_controller_identity(
        &mut self,
        vendor: &str,
        product: &str,
        revision: &str,
    ) -> Result<(), EditError> {
        self.set_identity(
            [
                ("rdb_ControllerVendor", rdsk::CONTROLLER_VENDOR, vendor),
                ("rdb_ControllerProduct", rdsk::CONTROLLER_PRODUCT, product),
                (
                    "rdb_ControllerRevision",
                    rdsk::CONTROLLER_REVISION,
                    revision,
                ),
            ],
            rdb_flags::CTRLR_ID,
        )
    }

    fn set_identity(
        &mut self,
        fields: [(&'static str, (usize, usize), &str); 3],
        flag: u32,
    ) -> Result<(), EditError> {
        // Checked in full before anything is written, so a rejected
        // triple leaves none of its three fields changed.
        for (name, (_, len), value) in fields {
            if value.len() > len {
                return Err(EditError::IdentityTooLong {
                    field: name,
                    value: String::from(value),
                    max: len,
                });
            }
        }
        for (_, (off, len), value) in fields {
            // Space-padded ASCII, as SCSI INQUIRY writes it — not BCPL,
            // and not NUL-terminated.
            self.rdsk[off..off + len].fill(b' ');
            self.rdsk[off..off + value.len()].copy_from_slice(value.as_bytes());
        }
        let flags = be32(&self.rdsk, rdsk::FLAGS) | flag;
        put_be32(&mut self.rdsk, rdsk::FLAGS, flags);
        self.resync_rdsk();
        Ok(())
    }

    /// Set `rdb_Flags` wholesale — including the
    /// [`DISK_ID`](rdb_flags::DISK_ID) and
    /// [`CTRLR_ID`](rdb_flags::CTRLR_ID) bits the identity setters
    /// manage, and any bit this crate does not name.
    pub fn set_rdb_flags(&mut self, flags: u32) {
        put_be32(&mut self.rdsk, rdsk::FLAGS, flags);
        self.resync_rdsk();
    }

    /// Bring the modelled `RDSK` fields back in step with the block, the
    /// way [`edit_part`](Self::edit_part) does for a `PART`.
    fn resync_rdsk(&mut self) {
        self.rdb.flags = be32(&self.rdsk, rdsk::FLAGS);
        self.rdb.disk_vendor = padded_ascii(&self.rdsk, rdsk::DISK_VENDOR);
        self.rdb.disk_product = padded_ascii(&self.rdsk, rdsk::DISK_PRODUCT);
        self.rdb.disk_revision = padded_ascii(&self.rdsk, rdsk::DISK_REVISION);
        self.rdb.controller_vendor = padded_ascii(&self.rdsk, rdsk::CONTROLLER_VENDOR);
        self.rdb.controller_product = padded_ascii(&self.rdsk, rdsk::CONTROLLER_PRODUCT);
        self.rdb.controller_revision = padded_ascii(&self.rdsk, rdsk::CONTROLLER_REVISION);
    }

    // ---- disk-level edits: the geometry and the RDB area ------------

    /// How many blocks the disk has, for an edit that claims some of
    /// them: what the [`BlockSource`] reported to
    /// [`open`](Self::open), the RDB geometry's own
    /// `rdb_Cylinders * rdb_Heads * rdb_Sectors`, or the lower of the
    /// two when both are known.
    ///
    /// `None` only when the source declined to say *and* the geometry
    /// multiplies out to nothing, in which case there is no bound to
    /// check against and the edit is taken at its word.
    fn disk_block_limit(&self) -> Option<u64> {
        let geometry = (self.rdb.cylinders as u64)
            .saturating_mul(self.rdb.heads as u64)
            .saturating_mul(self.rdb.sectors as u64);
        match (self.disk_blocks, geometry) {
            (Some(n), 0) => Some(n),
            (Some(n), g) => Some(n.min(g)),
            (None, 0) => None,
            (None, g) => Some(g),
        }
    }

    /// Grow the RDB area: raise `rdb_RDBBlocksHi` to `new_hi`.
    ///
    /// The realistic way to edit an old image, whose RDB area is one
    /// cylinder or less and cannot hold a modern filesystem driver at
    /// all. It is the answer to
    /// [`CommitError::RdbAreaTooSmall`]: expand, then re-try the add.
    ///
    /// # The predicate
    ///
    /// Permitted **iff the blocks being claimed — `old_hi + 1 ..=
    /// new_hi` — intersect no partition's extent**. That is AmiPart's
    /// rule (`docs/amipart-survey.md` §7.3) stated in device blocks
    /// rather than its cylinders, using the same arithmetic
    /// [`ValidationIssue::PartitionOverlapsRdbArea`] reports with — so
    /// an expansion is refused exactly when a parse of the result would
    /// have complained about it, and no edit can produce a layout
    /// `validate()` would then flag. On refusal
    /// [`EditError::RdbAreaBlocked`] names the partition in the way
    /// *and* the cylinder it would have to move to, AmiPart's
    /// `MSG_PV_OVERFLOW_BLOCKED` being the model: "there is no room" on
    /// its own is a dead end for whoever has to act on it.
    ///
    /// Only the *newly claimed* blocks are checked. A partition already
    /// overlapping the old area is damage this crate did not cause and
    /// will not deepen — [`Rdb::validate`] is where a consumer sees it —
    /// and requiring it to be repaired first would block the expansion
    /// that is very often how it gets repaired.
    ///
    /// The area may not reach past the end of the disk either
    /// ([`EditError::ClaimsBlocksPastEndOfDisk`]) — the bound being the
    /// lower of the [`BlockSource::block_count`] reported to
    /// [`open`](Self::open) and the RDB geometry's own total — and
    /// [`commit`](Self::commit) checks the same thing again against the
    /// sink.
    ///
    /// # What it does not touch
    ///
    /// `rdb_HighRDSKBlock` — the high-water mark of what the layout
    /// actually occupies, which a commit recomputes. Expanding the area
    /// makes room; it does not itself use any of it. And
    /// `rdb_LoCylinder`, which is the *other* lever
    /// ([`set_lo_cylinder`](Self::set_lo_cylinder)): the two are
    /// independent, and on the common layout — a first partition
    /// starting well above the area — this one alone is enough.
    ///
    /// # Never the other way
    ///
    /// A `new_hi` below the current one is
    /// [`EditError::RdbAreaWouldShrink`], not a shrink: the declared
    /// area is a lease. Equal is a no-op and succeeds.
    pub fn expand_rdb_area(&mut self, new_hi: u32) -> Result<(), EditError> {
        let hi = self.rdb.rdb_blocks_hi;
        if new_hi < hi {
            return Err(EditError::RdbAreaWouldShrink { hi, new_hi });
        }
        if new_hi == hi {
            return Ok(());
        }
        if let Some(disk_blocks) = self.disk_block_limit() {
            if new_hi as u64 >= disk_blocks {
                return Err(EditError::ClaimsBlocksPastEndOfDisk {
                    last_block: new_hi as u64,
                    disk_blocks,
                });
            }
        }

        // The blocks being claimed, inclusive at both ends.
        let (claim_lo, claim_hi) = (hi as u64 + 1, new_hi as u64);
        for (index, p) in self.rdb.partitions.iter().enumerate() {
            // An inverted range claims no blocks, which every overlap
            // check in this crate already skips.
            if p.block_len == 0 {
                continue;
            }
            let end = p.start_lba.saturating_add(p.block_len);
            if p.start_lba <= claim_hi && end > claim_lo {
                // Where it would have to start instead, in its own
                // cylinders — `de_Surfaces`/`de_BlocksPerTrack` are per
                // partition and need not match the drive's. `max(1)`
                // cannot bite: a zero-block cylinder gives a zero-block
                // extent, which the `continue` above already took.
                let cyl_blocks = p.cylinder_blocks.max(1);
                let move_to_cylinder = ((claim_hi + cyl_blocks) / cyl_blocks) as u32;
                return Err(EditError::RdbAreaBlocked {
                    index,
                    name: p.name.clone(),
                    new_hi,
                    move_to_cylinder,
                });
            }
        }

        put_be32(&mut self.rdsk, rdsk::RDB_BLOCKS_HI, new_hi);
        self.rdb.rdb_blocks_hi = new_hi;
        Ok(())
    }

    /// Set `rdb_LoCylinder`, the first cylinder available to partitions
    /// — the *second* area lever.
    ///
    /// AmiPart handles a too-small area at this level rather than at
    /// `rdb_RDBBlocksHi`, because for it the reserved area simply *is*
    /// everything below `rdb_LoCylinder`
    /// (`docs/amipart-survey.md` §7.3). Both levers exist here because
    /// they are genuinely different operations:
    /// [`expand_rdb_area`](Self::expand_rdb_area) can often be done with
    /// no partition change at all, while this one is what a caller
    /// reproducing another tool's layout — or one about to
    /// [`add_partition`](Self::add_partition) and wanting the boundary
    /// stated — reaches for. Raising both is the usual pair on an image
    /// whose first partition starts immediately above a one-cylinder
    /// area.
    ///
    /// Refused if any partition starts below the new value
    /// ([`EditError::LoCylinderBlocked`], naming it): the boundary would
    /// otherwise swallow a partition whole. *Lowering* it is permitted
    /// and passes the same predicate trivially — it hands cylinders back
    /// to the partition area, and where partitions may actually start is
    /// still floored by the cylinder after `rdb_RDBBlocksHi`, which
    /// [`add_partition`](Self::add_partition) computes rather than
    /// trusting this field for.
    pub fn set_lo_cylinder(&mut self, lo_cylinder: u32) -> Result<(), EditError> {
        for (index, p) in self.rdb.partitions.iter().enumerate() {
            if p.block_len != 0 && p.low_cyl < lo_cylinder {
                return Err(EditError::LoCylinderBlocked {
                    index,
                    name: p.name.clone(),
                    low_cyl: p.low_cyl,
                    lo_cylinder,
                });
            }
        }
        put_be32(&mut self.rdsk, rdsk::LO_CYLINDER, lo_cylinder);
        self.rdb.lo_cylinder = lo_cylinder;
        Ok(())
    }

    /// Set `rdb_Cylinders`, and `rdb_HiCylinder` with it — AmiPart's
    /// `INIT NEWGEO`, the disk-got-bigger case.
    ///
    /// The image was cloned onto a larger medium and the RDB still
    /// describes the old one, so every cylinder past the old count is
    /// unreachable. `rdb_Heads`, `rdb_Sectors` and `rdb_LoCylinder` are
    /// kept — the geometry's *shape* is what makes existing extents mean
    /// what they meant — and `rdb_HiCylinder` becomes `cylinders - 1`.
    ///
    /// **`rdb_Park`, `rdb_WritePreComp` and `rdb_ReducedWrite` are left
    /// alone**, though tools that create an RDB commonly set all three
    /// to the cylinder count. AmiPart rewrites them here; this crate does
    /// not, on the same rule that keeps `rdb_DriveInit` and the
    /// controller strings intact — an edit changes what it was asked to
    /// change. [`Rdb`] exposes all three for a caller who wants them to
    /// track.
    ///
    /// # What it refuses
    ///
    /// Shrinking below any partition's last *block*
    /// ([`EditError::CylindersBelowPartition`], naming it — in blocks
    /// because a partition's `de_Surfaces`/`de_BlocksPerTrack` cylinder
    /// need not be the drive's), and shrinking
    /// so far that the RDB area itself no longer fits the disk
    /// ([`EditError::ClaimsBlocksPastEndOfDisk`]). Growing past the block
    /// count the source reported is refused by the same variant; when
    /// the source declined to say, the new count is taken at its word,
    /// since a claim about the medium is exactly what this call is.
    pub fn set_geometry_cylinders(&mut self, cylinders: u32) -> Result<(), EditError> {
        let cyl_blocks = self.rdb.heads as u64 * self.rdb.sectors as u64;
        if cyl_blocks == 0 {
            return Err(EditError::UnusableGeometry {
                heads: self.rdb.heads,
                sectors: self.rdb.sectors,
            });
        }
        let hi_cylinder = cylinders.saturating_sub(1);
        let total = (cylinders as u64).saturating_mul(cyl_blocks);
        // In device blocks, for the reason [`EditError::PastEndOfDisk`]
        // gives: a partition's own cylinder need not be the drive's, so
        // comparing its `de_HighCyl` against the drive's new last
        // cylinder can truncate a divergent-geometry partition while the
        // guard reads as passed. Its last block against the medium's
        // block count is the comparison that means something.
        for (index, p) in self.rdb.partitions.iter().enumerate() {
            if p.block_len != 0 && p.start_lba.saturating_add(p.block_len) > total {
                return Err(EditError::CylindersBelowPartition {
                    index,
                    name: p.name.clone(),
                    high_cyl: p.high_cyl,
                    cylinders,
                });
            }
        }
        // The area is inside the disk or the disk is not the disk. This
        // also disposes of `cylinders == 0`, whose zero blocks cannot
        // hold an area that always has at least block 0 in it.
        if total <= self.rdb.rdb_blocks_hi as u64 {
            return Err(EditError::ClaimsBlocksPastEndOfDisk {
                last_block: self.rdb.rdb_blocks_hi as u64,
                disk_blocks: total,
            });
        }
        if let Some(disk_blocks) = self.disk_blocks {
            if total > disk_blocks {
                return Err(EditError::ClaimsBlocksPastEndOfDisk {
                    last_block: total - 1,
                    disk_blocks,
                });
            }
        }
        put_be32(&mut self.rdsk, rdsk::CYLINDERS, cylinders);
        put_be32(&mut self.rdsk, rdsk::HI_CYLINDER, hi_cylinder);
        self.rdb.cylinders = cylinders;
        self.rdb.hi_cylinder = hi_cylinder;
        Ok(())
    }

    // ---- structural edits: add, remove, resize ---------------------

    /// The disk-level values a new `PART` block copies out of the
    /// `RDSK`, so an added partition and a created one are filled by the
    /// same function from the same four numbers.
    fn part_context(&self) -> PartContext {
        PartContext {
            host_id: self.rdb.host_id,
            heads: self.rdb.heads,
            sectors: self.rdb.sectors,
            block_size: self.block_size,
        }
    }

    /// The last cylinder a partition may use: the lower of
    /// `rdb_HiCylinder` and `rdb_Cylinders - 1`.
    ///
    /// The two disagree on real images — AmiPart clamps the same pair on
    /// read, naming `lide` as a tool that writes the off-by-one — and
    /// the lower is the only safe answer: a partition placed past the
    /// medium's last cylinder points at bytes that are not there.
    fn last_cylinder(&self) -> u32 {
        self.rdb
            .hi_cylinder
            .min(self.rdb.cylinders.saturating_sub(1))
    }

    /// How many device blocks the drive geometry says the medium has:
    /// [`last_cylinder`](Self::last_cylinder) plus one, times the
    /// *drive's* `rdb_Heads * rdb_Sectors`.
    ///
    /// Blocks rather than cylinders because a cylinder is not one unit
    /// on an RDB: `de_Surfaces` and `de_BlocksPerTrack` are per
    /// partition and are allowed to disagree with the drive's, so
    /// "cylinder 100" means a different block on a partition with a
    /// divergent geometry than it does on the drive. Comparing the two
    /// numbers directly — which this crate used to do — lets such a
    /// partition extend past the end of the medium while every check
    /// passes. Device blocks are the unit both sides genuinely share,
    /// and the one [`Rdb::validate`] already thinks in.
    fn disk_blocks_from_geometry(&self) -> u64 {
        let cyl_blocks = self.rdb.heads as u64 * self.rdb.sectors as u64;
        (self.last_cylinder() as u64 + 1).saturating_mul(cyl_blocks)
    }

    /// The first device block a partition may claim: `rdb_LoCylinder`
    /// in the *drive's* cylinders, or the block after `rdb_RDBBlocksHi`
    /// when the `RDSK` understates its own area.
    ///
    /// In blocks for [`disk_blocks_from_geometry`](Self::disk_blocks_from_geometry)'s
    /// reason: `rdb_LoCylinder` is a drive cylinder and the extent being
    /// checked may measure cylinders differently, so the floor is
    /// converted to the extent's own unit only where it is *reported*.
    fn first_partition_block(&self) -> u64 {
        let drive_cyl_blocks = self.rdb.heads as u64 * self.rdb.sectors as u64;
        (self.rdb.lo_cylinder as u64)
            .saturating_mul(drive_cyl_blocks)
            .max(self.rdb.rdb_blocks_hi as u64 + 1)
    }

    /// [`first_partition_block`](Self::first_partition_block) rounded up
    /// into cylinders of `cyl_blocks` blocks — the number an error
    /// message about a `de_LowCyl` has to be in to mean anything.
    fn first_partition_cylinder(&self, cyl_blocks: u64) -> u32 {
        if cyl_blocks == 0 {
            return self.rdb.lo_cylinder;
        }
        // `div_ceil` would say this, but it is newer than the MSRV.
        // The addition saturates: `cyl_blocks` is two parsed longwords
        // multiplied together and can sit within a block of the top of
        // the range.
        let first = self.first_partition_block();
        let cylinder = first.saturating_add(cyl_blocks - 1) / cyl_blocks;
        cylinder.min(u32::MAX as u64) as u32
    }

    /// Does this extent reach into the RDB area? The same arithmetic
    /// [`Rdb::validate`] performs, in the same unit — device blocks —
    /// so an edit refuses exactly what a parse would report.
    fn overlaps_rdb_area(&self, start_lba: u64, block_len: u64) -> bool {
        let (lo, hi) = (self.rdb.rdb_blocks_lo as u64, self.rdb.rdb_blocks_hi as u64);
        lo <= hi && block_len != 0 && start_lba <= hi && start_lba.saturating_add(block_len) > lo
    }

    /// Refuse `low_cyl..=high_cyl` unless it is a legal extent on this
    /// disk: forwards, inside the disk, clear of the RDB area, and clear
    /// of every partition except `exclude` (the one being resized).
    fn check_extent(
        &self,
        exclude: Option<usize>,
        (low_cyl, high_cyl): (u32, u32),
        cyl_blocks: u64,
    ) -> Result<(), EditError> {
        if high_cyl < low_cyl {
            return Err(EditError::CylindersInverted { low_cyl, high_cyl });
        }
        let start_lba = (low_cyl as u64).saturating_mul(cyl_blocks);
        let block_len = (high_cyl as u64 - low_cyl as u64 + 1).saturating_mul(cyl_blocks);
        // In blocks, not cylinders: `cyl_blocks` is the *extent's* own
        // cylinder and the drive's may be a different size entirely, so
        // `high_cyl` and `rdb_Cylinders - 1` are not comparable
        // quantities. The reported `last_cylinder` is therefore the last
        // cylinder *this extent's geometry* can reach on the medium,
        // derived from the block count — which is the same number the
        // drive's own last cylinder is, whenever the two geometries
        // agree.
        let disk_blocks = self.disk_blocks_from_geometry();
        if start_lba.saturating_add(block_len) > disk_blocks {
            let last_cylinder = (disk_blocks / cyl_blocks).saturating_sub(1);
            return Err(EditError::PastEndOfDisk {
                high_cyl,
                last_cylinder: last_cylinder.min(u32::MAX as u64) as u32,
            });
        }
        // The low end is the same question in the same unit: the floor
        // `rdb_LoCylinder` sets is a *drive* cylinder, so an extent
        // measured in cylinders of another size is compared to it as
        // blocks and only reported in cylinders.
        if start_lba < self.first_partition_block() || self.overlaps_rdb_area(start_lba, block_len)
        {
            return Err(EditError::OverlapsRdbArea {
                low_cyl,
                lo_cylinder: self.first_partition_cylinder(cyl_blocks),
            });
        }
        for (index, p) in self.rdb.partitions.iter().enumerate() {
            // An inverted extent claims no blocks — the length
            // `parse_part` gives it and the one every overlap check in
            // this crate already skips — so it cannot be collided with.
            if Some(index) == exclude || p.block_len == 0 {
                continue;
            }
            let start = start_lba.max(p.start_lba);
            let end = start_lba
                .saturating_add(block_len)
                .min(p.start_lba.saturating_add(p.block_len));
            if start < end {
                return Err(EditError::PartitionsOverlap {
                    index,
                    name: p.name.clone(),
                    start,
                    len: end - start,
                });
            }
        }
        Ok(())
    }

    /// Turn a [`Placement::Size`] into a cylinder range.
    ///
    /// **First fit, from the lowest free cylinder upward.** The gaps
    /// exist because explicit ranges and deletes both leave them, and
    /// the choice between first fit and best fit is a real one: best fit
    /// would keep large runs intact, but it makes where a partition
    /// lands depend on partitions the caller was not thinking about,
    /// and on a disk with single-digit partitions there is nothing to
    /// optimise. First fit is deterministic, explains itself to a user
    /// ("it went in the first hole big enough"), reproduces `rdbtool`'s
    /// and AmiPart's pack-after-the-last behaviour on the usual
    /// hole-free layout, and fills a hole a delete left instead of
    /// growing the disk's used tail — which is the whole point of having
    /// a policy. A caller who wants some other answer says
    /// [`Placement::Cylinders`], which is exactly why the pair exists.
    ///
    /// The count is **floored**, as on create: a size is a ceiling, and
    /// below one cylinder there is no partition to write.
    ///
    /// `cyl_blocks` comes from two parsed longwords and so is only
    /// bounded by `u32::MAX * u32::MAX`: a hostile `rdb_Heads` and
    /// `rdb_Sectors` make a cylinder larger than the address space.
    /// The multiplication saturates rather than overflowing (a debug
    /// panic, and worse in release: a wrap to zero and then a division
    /// by it), which lands such a geometry in
    /// [`EditError::PartitionTooSmall`] — the honest answer, since no
    /// size a caller can express reaches one of those cylinders.
    fn place_by_size(&self, bytes: u64, cyl_blocks: u64) -> Result<(u32, u32), EditError> {
        if cyl_blocks == 0 {
            return Err(EditError::UnusableGeometry {
                heads: self.rdb.heads,
                sectors: self.rdb.sectors,
            });
        }
        let cylinder_bytes = cyl_blocks.saturating_mul(self.block_size as u64);
        let want = bytes / cylinder_bytes;
        if want == 0 {
            return Err(EditError::PartitionTooSmall {
                bytes,
                cylinder_bytes,
            });
        }

        let last = self.last_cylinder();
        let mut used: Vec<(u32, u32)> = self
            .rdb
            .partitions
            .iter()
            .filter(|p| p.block_len != 0)
            .map(|p| (p.low_cyl, p.high_cyl))
            .collect();
        used.sort_unstable();

        let mut cursor = self.first_partition_cylinder(cyl_blocks);
        let mut largest_gap = 0u64;
        let fits = |from: u32, to: u32, largest: &mut u64| {
            let gap = to as u64 - from as u64 + 1;
            *largest = (*largest).max(gap);
            // `want <= gap` and `gap` ends at `to`, so the sum is a
            // cylinder this disk has and the cast cannot truncate.
            (gap >= want).then(|| (from, (from as u64 + want - 1) as u32))
        };
        for (low, high) in used {
            if low > cursor {
                if let Some(range) = fits(cursor, low - 1, &mut largest_gap) {
                    return Ok(range);
                }
            }
            cursor = cursor.max(high.saturating_add(1));
        }
        if cursor <= last {
            if let Some(range) = fits(cursor, last, &mut largest_gap) {
                return Ok(range);
            }
        }
        Err(EditError::NoRoomForPartition {
            cylinders: want,
            largest_gap,
        })
    }

    /// The `pb_DriveName` for a new partition: the one asked for, or the
    /// first `DH`*n* no existing partition carries.
    ///
    /// The same rule [`RdbBuilder::build`] follows — assigned names
    /// avoid every name already on the disk, explicit ones are never
    /// renamed, and a collision is refused rather than resolved.
    fn assign_name(&self, wanted: Option<&str>) -> Result<String, EditError> {
        match wanted {
            Some(name) => {
                if name.is_empty() || name.len() > MAX_DRIVE_NAME {
                    return Err(EditError::InvalidName {
                        name: String::from(name),
                        max: MAX_DRIVE_NAME,
                    });
                }
                if self.rdb.partitions.iter().any(|p| p.name == name) {
                    return Err(EditError::DuplicateName {
                        name: String::from(name),
                    });
                }
                Ok(String::from(name))
            }
            None => {
                let mut n = 0u32;
                loop {
                    let candidate = alloc::format!("DH{n}");
                    if !self.rdb.partitions.iter().any(|p| p.name == candidate) {
                        return Ok(candidate);
                    }
                    n += 1;
                }
            }
        }
    }

    /// Add a partition, returning its index.
    ///
    /// The [`PartitionSpec`] is the builder's, so a partition created on
    /// a fresh disk and one added to an existing table are described the
    /// same way and written by the same code — including the
    /// [`envec_defaults`] and the `DH`*n* naming rule.
    ///
    /// # Where it goes
    ///
    /// [`Placement::Cylinders`] puts it exactly where it says, and
    /// [`Placement::Size`] into the first free run of cylinders long
    /// enough — **first fit**, from the lowest free cylinder upward.
    /// Best fit would keep large runs intact, but it makes where a
    /// partition lands depend on partitions the caller was not thinking
    /// about, and on a disk with single-digit partitions there is
    /// nothing to optimise; first fit is deterministic, explains itself
    /// ("it went in the first hole big enough"), reproduces `rdbtool`'s
    /// and AmiPart's pack-after-the-last behaviour on the usual
    /// hole-free layout, and fills a hole a delete left instead of
    /// growing the disk's used tail. A caller who wants some other
    /// answer says [`Placement::Cylinders`], which is exactly why the
    /// pair exists. Either way the extent is refused unless it is clear of every
    /// other partition, of the RDB area, and of the end of the disk;
    /// there is no overlapping outcome to opt into.
    ///
    /// # Where its `PART` block goes
    ///
    /// Nowhere yet: [`commit`](Self::commit) allocates one, preferring a
    /// block neither the old nor the new layout uses, and reports it in
    /// [`CommitReport::part_blocks`]. Until then
    /// [`Partition::part_block`] reads [`UNPLACED_BLOCK`]. Whether the
    /// RDB area *has* a free block is therefore a commit-time answer too
    /// — [`CommitError::RdbAreaTooSmall`], before a byte is written.
    pub fn add_partition(&mut self, spec: PartitionSpec) -> Result<usize, EditError> {
        let ctx = self.part_context();
        let cyl_blocks = ctx.heads as u64 * ctx.sectors as u64;
        if cyl_blocks == 0 {
            return Err(EditError::UnusableGeometry {
                heads: ctx.heads,
                sectors: ctx.sectors,
            });
        }
        // Everything that can be refused is refused before anything is
        // pushed, so a rejected add leaves the editor exactly as it was.
        let name = self.assign_name(spec.name.as_deref())?;
        let range = match spec.placement {
            Placement::Cylinders { low, high } => (low, high),
            Placement::Size(bytes) => self.place_by_size(bytes, cyl_blocks)?,
        };
        self.check_extent(None, range, cyl_blocks)?;

        let mut bytes = alloc::vec![0u8; self.block_size];
        fill_part_fields(&mut bytes, &spec, &name, range, CHAIN_END, ctx);
        // The block is sealed by `prepare`, like every other block a
        // commit writes; sealing it here as well would be dead work.
        let parsed = parse_part(&bytes, UNPLACED_BLOCK);
        self.parts.push(RawBlock {
            lba: UNPLACED_BLOCK,
            bytes,
        });
        self.rdb.partitions.push(parsed);
        Ok(self.parts.len() - 1)
    }

    /// Remove partition `index` from the table. Later partitions shift
    /// down, as in any `Vec`.
    ///
    /// # What this does *not* do
    ///
    /// **The partition's contents are not erased.** Removing the table
    /// entry unchains one `PART` block and nothing else; every byte
    /// between `de_LowCyl` and `de_HighCyl` is exactly where it was, and
    /// re-adding the same extent with the same `de_DosType` gets the
    /// filesystem back. That is deliberate — the crate stops at the
    /// partition boundary by its founding non-goal, and a delete that
    /// scribbled on a filesystem would be doing the one thing this
    /// crate's never-touch guarantee promises it cannot.
    ///
    /// The vacated `PART` block *is* zeroed, on commit and after the
    /// `RDSK` has landed. It is inside the RDB area, it is ours, and a
    /// checksum-valid unreferenced `PART` block left lying there is
    /// something the next tool's scan can find (`docs/amipart-survey.md`
    /// §3, where AmiPart leaves exactly that).
    pub fn remove_partition(&mut self, index: usize) -> Result<(), EditError> {
        if index >= self.parts.len() {
            return Err(EditError::NoSuchPartition {
                index,
                count: self.parts.len(),
            });
        }
        self.parts.remove(index);
        self.rdb.partitions.remove(index);
        Ok(())
    }

    /// Set a partition's cylinder range, `high_cyl` *inclusive*.
    ///
    /// # This edits the table entry and nothing else
    ///
    /// **Shrinking is destructive to the filesystem inside the
    /// partition.** The blocks past the new `de_HighCyl` stop belonging
    /// to it while its filesystem still believes they do; the next mount
    /// writes a bitmap or a directory block past the new end — into
    /// whatever now owns that space — or reads one that is no longer
    /// there. Nothing here moves data or resizes a filesystem: AmiPart's
    /// `GROW`/`SHRINK` do that by reaching into FFS/SFS/PFS internals,
    /// which is out of scope by this crate's founding non-goal
    /// (`docs/amipart-survey.md` §7.5). Shrink only a partition you are
    /// about to reformat, or after the filesystem's own tool has shrunk
    /// it.
    ///
    /// **Growing does not grow the filesystem** either. The partition
    /// gets bigger and the filesystem inside it does not notice; the new
    /// blocks are unreachable until something reformats or extends it.
    /// That direction is at least harmless.
    ///
    /// **Moving `de_LowCyl` moves every block of the filesystem relative
    /// to the partition start**, so it destroys the contents outright
    /// rather than merely truncating them. It is offered because
    /// reproducing a known layout needs it; AmiPart refuses the same
    /// edit in its GUI for the same reason it is dangerous.
    ///
    /// # What it refuses
    ///
    /// An inverted range, one reaching past the disk's last cylinder,
    /// one reaching into the RDB area, and one overlapping any *other*
    /// partition — the same checks [`add_partition`](Self::add_partition)
    /// makes, so no edit can produce a layout [`Rdb::validate`] would
    /// complain about.
    pub fn set_extent(
        &mut self,
        index: usize,
        low_cyl: u32,
        high_cyl: u32,
    ) -> Result<(), EditError> {
        let p = self
            .rdb
            .partitions
            .get(index)
            .ok_or(EditError::NoSuchPartition {
                index,
                count: self.rdb.partitions.len(),
            })?;
        // The *partition's* own cylinder, not the drive's: `de_Surfaces`
        // and `de_BlocksPerTrack` are per partition and allowed to
        // disagree with `rdb_Heads`/`rdb_Sectors`, and the extent this
        // edit describes is computed from the pair that will be on the
        // block.
        let cyl_blocks = p.cylinder_blocks;
        if cyl_blocks == 0 {
            let env = |i: usize| be32(&self.parts[index].bytes, part::ENVIRONMENT + i * 4);
            return Err(EditError::UnusableGeometry {
                heads: env(de::SURFACES),
                sectors: env(de::BLOCKS_PER_TRACK),
            });
        }
        self.check_extent(Some(index), (low_cyl, high_cyl), cyl_blocks)?;
        self.edit_part(index, |bytes| {
            put_be32(bytes, part::ENVIRONMENT + de::LOW_CYL * 4, low_cyl);
            put_be32(bytes, part::ENVIRONMENT + de::HIGH_CYL * 4, high_cyl);
        })
    }

    /// Move a partition's last cylinder, keeping its first — the
    /// everyday half of [`set_extent`](Self::set_extent), and the only
    /// direction AmiPart's GUI offers.
    ///
    /// Read [`set_extent`](Self::set_extent) before using it: shrinking
    /// is destructive to the filesystem inside, and growing does not
    /// grow it.
    pub fn resize_partition(&mut self, index: usize, new_high_cyl: u32) -> Result<(), EditError> {
        let low_cyl = self
            .rdb
            .partitions
            .get(index)
            .ok_or(EditError::NoSuchPartition {
                index,
                count: self.rdb.partitions.len(),
            })?
            .low_cyl;
        self.set_extent(index, low_cyl, new_high_cyl)
    }

    // ---- structural edits: loadable filesystems --------------------

    /// Build the `FSHD` block and `LSEG` chain for one
    /// [`FileSystemSpec`], all of it unplaced.
    fn build_filesystem(&self, spec: &FileSystemSpec) -> (RawBlock, Vec<RawBlock>, FileSysHeader) {
        let payload = lseg_payload_bytes(self.block_size);
        let mut chain = Vec::new();
        let mut at = 0usize;
        while at < spec.binary.len() {
            let end = (at + payload).min(spec.binary.len());
            let mut bytes = alloc::vec![0u8; self.block_size];
            fill_lseg_fields(&mut bytes, spec.host_id, &spec.binary[at..end], CHAIN_END);
            chain.push(RawBlock {
                lba: UNPLACED_BLOCK,
                bytes,
            });
            at = end;
        }

        let mut bytes = alloc::vec![0u8; self.block_size];
        // A placeholder head: `prepare` writes the real one from the
        // plan, and only the *value* is unknown here — whether the
        // `SEG_LIST` patch bit is set is decided by whether there is a
        // chain at all, which is known now.
        let head = if chain.is_empty() { CHAIN_END } else { 0 };
        fill_fshd_fields(&mut bytes, spec, head, CHAIN_END);
        let mut header = parse_fshd(&bytes, UNPLACED_BLOCK);
        header.seg_list_blocks = UNPLACED_BLOCK as u32;
        (
            RawBlock {
                lba: UNPLACED_BLOCK,
                bytes,
            },
            chain,
            header,
        )
    }

    /// Add a loadable filesystem driver, returning its index.
    ///
    /// The `FSHD` block and the `LSEG` chain carrying the binary are
    /// built here and placed by [`commit`](Self::commit), which is where
    /// "the driver does not fit the RDB area" is answered
    /// ([`CommitError::RdbAreaTooSmall`], before any write) — a driver is
    /// hundreds of blocks where a partition is one, so that is the usual
    /// reason an edit does not fit.
    ///
    /// **No dedupe by `fhb_DosType`.** Two `FSHD`s for one dostype is a
    /// layout the format permits and a caller may want (an old version
    /// kept beside a new one), and silently dropping one would be a
    /// decision taken behind the caller's back. AmiPart's `ADDFS`
    /// documents "add or replace" and then always appends
    /// (`docs/amipart-survey.md` §7.2); this crate does not promise the
    /// replace and then not do it — it offers
    /// [`replace_filesystem`](Self::replace_filesystem) instead.
    pub fn add_filesystem(&mut self, spec: FileSystemSpec) -> Result<usize, EditError> {
        let (header_block, chain, header) = self.build_filesystem(&spec);
        self.fshds.push(header_block);
        self.lsegs.push(chain);
        self.rdb.filesystems.push(header);
        self.sync_filesys_header_list();
        Ok(self.fshds.len() - 1)
    }

    /// Replace the filesystem at `index` — the explicit form of the
    /// operation AmiPart's `ADDFS` documents and does not perform.
    ///
    /// The old `FSHD` and every block of its old `LSEG` chain are
    /// released; a commit zeroes whichever of them the new chain does
    /// not land on.
    pub fn replace_filesystem(
        &mut self,
        index: usize,
        spec: FileSystemSpec,
    ) -> Result<(), EditError> {
        if index >= self.fshds.len() {
            return Err(EditError::NoSuchFileSystem {
                index,
                count: self.fshds.len(),
            });
        }
        let (header_block, chain, header) = self.build_filesystem(&spec);
        self.fshds[index] = header_block;
        self.lsegs[index] = chain;
        self.rdb.filesystems[index] = header;
        self.sync_filesys_header_list();
        Ok(())
    }

    /// Which partitions the filesystem at `index` serves: every one
    /// whose `de_DosType` equals its `fhb_DosType`.
    ///
    /// Indices into [`partitions`](Self::partitions), so the answer can
    /// be fed straight back into [`set_dos_type`](Self::set_dos_type) or
    /// [`remove_partition`](Self::remove_partition).
    pub fn partitions_using_filesystem(&self, index: usize) -> Result<Vec<usize>, EditError> {
        let fs = self
            .rdb
            .filesystems
            .get(index)
            .ok_or(EditError::NoSuchFileSystem {
                index,
                count: self.rdb.filesystems.len(),
            })?;
        Ok(self
            .rdb
            .partitions
            .iter()
            .enumerate()
            .filter(|(_, p)| p.dos_type == fs.dos_type)
            .map(|(i, _)| i)
            .collect())
    }

    /// Remove the filesystem at `index`, returning the partitions that
    /// were relying on it — the indices
    /// [`partitions_using_filesystem`](Self::partitions_using_filesystem)
    /// reports.
    ///
    /// **Their `de_DosType` is not touched.** AmiPart's filesystem
    /// dialog rewrites every matching partition's dostype to `DOS\0` on
    /// delete (`docs/amipart-survey.md` §1a), which silently changes
    /// which handler mounts a partition that may be perfectly happy with
    /// a ROM filesystem of the same dostype. Removing a driver from the
    /// RDB is a statement about the *driver*; what should happen to the
    /// partitions is the caller's decision, and this return value is
    /// what it needs to make it.
    ///
    /// The vacated `FSHD` and every block of its `LSEG` chain are zeroed
    /// on commit, after the `RDSK` no longer refers to them.
    pub fn remove_filesystem(&mut self, index: usize) -> Result<Vec<usize>, EditError> {
        let affected = self.partitions_using_filesystem(index)?;
        self.fshds.remove(index);
        self.lsegs.remove(index);
        self.rdb.filesystems.remove(index);
        self.sync_filesys_header_list();
        Ok(affected)
    }

    /// Bring [`Rdb::filesys_header_list`] back in step with the `FSHD`
    /// list after it has grown or shrunk.
    ///
    /// The head is a *block pointer*, so removing the first filesystem
    /// (or removing the last one altogether) leaves it naming a block
    /// the model no longer has anything at — a caller reading
    /// [`rdb`](Self::rdb) between the edit and the commit would see a
    /// chain head that contradicts `filesystems`. It reads the head the
    /// current list implies: the first `FSHD`'s block, or [`CHAIN_END`]
    /// when there are none. An `FSHD` the editor added has no block yet
    /// and reads [`UNPLACED_BLOCK`], whose low 32 bits are `CHAIN_END`
    /// — provisional exactly like every other block LBA in the model,
    /// and settled by the commit.
    fn sync_filesys_header_list(&mut self) {
        self.rdb.filesys_header_list = match self.fshds.first() {
            Some(b) => b.lba as u32,
            None => CHAIN_END,
        };
    }

    // ---- structural edits: the bad-block list ----------------------

    /// Replace the `BADB` chain with exactly these entries.
    ///
    /// Effectively extinct — drives have remapped their own defects
    /// internally since before the format stopped being used — but the
    /// entries exist on old images, and being the tool that can
    /// *preserve and rewrite* a list rather than dropping it is the
    /// point: AmiPart zeroes `rdb_BadBlockList` on every write, which
    /// orphans the very blocks its own bad-block dialog appended
    /// (`docs/amipart-survey.md` §4).
    ///
    /// The entries are repacked into as many blocks as they need —
    /// `(block_size - 24) / 8` per block — and each block's
    /// `SummedLongs` is the header plus its own entries, which is what
    /// makes the count readable. Which entry sat in which block carries
    /// no information and is not preserved, exactly as
    /// [`Rdb::bad_blocks`] says on the read side.
    ///
    /// An empty list is a `BADB` chain of no blocks and an
    /// `rdb_BadBlockList` of [`CHAIN_END`] —
    /// [`remove_bad_blocks`](Self::remove_bad_blocks) says the same
    /// thing more clearly.
    pub fn set_bad_blocks(&mut self, entries: Vec<BadBlockEntry>) {
        let per_block = (self.block_size - badb::ENTRIES) / 8;
        let mut blocks = Vec::new();
        for chunk in entries.chunks(per_block) {
            let mut bytes = alloc::vec![0u8; self.block_size];
            put_be32(&mut bytes, hdr::ID, id::BADB);
            put_be32(&mut bytes, hdr::HOST_ID, self.rdb.host_id);
            put_be32(&mut bytes, chain::NEXT, CHAIN_END);
            for (i, e) in chunk.iter().enumerate() {
                put_be32(&mut bytes, badb::ENTRIES + i * 8, e.bad);
                put_be32(&mut bytes, badb::ENTRIES + i * 8 + 4, e.good);
            }
            // Header plus entries, the count `parse_badb` reads the
            // entry count back out of.
            put_be32(
                &mut bytes,
                hdr::SUMMED_LONGS,
                (badb::HEADER_LONGS + chunk.len() * 2) as u32,
            );
            blocks.push(RawBlock {
                lba: UNPLACED_BLOCK,
                bytes,
            });
        }
        self.badbs = blocks;
        self.rdb.badb_blocks = alloc::vec![UNPLACED_BLOCK; self.badbs.len()];
        // Not placed until the commit, and `UNPLACED_BLOCK as u32` is
        // `CHAIN_END` — which is also the truth when the list is empty.
        self.rdb.bad_block_list = CHAIN_END;
        self.rdb.bad_blocks = entries;
    }

    /// Drop the `BADB` chain entirely: no entries, `rdb_BadBlockList`
    /// [`CHAIN_END`], and every block it used zeroed on commit.
    pub fn remove_bad_blocks(&mut self) {
        self.set_bad_blocks(Vec::new());
    }

    /// Write the whole RDB area back, edits and all.
    ///
    /// # Order, which is the crash shape
    ///
    /// The format has no journal, so ordering is the only protection
    /// there is. Blocks go out in this order:
    ///
    /// 1. every chained block — `LSEG` before the `FSHD` that heads it,
    ///    each chain written from its **tail forward**, so no block is
    ///    ever written before the block it points at;
    /// 2. the `RDSK`, last and alone: the single pointer flip that
    ///    publishes the new table;
    /// 3. the blocks the old layout used and the new one does not,
    ///    zeroed — after the `RDSK` no longer refers to them, and only
    ///    inside the area.
    ///
    /// This is the reverse of AmiPart's ascending order, which writes the
    /// `RDSK` *first* and so publishes a table before the blocks it
    /// points at exist (`docs/amipart-survey.md` §5). There is no
    /// compatibility reason to reproduce that.
    ///
    /// # Where the blocks go
    ///
    /// Every structure keeps the block it was found on when that block
    /// is inside the area **and its chain pointers do not change**, so a
    /// metadata edit moves nothing at all. A block whose `Next` (or, for
    /// an `FSHD`, whose seg-list head) would differ is relocated
    /// instead: it is the still-published `RDSK` that walks through that
    /// block until the flip, and rewriting it there would splice the new
    /// table into the old one. The rule cascades back along the chain —
    /// re-pointing a predecessor is itself a pointer change — so a
    /// structural edit typically relocates the whole chain and zeroes
    /// what it vacated, afterwards.
    ///
    /// The result is that an interrupted commit leaves *either the old
    /// table or the new one*, never a mixture of the two: each block is
    /// self-describing and sealed, no live block is overwritten by a
    /// different structure, and no live block changes what it points at.
    /// Metadata is the one thing a prefix can show early — the old chain
    /// reading a renamed partition is old-or-new per field on a table
    /// whose shape did not change — and every partition is still there,
    /// whole, exactly once.
    ///
    /// A structure that has nowhere to go — one added by
    /// [`add_partition`](Self::add_partition),
    /// [`add_filesystem`](Self::add_filesystem) or
    /// [`set_bad_blocks`](Self::set_bad_blocks), one found *outside*
    /// the area (the damaged-by-construction case
    /// [`ValidationIssue::BlockOutsideRdbArea`] reports), or one the
    /// pointer rule above evicted — is allocated
    /// in two tiers. First choice is the lowest block **neither** layout
    /// uses: a hole an earlier edit left, or headroom below
    /// `rdb_RDBBlocksHi`. That keeps the old chains walkable right up to
    /// the `RDSK` flip, which is a genuine atomic swap and the reason
    /// the area is never shrunk. Only when no such block is left does it
    /// fall back to a block the old layout is *vacating* in this same
    /// commit — still correct, since the new `RDSK` publishes the new
    /// chains and the zeroing pass runs after it, but from the moment
    /// that block is overwritten the *old* table can no longer be walked
    /// past it. That is the cost of a full area, and the alternative
    /// would be refusing an edit the format allows.
    ///
    /// # What it refuses
    ///
    /// Anything that does not fit the area, before writing a byte —
    /// [`CommitError::RdbAreaTooSmall`], whose remedy is
    /// [`expand_rdb_area`](Self::expand_rdb_area). The area is never
    /// grown *by a commit* and never shrunk at all: growing it is an
    /// explicit edit with a predicate of its own, because it claims
    /// blocks, and nothing here claims blocks behind the caller's back.
    ///
    /// # An expanding commit
    ///
    /// When [`expand_rdb_area`](Self::expand_rdb_area) moved the
    /// ceiling, this commit leases the **new** window and so may write
    /// above the old one — which is the entire point of the operation,
    /// and is safe because the expansion proved those blocks belong to
    /// no partition. The `RDSK` still goes last, so there is a window in
    /// which blocks above the *old* published `rdb_RDBBlocksHi` hold new
    /// chain blocks that the old `RDSK` does not claim. If the commit is
    /// interrupted there, those bytes are unreferenced: the old table is
    /// intact and walks entirely within the old area, nothing points at
    /// the new blocks, and no partition owns them either. Harmless, and
    /// tidied by the next successful commit or by nothing at all.
    /// [`CommitError::RdbAreaPastEndOfDisk`] is the one extra refusal an
    /// expansion brings, checked before any write.
    pub fn commit<S: BlockSink>(
        &self,
        sink: &mut S,
    ) -> Result<CommitReport, CommitError<S::Error>> {
        let block_size = sink.block_size();
        if !block_size_ok(block_size) {
            return Err(CommitError::UnsupportedBlockSize { block_size });
        }
        if block_size != self.block_size {
            return Err(CommitError::BlockSizeMismatch {
                rdb: self.block_size,
                sink: block_size,
            });
        }
        // Only when *this* editor moved the ceiling: an expansion is a
        // claim about how big the disk is, and the sink is the only
        // authority on that present at commit time. An untouched
        // ceiling is not re-litigated — see
        // [`CommitError::RdbAreaPastEndOfDisk`].
        if self.rdb.rdb_blocks_hi != self.opened_rdb_blocks_hi {
            if let Some(block_count) = sink.block_count() {
                if self.rdb.rdb_blocks_hi as u64 >= block_count {
                    return Err(CommitError::RdbAreaPastEndOfDisk {
                        hi: self.rdb.rdb_blocks_hi,
                        block_count,
                    });
                }
            }
        }

        // Everything above and below this pair of calls is arithmetic:
        // neither has a sink in scope, so "compute the whole thing, then
        // write it" is structural rather than a discipline — the same
        // property `RdbBuilder::layout` has, for the same reason.
        let plan = self.plan()?;
        let prepared = self.prepare(&plan)?;

        let mut area = LeasedSink {
            sink,
            hi: self.rdb.rdb_blocks_hi as u64,
        };
        let mut blocks_written = Vec::with_capacity(prepared.len() + plan.zeroed.len());
        for block in &prepared {
            area.write(block.lba, &block.bytes)?;
            blocks_written.push(block.lba);
        }
        let zeros = alloc::vec![0u8; block_size];
        for &lba in &plan.zeroed {
            area.write(lba, &zeros)?;
            blocks_written.push(lba);
        }

        Ok(CommitReport {
            rdsk_block: self.rdb.rdsk_block,
            high_rdsk_block: plan.high_rdsk_block,
            rdb_blocks_hi: self.rdb.rdb_blocks_hi,
            part_blocks: plan.part_lbas,
            fshd_blocks: plan.fshd_lbas,
            blocks_written,
            blocks_zeroed: plan.zeroed,
        })
    }

    /// Assign every structure a block inside the area, or say why it
    /// cannot be done. No sink in scope, on purpose.
    ///
    /// **Minimal motion**: a structure already inside the area stays
    /// exactly where it is, and only a structure that has nowhere (a
    /// block outside the area, a structure that did not exist before,
    /// or one the rule below evicts) is allocated the lowest block the
    /// current layout does not occupy. Nothing live is ever overwritten
    /// by a *different* structure, which is half of what makes the crash
    /// shape hold: the only blocks rewritten in place are the ones being
    /// rewritten as themselves.
    ///
    /// # The other half: a kept block may not change where it points
    ///
    /// Rewriting a block as itself is safe only while the *old* chain
    /// still means what it meant. A block written before the `RDSK`
    /// flip is a block the old, still-published `RDSK` walks through, so
    /// changing its `Next` splices the new structure into the old table:
    /// interrupt the commit there and the disk carries a chain that is
    /// neither the old table nor the new one but a checksum-valid
    /// mixture of both — a deleted partition still at the head, a new
    /// one overlapping it spliced in behind. Every block of it sealed,
    /// every extent mutually destructive.
    ///
    /// So a kept block whose successors change is **relocated** instead:
    /// it is written at a block the old layout does not use, its old
    /// block is left alone until the zeroing pass, and the old chain
    /// walks through untouched bytes right up to the flip. Successors
    /// means both pointers a block can carry — `Next`, and an `FSHD`'s
    /// `fhb_SegListBlocks`. Since relocating a block changes what its
    /// predecessor must point at, the rule cascades back along the
    /// chain; the loop below runs it to a fixed point, which terminates
    /// because a structure is only ever added to the relocated set.
    ///
    /// **A change that is not a pointer stays in place.** A renamed
    /// partition, a new `de_DosType`, a different `boot_pri`: the old
    /// chain reads the new metadata, which is a per-field old-or-new
    /// answer on a table whose *shape* did not change, and every
    /// partition is still there, whole, exactly once. That is the
    /// interruption this crate has always promised, and it needs no
    /// motion.
    ///
    /// The property survives only while the *first* allocation tier has
    /// blocks: falling back to a block the old layout is vacating
    /// overwrites the old table by definition. That is the documented
    /// cost of a full area, and the reason [`commit`](Self::commit)
    /// never shrinks the lease.
    /// Was `lba` part of the layout [`open`](Self::open) found? Blocks
    /// that were are off-limits to the first allocation tier and are
    /// zeroed if nothing lands on them.
    fn was_original(&self, lba: u64) -> bool {
        self.original.contains_key(&lba)
    }

    /// What the block at `lba` pointed at when it was read, or `None`
    /// if the published layout does not use that block at all.
    fn original_links(&self, lba: u64) -> Option<OriginalLinks> {
        self.original.get(&lba).copied()
    }

    fn plan<E>(&self) -> Result<CommitPlan, CommitError<E>> {
        let lo = self.rdb.rdb_blocks_lo as u64;
        let hi = self.rdb.rdb_blocks_hi as u64;
        if lo > hi {
            return Err(CommitError::RdbAreaInvalid {
                lo: self.rdb.rdb_blocks_lo,
                hi: self.rdb.rdb_blocks_hi,
            });
        }
        let rdsk = self.rdb.rdsk_block;
        if rdsk > hi {
            return Err(CommitError::RdskOutsideRdbArea {
                rdsk_block: rdsk,
                hi,
            });
        }

        // Every structure, flattened into one list in a fixed order —
        // PART blocks, then each filesystem's FSHD and its LSEG chain,
        // then BADB — so the allocation is one pass and the shapes are
        // restored from the same counts afterwards.
        let mut current: Vec<u64> = Vec::new();
        for p in &self.parts {
            current.push(p.lba);
        }
        for (i, f) in self.fshds.iter().enumerate() {
            current.push(f.lba);
            for b in &self.lsegs[i] {
                current.push(b.lba);
            }
        }
        for b in &self.badbs {
            current.push(b.lba);
        }

        // Who follows whom, as flat indices into `current`: the new
        // chain shape, which is what every `Next` in `prepare` is
        // written from. Built once, from the same flattening.
        let parts_n = self.parts.len();
        let mut next_of: Vec<Option<usize>> = alloc::vec![None; current.len()];
        let mut seg_of: Vec<Option<usize>> = alloc::vec![None; current.len()];
        for i in 1..parts_n {
            next_of[i - 1] = Some(i);
        }
        let mut at = parts_n;
        let mut fshd_positions: Vec<usize> = Vec::with_capacity(self.fshds.len());
        for chain in &self.lsegs {
            fshd_positions.push(at);
            if !chain.is_empty() {
                seg_of[at] = Some(at + 1);
            }
            for j in 1..chain.len() {
                next_of[at + j] = Some(at + j + 1);
            }
            at += 1 + chain.len();
        }
        for w in fshd_positions.windows(2) {
            next_of[w[0]] = Some(w[1]);
        }
        for i in at + 1..current.len() {
            next_of[i - 1] = Some(i);
        }

        // Structures the chain-shape rule has evicted from the block
        // they were found on. Never cleared, only added to, which is
        // what makes the loop below terminate.
        let mut relocate: Vec<bool> = alloc::vec![false; current.len()];
        let (assigned, taken) = loop {
            // A set, not a list: the area holds a few dozen blocks plus
            // whatever driver payload the image carries, and "whatever
            // the image carries" is the attacker's number — a linear
            // membership scan per placement is quadratic in it. A
            // `BTreeSet` is `alloc`, so it costs no dependency.
            let mut taken: BTreeSet<u64> = BTreeSet::new();
            taken.insert(rdsk);
            let mut assigned: Vec<u64> = alloc::vec![0; current.len()];
            let mut placed: Vec<bool> = alloc::vec![false; current.len()];
            for (i, &lba) in current.iter().enumerate() {
                if !relocate[i] && (lo..=hi).contains(&lba) && !taken.contains(&lba) {
                    assigned[i] = lba;
                    placed[i] = true;
                    taken.insert(lba);
                }
            }
            // Allocation is two tiers, and the order is the crash shape.
            //
            // A structure with nowhere to go takes the lowest block that
            // *neither* layout uses — a hole a previous edit left, or
            // headroom below `rdb_RDBBlocksHi` — so the old chains stay
            // walkable right up to the `RDSK` flip and the swap is
            // genuinely atomic. That is §7.4's option (b), and it is why
            // the area is never shrunk.
            //
            // Only when no such block is left does it fall back to a
            // block the old layout is *vacating* in this same commit.
            // Still correct — the new `RDSK` publishes the new chains,
            // and the zeroing pass runs after it — but from the moment
            // such a block is overwritten the *old* table can no longer
            // be walked past it. That is the honest cost of a full area,
            // not something to hide: the alternative is refusing an edit
            // the format allows.
            let mut fresh = lo;
            let mut vacated = lo;
            for i in 0..assigned.len() {
                if placed[i] {
                    continue;
                }
                while fresh <= hi && (taken.contains(&fresh) || self.was_original(fresh)) {
                    fresh += 1;
                }
                let lba = if fresh <= hi {
                    fresh
                } else {
                    while vacated <= hi && taken.contains(&vacated) {
                        vacated += 1;
                    }
                    if vacated > hi {
                        let needed = current.len() as u64 + u64::from(rdsk >= lo);
                        return Err(CommitError::RdbAreaTooSmall {
                            needed,
                            available: hi - lo + 1,
                            lo,
                            hi,
                        });
                    }
                    vacated
                };
                assigned[i] = lba;
                placed[i] = true;
                taken.insert(lba);
            }

            // The chain-shape rule, run to a fixed point: a block kept
            // where the old table has it must be written with the
            // pointers the old table reads there, or the old table stops
            // being the old table the moment it lands.
            let pointer = |slot: Option<usize>| match slot {
                Some(k) => assigned[k] as u32,
                None => CHAIN_END,
            };
            let mut changed = false;
            for i in 0..current.len() {
                if relocate[i] || assigned[i] != current[i] {
                    continue;
                }
                // A block the old layout never used is not on the old
                // chain, so nothing reads it before the flip and its
                // pointers are free to say anything.
                if let Some(old) = self.original_links(assigned[i]) {
                    if old.next != pointer(next_of[i]) || old.seg_list != pointer(seg_of[i]) {
                        relocate[i] = true;
                        changed = true;
                    }
                }
            }
            if !changed {
                break (assigned, taken);
            }
        };

        // Back into the shapes, in the order they were flattened.
        let mut at = 0usize;
        let part_lbas = assigned[at..at + self.parts.len()].to_vec();
        at += self.parts.len();
        let mut fshd_lbas = Vec::with_capacity(self.fshds.len());
        let mut lseg_lbas = Vec::with_capacity(self.fshds.len());
        for chain in &self.lsegs {
            fshd_lbas.push(assigned[at]);
            at += 1;
            lseg_lbas.push(assigned[at..at + chain.len()].to_vec());
            at += chain.len();
        }
        let badb_lbas = assigned[at..].to_vec();

        // `rdb_HighRDSKBlock` is the high-water mark of what is actually
        // used, recomputed — unlike `rdb_RDBBlocksHi`, which is the
        // lease and is preserved. AmiPart writes the two equal, which is
        // why an image edited by it loses its headroom and why the BADB
        // chain it appends above the old ceiling is orphaned by the next
        // write (`docs/amipart-survey.md` §4).
        let high_rdsk_block = taken.iter().next_back().copied().unwrap_or(rdsk) as u32;

        // What the old layout used, the new one does not, and lies
        // inside the area. A block outside it is left alone whatever it
        // holds: it belongs to whoever owns that space now, and the
        // never-touch guarantee outranks tidiness.
        //
        // Taken from the layout `open` read rather than from the
        // structures that survive the edits, which is what makes a
        // *delete* reach this at all: the block a removed `PART` sat on
        // is gone from every list above, and this is the only record
        // that it was ever ours. AmiPart leaves such a block on disk,
        // checksum-valid and unreferenced, where the next tool's `RDSK`
        // scan can find it (`docs/amipart-survey.md` §3).
        // Already sorted and deduplicated: `original` is keyed by LBA.
        let zeroed: Vec<u64> = self
            .original
            .keys()
            .copied()
            .filter(|lba| (lo..=hi).contains(lba) && !taken.contains(lba))
            .collect();

        Ok(CommitPlan {
            part_lbas,
            fshd_lbas,
            lseg_lbas,
            badb_lbas,
            high_rdsk_block,
            zeroed,
        })
    }

    /// Patch and seal every block the plan will write, in write order —
    /// chains from their tails, `RDSK` last. No sink in scope here
    /// either, so a block that cannot be sealed fails before any block
    /// is written.
    fn prepare<E>(&self, plan: &CommitPlan) -> Result<Vec<PreparedBlock>, CommitError<E>> {
        let mut out = Vec::new();

        // Filesystems back to front, and each LSEG chain tail first, so
        // every `Next` points at a block that is already on the disk.
        for i in (0..self.fshds.len()).rev() {
            let chain = &self.lsegs[i];
            for j in (0..chain.len()).rev() {
                let mut bytes = chain[j].bytes.clone();
                let next = match plan.lseg_lbas[i].get(j + 1) {
                    Some(&lba) => lba as u32,
                    None => CHAIN_END,
                };
                put_be32(&mut bytes, chain::NEXT, next);
                reseal_preserving(&mut bytes)?;
                out.push(PreparedBlock {
                    lba: plan.lseg_lbas[i][j],
                    bytes,
                });
            }

            let mut bytes = self.fshds[i].bytes.clone();
            let next = match plan.fshd_lbas.get(i + 1) {
                Some(&lba) => lba as u32,
                None => CHAIN_END,
            };
            put_be32(&mut bytes, chain::NEXT, next);
            // `fhb_PatchFlags` is *not* recomputed: whether the FSHD asks
            // for its seglist pointer to be patched into the device node
            // is the image's business, and only the pointer itself moved.
            let head = match plan.lseg_lbas[i].first() {
                Some(&lba) => lba as u32,
                None => CHAIN_END,
            };
            put_be32(&mut bytes, fshd::PATCHED + fshd::SEG_LIST_INDEX * 4, head);
            reseal_preserving(&mut bytes)?;
            out.push(PreparedBlock {
                lba: plan.fshd_lbas[i],
                bytes,
            });
        }

        for i in (0..self.badbs.len()).rev() {
            let mut bytes = self.badbs[i].bytes.clone();
            let next = match plan.badb_lbas.get(i + 1) {
                Some(&lba) => lba as u32,
                None => CHAIN_END,
            };
            put_be32(&mut bytes, chain::NEXT, next);
            reseal_preserving(&mut bytes)?;
            out.push(PreparedBlock {
                lba: plan.badb_lbas[i],
                bytes,
            });
        }

        for i in (0..self.parts.len()).rev() {
            let mut bytes = self.parts[i].bytes.clone();
            let next = match plan.part_lbas.get(i + 1) {
                Some(&lba) => lba as u32,
                None => CHAIN_END,
            };
            put_be32(&mut bytes, chain::NEXT, next);
            reseal_preserving(&mut bytes)?;
            out.push(PreparedBlock {
                lba: plan.part_lbas[i],
                bytes,
            });
        }

        // The RDSK last: the one block whose arrival publishes
        // everything above it.
        let mut bytes = self.rdsk.clone();
        let head = |lbas: &[u64]| match lbas.first() {
            Some(&lba) => lba as u32,
            None => CHAIN_END,
        };
        put_be32(&mut bytes, rdsk::PARTITION_LIST, head(&plan.part_lbas));
        put_be32(&mut bytes, rdsk::FILESYS_HEADER_LIST, head(&plan.fshd_lbas));
        put_be32(&mut bytes, rdsk::BAD_BLOCK_LIST, head(&plan.badb_lbas));
        put_be32(&mut bytes, rdsk::HIGH_RDSK_BLOCK, plan.high_rdsk_block);
        reseal_preserving(&mut bytes)?;
        out.push(PreparedBlock {
            lba: self.rdb.rdsk_block,
            bytes,
        });

        Ok(out)
    }
}

/// A [`BlockSource`] view of one partition: LBA 0 here is
/// `partition.start_lba` on the parent, in the parent's device blocks
/// (the block size passes through unchanged — this adapter never speaks
/// `de_SizeBlock` filesystem blocks). This is the composition seam with
/// filesystem crates — they mount one of these, never the disk.
pub struct PartitionSource<'a, S: BlockSource> {
    parent: &'a mut S,
    start_lba: u64,
    block_len: u64,
}

/// Error from [`PartitionSource`] and [`PartitionSink`]: the parent's
/// own error, an access past the partition's end (which the parent could
/// not catch — the block may exist on disk, just not in this partition),
/// or an access that is inside the partition and past the end of the
/// *parent* (which the parent should catch, and which is refused here
/// rather than trusted to).
///
/// One type for both directions rather than a parallel
/// `PartitionSinkError`: the two adapters apply the *same* two-tier
/// bound to the same arithmetic, so the failure modes are identical and
/// a second enum would only make a caller that both reads and writes
/// match twice on the same three cases. The name is the read side's for
/// compatibility — it was public before the write side existed — and
/// every variant is worded direction-neutrally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartitionSourceError<E> {
    /// The parent [`BlockSource`] or [`BlockSink`] failed on the
    /// underlying transfer.
    Parent(E),
    /// An access past the partition's last block.
    OutOfRange {
        /// The partition-relative block asked for.
        lba: u64,
        /// How many blocks the partition has, so `lba` had to be below it.
        len: u64,
    },
    /// The block is inside the partition but off the end of the *parent*
    /// device: the partition's extent, which came off an attacker- or
    /// corruption-supplied `PART` block, claims blocks the disk does not
    /// have.
    ///
    /// Caught here rather than forwarded, because a parent is not
    /// obliged to notice: a source whose own bounds check is a multiply
    /// away from wrapping (`lba * block_size`) would answer a nonsense
    /// LBA with the *wrong block* — and on the write side would *damage*
    /// it. Only reported when the parent reports a
    /// [`block_count`](BlockSource::block_count); without one there is
    /// nothing to check against and the transfer is forwarded as before.
    BeyondParent {
        /// The partition-relative block asked for.
        lba: u64,
        /// Where it lands on the parent — at or above `block_count`.
        parent_lba: u64,
        /// How many blocks the parent says it has.
        block_count: u64,
    },
}

impl<E: core::fmt::Display> core::fmt::Display for PartitionSourceError<E> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PartitionSourceError::Parent(e) => {
                write!(f, "the parent device failed: {e}")
            }
            PartitionSourceError::OutOfRange { lba, len } => write!(
                f,
                "block {lba} is past the end of the partition, which has {len} blocks"
            ),
            PartitionSourceError::BeyondParent {
                lba,
                parent_lba,
                block_count,
            } => write!(
                f,
                "block {lba} of the partition is block {parent_lba} of the device, \
                 which has only {block_count} blocks"
            ),
        }
    }
}

#[cfg(feature = "std")]
impl<E: std::error::Error + 'static> std::error::Error for PartitionSourceError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PartitionSourceError::Parent(e) => Some(e),
            PartitionSourceError::OutOfRange { .. } | PartitionSourceError::BeyondParent { .. } => {
                None
            }
        }
    }
}

impl<'a, S: BlockSource> PartitionSource<'a, S> {
    /// View `partition` of `parent` as a [`BlockSource`] of its own,
    /// borrowing the parent for as long as the view lives.
    pub fn new(parent: &'a mut S, partition: &Partition) -> Self {
        Self {
            parent,
            start_lba: partition.start_lba,
            block_len: partition.block_len,
        }
    }
}

impl<'a, S: BlockSource> BlockSource for PartitionSource<'a, S> {
    type Error = PartitionSourceError<S::Error>;

    fn block_size(&self) -> usize {
        self.parent.block_size()
    }

    fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
        // The second half of the guard is not paranoia about the
        // caller: a `Partition` parsed from a hostile PART block can
        // carry a saturated `start_lba` and `block_len`, and then an
        // in-range `lba` still runs off the end of the *address space*.
        // Out of range is out of range whichever end it falls off.
        let parent_lba = match self.start_lba.checked_add(lba) {
            Some(l) if lba < self.block_len => l,
            _ => {
                return Err(PartitionSourceError::OutOfRange {
                    lba,
                    len: self.block_len,
                })
            }
        };
        // And the parent's own end, when it will say where that is. A
        // hostile `start_lba` produces a parent LBA no disk has, and a
        // parent that computes a byte offset from it can wrap the
        // multiply and answer with the *wrong* block rather than an
        // error. Refusing here means the adapter never forwards an LBA
        // it already knows is nonsense.
        if let Some(block_count) = self.parent.block_count() {
            if parent_lba >= block_count {
                return Err(PartitionSourceError::BeyondParent {
                    lba,
                    parent_lba,
                    block_count,
                });
            }
        }
        self.parent
            .read_block(parent_lba, buf)
            .map_err(PartitionSourceError::Parent)
    }

    fn block_count(&self) -> Option<u64> {
        Some(self.block_len)
    }
}

/// The other half of the same precedent [`SeekBlockSource`] already
/// sets in this crate: one struct gains a second capability's
/// `impl` when its inner type supports it, rather than a second struct
/// or a third "does everything" type appearing beside it. Here the inner
/// type is the *parent* `S`, not a stream — `PartitionSource` becomes a
/// [`BlockSink`] too exactly when `S` is both a [`BlockSource`] and a
/// [`BlockSink`].
///
/// `PartitionSource`, not [`PartitionSink`], is the one that gains the
/// second `impl`: it is the primary of the pair (built first, carries
/// the composition-seam doc comment), so a caller constructs exactly one
/// `PartitionSource` and gets a type usable as a `BlockSource`, a
/// `BlockSink`, or — for a consumer crate with a marker trait like
/// `amiga-ffs-rs`'s `BlockMedium: BlockSource + BlockSink<Error = <Self
/// as BlockSource>::Error>` and a blanket impl of it — a single
/// combined-capability object, without a second `PartitionSink` to juggle
/// alongside it or touching `PartitionSink` itself.
///
/// The `S: BlockSink<Error = <S as BlockSource>::Error>` bound matters
/// for the same reason: without pinning the two `Error` types together,
/// a parent whose read and write errors differ would leave
/// `PartitionSource`'s own `BlockSource::Error` and `BlockSink::Error`
/// disagreeing too, and a `BlockMedium`-style blanket impl (which
/// requires them to match exactly) would never fire.
///
/// The bound check below is not new logic — it is
/// [`PartitionSink::write_block`]'s existing two-tier check, copied
/// verbatim, and [`PartitionSourceError`] already covers both failure
/// modes, so no new error variant was needed for this direction either.
impl<'a, S> BlockSink for PartitionSource<'a, S>
where
    S: BlockSource + BlockSink<Error = <S as BlockSource>::Error>,
{
    type Error = PartitionSourceError<<S as BlockSource>::Error>;

    fn block_size(&self) -> usize {
        // `S` is both `BlockSource` and `BlockSink`, so the method name
        // alone is ambiguous — pick the same trait `PartitionSource`'s
        // own `BlockSource` impl delegates through.
        BlockSource::block_size(self.parent)
    }

    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error> {
        // Same two-tier guard as `PartitionSink::write_block`: a
        // `Partition` parsed from a hostile PART block can carry a
        // saturated `start_lba` and `block_len`, and then an in-range
        // `lba` still runs off the end of the *address space*.
        let parent_lba = match self.start_lba.checked_add(lba) {
            Some(l) if lba < self.block_len => l,
            _ => {
                return Err(PartitionSourceError::OutOfRange {
                    lba,
                    len: self.block_len,
                })
            }
        };
        // And the parent's own end, when it will say where that is —
        // asked of `BlockSink`, not `BlockSource`: this is a write
        // bound, and unlike `block_size` (whose two traits are
        // documented to agree), `block_count` carries no such contract,
        // so a parent is free to answer differently for reading and
        // writing (a source that has not grown into a sink's larger
        // backing store yet, say). The parent is not obliged to notice
        // this on its own — one whose bounds check is a multiply away
        // from wrapping would write the *wrong block* — and a wrong
        // write is not recoverable the way a wrong read is, so the
        // adapter refuses rather than forwards.
        if let Some(block_count) = BlockSink::block_count(self.parent) {
            if parent_lba >= block_count {
                return Err(PartitionSourceError::BeyondParent {
                    lba,
                    parent_lba,
                    block_count,
                });
            }
        }
        self.parent
            .write_block(parent_lba, buf)
            .map_err(PartitionSourceError::Parent)
    }

    fn block_count(&self) -> Option<u64> {
        Some(self.block_len)
    }
}

/// A [`BlockSink`] view of one partition: the write-side mirror of
/// [`PartitionSource`], and the other half of the composition seam with
/// filesystem crates — they format into one of these, never the disk.
/// LBA 0 here is `partition.start_lba` on the parent, in the parent's
/// device blocks (the block size passes through unchanged — this adapter
/// never speaks `de_SizeBlock` filesystem blocks).
///
/// Bounds are checked on the way *in*, on the same two tiers
/// [`PartitionSource::read_block`] uses, which matters more here than
/// there: a forwarded nonsense LBA loses a caller a block of data on the
/// read side and destroys someone else's on the write side.
///
/// There is no flush: this crate does not buffer, so there would be
/// nothing to flush, and durability stays where
/// [`BlockSink::write_block`] already puts it — with the caller and the
/// underlying sink. Growing a partition into free space is likewise not
/// here: a sink only ever sees one `Partition`'s extent and has no view
/// of its siblings or the RDB's free space, so that is a question for
/// [`RdbEditor::resize_partition`] before the sink is constructed.
///
/// A parent that is both `BlockSource + BlockSink` can be viewed either
/// way, one view at a time — the borrow is exclusive, exactly as
/// [`PartitionSource`]'s is:
///
/// ```no_run
/// # use amiga_rdb::{BlockSink, BlockSource, PartitionSink, PartitionSource, Rdb};
/// # fn go<S: BlockSource<Error = E> + BlockSink<Error = E>, E>(
/// #     disk: &mut S, rdb: &Rdb,
/// # ) -> Result<(), Box<dyn core::fmt::Debug>> {
/// let part = rdb.partitions[0].clone();
/// let mut buf = vec![0u8; BlockSource::block_size(disk)];
/// {
///     let mut sink = PartitionSink::new(disk, &part);
///     sink.write_block(0, &buf).ok();
/// }
/// let mut source = PartitionSource::new(disk, &part);
/// source.read_block(0, &mut buf).ok();
/// # Ok(())
/// # }
/// ```
pub struct PartitionSink<'a, S: BlockSink> {
    parent: &'a mut S,
    start_lba: u64,
    block_len: u64,
}

impl<'a, S: BlockSink> PartitionSink<'a, S> {
    /// View `partition` of `parent` as a [`BlockSink`] of its own,
    /// borrowing the parent for as long as the view lives.
    pub fn new(parent: &'a mut S, partition: &Partition) -> Self {
        Self {
            parent,
            start_lba: partition.start_lba,
            block_len: partition.block_len,
        }
    }
}

impl<'a, S: BlockSink> BlockSink for PartitionSink<'a, S> {
    type Error = PartitionSourceError<S::Error>;

    fn block_size(&self) -> usize {
        self.parent.block_size()
    }

    fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error> {
        // Same guard as the read side, for the same reason: a
        // `Partition` parsed from a hostile PART block can carry a
        // saturated `start_lba` and `block_len`, and then an in-range
        // `lba` still runs off the end of the *address space*.
        let parent_lba = match self.start_lba.checked_add(lba) {
            Some(l) if lba < self.block_len => l,
            _ => {
                return Err(PartitionSourceError::OutOfRange {
                    lba,
                    len: self.block_len,
                })
            }
        };
        // And the parent's own end, when it will say where that is. The
        // parent is not obliged to notice — one whose bounds check is a
        // multiply away from wrapping would write the *wrong block* —
        // and a wrong write is not recoverable the way a wrong read is,
        // so the adapter refuses rather than forwards.
        if let Some(block_count) = self.parent.block_count() {
            if parent_lba >= block_count {
                return Err(PartitionSourceError::BeyondParent {
                    lba,
                    parent_lba,
                    block_count,
                });
            }
        }
        self.parent
            .write_block(parent_lba, buf)
            .map_err(PartitionSourceError::Parent)
    }

    fn block_count(&self) -> Option<u64> {
        Some(self.block_len)
    }
}

#[cfg(feature = "std")]
mod std_support {
    use super::{block_size_ok, BlockSink, BlockSource, MIN_BLOCK_SIZE};
    use std::io::{Read, Seek, SeekFrom, Write};

    /// A [`BlockSource`] over anything `Read + Seek` — a `File`, a
    /// `Cursor<Vec<u8>>`. The convenience the `std` feature exists for.
    ///
    /// A byte stream carries no sector size of its own, so the caller
    /// supplies it: [`new`](Self::new) assumes the classic 512, and
    /// [`with_block_size`](Self::with_block_size) takes the size of the
    /// device the image was taken from.
    ///
    /// It is also a [`BlockSink`] when — and only when — the inner type
    /// is `Write` as well. One type serves both directions rather than a
    /// `SeekBlockSink` beside it: the seek-and-transfer logic is the
    /// same and the block size must not be allowed to differ between a
    /// read view and a write view of one file. A read-only `T` simply
    /// does not get the [`BlockSink`] impl, so "this file was opened for
    /// reading" stays a compile-time fact — which is the whole reason
    /// [`BlockSink`] is a separate trait.
    pub struct SeekBlockSource<T: Read + Seek> {
        inner: T,
        block_size: usize,
        blocks: Option<u64>,
    }

    impl<T: Read + Seek> SeekBlockSource<T> {
        /// A 512-byte-block view of `inner`.
        pub fn new(inner: T) -> std::io::Result<Self> {
            Self::with_block_size(inner, MIN_BLOCK_SIZE)
        }

        /// A view of `inner` with the given device block size.
        ///
        /// `block_count` comes from the stream length; a stream whose
        /// length is not a block multiple keeps its trailing fragment
        /// invisible, the same as a real disk with a partial final
        /// sector. An unsupported `block_size` is
        /// `std::io::ErrorKind::InvalidInput`.
        pub fn with_block_size(mut inner: T, block_size: usize) -> std::io::Result<Self> {
            if !block_size_ok(block_size) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("unsupported block size {block_size}"),
                ));
            }
            let len = inner.seek(SeekFrom::End(0))?;
            Ok(Self {
                inner,
                block_size,
                blocks: Some(len / block_size as u64),
            })
        }
    }

    impl<T: Read + Seek> BlockSource for SeekBlockSource<T> {
        type Error = std::io::Error;

        fn block_size(&self) -> usize {
            self.block_size
        }

        fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error> {
            self.inner
                .seek(SeekFrom::Start(byte_offset(lba, self.block_size)?))?;
            self.inner.read_exact(buf)
        }

        fn block_count(&self) -> Option<u64> {
            self.blocks
        }
    }

    /// `lba * block_size`, refused rather than wrapped.
    ///
    /// An LBA is attacker-controlled: a `PART` block can declare a
    /// `de_LowCyl` that saturates a partition's `start_lba`, and
    /// [`PartitionSource`](super::PartitionSource) forwards
    /// partition-relative blocks to the parent unchanged. An unchecked
    /// multiply panics in a debug build and, far worse, *wraps* in a
    /// release one — `2^55 * 512` is 0, so a read meant for a block that
    /// is not on the disk returns `Ok` with the contents of block 0, and
    /// a write lands on the `RDSK`. Out of the address space is out of
    /// range, and says so.
    fn byte_offset(lba: u64, block_size: usize) -> std::io::Result<u64> {
        lba.checked_mul(block_size as u64).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("block {lba} is past the end of the byte address space"),
            )
        })
    }

    impl<T: Read + Write + Seek> BlockSink for SeekBlockSource<T> {
        type Error = std::io::Error;

        fn block_size(&self) -> usize {
            self.block_size
        }

        fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error> {
            self.inner
                .seek(SeekFrom::Start(byte_offset(lba, self.block_size)?))?;
            self.inner.write_all(buf)
        }

        /// The block count sampled when the source was constructed, and
        /// so *not* updated by a write past the end that grows a file.
        /// Deliberate: it describes the device this view was opened on,
        /// and a writer asking "does my layout fit" wants that answer,
        /// not one that moves as it writes.
        fn block_count(&self) -> Option<u64> {
            self.blocks
        }
    }
}

#[cfg(feature = "std")]
pub use std_support::SeekBlockSource;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    /// In-memory disk for tests, with a configurable block size.
    struct MemDisk {
        data: Vec<u8>,
        block_size: usize,
    }

    impl MemDisk {
        fn new(data: Vec<u8>) -> Self {
            Self {
                data,
                block_size: 512,
            }
        }
    }

    impl BlockSource for MemDisk {
        type Error = ();

        fn block_size(&self) -> usize {
            self.block_size
        }

        fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
            let off = lba as usize * self.block_size;
            if off + self.block_size > self.data.len() {
                return Err(());
            }
            buf.copy_from_slice(&self.data[off..off + self.block_size]);
            Ok(())
        }

        fn block_count(&self) -> Option<u64> {
            Some((self.data.len() / self.block_size) as u64)
        }
    }

    /// The same buffer, writable — the in-memory sink the write path is
    /// developed against. It refuses a write past the end exactly as it
    /// refuses a read past the end: a sink that silently grew would hide
    /// the "layout runs off the disk" bug the checks exist to catch.
    impl BlockSink for MemDisk {
        type Error = ();

        fn block_size(&self) -> usize {
            self.block_size
        }

        fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
            let off = lba as usize * self.block_size;
            if off + self.block_size > self.data.len() {
                return Err(());
            }
            self.data[off..off + self.block_size].copy_from_slice(buf);
            Ok(())
        }

        fn block_count(&self) -> Option<u64> {
            Some((self.data.len() / self.block_size) as u64)
        }
    }

    fn put32(disk: &mut [u8], bs: usize, block: usize, off: usize, v: u32) {
        let o = block * bs + off;
        disk[o..o + 4].copy_from_slice(&v.to_be_bytes());
    }

    /// Seal block `block` of a whole-disk buffer over `longs`
    /// longwords.
    ///
    /// A thin address-arithmetic wrapper over the public
    /// [`seal_checksum`] rather than a second implementation of it: the
    /// fixtures below seal several hundred blocks between them, so the
    /// production sealer gets exercised by every parse test in the file
    /// — the round-trip property (`seal_checksum` then `checksum_ok`)
    /// proved incidentally, everywhere, in addition to
    /// [`seal_then_checksum_ok_round_trips`] proving it on purpose.
    fn seal(disk: &mut [u8], bs: usize, block: usize, longs: u32) {
        let base = block * bs;
        seal_checksum(&mut disk[base..base + bs], longs).unwrap();
    }

    /// Write a fixed-width, space-padded ASCII field (the RDSK
    /// identification convention) at `off`, truncating an over-long
    /// value the way a real controller's INQUIRY copy would.
    fn put_padded(disk: &mut [u8], bs: usize, block: usize, (off, len): (usize, usize), s: &str) {
        let base = block * bs + off;
        disk[base..base + len].fill(b' ');
        let b = s.as_bytes();
        let n = b.len().min(len);
        disk[base..base + n].copy_from_slice(&b[..n]);
    }

    /// Build a minimal valid image with the given device block size:
    /// RDSK at `rdsk_block`, one PART. Geometry: 10 cylinders of 32
    /// blocks, regardless of block size.
    fn one_partition_image_bs(rdsk_block: usize, bs: usize) -> Vec<u8> {
        one_partition_image_envec(rdsk_block, bs, 16)
    }

    /// As [`one_partition_image_bs`], but with an explicit
    /// `de_TableSize` and the longwords 17..=19 filled in, so the
    /// optional-tail cases (and a hostile TableSize) share one builder.
    fn one_partition_image_envec(rdsk_block: usize, bs: usize, table_size: u32) -> Vec<u8> {
        // Sized to the geometry it declares: 10 cylinders of 32 blocks.
        let mut d = vec![0u8; 320 * bs];
        let part_block = rdsk_block + 1;

        put32(&mut d, bs, rdsk_block, 0, id::RDSK);
        put32(&mut d, bs, rdsk_block, rdsk::BLOCK_BYTES, bs as u32);
        put32(&mut d, bs, rdsk_block, rdsk::BAD_BLOCK_LIST, CHAIN_END);
        put32(
            &mut d,
            bs,
            rdsk_block,
            rdsk::PARTITION_LIST,
            part_block as u32,
        );
        put32(&mut d, bs, rdsk_block, rdsk::FILESYS_HEADER_LIST, CHAIN_END);
        put32(&mut d, bs, rdsk_block, rdsk::CYLINDERS, 10);
        put32(&mut d, bs, rdsk_block, rdsk::SECTORS, 32);
        put32(&mut d, bs, rdsk_block, rdsk::HEADS, 1);
        put32(&mut d, bs, rdsk_block, rdsk::HOST_ID, 7);
        put32(&mut d, bs, rdsk_block, rdsk::DRIVE_INIT, CHAIN_END);
        put32(&mut d, bs, rdsk_block, rdsk::INTERLEAVE, 1);
        put32(&mut d, bs, rdsk_block, rdsk::PARK, 10);
        put32(&mut d, bs, rdsk_block, rdsk::WRITE_PRE_COMP, 10);
        put32(&mut d, bs, rdsk_block, rdsk::REDUCED_WRITE, 10);
        put32(&mut d, bs, rdsk_block, rdsk::STEP_RATE, 3);
        put32(&mut d, bs, rdsk_block, rdsk::LO_CYLINDER, 2);
        put32(&mut d, bs, rdsk_block, rdsk::HI_CYLINDER, 9);
        put32(&mut d, bs, rdsk_block, rdsk::CYL_BLOCKS, 32);
        put32(&mut d, bs, rdsk_block, rdsk::AUTO_PARK_SECONDS, 0);
        // A reserved area covering the whole of the first cylinder-ish
        // prefix, well below the first partition at block 64, so the
        // fixture is a *clean* layout and validate() has something
        // meaningful to say when a test then breaks it.
        put32(&mut d, bs, rdsk_block, rdsk::RDB_BLOCKS_LO, 0);
        put32(&mut d, bs, rdsk_block, rdsk::RDB_BLOCKS_HI, 15);
        put32(
            &mut d,
            bs,
            rdsk_block,
            rdsk::HIGH_RDSK_BLOCK,
            part_block as u32,
        );
        put32(
            &mut d,
            bs,
            rdsk_block,
            rdsk::FLAGS,
            rdb_flags::DISK_ID | rdb_flags::CTRLR_ID,
        );
        // Space-padded, and DISK_PRODUCT deliberately fills its whole
        // 16 bytes so the trimmer is exercised on both cases.
        put_padded(&mut d, bs, rdsk_block, rdsk::DISK_VENDOR, "QUANTUM ");
        put_padded(
            &mut d,
            bs,
            rdsk_block,
            rdsk::DISK_PRODUCT,
            "FIREBALL_TM3200S",
        );
        put_padded(&mut d, bs, rdsk_block, rdsk::DISK_REVISION, "300 ");
        put_padded(&mut d, bs, rdsk_block, rdsk::CONTROLLER_VENDOR, "CBM     ");
        put_padded(&mut d, bs, rdsk_block, rdsk::CONTROLLER_PRODUCT, "A4091");
        put_padded(&mut d, bs, rdsk_block, rdsk::CONTROLLER_REVISION, "40.9");
        seal(&mut d, bs, rdsk_block, 64);

        write_part(
            &mut d,
            bs,
            part_block,
            CHAIN_END,
            "DH0",
            (bs / 4) as u32,
            (2, 9),
            table_size,
        );

        d
    }

    /// Write one `PART` block: name, `de_SizeBlock` in longwords, the
    /// inclusive cylinder range, `de_TableSize`, and the next-block
    /// pointer. Split out of the image builders because a second
    /// partition differs from the first only in these, and a fixture
    /// that duplicated thirty `put32`s to change three of them would
    /// drift.
    #[allow(clippy::too_many_arguments)]
    fn write_part(
        d: &mut [u8],
        bs: usize,
        block: usize,
        next: u32,
        name: &str,
        size_block_longs: u32,
        (low_cyl, high_cyl): (u32, u32),
        table_size: u32,
    ) {
        put32(d, bs, block, 0, id::PART);
        put32(d, bs, block, chain::NEXT, next);
        put32(d, bs, block, part::FLAGS, 1); // bootable
        let name = name.as_bytes();
        d[block * bs + part::DRIVE_NAME] = name.len() as u8;
        d[block * bs + part::DRIVE_NAME + 1..block * bs + part::DRIVE_NAME + 1 + name.len()]
            .copy_from_slice(name);
        let e = part::ENVIRONMENT;
        put32(d, bs, block, e + de::TABLE_SIZE * 4, table_size);
        put32(d, bs, block, e + de::SIZE_BLOCK * 4, size_block_longs);
        put32(d, bs, block, e + de::SURFACES * 4, 1);
        put32(d, bs, block, e + de::BLOCKS_PER_TRACK * 4, 32);
        put32(d, bs, block, e + de::LOW_CYL * 4, low_cyl);
        put32(d, bs, block, e + de::HIGH_CYL * 4, high_cyl);
        put32(d, bs, block, e + de::BOOT_PRI * 4, 0);
        put32(d, bs, block, e + de::DOS_TYPE * 4, 0x444F_5303);
        // Written unconditionally: whether they are *readable* is
        // de_TableSize's business, and planting them even when it says
        // they are absent is what proves the gate works.
        put32(d, bs, block, e + de::BAUD * 4, 9600);
        put32(d, bs, block, e + de::CONTROL * 4, 0x1234);
        put32(d, bs, block, e + de::BOOT_BLOCKS * 4, 2);
        seal(d, bs, block, 64);
    }

    /// Write one minimal `FSHD` block: the chain pointer, the dostype,
    /// and a patched seg-list head. Everything else is zero, which is a
    /// legal `FSHD` — the fixtures that care about the patch flags
    /// build theirs by hand.
    fn write_fshd(d: &mut [u8], bs: usize, block: usize, next: u32, dos_type: u32, seg_head: u32) {
        put32(d, bs, block, 0, id::FSHD);
        put32(d, bs, block, chain::NEXT, next);
        put32(d, bs, block, fshd::HOST_ID, 7);
        put32(d, bs, block, fshd::DOS_TYPE, dos_type);
        put32(d, bs, block, fshd::PATCH_FLAGS, fshd_patch::SEG_LIST);
        put32(
            d,
            bs,
            block,
            fshd::PATCHED + fshd::SEG_LIST_INDEX * 4,
            seg_head,
        );
        seal(d, bs, block, 64);
    }

    /// A 512-byte-device-block image carrying *two* partitions chained
    /// off one RDSK, each with its own `de_SizeBlock` and cylinder
    /// range.
    ///
    /// The point of the fixture is that `de_SizeBlock` is per partition:
    /// one disk legitimately carries a 4 KB-filesystem-block partition
    /// beside a 32 KB one, and nothing in this crate may assume a single
    /// value per disk. It doubles as the two-extent fixture the overlap
    /// validation needs, since overlapping partitions are just a
    /// different cylinder range here.
    fn two_partition_image(
        (name_a, size_block_a, cyls_a): (&str, u32, (u32, u32)),
        (name_b, size_block_b, cyls_b): (&str, u32, (u32, u32)),
    ) -> Vec<u8> {
        let bs = 512;
        let mut d = one_partition_image(2);
        write_part(&mut d, bs, 3, 4, name_a, size_block_a, cyls_a, 16);
        write_part(&mut d, bs, 4, CHAIN_END, name_b, size_block_b, cyls_b, 16);
        put32(&mut d, bs, 2, rdsk::HIGH_RDSK_BLOCK, 4);
        seal(&mut d, bs, 2, 64);
        d
    }

    fn one_partition_image(rdsk_block: usize) -> Vec<u8> {
        one_partition_image_bs(rdsk_block, 512)
    }

    /// Blocks the [`fs_image`] fixture lays its extra chains out on,
    /// after the RDSK at 2 and the PART at 3.
    const FSHD_BLOCK: usize = 4;
    const LSEG_BLOCK: usize = 5; // and 6
    const BADB_BLOCK: usize = 7; // and 8

    /// Bytes of driver payload one 512-byte `LSEG` block carries.
    const LSEG_PAYLOAD: usize = 512 - lseg::LOAD_DATA;

    /// [`one_partition_image`] plus a one-entry `FSHD` chain with a
    /// two-block `LSEG` chain, and a two-block `BADB` chain. Returns the
    /// image and the exact driver bytes the `LSEG` blocks were filled
    /// with, so a reassembly test can compare against a known payload.
    ///
    /// The payload is sized to fill both blocks exactly (2 × 492 bytes);
    /// a real driver leaves slack in its last block, which the format
    /// cannot record and the reassembly therefore returns.
    fn fs_image() -> (Vec<u8>, Vec<u8>) {
        let bs = 512;
        let mut d = one_partition_image(2);
        put32(&mut d, bs, 2, rdsk::FILESYS_HEADER_LIST, FSHD_BLOCK as u32);
        put32(&mut d, bs, 2, rdsk::BAD_BLOCK_LIST, BADB_BLOCK as u32);
        seal(&mut d, bs, 2, 64);

        put32(&mut d, bs, FSHD_BLOCK, 0, id::FSHD);
        put32(&mut d, bs, FSHD_BLOCK, chain::NEXT, CHAIN_END);
        put32(&mut d, bs, FSHD_BLOCK, fshd::HOST_ID, 7);
        put32(&mut d, bs, FSHD_BLOCK, fshd::FLAGS, 0);
        put32(&mut d, bs, FSHD_BLOCK, fshd::DOS_TYPE, 0x444F_5307);
        put32(&mut d, bs, FSHD_BLOCK, fshd::VERSION, (43 << 16) | 4);
        // Type, Handler, StackSize, Priority, SegList, GlobalVec patched;
        // Task, Lock and Startup deliberately not.
        put32(
            &mut d,
            bs,
            FSHD_BLOCK,
            fshd::PATCH_FLAGS,
            fshd_patch::TYPE
                | fshd_patch::HANDLER
                | fshd_patch::STACK_SIZE
                | fshd_patch::PRIORITY
                | fshd_patch::SEG_LIST
                | fshd_patch::GLOBAL_VEC,
        );
        // Every patched longword is written, gated or not: planting a
        // value behind a clear bit is what proves the gate works.
        for (i, v) in [
            1u32,        // Type
            0xDEAD_0001, // Task      (bit clear)
            0xDEAD_0002, // Lock      (bit clear)
            0x0000_0100, // Handler
            8192,        // StackSize
            10,          // Priority
            0xDEAD_0003, // Startup   (bit clear)
            LSEG_BLOCK as u32,
            0xFFFF_FFFF, // GlobalVec (-1: not BCPL)
        ]
        .into_iter()
        .enumerate()
        {
            put32(&mut d, bs, FSHD_BLOCK, fshd::PATCHED + i * 4, v);
        }
        seal(&mut d, bs, FSHD_BLOCK, 64);

        let payload: Vec<u8> = (0..2 * LSEG_PAYLOAD).map(|i| (i % 251) as u8).collect();
        for (i, block) in [LSEG_BLOCK, LSEG_BLOCK + 1].into_iter().enumerate() {
            put32(&mut d, bs, block, 0, id::LSEG);
            put32(
                &mut d,
                bs,
                block,
                chain::NEXT,
                if i == 0 {
                    (LSEG_BLOCK + 1) as u32
                } else {
                    CHAIN_END
                },
            );
            let base = block * bs + lseg::LOAD_DATA;
            d[base..base + LSEG_PAYLOAD]
                .copy_from_slice(&payload[i * LSEG_PAYLOAD..(i + 1) * LSEG_PAYLOAD]);
            // LSEG sums the whole block: SummedLongs == block_size / 4.
            seal(&mut d, bs, block, (bs / 4) as u32);
        }

        // Two BADB blocks, so the flattening across the chain is tested
        // rather than just the entries within one block.
        for (block, next, entries) in [
            (
                BADB_BLOCK,
                (BADB_BLOCK + 1) as u32,
                &[(100u32, 900u32), (101, 901)][..],
            ),
            (BADB_BLOCK + 1, CHAIN_END, &[(102, 902)][..]),
        ] {
            put32(&mut d, bs, block, 0, id::BADB);
            put32(&mut d, bs, block, chain::NEXT, next);
            for (i, (bad, good)) in entries.iter().enumerate() {
                put32(&mut d, bs, block, badb::ENTRIES + i * 8, *bad);
                put32(&mut d, bs, block, badb::ENTRIES + i * 8 + 4, *good);
            }
            seal(
                &mut d,
                bs,
                block,
                (badb::HEADER_LONGS + entries.len() * 2) as u32,
            );
        }

        (d, payload)
    }

    #[test]
    fn parses_a_minimal_image() {
        let mut disk = MemDisk::new(one_partition_image(2));
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.rdsk_block, 2);
        assert_eq!(rdb.block_bytes, 512);
        assert_eq!(rdb.partitions.len(), 1);
        let p = &rdb.partitions[0];
        assert_eq!(p.name, "DH0");
        assert!(p.bootable);
        assert_eq!(p.dos_type, 0x444F_5303);
        assert_eq!(p.cylinder_blocks, 32);
        assert_eq!(p.start_lba, 64); // LowCyl 2 * 32
        assert_eq!(p.block_len, 256); // cyls 2..=9
    }

    /// A 4 KB-block disk parses identically: same LBAs, same extents —
    /// everything is in device blocks, whatever their size.
    #[test]
    fn parses_a_4k_block_image() {
        let mut disk = MemDisk {
            data: one_partition_image_bs(2, 4096),
            block_size: 4096,
        };
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.rdsk_block, 2);
        assert_eq!(rdb.block_bytes, 4096);
        let p = &rdb.partitions[0];
        assert_eq!(p.start_lba, 64);
        assert_eq!(p.block_len, 256);
        assert_eq!(p.size_block_longs, 1024); // 4 KB filesystem blocks
    }

    /// A 4 KB-sector image read through a 512-byte source must fail
    /// loudly, not misaddress every chained block.
    #[test]
    fn block_bytes_mismatch_is_an_error() {
        // RDSK at 0 so the 512-byte scan still lands on it.
        let mut disk = MemDisk::new(one_partition_image_bs(0, 4096));
        assert_eq!(
            Rdb::parse(&mut disk).unwrap_err(),
            RdbError::BlockBytesMismatch {
                block_bytes: 4096,
                block_size: 512
            }
        );
    }

    #[test]
    fn unsupported_source_block_size_is_an_error() {
        for bad in [0usize, 256, 768, 65536] {
            let mut disk = MemDisk {
                data: vec![0u8; 4 * 65536],
                block_size: bad,
            };
            assert_eq!(
                Rdb::parse(&mut disk).unwrap_err(),
                RdbError::UnsupportedBlockSize { block_size: bad }
            );
        }
    }

    #[test]
    fn no_rdsk_is_reported_not_invented() {
        let mut disk = MemDisk::new(vec![0u8; 32 * 512]);
        assert_eq!(Rdb::parse(&mut disk).unwrap_err(), RdbError::NoRdsk);
    }

    /// A source that declines to say how many blocks it has, and runs
    /// out before the scan does, is `NoRdsk` — not the `Io` its own
    /// end-of-medium produced.
    ///
    /// `block_count() == None` is legitimate (a raw device that will not
    /// answer), and such a source can only signal its end by failing a
    /// read. The probe is a search: it stops when the medium does, and
    /// reports what it was looking for and did not find.
    #[test]
    fn a_short_source_that_will_not_say_its_size_is_no_rdsk() {
        struct EndlessProbe {
            data: Vec<u8>,
        }
        impl BlockSource for EndlessProbe {
            type Error = &'static str;
            fn block_size(&self) -> usize {
                512
            }
            fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), &'static str> {
                let off = lba as usize * 512;
                if off + 512 > self.data.len() {
                    return Err("past the end of the medium");
                }
                buf.copy_from_slice(&self.data[off..off + 512]);
                Ok(())
            }
            fn block_count(&self) -> Option<u64> {
                None
            }
        }

        let mut disk = EndlessProbe {
            data: vec![0u8; 4 * 512],
        };
        assert_eq!(Rdb::parse(&mut disk), Err(RdbError::NoRdsk));

        // But a read failure *after* the RDSK is found is still an
        // error: the probe's tolerance ends where the parse begins.
        let mut disk = EndlessProbe {
            data: one_partition_image(3),
        };
        // The RDSK is on block 3 and its PART chain leads to block 4,
        // which the medium no longer has.
        disk.data.truncate(4 * 512);
        assert_eq!(
            Rdb::parse(&mut disk),
            Err(RdbError::Io("past the end of the medium"))
        );
    }

    /// An `RDSK` ID with a bad checksum must be skipped, not trusted —
    /// this is the stale-copy-shadows-live-RDB case.
    #[test]
    fn bad_checksum_rdsk_is_skipped_in_the_scan() {
        let mut img = one_partition_image(3);
        // Plant a checksummed-wrong RDSK *earlier* than the real one.
        put32(&mut img, 512, 1, 0, id::RDSK);
        put32(&mut img, 512, 1, 4, 64);
        put32(&mut img, 512, 1, 8, 0xDEAD_BEEF);
        let mut disk = MemDisk::new(img);
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.rdsk_block, 3);
    }

    #[test]
    fn part_chain_cycle_is_an_error_not_a_hang() {
        let mut img = one_partition_image(0);
        // PART at 1 points to itself.
        put32(&mut img, 512, 1, chain::NEXT, 1);
        seal(&mut img, 512, 1, 64);
        let mut disk = MemDisk::new(img);
        assert_eq!(
            Rdb::parse(&mut disk).unwrap_err(),
            RdbError::ChainCycle { lba: 1 }
        );
    }

    #[test]
    fn part_chain_past_disk_end_is_an_error() {
        let mut img = one_partition_image(0);
        put32(&mut img, 512, 0, rdsk::PARTITION_LIST, 1000);
        seal(&mut img, 512, 0, 64);
        let mut disk = MemDisk::new(img);
        assert_eq!(
            Rdb::parse(&mut disk).unwrap_err(),
            RdbError::ChainOutOfRange { lba: 1000 }
        );
    }

    #[test]
    fn partition_source_offsets_and_bounds() {
        let mut disk = MemDisk::new(one_partition_image(2));
        // Stamp a marker at the partition's first block (LBA 64).
        disk.data[64 * 512] = 0xAB;
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = rdb.partitions[0].clone();
        let mut ps = PartitionSource::new(&mut disk, &p);
        assert_eq!(BlockSource::block_count(&ps), Some(256));
        assert_eq!(BlockSource::block_size(&ps), 512);
        let mut buf = [0u8; 512];
        ps.read_block(0, &mut buf).unwrap();
        assert_eq!(buf[0], 0xAB);
        assert!(matches!(
            ps.read_block(256, &mut buf),
            Err(PartitionSourceError::OutOfRange { lba: 256, len: 256 })
        ));
    }

    /// The write-side mirror of `partition_source_offsets_and_bounds`:
    /// the same translation, checked by where the bytes land on the
    /// parent rather than by what comes back.
    #[test]
    fn partition_sink_offsets_and_bounds() {
        let mut disk = MemDisk::new(one_partition_image(2));
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = rdb.partitions[0].clone();
        let mut sink = PartitionSink::new(&mut disk, &p);
        assert_eq!(sink.block_count(), Some(256));
        assert_eq!(BlockSink::block_size(&sink), 512);
        let buf = vec![0xCDu8; 512];
        sink.write_block(1, &buf).unwrap();
        // Partition block 1 is disk block 65, and nothing either side
        // of it moved.
        assert_eq!(disk.data[65 * 512..66 * 512], buf[..]);
        assert_eq!(disk.data[64 * 512], 0);
        assert_eq!(disk.data[66 * 512], 0);
    }

    /// A write past the partition's end is refused without the parent
    /// ever seeing it — the block exists on the disk, which is exactly
    /// why the parent cannot be trusted to catch this one.
    #[test]
    fn partition_sink_refuses_a_write_past_the_partition() {
        let mut disk = MemDisk::new(one_partition_image(2));
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = rdb.partitions[0].clone();
        let before = disk.data.clone();
        let mut sink = PartitionSink::new(&mut disk, &p);
        let buf = vec![0xCDu8; 512];
        assert!(matches!(
            sink.write_block(256, &buf),
            Err(PartitionSourceError::OutOfRange { lba: 256, len: 256 })
        ));
        assert_eq!(disk.data, before, "the refused write touched the parent");
    }

    /// The second tier: a hostile `start_lba` off the image puts an
    /// in-partition block past the end of the disk. Refused on the
    /// parent's own block count, before the parent sees the LBA.
    #[test]
    fn partition_sink_refuses_a_block_past_the_parent() {
        let mut disk = MemDisk::new(wrapping_start_lba_image());
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = rdb.partitions[0].clone();
        let before = disk.data.clone();
        let mut sink = PartitionSink::new(&mut disk, &p);
        let buf = vec![0xCDu8; 512];
        assert!(matches!(
            sink.write_block(1, &buf),
            Err(PartitionSourceError::BeyondParent { .. })
        ));
        assert_eq!(disk.data, before, "the refused write touched the parent");
    }

    /// Both halves of the seam over one partition of one disk: what
    /// `PartitionSink` writes at a partition-relative LBA is what
    /// `PartitionSource` reads back at the same LBA.
    #[test]
    fn partition_sink_and_source_round_trip() {
        let mut disk = MemDisk::new(one_partition_image(2));
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = rdb.partitions[0].clone();

        let mut written = vec![0u8; 512];
        for (i, b) in written.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        for lba in [0u64, 1, 255] {
            let mut sink = PartitionSink::new(&mut disk, &p);
            sink.write_block(lba, &written).unwrap();
            let mut source = PartitionSource::new(&mut disk, &p);
            let mut read = vec![0u8; 512];
            source.read_block(lba, &mut read).unwrap();
            assert_eq!(read, written, "round trip at partition block {lba}");
        }
    }

    /// `PartitionSource` as a `BlockSink`: the write lands at the correct
    /// parent offset, exactly as `partition_sink_offsets_and_bounds`
    /// proves for `PartitionSink` — but through the read-side struct.
    #[test]
    fn partition_source_write_block_offsets_and_bounds() {
        let mut disk = MemDisk::new(one_partition_image(2));
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = rdb.partitions[0].clone();
        let mut source = PartitionSource::new(&mut disk, &p);
        assert_eq!(BlockSink::block_size(&source), 512);
        let buf = vec![0xCDu8; 512];
        source.write_block(1, &buf).unwrap();
        // Partition block 1 is disk block 65, and nothing either side
        // of it moved.
        assert_eq!(disk.data[65 * 512..66 * 512], buf[..]);
        assert_eq!(disk.data[64 * 512], 0);
        assert_eq!(disk.data[66 * 512], 0);
    }

    /// A write through `PartitionSource::write_block` past the
    /// partition's end is refused without the parent ever seeing it —
    /// mirrors `partition_sink_refuses_a_write_past_the_partition`.
    #[test]
    fn partition_source_write_block_refuses_a_write_past_the_partition() {
        let mut disk = MemDisk::new(one_partition_image(2));
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = rdb.partitions[0].clone();
        let before = disk.data.clone();
        let mut source = PartitionSource::new(&mut disk, &p);
        let buf = vec![0xCDu8; 512];
        assert!(matches!(
            source.write_block(256, &buf),
            Err(PartitionSourceError::OutOfRange { lba: 256, len: 256 })
        ));
        assert_eq!(disk.data, before, "the refused write touched the parent");
    }

    /// The second tier on `PartitionSource::write_block`: a hostile
    /// `start_lba` puts an in-partition block past the end of the disk.
    /// Mirrors `partition_sink_refuses_a_block_past_the_parent`.
    #[test]
    fn partition_source_write_block_refuses_a_block_past_the_parent() {
        let mut disk = MemDisk::new(wrapping_start_lba_image());
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = rdb.partitions[0].clone();
        let before = disk.data.clone();
        let mut source = PartitionSource::new(&mut disk, &p);
        let buf = vec![0xCDu8; 512];
        assert!(matches!(
            source.write_block(1, &buf),
            Err(PartitionSourceError::BeyondParent { .. })
        ));
        assert_eq!(disk.data, before, "the refused write touched the parent");
    }

    /// The point of the whole change: one `PartitionSource` instance
    /// used as both a `BlockSink` and a `BlockSource`, unlike
    /// `partition_sink_and_source_round_trip`'s two objects over the
    /// same partition.
    #[test]
    fn partition_source_write_then_read_back_on_the_same_instance() {
        let mut disk = MemDisk::new(one_partition_image(2));
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = rdb.partitions[0].clone();

        let mut written = vec![0u8; 512];
        for (i, b) in written.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let mut source = PartitionSource::new(&mut disk, &p);
        source.write_block(3, &written).unwrap();
        let mut read = vec![0u8; 512];
        source.read_block(3, &mut read).unwrap();
        assert_eq!(read, written);
    }

    /// The closest local proof this crate can offer that the
    /// `Error =` bound actually closes the gap `amiga-ffs-rs` hit: a
    /// small stand-in for its `BlockMedium` marker trait, with the same
    /// blanket impl shape, and a check that `PartitionSource<'_,
    /// MemDisk>` satisfies it — a single object usable through a bound
    /// that requires both traits with matching errors.
    trait LocalBlockMedium: BlockSource + BlockSink<Error = <Self as BlockSource>::Error> {}
    impl<T: BlockSource + BlockSink<Error = <T as BlockSource>::Error>> LocalBlockMedium for T {}

    fn assert_medium<T: LocalBlockMedium>() {}

    #[test]
    fn partition_source_over_a_matching_error_parent_satisfies_a_block_medium_bound() {
        assert_medium::<PartitionSource<'_, MemDisk>>();
    }

    /// The full envec: TableSize 20 means longwords 1..=20 follow, so
    /// Baud/Control/BootBlocks are all present and `envec_raw` holds 21
    /// entries (TableSize itself included).
    #[test]
    fn table_size_20_envec_exposes_the_optional_tail() {
        let mut disk = MemDisk::new(one_partition_image_envec(2, 512, 20));
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = &rdb.partitions[0];
        assert_eq!(p.baud, Some(9600));
        assert_eq!(p.control, Some(0x1234));
        assert_eq!(p.boot_blocks, Some(2));
        assert_eq!(p.envec_raw.len(), 21);
        assert_eq!(p.envec_raw[de::TABLE_SIZE], 20);
        assert_eq!(p.envec_raw[de::DOS_TYPE], 0x444F_5303);
        assert_eq!(p.envec_raw[de::BOOT_BLOCKS], 2);
    }

    /// TableSize 16 stops at DosType. The tail longwords are physically
    /// present in the block (the builder writes them) and must still
    /// read as `None` — absent is not the same as zero, and a
    /// round-tripping writer must not resurrect them.
    #[test]
    fn table_size_16_envec_has_no_optional_tail() {
        let mut disk = MemDisk::new(one_partition_image_envec(2, 512, 16));
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = &rdb.partitions[0];
        assert_eq!(p.baud, None);
        assert_eq!(p.control, None);
        assert_eq!(p.boot_blocks, None);
        assert_eq!(p.envec_raw.len(), 17);
    }

    /// A hostile `de_TableSize` must clamp to what the block holds, not
    /// index past it. 512-byte block, envec at 128 → 96 longwords.
    #[test]
    fn hostile_table_size_clamps_envec_raw() {
        for bogus in [96u32, 1000, 0x7FFF_FFFF, u32::MAX] {
            let mut disk = MemDisk::new(one_partition_image_envec(2, 512, bogus));
            let rdb = Rdb::parse(&mut disk).unwrap();
            let p = &rdb.partitions[0];
            assert_eq!(p.envec_raw.len(), (512 - part::ENVIRONMENT) / 4);
            // The tail fields still fit the block, so they read fine —
            // clamping bounds the read, it does not suppress it.
            assert_eq!(p.boot_blocks, Some(2));
        }
    }

    /// A `de_TableSize` too short to reach `de_DosType` is *tolerated*:
    /// the partition is parsed with what the envec declares, the fields
    /// below the declaration read as zero however tempting the bytes at
    /// their offsets are, and [`Rdb::validate`] is where the damage is
    /// reported. One short envec must never cost a recovery tool the
    /// rest of the table.
    #[test]
    fn a_short_envec_is_a_validation_issue_not_a_parse_failure() {
        // 9 reaches de_LowCyl and stops: no HighCyl, no DosType, and the
        // geometry longwords below it still there.
        for table_size in [0u32, 9, 15] {
            let mut disk = MemDisk::new(one_partition_image_envec(2, 512, table_size));
            let rdb = Rdb::parse(&mut disk).unwrap_or_else(|e| {
                panic!("de_TableSize {table_size} failed the whole parse: {e:?}")
            });
            assert_eq!(rdb.partitions.len(), 1);
            let p = &rdb.partitions[0];
            assert_eq!(p.name, "DH0", "the block still parses as itself");
            assert_eq!(p.envec_raw.len(), table_size as usize + 1);
            // Absent is zero: de_DosType is above every one of these
            // table sizes, and a partition missing either half of its
            // range has no extent at all — the inverted case's answer.
            assert_eq!(p.dos_type, 0);
            assert_eq!(
                p.low_cyl,
                if table_size as usize >= de::LOW_CYL {
                    2
                } else {
                    0
                }
            );
            assert_eq!(
                p.high_cyl,
                if table_size as usize >= de::HIGH_CYL {
                    9
                } else {
                    0
                }
            );
            assert_eq!(p.block_len, if table_size == 15 { 8 * 32 } else { 0 });
            // A `de_LowCyl` without its `de_HighCyl` is also an
            // inverted range, and validate says both.
            let mut expected = Vec::new();
            if p.low_cyl > p.high_cyl {
                expected.push(ValidationIssue::PartitionCylindersInverted {
                    index: 0,
                    name: String::from("DH0"),
                    low_cyl: p.low_cyl,
                    high_cyl: p.high_cyl,
                });
            }
            expected.push(ValidationIssue::EnvecTooShort {
                index: 0,
                name: String::from("DH0"),
                table_size,
            });
            assert_eq!(rdb.validate(), expected);
        }
    }

    /// And the editor round-trips such a partition: the block is
    /// preserved whole, so an edit to another partition leaves the short
    /// envec exactly as short as it was.
    #[test]
    fn a_short_envec_partition_round_trips_through_an_edit() {
        let bs = 512;
        let mut image = one_partition_image_envec(2, bs, 9);
        // A second, sound partition to edit, so the commit has a reason
        // to rewrite the chain at all.
        write_part(&mut image, bs, 4, CHAIN_END, "DH1", 128, (5, 7), 16);
        put32(&mut image, bs, 3, chain::NEXT, 4);
        seal(&mut image, bs, 3, 64);
        put32(&mut image, bs, 2, rdsk::HIGH_RDSK_BLOCK, 4);
        seal(&mut image, bs, 2, 64);

        let before = image.clone();
        let mut disk = MemDisk::new(image);
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.set_boot_priority(1, 5).unwrap();
        let report = editor.commit(&mut disk).unwrap();

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.partitions.len(), 2);
        assert_eq!(rdb.partitions[0].envec_raw.len(), 10);
        assert_eq!(rdb.partitions[1].boot_pri, 5);
        assert!(rdb.validate().contains(&ValidationIssue::EnvecTooShort {
            index: 0,
            name: String::from("DH0"),
            table_size: 9,
        }));
        // Neither pointer changed, so nothing moved and the short block
        // is byte for byte what it was.
        assert_eq!(report.part_blocks, vec![3, 4]);
        assert_eq!(&disk.data[3 * bs..4 * bs], &before[3 * bs..4 * bs]);
    }

    /// The same clamp on a 4 KB block, where the *declared* size is the
    /// binding one: 20 longwords is far less than the 992 that fit.
    #[test]
    fn envec_raw_follows_table_size_when_it_fits() {
        let mut disk = MemDisk {
            data: one_partition_image_envec(2, 4096, 20),
            block_size: 4096,
        };
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.partitions[0].envec_raw.len(), 21);
    }

    #[test]
    fn identification_strings_parse_with_padding_trimmed() {
        let mut disk = MemDisk::new(one_partition_image(2));
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.disk_vendor, "QUANTUM");
        assert_eq!(rdb.disk_product, "FIREBALL_TM3200S"); // fills all 16
        assert_eq!(rdb.disk_revision, "300");
        assert_eq!(rdb.controller_vendor, "CBM");
        assert_eq!(rdb.controller_product, "A4091");
        assert_eq!(rdb.controller_revision, "40.9");
        assert!(rdb.flags & rdb_flags::DISK_ID != 0);
        assert!(rdb.flags & rdb_flags::CTRLR_ID != 0);
        assert!(rdb.flags & rdb_flags::LAST == 0);
    }

    #[test]
    fn remaining_rdsk_fields_are_surfaced() {
        let mut disk = MemDisk::new(one_partition_image(2));
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.host_id, 7);
        assert_eq!(rdb.drive_init, CHAIN_END);
        assert_eq!(rdb.interleave, 1);
        assert_eq!(rdb.park, 10);
        assert_eq!(rdb.write_pre_comp, 10);
        assert_eq!(rdb.reduced_write, 10);
        assert_eq!(rdb.step_rate, 3);
        assert_eq!(rdb.lo_cylinder, 2);
        assert_eq!(rdb.hi_cylinder, 9);
        assert_eq!(rdb.cyl_blocks, 32);
        assert_eq!(rdb.auto_park_seconds, 0);
        assert_eq!(rdb.high_rdsk_block, 3);
    }

    /// The FSHD is read, and `PatchFlags` decides which tail fields
    /// exist. The three ungated ones hold planted values in the block
    /// and must still read as `None`.
    #[test]
    fn fshd_chain_parses_with_patch_flags_gating() {
        let (img, _) = fs_image();
        let mut disk = MemDisk::new(img);
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.filesys_header_list, FSHD_BLOCK as u32);
        assert_eq!(rdb.filesystems.len(), 1);
        let f = &rdb.filesystems[0];
        assert_eq!(f.fshd_block, FSHD_BLOCK as u64);
        assert_eq!(f.host_id, 7);
        assert_eq!(f.dos_type, 0x444F_5307);
        assert_eq!(f.version_major(), 43);
        assert_eq!(f.version_minor(), 4);
        assert_eq!(f.node_type, Some(1));
        assert_eq!(f.handler, Some(0x100));
        assert_eq!(f.stack_size, Some(8192));
        assert_eq!(f.priority, Some(10));
        assert_eq!(f.global_vec, Some(-1));
        assert_eq!(f.task, None);
        assert_eq!(f.lock, None);
        assert_eq!(f.startup, None);
        // Always read, whatever the bit says — the chain must be walkable.
        assert_eq!(f.seg_list_blocks, LSEG_BLOCK as u32);
    }

    #[test]
    fn lseg_chain_reassembles_the_driver_binary() {
        let (img, payload) = fs_image();
        let mut disk = MemDisk::new(img);
        let rdb = Rdb::parse(&mut disk).unwrap();
        let bin = rdb.load_filesystem(&rdb.filesystems[0], &mut disk).unwrap();
        assert_eq!(bin.len(), 2 * LSEG_PAYLOAD);
        assert_eq!(bin, payload);
    }

    /// No `LSEG` chain is an empty binary, not an error: an FSHD that
    /// only patches device-node fields is legitimate.
    #[test]
    fn fshd_without_a_seg_list_loads_nothing() {
        let (mut img, _) = fs_image();
        put32(
            &mut img,
            512,
            FSHD_BLOCK,
            fshd::PATCHED + fshd::SEG_LIST_INDEX * 4,
            CHAIN_END,
        );
        seal(&mut img, 512, FSHD_BLOCK, 64);
        let mut disk = MemDisk::new(img);
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb
            .load_filesystem(&rdb.filesystems[0], &mut disk)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn badb_entries_are_flattened_across_the_chain() {
        let (img, _) = fs_image();
        let mut disk = MemDisk::new(img);
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.bad_block_list, BADB_BLOCK as u32);
        assert_eq!(
            rdb.bad_blocks,
            vec![
                BadBlockEntry {
                    bad: 100,
                    good: 900
                },
                BadBlockEntry {
                    bad: 101,
                    good: 901
                },
                BadBlockEntry {
                    bad: 102,
                    good: 902
                },
            ]
        );
    }

    /// A disk with no BADB chain has no entries, and the head field
    /// still round-trips.
    #[test]
    fn no_badb_chain_is_an_empty_list() {
        let mut disk = MemDisk::new(one_partition_image(2));
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.bad_block_list, CHAIN_END);
        assert!(rdb.bad_blocks.is_empty());
        assert!(rdb.filesystems.is_empty());
    }

    #[test]
    fn lseg_chain_cycle_is_an_error_not_a_hang() {
        let (mut img, _) = fs_image();
        // The second LSEG points back at the first.
        put32(
            &mut img,
            512,
            LSEG_BLOCK + 1,
            chain::NEXT,
            LSEG_BLOCK as u32,
        );
        seal(&mut img, 512, LSEG_BLOCK + 1, 128);
        let mut disk = MemDisk::new(img);
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(
            rdb.load_filesystem(&rdb.filesystems[0], &mut disk)
                .unwrap_err(),
            RdbError::ChainCycle {
                lba: LSEG_BLOCK as u64
            }
        );
    }

    #[test]
    fn fshd_chain_with_wrong_id_is_an_error() {
        let (mut img, _) = fs_image();
        put32(&mut img, 512, FSHD_BLOCK, 0, id::PART);
        seal(&mut img, 512, FSHD_BLOCK, 64);
        let mut disk = MemDisk::new(img);
        assert_eq!(
            Rdb::parse(&mut disk).unwrap_err(),
            RdbError::WrongId {
                lba: FSHD_BLOCK as u64,
                expected: id::FSHD,
                found: id::PART,
            }
        );
    }

    /// A `BADB` whose `SummedLongs` claims more entries than the block
    /// holds must clamp. Such a block also fails `checksum_ok`, so the
    /// clamp is reached here by calling the parser directly — the point
    /// is that the arithmetic is safe on its own.
    #[test]
    fn hostile_badb_summed_longs_clamps_entry_count() {
        for bogus in [128u32, 1000, u32::MAX] {
            let mut b = vec![0u8; 512];
            b[4..8].copy_from_slice(&bogus.to_be_bytes());
            let mut out = Vec::new();
            parse_badb(&b, &mut out);
            assert_eq!(out.len(), (512 - badb::ENTRIES) / 8);
        }
    }

    /// `de_SizeBlock` is a *per partition* property: one 512-byte-device
    /// -block disk carries a 4 KB-filesystem-block partition beside a
    /// 32 KB one, and everything this crate reports — extents,
    /// [`PartitionSource`] addressing — stays in the device's 512-byte
    /// blocks regardless. Nothing may assume one filesystem block size
    /// per disk.
    #[test]
    fn mixed_size_block_partitions_on_one_disk() {
        let mut disk = MemDisk::new(two_partition_image(
            ("DH0", 1024, (2, 4)), // 4 KB filesystem blocks
            ("DH1", 8192, (5, 9)), // 32 KB filesystem blocks
        ));
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.block_bytes, 512);
        assert_eq!(rdb.partitions.len(), 2);

        let (a, b) = (rdb.partitions[0].clone(), rdb.partitions[1].clone());
        assert_eq!(a.name, "DH0");
        assert_eq!(a.size_block_longs, 1024);
        assert_eq!(b.name, "DH1");
        assert_eq!(b.size_block_longs, 8192);
        // 64× apart in filesystem block size, identical arithmetic:
        // extents are device blocks, cylinders × 32.
        assert_eq!((a.start_lba, a.block_len), (64, 96)); // cyls 2..=4
        assert_eq!((b.start_lba, b.block_len), (160, 160)); // cyls 5..=9

        // ...and the adapter a filesystem crate mounts still speaks the
        // device's block size for both, not de_SizeBlock's.
        for (p, blocks) in [(&a, 96u64), (&b, 160)] {
            let ps = PartitionSource::new(&mut disk, p);
            assert_eq!(BlockSource::block_size(&ps), 512);
            assert_eq!(BlockSource::block_count(&ps), Some(blocks));
        }

        // A legal layout, mixed block sizes and all.
        assert!(rdb.validate().is_empty());
    }

    /// The fixtures are clean layouts: every chained block inside the
    /// reserved area, every partition clear of it, no two partitions
    /// claiming a block. Both halves of the check agree.
    #[test]
    fn validate_accepts_a_clean_image() {
        let (img, _) = fs_image();
        let mut disk = MemDisk::new(img);
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.rdb_blocks_lo, 0);
        assert_eq!(rdb.rdb_blocks_hi, 15);
        assert_eq!(
            rdb.badb_blocks,
            vec![BADB_BLOCK as u64, BADB_BLOCK as u64 + 1]
        );
        assert!(rdb.validate().is_empty());
        assert!(rdb.validate_seg_lists(&mut disk).unwrap().is_empty());
    }

    /// A `PART` block past `rdb_RDBBlocksHi` sits in space the RDB says
    /// is not reserved — readable, and a repartitioner is entitled to
    /// hand that block to a filesystem.
    #[test]
    fn validate_reports_a_part_block_outside_the_rdb_area() {
        let mut img = one_partition_image(2);
        put32(&mut img, 512, 2, rdsk::RDB_BLOCKS_HI, 2); // PART is at 3
        seal(&mut img, 512, 2, 64);
        let mut disk = MemDisk::new(img);
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(
            rdb.validate(),
            vec![ValidationIssue::BlockOutsideRdbArea {
                kind: BlockKind::Part,
                lba: 3,
                lo: 0,
                hi: 2,
            }]
        );
    }

    /// The real-world case: a tool wrote RDB structures past the
    /// reserved area, so the area now runs into the first partition and
    /// each side will trash the other. The image still parses — a
    /// recovery tool needs the data — and `validate` is how a consumer
    /// finds out before writing.
    #[test]
    fn validate_reports_a_partition_overlapping_the_rdb_area() {
        let mut img = one_partition_image(2);
        put32(&mut img, 512, 2, rdsk::RDB_BLOCKS_HI, 100); // DH0 starts at 64
        seal(&mut img, 512, 2, 64);
        let mut disk = MemDisk::new(img);
        let rdb = Rdb::parse(&mut disk).unwrap();
        // Still fully readable: that is the whole point.
        assert_eq!(rdb.partitions[0].start_lba, 64);
        assert_eq!(
            rdb.validate(),
            vec![ValidationIssue::PartitionOverlapsRdbArea {
                index: 0,
                name: String::from("DH0"),
                start_lba: 64,
                block_len: 256,
                lo: 0,
                hi: 100,
            }]
        );
    }

    /// Beyond the plan item, same failure family: two partitions
    /// claiming the same blocks, each filesystem destroying the other.
    #[test]
    fn validate_reports_overlapping_partitions() {
        let mut disk = MemDisk::new(two_partition_image(
            ("DH0", 1024, (2, 4)), // 64..160
            ("DH1", 8192, (4, 9)), // 128..320
        ));
        let rdb = Rdb::parse(&mut disk).unwrap();
        let issues = rdb.validate();
        assert_eq!(
            issues,
            vec![ValidationIssue::PartitionsOverlap {
                a: 0,
                b: 1,
                a_name: String::from("DH0"),
                b_name: String::from("DH1"),
                start: 128,
                len: 32,
            }]
        );
        assert_eq!(
            alloc::format!("{}", issues[0]),
            "partitions 0 (DH0) and 1 (DH1) both claim blocks 128..160"
        );
    }

    /// An inverted `de_LowCyl`/`de_HighCyl` pair. Found by the fuzzer:
    /// the span used to be computed as an unchecked `high - low + 1`,
    /// which panicked in a debug build and — far worse — wrapped in a
    /// release one, turning a backwards range into a partition claiming
    /// most of the address space. The parse still succeeds (a recovery
    /// tool needs to see the damage), the extent is empty, and
    /// validation names it.
    #[test]
    fn inverted_cylinder_range_is_an_empty_extent_not_an_overflow() {
        let mut d = one_partition_image(2);
        put32(&mut d, 512, 3, part::ENVIRONMENT + de::LOW_CYL * 4, 9);
        put32(&mut d, 512, 3, part::ENVIRONMENT + de::HIGH_CYL * 4, 2);
        seal(&mut d, 512, 3, 64);

        let mut disk = MemDisk::new(d);
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = &rdb.partitions[0];
        assert_eq!((p.low_cyl, p.high_cyl), (9, 2));
        assert_eq!(p.block_len, 0);
        // Zero-length, so it cannot collide with anything: the only
        // issue is the inversion itself.
        let issues = rdb.validate();
        assert_eq!(
            issues,
            vec![ValidationIssue::PartitionCylindersInverted {
                index: 0,
                name: String::from("DH0"),
                low_cyl: 9,
                high_cyl: 2,
            }]
        );
        assert_eq!(
            alloc::format!("{}", issues[0]),
            "partition 0 (DH0) has an inverted cylinder range: LowCyl 9 is above HighCyl 2"
        );
    }

    /// The other half of the same arithmetic: a plausible-looking
    /// geometry whose cylinder product times `de_LowCyl` does not fit a
    /// `u64`. Saturating is the only answer that is not a lie; what
    /// matters is that it does not panic or wrap.
    #[test]
    fn absurd_geometry_saturates_rather_than_overflowing() {
        let mut d = one_partition_image(2);
        let env = |i: usize| part::ENVIRONMENT + i * 4;
        put32(&mut d, 512, 3, env(de::SURFACES), u32::MAX);
        put32(&mut d, 512, 3, env(de::BLOCKS_PER_TRACK), u32::MAX);
        put32(&mut d, 512, 3, env(de::LOW_CYL), u32::MAX - 1);
        put32(&mut d, 512, 3, env(de::HIGH_CYL), u32::MAX);
        seal(&mut d, 512, 3, 64);

        let mut disk = MemDisk::new(d);
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = &rdb.partitions[0];
        assert_eq!(p.start_lba, u64::MAX);
        assert_eq!(p.block_len, u64::MAX);

        // And the adapter over that extent refuses rather than
        // overflowing the parent LBA: `block_len` says block 1 is in
        // range, but `start_lba + 1` does not exist.
        let mut parent = MemDisk::new(vec![0u8; 512]);
        let mut view = PartitionSource::new(&mut parent, p);
        let mut block = vec![0u8; 512];
        assert_eq!(
            view.read_block(1, &mut block),
            Err(PartitionSourceError::OutOfRange {
                lba: 1,
                len: u64::MAX
            })
        );
    }

    /// `LSEG` blocks are lazy, so they are checked with the disk in
    /// hand. Here the area stops at the FSHD, leaving the driver's own
    /// blocks — and the BADB chain behind them — outside it.
    #[test]
    fn validate_seg_lists_reports_lseg_outside_the_rdb_area() {
        let (mut img, _) = fs_image();
        put32(&mut img, 512, 2, rdsk::RDB_BLOCKS_HI, FSHD_BLOCK as u32);
        seal(&mut img, 512, 2, 64);
        let mut disk = MemDisk::new(img);
        let rdb = Rdb::parse(&mut disk).unwrap();

        let outside = |kind, lba| ValidationIssue::BlockOutsideRdbArea {
            kind,
            lba,
            lo: 0,
            hi: FSHD_BLOCK as u64,
        };
        // The eager chains: PART and FSHD are inside, both BADBs are not.
        assert_eq!(
            rdb.validate(),
            vec![
                outside(BlockKind::Badb, BADB_BLOCK as u64),
                outside(BlockKind::Badb, BADB_BLOCK as u64 + 1),
            ]
        );
        assert_eq!(
            rdb.validate_seg_lists(&mut disk).unwrap(),
            vec![
                outside(BlockKind::Lseg, LSEG_BLOCK as u64),
                outside(BlockKind::Lseg, LSEG_BLOCK as u64 + 1),
            ]
        );
    }

    /// An inverted reserved area says nothing about what is inside it,
    /// so it is reported once instead of once per block — but the
    /// partition-versus-partition check does not consult the area and
    /// still runs.
    #[test]
    fn validate_reports_an_inverted_rdb_area_once() {
        let mut img = two_partition_image(("DH0", 1024, (2, 4)), ("DH1", 8192, (4, 9)));
        put32(&mut img, 512, 2, rdsk::RDB_BLOCKS_LO, 16);
        put32(&mut img, 512, 2, rdsk::RDB_BLOCKS_HI, 0);
        seal(&mut img, 512, 2, 64);
        let mut disk = MemDisk::new(img);
        let rdb = Rdb::parse(&mut disk).unwrap();
        let issues = rdb.validate();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0], ValidationIssue::RdbAreaInvalid { lo: 16, hi: 0 });
        assert!(matches!(
            issues[1],
            ValidationIssue::PartitionsOverlap { a: 0, b: 1, .. }
        ));
        assert!(rdb.validate_seg_lists(&mut disk).unwrap().is_empty());
    }

    /// The error types render one line a user can act on: which block,
    /// and both sides of every disagreement. A wrong ID prints the four
    /// characters the format actually writes.
    #[test]
    fn errors_display_as_one_useful_line() {
        let line = |e: RdbError<&str>| alloc::format!("{e}");
        assert_eq!(
            line(RdbError::WrongId {
                lba: 4,
                expected: id::FSHD,
                found: id::PART,
            }),
            "block 4 has ID PART where FSHD was expected"
        );
        // A corrupt ID is not four printable characters; hex, not mojibake.
        assert_eq!(
            line(RdbError::WrongId {
                lba: 4,
                expected: id::PART,
                found: 0,
            }),
            "block 4 has ID 0x00000000 where PART was expected"
        );
        assert_eq!(
            line(RdbError::BadChecksum { lba: 7 }),
            "block 7 has a bad checksum"
        );
        assert_eq!(
            line(RdbError::BlockBytesMismatch {
                block_bytes: 4096,
                block_size: 512,
            }),
            "the RDB declares 4096-byte blocks but the source reads 512-byte blocks"
        );
        assert_eq!(
            line(RdbError::Io("disk on fire")),
            "reading a block failed: disk on fire"
        );
        assert_eq!(
            line(RdbError::ChainTooLong { limit: 1024 }),
            "a chain is longer than the 1024-block limit"
        );
        assert_eq!(
            line(RdbError::SharedChain { lba: 9 }),
            "two filesystems' LSEG chains share block 9, so neither can be edited"
        );
        assert_eq!(
            alloc::format!(
                "{}",
                PartitionSourceError::<&str>::OutOfRange { lba: 256, len: 256 }
            ),
            "block 256 is past the end of the partition, which has 256 blocks"
        );
        assert_eq!(
            alloc::format!(
                "{}",
                PartitionSourceError::<&str>::BeyondParent {
                    lba: 3,
                    parent_lba: 4096,
                    block_count: 320,
                }
            ),
            "block 3 of the partition is block 4096 of the device, which has only 320 blocks"
        );
        assert_eq!(
            alloc::format!(
                "{}",
                ValidationIssue::SharedLsegChain {
                    index: 1,
                    other: 0,
                    lba: 5
                }
            ),
            "filesystems 0 and 1 share LSEG block 5"
        );
    }

    #[test]
    fn checksum_rejects_hostile_summed_longs() {
        let mut b = [0u8; 512];
        b[4..8].copy_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        assert!(!checksum_ok(&b));
        b[4..8].copy_from_slice(&0u32.to_be_bytes());
        assert!(!checksum_ok(&b));
    }

    /// The round-trip property in miniature, swept over every supported
    /// block size, a spread of `SummedLongs` within each, and contents
    /// chosen to make the wrapping arithmetic actually wrap: whatever
    /// [`seal_checksum`] accepts, [`checksum_ok`] must then accept.
    ///
    /// Not a `proptest` — no new dependencies — but the same shape: the
    /// generator is a cheap LCG over the block bytes, so each case is a
    /// different pattern rather than the zeros a hand-written fixture
    /// would have. The all-`0xFF` and all-zero patterns are included
    /// explicitly because they are the two the sum degenerates on.
    #[test]
    fn seal_then_checksum_ok_round_trips() {
        for bs in [512usize, 1024, 2048, 4096, 8192, 16384, 32768] {
            let capacity = (bs / 4) as u32;
            for pattern in 0u32..6 {
                let mut block = vec![0u8; bs];
                for (i, byte) in block.iter_mut().enumerate() {
                    *byte = match pattern {
                        0 => 0,
                        1 => 0xFF,
                        // A cheap LCG, so each pattern is a different
                        // spread of longwords rather than a ramp that
                        // sums to something tidy.
                        p => {
                            (((i as u32)
                                .wrapping_mul(1_103_515_245)
                                .wrapping_add(p * 12_345))
                                >> 16) as u8
                        }
                    };
                }
                for longs in [
                    MIN_SUMMED_LONGS,
                    MIN_SUMMED_LONGS + 1,
                    64,
                    capacity / 2,
                    capacity - 1,
                    capacity,
                ] {
                    seal_checksum(&mut block, longs).unwrap();
                    assert!(
                        checksum_ok(&block),
                        "bs {bs} pattern {pattern} longs {longs} did not check out"
                    );
                    // And the header says what was asked for, which is
                    // the half a clamp would have quietly changed.
                    assert_eq!(be32(&block, 4), longs);
                }
            }
        }
    }

    /// Sealing refuses counts no block can satisfy rather than clamping
    /// them, and refuses without touching the block.
    #[test]
    fn seal_refuses_impossible_summed_longs() {
        let mut block = vec![0xAAu8; 512];
        let untouched = block.clone();

        for longs in 0..MIN_SUMMED_LONGS {
            assert_eq!(
                seal_checksum(&mut block, longs),
                Err(SealError::SummedLongsTooShort {
                    summed_longs: longs
                })
            );
        }
        for longs in [129u32, 1024, u32::MAX] {
            assert_eq!(
                seal_checksum(&mut block, longs),
                Err(SealError::SummedLongsTooLong {
                    summed_longs: longs,
                    capacity: 128,
                })
            );
        }
        assert_eq!(block, untouched);

        // A block too short to hold the header is the same refusal, not
        // a panic — the property that matters is that no length panics.
        let mut tiny = [0u8; 8];
        assert_eq!(
            seal_checksum(&mut tiny, MIN_SUMMED_LONGS),
            Err(SealError::SummedLongsTooLong {
                summed_longs: MIN_SUMMED_LONGS,
                capacity: 2,
            })
        );
    }

    /// A `SummedLongs` below [`MIN_SUMMED_LONGS`] is unsatisfiable, not
    /// merely refused by convention: even a hand-built block gets no
    /// value at `ChkSum` that makes such a sum come out zero, which is
    /// why sealing declines to try.
    #[test]
    fn short_summed_longs_can_never_check_out() {
        for longs in 1u32..MIN_SUMMED_LONGS {
            let mut block = [0u8; 512];
            put_be32(&mut block, 4, longs);
            for chk in [0u32, 1, 0xFFFF_FFFF, 0x8000_0000] {
                put_be32(&mut block, 8, chk);
                // The first `longs` longwords are ID and SummedLongs,
                // both non-zero here, so the sum cannot be zero however
                // ChkSum is set.
                put_be32(&mut block, 0, id::RDSK);
                assert!(!checksum_ok(&block));
            }
        }

        // And the arithmetic loophole: with `SummedLongs` below the
        // floor the sum does not cover `ChkSum`, so a block whose *other*
        // longwords happen to add to zero would pass a naive check while
        // carrying any `ChkSum` at all. The count is the reason to
        // reject it, not the sum.
        for (longs, first) in [(1u32, 0u32), (2, 0xFFFF_FFFEu32)] {
            let mut block = [0u8; 512];
            put_be32(&mut block, 0, first);
            put_be32(&mut block, 4, longs);
            put_be32(&mut block, 8, 0xDEAD_BEEF);
            let sum: u32 =
                (0..longs).fold(0u32, |a, i| a.wrapping_add(be32(&block, i as usize * 4)));
            assert_eq!(sum, 0, "the fixture must be the loophole case");
            assert!(!checksum_ok(&block));
        }
    }

    /// [`checksum_ok`] takes whatever slice a caller has, so a slice too
    /// short to hold the header longwords it reads must be `false`
    /// rather than an index panic — the same no-panic-on-data rule the
    /// parse side follows.
    #[test]
    fn checksum_of_a_fragment_is_false_not_a_panic() {
        let full = {
            let mut b = vec![0u8; 512];
            put_be32(&mut b, 0, id::RDSK);
            seal_checksum(&mut b, 64).unwrap();
            b
        };
        assert!(checksum_ok(&full));
        for len in 0..16usize {
            assert!(!checksum_ok(&full[..len]), "a {len}-byte slice checked out");
        }
    }

    /// The write seam, end to end through the crate's own reader: seal a
    /// block, hand it to a [`BlockSink`], and parse the result back.
    /// A `MemDisk` is both traits, which is exactly the
    /// `BlockSource + BlockSink` bound the write path will take.
    #[test]
    fn block_sink_writes_blocks_a_parse_reads_back() {
        fn stamp<D: BlockSource + BlockSink>(disk: &mut D, src: &[u8], block: usize)
        where
            <D as BlockSink>::Error: core::fmt::Debug,
        {
            let bs = BlockSink::block_size(disk);
            disk.write_block(block as u64, &src[block * bs..(block + 1) * bs])
                .unwrap();
        }

        // A blank disk, filled a block at a time from a known-good image
        // through the sink rather than by slicing the buffer.
        let image = one_partition_image(2);
        let mut disk = MemDisk::new(vec![0u8; image.len()]);
        for block in 0..image.len() / 512 {
            stamp(&mut disk, &image, block);
        }
        assert_eq!(disk.data, image);

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.rdsk_block, 2);
        assert_eq!(rdb.partitions[0].name, "DH0");
    }

    /// A sink refuses a block past its end, so a layout that does not
    /// fit fails at the write rather than growing the disk under it.
    #[test]
    fn block_sink_refuses_a_write_past_the_end() {
        let mut disk = MemDisk::new(vec![0u8; 4 * 512]);
        assert_eq!(disk.write_block(3, &[0u8; 512]), Ok(()));
        assert_eq!(disk.write_block(4, &[0u8; 512]), Err(()));
        assert_eq!(BlockSink::block_count(&disk), Some(4));
    }

    /// `SeekBlockSource` gains [`BlockSink`] from a `Write` inner type
    /// and keeps its block size across both directions.
    #[cfg(feature = "std")]
    #[test]
    fn seek_block_source_writes_when_the_inner_type_can() {
        let mut disk = SeekBlockSource::new(std::io::Cursor::new(vec![0u8; 8 * 512])).unwrap();
        let mut block = vec![0u8; 512];
        put_be32(&mut block, 0, id::RDSK);
        seal_checksum(&mut block, 64).unwrap();
        disk.write_block(5, &block).unwrap();

        let mut back = vec![0u8; 512];
        disk.read_block(5, &mut back).unwrap();
        assert_eq!(back, block);
        assert!(checksum_ok(&back));
        assert_eq!(BlockSink::block_size(&disk), 512);
        assert_eq!(BlockSink::block_count(&disk), Some(8));
    }

    #[test]
    fn put_be32_is_be32s_inverse() {
        let mut block = [0u8; 512];
        for (i, v) in [0u32, 1, 0x4441_5441, u32::MAX, 0x8000_0000]
            .into_iter()
            .enumerate()
        {
            put_be32(&mut block, i * 4, v);
            assert_eq!(be32(&block, i * 4), v);
        }
        // And the byte order really is the format's, not the host's.
        put_be32(&mut block, 0, 0x5244_534B);
        assert_eq!(&block[..4], b"RDSK");
    }

    /// Every case below was read out of an image `rdbtool` actually
    /// created — `rdbtool <img> create size=<n> [bs=<n>] + init`, then
    /// the geometry read back — so these are observations, not a
    /// restatement of the algorithm.
    ///
    /// **Pinned against amitools 0.8.1.** If a future amitools changes
    /// its convention this test is where it will be noticed, and the
    /// decision (follow, or diverge deliberately) belongs there rather
    /// than in silently drifting images.
    const RDBTOOL_0_8_1: &[(u64, usize, u32, u32, u32)] = &[
        // (requested bytes, block size, cylinders, heads, sectors)
        //
        // The Amiga-ish candidate wins almost everywhere: its cylinder
        // is 16 KiB, so it wastes less than the 63-sector one can.
        (512 * 1024, 512, 32, 1, 32),
        (1024 * 1024, 512, 64, 1, 32),
        (10 * 1024 * 1024, 512, 640, 1, 32),
        (100 * 1024 * 1024, 512, 6400, 1, 32),
        (512 * 1024 * 1024, 512, 32768, 1, 32),
        (700 * 1024 * 1024, 512, 44800, 1, 32),
        // Past 65535 cylinders the halving starts, so the head count
        // doubles with every doubling of the disk and the cylinder
        // count sticks at 32768.
        (1024 * 1024 * 1024, 512, 32768, 2, 32),
        (2 * 1024 * 1024 * 1024, 512, 32768, 4, 32),
        (4 * 1024 * 1024 * 1024, 512, 32768, 8, 32),
        (8 * 1024 * 1024 * 1024, 512, 32768, 16, 32),
        (16 * 1024 * 1024 * 1024, 512, 32768, 32, 32),
        (64 * 1024 * 1024 * 1024, 512, 32768, 128, 32),
        // Either side of the ceiling: 65535 cylinders is allowed,
        // 65536 is not, and 65537 loses a cylinder to the halving.
        (1_073_725_440, 512, 65535, 1, 32),
        (1_073_741_824, 512, 32768, 2, 32),
        (1_073_758_208, 512, 32768, 2, 32),
        (2_147_467_264, 512, 65535, 2, 32),
        // Sizes that are an exact multiple of the PC-ish cylinder:
        // both candidates waste nothing and the tie goes to 63 sectors.
        // The head count walks the breakpoint table.
        (516_096, 512, 1, 16, 63),
        (1_032_192, 512, 2, 16, 63),
        (51_609_600, 512, 100, 16, 63),
        (527_966_208, 512, 1023, 16, 63),
        (528_482_304, 512, 1024, 16, 63), // exactly 504 MiB: still 16 heads
        (528_482_305, 512, 1024, 16, 63), // one byte over: still 16 heads
        (1_056_964_608, 512, 1024, 32, 63),
        (2_113_929_216, 512, 1024, 64, 63),
        (4_227_858_432, 512, 1024, 128, 63),
        (8_455_716_864, 512, 1024, 256, 63),
        // Sizes that divide evenly into no cylinder at all: the count
        // rounds down and the tail is unaddressable.
        (123_456_789, 512, 7535, 1, 32),
        (33_333_333, 512, 2034, 1, 32),
        // 4 KB blocks: heads and sectors are unchanged, only the
        // cylinder count scales — and with it where the ceiling bites.
        (10 * 1024 * 1024, 4096, 80, 1, 32),
        (100 * 1024 * 1024, 4096, 800, 1, 32),
        (1024 * 1024 * 1024, 4096, 8192, 1, 32),
        (8 * 1024 * 1024 * 1024, 4096, 32768, 2, 32),
        (123_456_789, 4096, 941, 1, 32),
        // The smallest disk that has a geometry at all: one 32-block
        // cylinder. 16383 bytes is the TooSmall case, tested separately.
        (16384, 512, 1, 1, 32),
    ];

    #[test]
    fn geometry_matches_rdbtool_0_8_1() {
        for &(bytes, bs, cylinders, heads, sectors) in RDBTOOL_0_8_1 {
            let g = synthesize_geometry(bytes, bs)
                .unwrap_or_else(|e| panic!("{bytes} bytes at bs {bs}: {e}"));
            assert_eq!(
                (g.cylinders, g.heads, g.sectors),
                (cylinders, heads, sectors),
                "{bytes} bytes at bs {bs}"
            );
            assert_eq!(g.block_size, bs);
            assert_eq!(g.cylinder_blocks(), heads as u64 * sectors as u64);
        }
    }

    /// The rounding direction, asserted as a property over every pinned
    /// case rather than only where the numbers happen to be untidy: a
    /// geometry describes at most the disk asked for, and falls short by
    /// less than one cylinder. Rounding the other way would put a
    /// partition's last cylinder past the end of the medium.
    #[test]
    fn geometry_never_describes_more_than_the_disk() {
        for &(bytes, bs, ..) in RDBTOOL_0_8_1 {
            let g = synthesize_geometry(bytes, bs).unwrap();
            let cylinder_bytes = g.cylinder_blocks() * bs as u64;
            assert!(g.total_bytes() <= bytes, "{bytes} at bs {bs} overshot");
            assert!(
                bytes - g.total_bytes() < cylinder_bytes,
                "{bytes} at bs {bs} wasted a whole cylinder"
            );
        }
    }

    /// A disk short of one 32-block cylinder has no geometry, and says
    /// so rather than returning a zero-cylinder one that would describe
    /// a disk of no size.
    #[test]
    fn geometry_refuses_a_disk_below_one_cylinder() {
        assert_eq!(
            synthesize_geometry(16383, 512),
            Err(GeometryError::TooSmall {
                total_bytes: 16383,
                block_size: 512,
                minimum_bytes: 16384,
            })
        );
        assert_eq!(
            synthesize_geometry(0, 512),
            Err(GeometryError::TooSmall {
                total_bytes: 0,
                block_size: 512,
                minimum_bytes: 16384,
            })
        );
        // The floor scales with the block size: 32 blocks, whatever
        // they are worth.
        assert!(synthesize_geometry(131_071, 4096).is_err());
        assert!(synthesize_geometry(131_072, 4096).is_ok());
    }

    /// A size no 32-bit geometry can describe is an error, not a wrap.
    /// Both candidates fail here: the PC-ish one needs 2.2e12 cylinders
    /// and the Amiga-ish one 2^35 heads.
    #[test]
    fn geometry_refuses_a_disk_too_large_for_the_u32_fields() {
        assert_eq!(
            synthesize_geometry(u64::MAX, 512),
            Err(GeometryError::TooLarge {
                total_bytes: u64::MAX,
                block_size: 512,
            })
        );
        // An eighth of that in 32 KB blocks *is* describable, so the
        // refusal above is a real limit and not a blanket cap on large
        // inputs. It is also the one-candidate-survives case: the
        // PC-ish geometry needs 4.4e9 cylinders and is discarded, while
        // the Amiga-ish one lands on 2^25 heads and stands.
        let g = synthesize_geometry(u64::MAX / 8, 32768).unwrap();
        assert_eq!((g.cylinders, g.heads, g.sectors), (65535, 1 << 25, 32));
        assert!(g.cylinder_blocks() <= u32::MAX as u64);
    }

    #[test]
    fn geometry_refuses_an_unsupported_block_size() {
        for bad in [0usize, 256, 768, 65536] {
            assert_eq!(
                synthesize_geometry(1024 * 1024 * 1024, bad),
                Err(GeometryError::UnsupportedBlockSize { block_size: bad })
            );
        }
    }

    #[test]
    fn geometry_error_displays_a_line() {
        assert_eq!(
            alloc::format!(
                "{}",
                GeometryError::TooSmall {
                    total_bytes: 16383,
                    block_size: 512,
                    minimum_bytes: 16384
                }
            ),
            "16383 bytes is too small for a geometry in 512-byte blocks: \
             at least 16384 bytes are needed for one cylinder"
        );
        assert_eq!(
            alloc::format!(
                "{}",
                GeometryError::TooLarge {
                    total_bytes: 1,
                    block_size: 512
                }
            ),
            "1 bytes in 512-byte blocks exceeds what the RDB's 32-bit geometry \
             fields can describe"
        );
    }

    #[test]
    fn seal_error_displays_a_line() {
        assert_eq!(
            alloc::format!("{}", SealError::SummedLongsTooShort { summed_longs: 1 }),
            "SummedLongs 1 is below the 3 needed to cover ChkSum itself"
        );
        assert_eq!(
            alloc::format!(
                "{}",
                SealError::SummedLongsTooLong {
                    summed_longs: 200,
                    capacity: 128
                }
            ),
            "SummedLongs 200 exceeds the 128 longwords the block holds"
        );
    }

    // ---- the write path: RdbBuilder -------------------------------

    /// A zeroed target — the state every refusal test asserts the sink
    /// is still in afterwards.
    fn blank_disk(blocks: usize, bs: usize) -> MemDisk {
        MemDisk {
            data: vec![0u8; blocks * bs],
            block_size: bs,
        }
    }

    /// Ten mebibytes at 512-byte blocks: 640 cylinders of 32 blocks, so
    /// one cylinder is 16 KiB and the arithmetic in the assertions below
    /// stays checkable by eye.
    const TEN_MIB: u64 = 10 * 1024 * 1024;
    const TEN_MIB_BLOCKS: usize = (TEN_MIB / 512) as usize;

    /// Build into a blank disk and hand back both. Every builder test
    /// then parses what it wrote: the disk is the only authority on what
    /// is on the disk, and a layout that agrees with itself proves
    /// nothing.
    fn build_on(builder: RdbBuilder, blocks: usize, bs: usize) -> (MemDisk, RdbLayout, Rdb) {
        let mut disk = blank_disk(blocks, bs);
        let layout = builder.build(&mut disk).expect("build");
        let rdb = Rdb::parse(&mut disk).expect("parse back");
        assert_eq!(rdb.validate(), Vec::new(), "builder produced a bad layout");
        (disk, layout, rdb)
    }

    /// A build that must be refused, and must have written nothing.
    fn assert_refused(builder: RdbBuilder, blocks: usize, bs: usize, expected: BuildError<()>) {
        let mut disk = blank_disk(blocks, bs);
        assert_eq!(builder.build(&mut disk), Err(expected));
        assert!(
            disk.data.iter().all(|&b| b == 0),
            "a refused build wrote to the sink"
        );
    }

    /// The whole round trip: two partitions in, an image out, and every
    /// field read back off the disk — geometry, extents, names, flags,
    /// dostypes and the envec defaults.
    #[test]
    fn builder_round_trips_through_parse() {
        let (_disk, layout, rdb) = build_on(
            RdbBuilder::for_size(TEN_MIB, 512)
                .unwrap()
                .partition(
                    PartitionSpec::by_size(4 * 1024 * 1024)
                        .bootable(5)
                        .dos_type(0x444F_5307),
                )
                .partition(
                    PartitionSpec::by_size(2 * 1024 * 1024)
                        .named("WORK")
                        .size_block_longs(256),
                ),
            TEN_MIB_BLOCKS,
            512,
        );

        // The RDSK, as the geometry and the reserved-area policy decided.
        assert_eq!(rdb.rdsk_block, 0);
        assert_eq!(rdb.block_bytes, 512);
        assert_eq!((rdb.cylinders, rdb.heads, rdb.sectors), (640, 1, 32));
        assert_eq!(rdb.cyl_blocks, 32);
        assert_eq!((rdb.rdb_blocks_lo, rdb.rdb_blocks_hi), (0, 31));
        assert_eq!((rdb.lo_cylinder, rdb.hi_cylinder), (1, 639));
        assert_eq!(rdb.high_rdsk_block, 2);
        assert_eq!(rdb.flags, 0x7);
        assert_eq!(rdb.host_id, 7);
        assert_eq!(rdb.drive_init, CHAIN_END);
        assert_eq!(rdb.filesys_header_list, CHAIN_END);
        assert_eq!(rdb.bad_block_list, CHAIN_END);
        assert!(rdb.filesystems.is_empty() && rdb.bad_blocks.is_empty());

        // 4 MiB over a 16 KiB cylinder is 256 cylinders exactly, from
        // cylinder 1; 2 MiB is 128, packed straight after it.
        let a = &rdb.partitions[0];
        assert_eq!(a.part_block, 1);
        assert_eq!(a.name, "DH0");
        assert_eq!((a.low_cyl, a.high_cyl), (1, 256));
        assert_eq!((a.start_lba, a.block_len), (32, 256 * 32));
        assert_eq!(a.cylinder_blocks, 32);
        assert!(a.bootable && !a.no_automount);
        assert_eq!(a.boot_pri, 5);
        assert_eq!(a.dos_type, 0x444F_5307);

        let b = &rdb.partitions[1];
        assert_eq!(b.part_block, 2);
        assert_eq!(b.name, "WORK");
        assert_eq!((b.low_cyl, b.high_cyl), (257, 384));
        assert_eq!((b.start_lba, b.block_len), (257 * 32, 128 * 32));
        assert!(!b.bootable);
        assert_eq!(b.dos_type, envec_defaults::DOS_TYPE);
        assert_eq!(b.size_block_longs, 256);

        // The envec defaults, in the units the format stores them in.
        for p in &rdb.partitions {
            assert_eq!(p.num_buffers, 30);
            assert_eq!(p.buf_mem_type, 0);
            assert_eq!(p.max_transfer, 0x00FF_FFFF);
            assert_eq!(p.mask, 0x7FFF_FFFE);
            // de_TableSize 16 means the tail fields are *absent*, not
            // zero — which is what rdbtool writes and what a
            // round-tripping consumer must not turn into a zero.
            assert_eq!((p.baud, p.control, p.boot_blocks), (None, None, None));
            assert_eq!(p.envec_raw.len(), 17);
            assert_eq!(p.envec_raw[de::TABLE_SIZE], 16);
            assert_eq!(p.envec_raw[de::SEC_ORG], 0);
            assert_eq!(p.envec_raw[de::SECTORS_PER_BLOCK], 1);
            assert_eq!(p.envec_raw[de::SURFACES], 1);
            assert_eq!(p.envec_raw[de::BLOCKS_PER_TRACK], 32);
            assert_eq!(p.envec_raw[de::RESERVED], 2);
            assert_eq!(p.envec_raw[de::PRE_ALLOC], 0);
            assert_eq!(p.envec_raw[de::INTERLEAVE], 0);
        }
        assert_eq!(rdb.partitions[0].size_block_longs, 128);

        // And the layout says the same as the disk, since a caller may
        // act on it without re-parsing.
        assert_eq!(layout.rdsk_block, rdb.rdsk_block);
        assert_eq!(layout.rdb_blocks_hi, rdb.rdb_blocks_hi);
        assert_eq!(layout.high_rdsk_block, rdb.high_rdsk_block);
        assert_eq!(layout.lo_cylinder, rdb.lo_cylinder);
        for (placed, parsed) in layout.partitions.iter().zip(&rdb.partitions) {
            assert_eq!(placed.part_block, parsed.part_block);
            assert_eq!(placed.name, parsed.name);
            assert_eq!(
                (placed.low_cyl, placed.high_cyl),
                (parsed.low_cyl, parsed.high_cyl)
            );
            assert_eq!(
                (placed.start_lba, placed.block_len),
                (parsed.start_lba, parsed.block_len)
            );
        }
    }

    /// Assigned names step around the explicit ones wherever they
    /// appear, including an explicit name the assignment would otherwise
    /// have reached later.
    #[test]
    fn builder_assigns_names_around_explicit_ones() {
        let (_disk, _layout, rdb) = build_on(
            RdbBuilder::for_size(TEN_MIB, 512)
                .unwrap()
                .partition(PartitionSpec::by_size(1024 * 1024))
                .partition(PartitionSpec::by_size(1024 * 1024).named("DH0"))
                .partition(PartitionSpec::by_size(1024 * 1024))
                .partition(PartitionSpec::by_size(1024 * 1024).named("SCRATCH")),
            TEN_MIB_BLOCKS,
            512,
        );
        let names: Vec<&str> = rdb.partitions.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["DH1", "DH0", "DH2", "SCRATCH"]);
    }

    /// Sizes round **down** to whole cylinders, pinned against what
    /// `rdbtool` 0.8.1 does with the same requests on the same geometry
    /// (`create chs=1000,4,63`, cylinder = 129 024 bytes): 10 MiB
    /// becomes cylinders 1..=81 and 3 MB the 23 that follow, exactly as
    /// `rdbtool` places them.
    #[test]
    fn builder_size_rounds_down_like_rdbtool() {
        let geometry = Geometry {
            cylinders: 1000,
            heads: 4,
            sectors: 63,
            block_size: 512,
        };
        let (_disk, _layout, rdb) = build_on(
            RdbBuilder::new(geometry)
                .partition(PartitionSpec::by_size(10 * 1024 * 1024))
                .partition(PartitionSpec::by_size(3_000_000))
                // Either side of a cylinder boundary: one byte short of
                // two cylinders is one cylinder, exactly two is two.
                .partition(PartitionSpec::by_size(258_047))
                .partition(PartitionSpec::by_size(258_048)),
            30_000,
            512,
        );
        let extents: Vec<(u32, u32)> = rdb
            .partitions
            .iter()
            .map(|p| (p.low_cyl, p.high_cyl))
            .collect();
        assert_eq!(extents, [(1, 81), (82, 104), (105, 105), (106, 107)]);
    }

    /// Explicit cylinder ranges go exactly where they say, and a sized
    /// partition after one packs from the cylinder it left free.
    #[test]
    fn builder_places_explicit_cylinder_ranges() {
        let (_disk, _layout, rdb) = build_on(
            RdbBuilder::for_size(TEN_MIB, 512)
                .unwrap()
                .partition(PartitionSpec::by_cylinders(100, 199))
                .partition(PartitionSpec::by_size(1024 * 1024))
                // One cylinder is legal: de_HighCyl is inclusive.
                .partition(PartitionSpec::by_cylinders(500, 500)),
            TEN_MIB_BLOCKS,
            512,
        );
        let extents: Vec<(u32, u32)> = rdb
            .partitions
            .iter()
            .map(|p| (p.low_cyl, p.high_cyl))
            .collect();
        assert_eq!(extents, [(100, 199), (200, 263), (500, 500)]);
    }

    /// A partition-less RDB is legal — `rdbtool`'s `create` + `init`
    /// produces one — and is what a caller that partitions in a later
    /// step wants.
    #[test]
    fn builder_writes_an_rdb_with_no_partitions() {
        let (disk, layout, rdb) = build_on(
            RdbBuilder::for_size(TEN_MIB, 512).unwrap(),
            TEN_MIB_BLOCKS,
            512,
        );
        assert!(rdb.partitions.is_empty());
        assert!(layout.partitions.is_empty());
        // The chain head is CHAIN_END, not 0: block 0 is a real address.
        assert_eq!(be32(&disk.data, rdsk::PARTITION_LIST), CHAIN_END);
        // Nothing but the RDSK is in use, but the area is still reserved.
        assert_eq!(rdb.high_rdsk_block, 0);
        assert_eq!(rdb.rdb_blocks_hi, 31);
        assert_eq!(rdb.lo_cylinder, 1);
    }

    /// 4 KB device blocks: every LBA is in *those* blocks, and
    /// `de_SizeBlock` follows the device block size the way `rdbtool`
    /// writes it (1024 longwords) rather than staying at 128.
    #[test]
    fn builder_round_trips_at_4k_blocks() {
        let (_disk, _layout, rdb) = build_on(
            RdbBuilder::for_size(TEN_MIB, 4096)
                .unwrap()
                .partition(PartitionSpec::by_size(4 * 1024 * 1024).bootable(0)),
            (TEN_MIB / 4096) as usize,
            4096,
        );
        assert_eq!(rdb.block_bytes, 4096);
        assert_eq!((rdb.cylinders, rdb.heads, rdb.sectors), (80, 1, 32));
        assert_eq!(rdb.cyl_blocks, 32);
        assert_eq!((rdb.rdb_blocks_lo, rdb.rdb_blocks_hi), (0, 31));
        assert_eq!(rdb.lo_cylinder, 1);

        let p = &rdb.partitions[0];
        // A cylinder is 32 * 4096 = 128 KiB, so 4 MiB is 32 of them.
        assert_eq!((p.low_cyl, p.high_cyl), (1, 32));
        assert_eq!((p.start_lba, p.block_len), (32, 32 * 32));
        assert_eq!(p.size_block_longs, 1024);
        assert_eq!(p.max_transfer, 0x00FF_FFFF);
        assert_eq!(p.mask, 0x7FFF_FFFE);
    }

    /// The reserved area is `rdbtool`'s first cylinder by default, and
    /// grows past it — pushing `rdb_LoCylinder` up with it — when the
    /// `PART` blocks plus headroom do not fit. The alternative, which
    /// this crate will not produce, is RDB blocks written into the first
    /// partition.
    #[test]
    fn reserved_area_grows_past_the_first_cylinder_when_needed() {
        let mut builder = RdbBuilder::for_size(TEN_MIB, 512).unwrap();
        for _ in 0..40 {
            builder = builder.partition(PartitionSpec::by_size(16 * 1024));
        }
        let (_disk, layout, rdb) = build_on(builder, TEN_MIB_BLOCKS, 512);

        // 40 PART blocks + the RDSK is 41, past the 32-block cylinder,
        // so the area is 41 + RDB_HEADROOM_BLOCKS and the first
        // partition starts on cylinder 2 rather than 1.
        assert_eq!(rdb.rdb_blocks_hi, 41 + RDB_HEADROOM_BLOCKS as u32 - 1);
        assert_eq!(rdb.high_rdsk_block, 40);
        assert_eq!(rdb.lo_cylinder, 2);
        assert_eq!(layout.partitions[0].low_cyl, 2);
        assert_eq!(rdb.partitions.len(), 40);
        assert_eq!(rdb.partitions[39].part_block, 40);
    }

    /// An explicit reserved area is honoured as given — the override a
    /// caller reproducing an existing image needs.
    #[test]
    fn explicit_reserved_blocks_are_honoured() {
        let (_disk, _layout, rdb) = build_on(
            RdbBuilder::for_size(TEN_MIB, 512)
                .unwrap()
                .reserved_blocks(64)
                .partition(PartitionSpec::by_size(1024 * 1024)),
            TEN_MIB_BLOCKS,
            512,
        );
        assert_eq!(rdb.rdb_blocks_hi, 63);
        assert_eq!(rdb.lo_cylinder, 2);
        assert_eq!(rdb.partitions[0].low_cyl, 2);
    }

    /// Every way a layout can fail to fit, each refused **before a byte
    /// is written** — the property the whole build path is shaped
    /// around, asserted by handing each case a disk of zeros and
    /// checking it is still zeros afterwards.
    #[test]
    fn build_refuses_a_layout_that_does_not_fit() {
        let ten_mib = || RdbBuilder::for_size(TEN_MIB, 512).unwrap();

        // A cylinder range past the geometry's last cylinder.
        assert_refused(
            ten_mib().partition(PartitionSpec::by_cylinders(600, 700)),
            TEN_MIB_BLOCKS,
            512,
            BuildError::PartitionPastEndOfDisk {
                name: String::from("DH0"),
                high_cyl: 700,
                last_cylinder: 639,
            },
        );

        // A size larger than the space left after the RDB area.
        assert_refused(
            ten_mib().partition(PartitionSpec::by_size(TEN_MIB)),
            TEN_MIB_BLOCKS,
            512,
            BuildError::PartitionPastEndOfDisk {
                name: String::from("DH0"),
                high_cyl: 640,
                last_cylinder: 639,
            },
        );

        // A partition reaching into the reserved RDB area.
        assert_refused(
            ten_mib().partition(PartitionSpec::by_cylinders(0, 10)),
            TEN_MIB_BLOCKS,
            512,
            BuildError::PartitionOverlapsRdbArea {
                name: String::from("DH0"),
                low_cyl: 0,
                lo_cylinder: 1,
            },
        );

        // Two partitions claiming the same cylinders.
        assert_refused(
            ten_mib()
                .partition(PartitionSpec::by_cylinders(10, 20))
                .partition(PartitionSpec::by_cylinders(15, 25)),
            TEN_MIB_BLOCKS,
            512,
            BuildError::PartitionsOverlap {
                a_name: String::from("DH0"),
                b_name: String::from("DH1"),
                low_cyl: 15,
                high_cyl: 20,
            },
        );

        // More PART blocks than the caller's own reserved area holds.
        let mut cramped = ten_mib().reserved_blocks(4);
        for _ in 0..5 {
            cramped = cramped.partition(PartitionSpec::by_size(16 * 1024));
        }
        assert_refused(
            cramped,
            TEN_MIB_BLOCKS,
            512,
            BuildError::RdbAreaTooSmall {
                needed: 6,
                available: 4,
            },
        );

        // A target smaller than the layout: the geometry says 640
        // cylinders, the sink has 100 blocks.
        assert_refused(
            ten_mib().partition(PartitionSpec::by_size(1024 * 1024)),
            100,
            512,
            BuildError::SinkTooSmall {
                needed: 65 * 32,
                available: 100,
            },
        );

        // A geometry with no blocks in it at all.
        let empty = Geometry {
            cylinders: 0,
            heads: 1,
            sectors: 32,
            block_size: 512,
        };
        assert_refused(
            RdbBuilder::new(empty),
            TEN_MIB_BLOCKS,
            512,
            BuildError::EmptyGeometry { geometry: empty },
        );

        // A sink whose blocks are not the geometry's.
        assert_refused(
            ten_mib().partition(PartitionSpec::by_size(1024 * 1024)),
            (TEN_MIB / 4096) as usize,
            4096,
            BuildError::BlockSizeMismatch {
                geometry: 512,
                sink: 4096,
            },
        );
    }

    /// The same discipline for specs that are wrong rather than too big:
    /// nothing written, and an error naming the partition.
    #[test]
    fn build_refuses_a_malformed_spec() {
        let ten_mib = || RdbBuilder::for_size(TEN_MIB, 512).unwrap();

        assert_refused(
            ten_mib().partition(PartitionSpec::by_cylinders(200, 100)),
            TEN_MIB_BLOCKS,
            512,
            BuildError::CylindersInverted {
                name: String::from("DH0"),
                low_cyl: 200,
                high_cyl: 100,
            },
        );

        assert_refused(
            ten_mib()
                .partition(PartitionSpec::by_size(1024 * 1024).named("DH0"))
                .partition(PartitionSpec::by_size(1024 * 1024).named("DH0")),
            TEN_MIB_BLOCKS,
            512,
            BuildError::DuplicateName {
                name: String::from("DH0"),
            },
        );

        let long = "X".repeat(32);
        assert_refused(
            ten_mib().partition(PartitionSpec::by_size(1024 * 1024).named(&long)),
            TEN_MIB_BLOCKS,
            512,
            BuildError::InvalidName {
                name: long,
                max: 31,
            },
        );

        // Below one 16 KiB cylinder there is no partition to write, and
        // rounding up would hand out space the caller did not ask for.
        assert_refused(
            ten_mib().partition(PartitionSpec::by_size(16 * 1024 - 1)),
            TEN_MIB_BLOCKS,
            512,
            BuildError::PartitionTooSmall {
                name: String::from("DH0"),
                bytes: 16 * 1024 - 1,
                cylinder_bytes: 16 * 1024,
            },
        );
    }

    /// The write order: `PART` blocks first, `RDSK` last, so an
    /// interrupted build leaves a disk with no partition table rather
    /// than one pointing at blocks that were never written.
    #[test]
    fn build_writes_the_rdsk_last() {
        /// A sink that fails on the *n*th write, recording what it got.
        struct FlakySink {
            data: Vec<u8>,
            writes: usize,
            fail_after: usize,
        }
        impl BlockSink for FlakySink {
            type Error = ();
            fn block_size(&self) -> usize {
                512
            }
            fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
                if self.writes == self.fail_after {
                    return Err(());
                }
                self.writes += 1;
                let off = lba as usize * 512;
                self.data[off..off + 512].copy_from_slice(buf);
                Ok(())
            }
            fn block_count(&self) -> Option<u64> {
                Some(self.data.len() as u64 / 512)
            }
        }

        let mut sink = FlakySink {
            data: vec![0u8; TEN_MIB_BLOCKS * 512],
            writes: 0,
            fail_after: 2,
        };
        let err = RdbBuilder::for_size(TEN_MIB, 512)
            .unwrap()
            .partition(PartitionSpec::by_size(1024 * 1024))
            .partition(PartitionSpec::by_size(1024 * 1024))
            .partition(PartitionSpec::by_size(1024 * 1024))
            .build(&mut sink)
            .unwrap_err();
        assert_eq!(err, BuildError::Io(()));
        // Two PART blocks landed; block 0 is untouched, so the disk has
        // no RDSK and a parse says so rather than following a chain into
        // blocks that do not exist.
        assert_eq!(be32(&sink.data, 512), id::PART);
        assert!(sink.data[..512].iter().all(|&b| b == 0));
        let mut disk = MemDisk {
            data: sink.data,
            block_size: 512,
        };
        assert_eq!(Rdb::parse(&mut disk), Err(RdbError::NoRdsk));
    }

    /// Every `BuildError` renders one line fit to show a user, in
    /// `no_std` as much as `std` — the same contract the parse and
    /// geometry errors hold to.
    #[test]
    fn build_errors_display_as_one_useful_line() {
        let cases: [(BuildError<&str>, &str); 6] = [
            (
                BuildError::Io("device is read-only"),
                "writing a block failed: device is read-only",
            ),
            (
                BuildError::BlockSizeMismatch {
                    geometry: 512,
                    sink: 4096,
                },
                "the geometry is in 512-byte blocks but the sink writes 4096-byte blocks",
            ),
            (
                BuildError::DuplicateName {
                    name: String::from("DH0"),
                },
                "two partitions are both named \"DH0\"",
            ),
            (
                BuildError::PartitionPastEndOfDisk {
                    name: String::from("DH1"),
                    high_cyl: 700,
                    last_cylinder: 639,
                },
                "partition \"DH1\" ends on cylinder 700, past the disk's last cylinder 639",
            ),
            (
                BuildError::RdbAreaTooSmall {
                    needed: 6,
                    available: 4,
                },
                "the RDB area holds 4 blocks but the layout needs 6",
            ),
            (
                BuildError::PartitionTooSmall {
                    name: String::from("DH0"),
                    bytes: 100,
                    cylinder_bytes: 16384,
                },
                "partition \"DH0\" asks for 100 bytes, less than the 16384-byte cylinder \
                 that is the smallest partition",
            ),
        ];
        for (err, expected) in cases {
            assert_eq!(alloc::format!("{err}"), expected);
        }
    }

    // ---- the write path: FSHD + LSEG -------------------------------

    /// A driver binary of `len` bytes — not hunk format, and
    /// deliberately so: this crate does not parse hunks (a founding
    /// non-goal) and `rdbtool` does not either, which is how the FSHD
    /// defaults were observed in the first place.
    fn fake_driver(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i % 251) as u8).collect()
    }

    /// One filesystem in, an image out, and the driver read back off the
    /// disk — the `DOS\x07` shipping path end to end.
    #[test]
    fn builder_round_trips_a_filesystem() {
        // 1000 bytes over a 492-byte payload is three blocks, the last
        // of them partial — the case the SummedLongs rule is about.
        let driver = fake_driver(1000);
        let (mut disk, layout, rdb) = build_on(
            RdbBuilder::for_size(TEN_MIB, 512)
                .unwrap()
                .partition(PartitionSpec::by_size(4 * 1024 * 1024).dos_type(0x444F_5307))
                .filesystem(
                    FileSystemSpec::new(0x444F_5307, driver.clone())
                        .version(43, 4)
                        .stack_size(8192)
                        .priority(10),
                ),
            TEN_MIB_BLOCKS,
            512,
        );

        // The FSHD follows the single PART block, its LSEG chain follows
        // it, and the RDSK points at the head.
        assert_eq!(layout.filesystems.len(), 1);
        let placed = &layout.filesystems[0];
        assert_eq!(placed.fshd_block, 2);
        assert_eq!(placed.seg_list_blocks, 3);
        assert_eq!(placed.lseg_block_count, 3);
        assert_eq!(rdb.filesys_header_list, 2);
        assert_eq!(rdb.high_rdsk_block, 5);

        assert_eq!(rdb.filesystems.len(), 1);
        let f = &rdb.filesystems[0];
        assert_eq!(f.fshd_block, 2);
        assert_eq!(f.dos_type, 0x444F_5307);
        assert_eq!((f.version_major(), f.version_minor()), (43, 4));
        assert_eq!(f.host_id, fshd_defaults::HOST_ID);
        assert_eq!(f.flags, 0);
        assert_eq!(f.seg_list_blocks, 3);
        // Only the fields that were set are patched — and SegList, which
        // has a chain to point at. Everything else is absent, not zero.
        assert_eq!(
            f.patch_flags,
            fshd_patch::STACK_SIZE
                | fshd_patch::PRIORITY
                | fshd_patch::SEG_LIST
                | fshd_patch::GLOBAL_VEC
        );
        assert_eq!(f.stack_size, Some(8192));
        assert_eq!(f.priority, Some(10));
        assert_eq!(f.global_vec, Some(-1));
        assert_eq!(
            (f.node_type, f.task, f.lock, f.handler, f.startup),
            (None, None, None, None, None)
        );

        // The reassembled binary is the payload padded out to a whole
        // block: LSEG records no byte count, so the slack is unavoidable
        // and documented rather than trimmed by a guess.
        let loaded = rdb.load_filesystem(f, &mut disk).unwrap();
        assert_eq!(loaded.len(), 3 * LSEG_PAYLOAD);
        assert_eq!(&loaded[..driver.len()], &driver[..]);
        assert!(loaded[driver.len()..].iter().all(|&b| b == 0));

        assert_eq!(rdb.validate_seg_lists(&mut disk).unwrap(), Vec::new());
    }

    /// `SummedLongs` on the final, partial `LSEG` block is the number of
    /// longwords **actually summed** — five header longwords plus the
    /// payload's whole longwords, floored — not the whole block.
    ///
    /// The expectations are `rdbtool` 0.8.1's own output, read out of
    /// images it wrote at each of these lengths. It matters beyond
    /// cosmetics: `rdbtool fsget` recovers the driver's byte length from
    /// these counts, so a block-sized count on the last block would hand
    /// a reader a driver with slack glued to the end of it.
    #[test]
    fn lseg_summed_longs_match_rdbtool_0_8_1() {
        for (len, expected) in [
            (1usize, &[5u32][..]),
            (4, &[6][..]),
            (5, &[6][..]),
            (492, &[128][..]),
            (493, &[128, 5][..]),
            (495, &[128, 5][..]),
            (496, &[128, 6][..]),
            (984, &[128, 128][..]),
            (985, &[128, 128, 5][..]),
            (2560, &[128, 128, 128, 128, 128, 30][..]),
            (2563, &[128, 128, 128, 128, 128, 30][..]),
        ] {
            let (disk, layout, _rdb) = build_on(
                RdbBuilder::for_size(TEN_MIB, 512)
                    .unwrap()
                    .filesystem(FileSystemSpec::new(
                        envec_defaults::DOS_TYPE,
                        fake_driver(len),
                    )),
                TEN_MIB_BLOCKS,
                512,
            );
            let placed = &layout.filesystems[0];
            assert_eq!(
                placed.lseg_block_count as usize,
                expected.len(),
                "len {len}"
            );
            let summed: Vec<u32> = (0..placed.lseg_block_count as usize)
                .map(|i| {
                    let base = (placed.seg_list_blocks as usize + i) * 512;
                    be32(&disk.data, base + hdr::SUMMED_LONGS)
                })
                .collect();
            assert_eq!(summed, expected, "len {len}");
        }
    }

    /// The AROS case in full: a `DOS\x07` partition and the `DOS\x07`
    /// handler that lets a 3.1-era ROM mount it, beside the `DOS\x03`
    /// filesystem the other partition wants. Two FSHDs, chained in the
    /// order added, each with its own LSEG run.
    #[test]
    fn builder_writes_two_filesystems() {
        let dos3 = fake_driver(600);
        let dos7 = fake_driver(1500);
        let (mut disk, layout, rdb) = build_on(
            RdbBuilder::for_size(TEN_MIB, 512)
                .unwrap()
                .partition(PartitionSpec::by_size(1024 * 1024).dos_type(0x444F_5307))
                .partition(PartitionSpec::by_size(1024 * 1024))
                .filesystem(FileSystemSpec::new(0x444F_5307, dos7.clone()).version(45, 13))
                .filesystem(FileSystemSpec::new(0x444F_5303, dos3.clone())),
            TEN_MIB_BLOCKS,
            512,
        );

        // PARTs at 1..=2; then FSHD 3 with LSEG 4..=7, FSHD 8 with
        // LSEG 9..=10.
        let places: Vec<(u64, u32, u32)> = layout
            .filesystems
            .iter()
            .map(|f| (f.fshd_block, f.seg_list_blocks, f.lseg_block_count))
            .collect();
        assert_eq!(places, [(3, 4, 4), (8, 9, 2)]);
        assert_eq!(rdb.high_rdsk_block, 10);

        let dos_types: Vec<u32> = rdb.filesystems.iter().map(|f| f.dos_type).collect();
        assert_eq!(dos_types, [0x444F_5307, 0x444F_5303]);
        assert_eq!(
            (
                rdb.filesystems[0].version_major(),
                rdb.filesystems[0].version_minor()
            ),
            (45, 13)
        );

        // Each partition's dostype has a handler in the image to match.
        for p in &rdb.partitions {
            assert!(rdb.filesystems.iter().any(|f| f.dos_type == p.dos_type));
        }
        for (f, want) in rdb.filesystems.iter().zip([&dos7, &dos3]) {
            let loaded = rdb.load_filesystem(f, &mut disk).unwrap();
            assert_eq!(&loaded[..want.len()], &want[..]);
        }
        assert_eq!(rdb.validate_seg_lists(&mut disk).unwrap(), Vec::new());
    }

    /// A filesystem with no binary at all: legal, and the header that
    /// only patches device-node fields for a ROM filesystem. One block,
    /// `fhb_SegListBlocks` [`CHAIN_END`], and the `SegList` bit clear
    /// because there is nothing to patch in.
    #[test]
    fn builder_writes_a_filesystem_with_no_seglist() {
        let (mut disk, layout, rdb) = build_on(
            RdbBuilder::for_size(TEN_MIB, 512)
                .unwrap()
                .filesystem(FileSystemSpec::new(0x444F_5307, Vec::new())),
            TEN_MIB_BLOCKS,
            512,
        );
        assert_eq!(layout.filesystems[0].lseg_block_count, 0);
        assert_eq!(layout.filesystems[0].seg_list_blocks, CHAIN_END);
        assert_eq!(rdb.high_rdsk_block, 1);
        let f = &rdb.filesystems[0];
        assert_eq!(f.seg_list_blocks, CHAIN_END);
        assert_eq!(f.patch_flags & fshd_patch::SEG_LIST, 0);
        assert!(rdb.load_filesystem(f, &mut disk).unwrap().is_empty());
    }

    /// 4 KB device blocks: the payload per `LSEG` block is `block_size -
    /// 20` there too, so the same driver needs a tenth of the blocks and
    /// a full block sums 1024 longwords.
    #[test]
    fn builder_round_trips_a_filesystem_at_4k_blocks() {
        let driver = fake_driver(9000);
        let (mut disk, layout, rdb) = build_on(
            RdbBuilder::for_size(TEN_MIB, 4096)
                .unwrap()
                .partition(PartitionSpec::by_size(4 * 1024 * 1024).dos_type(0x444F_5307))
                .filesystem(FileSystemSpec::new(0x444F_5307, driver.clone())),
            (TEN_MIB / 4096) as usize,
            4096,
        );

        // 4076 bytes a block: 9000 bytes is three of them.
        let placed = &layout.filesystems[0];
        assert_eq!(placed.lseg_block_count, 3);
        assert_eq!((placed.fshd_block, placed.seg_list_blocks), (2, 3));
        let summed: Vec<u32> = (0..3)
            .map(|i| be32(&disk.data, (3 + i) * 4096 + hdr::SUMMED_LONGS))
            .collect();
        // Two full blocks, then 9000 - 2 * 4076 = 848 payload bytes.
        assert_eq!(summed, [1024, 1024, 5 + 848 / 4]);

        let f = &rdb.filesystems[0];
        let loaded = rdb.load_filesystem(f, &mut disk).unwrap();
        assert_eq!(loaded.len(), 3 * lseg_payload_bytes(4096));
        assert_eq!(&loaded[..driver.len()], &driver[..]);
        assert_eq!(rdb.validate_seg_lists(&mut disk).unwrap(), Vec::new());
    }

    /// The RDB area grows to fit a driver rather than spilling into the
    /// first partition — the growth path the FSHD payload is the first
    /// realistic reason to take. (`rdbtool` 0.8.1 refuses this case
    /// outright: "ERROR adding filesystem! (no space in RDB left)",
    /// having fixed the area at one cylinder when the disk was created.)
    #[test]
    fn reserved_area_grows_to_fit_a_filesystem() {
        let (_disk, layout, rdb) = build_on(
            RdbBuilder::for_size(TEN_MIB, 512)
                .unwrap()
                .partition(PartitionSpec::by_size(1024 * 1024))
                .filesystem(FileSystemSpec::new(0x444F_5307, fake_driver(50_000))),
            TEN_MIB_BLOCKS,
            512,
        );
        // 50 000 bytes over a 492-byte payload is 102 LSEG blocks, plus
        // the FSHD, the PART and the RDSK: 105 blocks, well past the
        // 32-block first cylinder.
        assert_eq!(layout.filesystems[0].lseg_block_count, 102);
        assert_eq!(rdb.high_rdsk_block, 104);
        assert_eq!(rdb.rdb_blocks_hi, 105 + RDB_HEADROOM_BLOCKS as u32 - 1);
        // The area now spans four 32-block cylinders, so partitions
        // start on cylinder 4 rather than 1.
        assert_eq!(rdb.lo_cylinder, 4);
        assert_eq!(rdb.partitions[0].low_cyl, 4);
    }

    /// A filesystem that does not fit is refused **before a byte is
    /// written**, exactly as an over-large partition is — whether the
    /// ceiling is the caller's own reserved area or the target's size.
    #[test]
    fn build_refuses_a_filesystem_that_does_not_fit() {
        // An explicit area with no room for the LSEG chain.
        assert_refused(
            RdbBuilder::for_size(TEN_MIB, 512)
                .unwrap()
                .reserved_blocks(32)
                .partition(PartitionSpec::by_size(1024 * 1024))
                .filesystem(FileSystemSpec::new(0x444F_5307, fake_driver(50_000))),
            TEN_MIB_BLOCKS,
            512,
            BuildError::RdbAreaTooSmall {
                needed: 105,
                available: 32,
            },
        );

        // A target smaller than the RDB area the driver forces.
        assert_refused(
            RdbBuilder::for_size(TEN_MIB, 512)
                .unwrap()
                .filesystem(FileSystemSpec::new(0x444F_5307, fake_driver(50_000))),
            64,
            512,
            BuildError::SinkTooSmall {
                // RDSK + FSHD + 102 LSEG blocks, plus the headroom the
                // default area adds; no partition reaches beyond it.
                needed: 104 + RDB_HEADROOM_BLOCKS,
                available: 64,
            },
        );
    }

    /// The round-trip property at its strongest: build an image, parse
    /// it, rebuild from nothing but the parsed values, and get the same
    /// bytes back.
    ///
    /// Everything the builder writes has to be reachable from the read
    /// API for this to pass — the geometry, the reserved area, each
    /// partition's cylinders and envec, each FSHD's patch flags and
    /// gated fields, and every driver binary — so it is a coverage
    /// assertion about the *read* surface as much as a fidelity one
    /// about the write surface.
    ///
    /// **The driver payload is a whole number of `LSEG` payloads on
    /// purpose.** `LSEG` records no byte count, so
    /// [`Rdb::load_filesystem`] returns the binary padded to a block
    /// boundary; feeding that back in reproduces the same *blocks*, but
    /// a driver that did not fill its last block would come back padded
    /// and the rebuilt final `LSEG` would sum the whole block where the
    /// original summed only as far as the driver reached. Byte-identity
    /// is therefore a property of block-aligned drivers, and the
    /// difference for any other is exactly two longwords in one block —
    /// which is the honest statement of what the format can round-trip,
    /// not a defect in the rebuild.
    #[test]
    fn rebuilding_from_the_parsed_values_reproduces_the_image() {
        let driver = fake_driver(2 * LSEG_PAYLOAD);
        let mut original = blank_disk(TEN_MIB_BLOCKS, 512);
        RdbBuilder::for_size(TEN_MIB, 512)
            .unwrap()
            .partition(
                PartitionSpec::by_size(4 * 1024 * 1024)
                    .bootable(5)
                    .dos_type(0x444F_5307),
            )
            .partition(
                PartitionSpec::by_size(2 * 1024 * 1024)
                    .named("WORK")
                    .size_block_longs(256),
            )
            .filesystem(
                FileSystemSpec::new(0x444F_5307, driver)
                    .version(45, 13)
                    .stack_size(8192)
                    .priority(10),
            )
            .build(&mut original)
            .expect("build");

        let rdb = Rdb::parse(&mut original).expect("parse");

        // Rebuild using only what the parse handed back.
        let mut builder = RdbBuilder::new(Geometry {
            cylinders: rdb.cylinders,
            heads: rdb.heads,
            sectors: rdb.sectors,
            block_size: rdb.block_bytes as usize,
        })
        .rdsk_block(rdb.rdsk_block as u32)
        .reserved_blocks(rdb.rdb_blocks_hi + 1)
        .flags(rdb.flags)
        .host_id(rdb.host_id);

        for p in &rdb.partitions {
            let mut spec = PartitionSpec::by_cylinders(p.low_cyl, p.high_cyl).named(&p.name);
            spec.bootable = p.bootable;
            spec.no_automount = p.no_automount;
            spec.boot_pri = p.boot_pri;
            spec.dos_type = p.dos_type;
            spec.size_block_longs = Some(p.size_block_longs);
            spec.num_buffers = p.num_buffers;
            spec.buf_mem_type = p.buf_mem_type;
            spec.max_transfer = p.max_transfer;
            spec.mask = p.mask;
            // The envec fields with no named accessor come off the raw
            // longwords, which is what they are there for.
            spec.sec_org = p.envec_raw[de::SEC_ORG];
            spec.sectors_per_block = p.envec_raw[de::SECTORS_PER_BLOCK];
            spec.reserved = p.envec_raw[de::RESERVED];
            spec.pre_alloc = p.envec_raw[de::PRE_ALLOC];
            spec.interleave = p.envec_raw[de::INTERLEAVE];
            builder = builder.partition(spec);
        }

        for f in &rdb.filesystems {
            let binary = rdb.load_filesystem(f, &mut original).expect("load driver");
            let mut spec = FileSystemSpec::new(f.dos_type, binary)
                .version(f.version_major(), f.version_minor());
            spec.host_id = f.host_id;
            spec.flags = f.flags;
            // Verbatim rather than derived: a rebuild must reproduce
            // bits this crate does not model, not re-decide them.
            spec.patch_flags = Some(f.patch_flags);
            spec.node_type = f.node_type;
            spec.task = f.task;
            spec.lock = f.lock;
            spec.handler = f.handler;
            spec.stack_size = f.stack_size;
            spec.priority = f.priority;
            spec.startup = f.startup;
            spec.global_vec = f.global_vec;
            builder = builder.filesystem(spec);
        }

        let mut rebuilt = blank_disk(TEN_MIB_BLOCKS, 512);
        builder.build(&mut rebuilt).expect("rebuild");
        assert_eq!(
            rebuilt.data, original.data,
            "a rebuild from the parsed values is not the same image"
        );
        assert_eq!(Rdb::parse(&mut rebuilt).unwrap(), rdb);
    }

    /// The differential smoke test: build an image here, and let
    /// `rdbtool` — the tool whose conventions every default in
    /// [`envec_defaults`] was read out of — say what it sees. Agreement
    /// on the partition extents is the claim; anything more is the
    /// round-trip suite's job.
    ///
    /// Gated on `AMIGA_RDB_DIFFERENTIAL=1` because it shells out to a
    /// tool that is not a build dependency of this crate; CI installs
    /// `amitools==0.8.1` — the pinned version every default here was
    /// observed against — and runs the gate in its own job.
    #[cfg(feature = "std")]
    #[test]
    fn rdbtool_reads_an_image_this_crate_built() {
        if !differential_enabled() {
            return;
        }

        let mut disk = blank_disk(TEN_MIB_BLOCKS, 512);
        RdbBuilder::for_size(TEN_MIB, 512)
            .unwrap()
            .partition(PartitionSpec::by_size(4 * 1024 * 1024).bootable(0))
            .partition(PartitionSpec::by_size(2 * 1024 * 1024).named("WORK"))
            .build(&mut disk)
            .expect("build");

        let path = std::env::temp_dir().join("amiga-rdb-differential.hdf");
        std::fs::write(&path, &disk.data).expect("write image");

        let out = std::process::Command::new("rdbtool")
            .arg(&path)
            .arg("list")
            .output()
            .expect("run rdbtool");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success(), "rdbtool failed: {stdout}");

        // `Partition: #0 'DH0'   1   256   8192  4.0Mi ...`
        let extents: Vec<(String, u32, u32)> = stdout
            .lines()
            .filter(|l| l.starts_with("Partition:"))
            .map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                (
                    f[2].trim_matches('\'').to_string(),
                    f[3].parse().unwrap(),
                    f[4].parse().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            extents,
            [
                (String::from("DH0"), 1, 256),
                (String::from("WORK"), 257, 384)
            ]
        );

        let _ = std::fs::remove_file(&path);
    }

    /// Is the differential suite switched on? `rdbtool` is not a build
    /// dependency, so these tests are opt-in.
    #[cfg(feature = "std")]
    fn differential_enabled() -> bool {
        std::env::var_os("AMIGA_RDB_DIFFERENTIAL").is_some()
    }

    /// A scratch path in the temp directory, named per test so the
    /// differential tests can run in parallel without fighting.
    #[cfg(feature = "std")]
    fn scratch(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(alloc::format!("amiga-rdb-differential-{name}"))
    }

    /// Run `rdbtool` over `image` with the given commands, and hand back
    /// its stdout.
    #[cfg(feature = "std")]
    fn rdbtool(image: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("rdbtool")
            .arg(image)
            .args(args)
            .output()
            .expect("run rdbtool");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "rdbtool {args:?} failed: {stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        stdout
    }

    /// The FSHD half of the differential, in this crate's direction:
    /// build an image carrying a driver and let `rdbtool` extract it.
    ///
    /// `fsget` is the sharp end of the comparison, because `rdbtool`
    /// recovers the driver's *byte length* from the `LSEG` blocks'
    /// `SummedLongs` — so a byte-for-byte match proves the chain, the
    /// split, the block order and the SummedLongs rule all at once, in a
    /// way no field-by-field assertion of ours could. `info` is checked
    /// alongside it for the FSHD fields themselves.
    #[cfg(feature = "std")]
    #[test]
    fn rdbtool_reads_a_filesystem_this_crate_built() {
        if !differential_enabled() {
            return;
        }

        // A length that is not a multiple of the 492-byte payload, so
        // the final partial block — the one the SummedLongs rule is
        // about — is what `fsget` has to get right. A multiple of four,
        // because SummedLongs counts longwords and cannot describe the
        // trailing one to three bytes of anything else.
        let driver = fake_driver(2564);
        let mut disk = blank_disk(TEN_MIB_BLOCKS, 512);
        RdbBuilder::for_size(TEN_MIB, 512)
            .unwrap()
            .partition(PartitionSpec::by_size(4 * 1024 * 1024).dos_type(0x444F_5307))
            .filesystem(
                FileSystemSpec::new(0x444F_5307, driver.clone())
                    .version(45, 13)
                    .stack_size(8192),
            )
            .build(&mut disk)
            .expect("build");

        let path = scratch("fshd.hdf");
        std::fs::write(&path, &disk.data).expect("write image");

        // `FileSystem #0 DOS7/0x444f5307 version=45.13 size=2564
        //  seg_list_blk=0x3 global_vec=0xffffffff`
        let info = rdbtool(&path, &["info"]);
        let line = info
            .lines()
            .find(|l| l.starts_with("FileSystem #0"))
            .unwrap_or_else(|| panic!("rdbtool saw no filesystem:\n{info}"))
            .to_string();
        for expected in [
            "DOS7/0x444f5307",
            "version=45.13",
            "size=2564",
            "global_vec=0xffffffff",
        ] {
            assert!(
                line.contains(expected),
                "{expected:?} missing from {line:?}"
            );
        }

        let extracted = scratch("fsget.bin");
        rdbtool(&path, &["fsget", "0", extracted.to_str().unwrap()]);
        assert_eq!(std::fs::read(&extracted).expect("read extracted"), driver);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&extracted);
    }

    /// The differential in the other direction: `rdbtool` creates the
    /// image and adds the filesystem, this crate parses it.
    ///
    /// The assertions are the FSHD defaults `rdbtool` 0.8.1 writes,
    /// which are the ones [`fshd_defaults`] documents and this crate
    /// therefore writes too — `fhb_PatchFlags` 0x180 (`SegList` and
    /// `GlobalVec` and nothing else), `fhb_GlobalVec` -1, `fhb_HostID` 0
    /// — so this is the test that would notice if a later amitools
    /// release moved them and left our defaults describing history.
    ///
    /// **The driver is a whole number of `LSEG` payloads on purpose.**
    /// `rdbtool` 0.8.1 writes a *reduced* `SummedLongs` on a final
    /// partial `LSEG` block while checksumming the whole block anyway,
    /// which produces a block that fails its own header whenever the
    /// driver's length is not a multiple of four — see
    /// [`RdbBuilder::fill_lseg`], and see
    /// [`rdbtool_writes_an_lseg_that_fails_its_own_checksum`], which
    /// pins that behaviour rather than working around it here.
    #[cfg(feature = "std")]
    #[test]
    fn this_crate_reads_a_filesystem_rdbtool_built() {
        if !differential_enabled() {
            return;
        }

        let driver = fake_driver(2 * LSEG_PAYLOAD);
        let path = scratch("rdbtool-fshd.hdf");
        rdbtool_fsadd(&path, &driver);

        let mut disk = MemDisk::new(std::fs::read(&path).expect("read image"));
        let rdb = Rdb::parse(&mut disk).expect("parse rdbtool's image");
        assert_eq!(rdb.validate(), Vec::new());
        assert_eq!(rdb.validate_seg_lists(&mut disk).unwrap(), Vec::new());

        assert_eq!(rdb.filesystems.len(), 1);
        let f = &rdb.filesystems[0];
        assert_eq!(f.dos_type, 0x444F_5307);
        assert_eq!((f.version_major(), f.version_minor()), (45, 13));
        assert_eq!(f.host_id, fshd_defaults::HOST_ID);
        assert_eq!(f.flags, fshd_defaults::FLAGS);
        assert_eq!(f.patch_flags, fshd_patch::SEG_LIST | fshd_patch::GLOBAL_VEC);
        assert_eq!(f.global_vec, Some(fshd_defaults::GLOBAL_VEC));
        assert_eq!(
            (f.node_type, f.task, f.lock, f.handler),
            (None, None, None, None)
        );
        assert_eq!((f.stack_size, f.priority, f.startup), (None, None, None));

        // The FSHD goes straight after the RDSK on a disk with no
        // partitions, and its chain straight after that.
        assert_eq!(f.fshd_block, 1);
        assert_eq!(f.seg_list_blocks, 2);
        let loaded = rdb.load_filesystem(f, &mut disk).unwrap();
        assert_eq!(loaded, driver);

        let _ = std::fs::remove_file(&path);
    }

    /// The finding the FSHD differential turned up, pinned so it stays a
    /// known quantity rather than a surprise: **`rdbtool` 0.8.1 writes a
    /// final partial `LSEG` block that fails its own checksum.**
    ///
    /// It reduces `SummedLongs` to the longwords the payload actually
    /// reaches — which is how `fsget` recovers a driver's byte length,
    /// and which this crate matches — but computes `ChkSum` over the
    /// whole block regardless, so summing the declared count does not
    /// give zero. Any reader that follows `SummedLongs` rejects the
    /// block, this crate's parser and a 68k ROM alike; `rdbtool` itself
    /// does not check, so it round-trips its own images happily.
    ///
    /// Asserted rather than worked around because a differential suite
    /// that quietly tolerated the oracle being wrong would be testing
    /// nothing. If a later amitools fixes it, this test fails and says
    /// so — which is the point, and why CI pins `amitools==0.8.1`.
    #[cfg(feature = "std")]
    #[test]
    fn rdbtool_writes_an_lseg_that_fails_its_own_checksum() {
        if !differential_enabled() {
            return;
        }

        // Three bytes past a whole payload, so the second block's
        // payload is not a whole number of longwords — which is exactly
        // when the two disagree: the bytes past the declared count are
        // the driver's trailing 1..=3, and they are not zero.
        let driver = fake_driver(LSEG_PAYLOAD + 3);
        let path = scratch("rdbtool-partial-lseg.hdf");
        rdbtool_fsadd(&path, &driver);

        let mut disk = MemDisk::new(std::fs::read(&path).expect("read image"));
        let rdb = Rdb::parse(&mut disk).expect("the RDSK/FSHD chains are fine");
        let f = &rdb.filesystems[0];
        // The declared count is the reduced one this crate also writes...
        let base = f.seg_list_blocks as usize * 512 + 512;
        assert_eq!(
            be32(&disk.data, base + hdr::SUMMED_LONGS),
            LSEG_HEADER_LONGS
        );
        // ...but the block does not sum to zero over it.
        assert_eq!(
            rdb.load_filesystem(f, &mut disk),
            Err(RdbError::BadChecksum {
                lba: f.seg_list_blocks as u64 + 1
            })
        );

        let _ = std::fs::remove_file(&path);
    }

    /// `rdbtool create + init + fsadd` at `path`, with `driver` as the
    /// filesystem binary. Overwrites whatever was there.
    #[cfg(feature = "std")]
    fn rdbtool_fsadd(path: &std::path::Path, driver: &[u8]) {
        let driver_path = path.with_extension("driver.bin");
        std::fs::write(&driver_path, driver).expect("write driver");
        let _ = std::fs::remove_file(path);

        let out = std::process::Command::new("rdbtool")
            .arg("-f")
            .arg(path)
            .args(["create", "size=10Mi", "+", "init", "+", "fsadd"])
            .arg(&driver_path)
            .args(["dostype=DOS7", "version=45.13"])
            .output()
            .expect("run rdbtool");
        assert!(
            out.status.success(),
            "rdbtool create failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let _ = std::fs::remove_file(&driver_path);
    }

    // ---- milestone 3: the editor ----------------------------------

    /// Blocks the [`foreign_image`] fixture uses.
    ///
    /// Deliberately neither contiguous nor in ascending chain order, and
    /// with the `RDSK` off block 0: an image written by a tool with a
    /// different layout policy looks like this, and an editor that
    /// quietly repacked the area would be caught by the byte-identity
    /// tests below rather than by a code review.
    const F_RDSK: usize = 1;
    const F_PART0: usize = 5;
    const F_PART1: usize = 3;
    const F_FSHD: usize = 7;
    const F_LSEG: usize = 8; // and 9
    const F_BADB: usize = 11; // and 12
    const F_HIGH_RDSK: u32 = 12;
    const F_BLOCKS_HI: u32 = 15;

    /// An image bearing everything a field-by-field rewrite would drop.
    ///
    /// `rdb_DriveInit` set, a `BADB` chain, controller identity strings,
    /// an unknown `rdb_Flags` bit, `rdb_Reserved1` filled, unknown
    /// `pb_Flags` bits, a `de_TableSize` of 21 with values in every tail
    /// longword *and* two past anything this crate models, non-zero
    /// `de_SecOrg`/`de_PreAlloc`/`de_Interleave`, slack bytes after the
    /// envec, a `PART` `SummedLongs` of 128 rather than 64, an `FSHD`
    /// with an unusual patch mask including a bit above `GlobalVec`, and
    /// an `LSEG` whose `SummedLongs` stops short of its own payload.
    ///
    /// Geometry: 10 cylinders of 32 blocks, partitions on 2..=5 and
    /// 6..=8, RDB area 0..=15 — a clean layout, so `validate()` is
    /// silent and any issue a test sees is one the test made. Cylinder 9
    /// is deliberately free, so an *added* partition has somewhere to go
    /// and the fidelity tests can be run over a structural edit too.
    fn foreign_image() -> Vec<u8> {
        let bs = 512;
        let mut d = vec![0u8; 320 * bs];

        put32(&mut d, bs, F_RDSK, hdr::ID, id::RDSK);
        put32(&mut d, bs, F_RDSK, rdsk::BLOCK_BYTES, bs as u32);
        put32(
            &mut d,
            bs,
            F_RDSK,
            rdsk::FLAGS,
            rdb_flags::LAST | rdb_flags::DISK_ID | rdb_flags::CTRLR_ID | (1 << 31),
        );
        put32(&mut d, bs, F_RDSK, rdsk::HOST_ID, 6);
        // Not CHAIN_END: a drive-init seglist this crate carries and
        // never follows, and which AmiPart would replace with -1.
        put32(&mut d, bs, F_RDSK, rdsk::DRIVE_INIT, 0x0000_1234);
        put32(&mut d, bs, F_RDSK, rdsk::BAD_BLOCK_LIST, F_BADB as u32);
        put32(&mut d, bs, F_RDSK, rdsk::PARTITION_LIST, F_PART0 as u32);
        put32(&mut d, bs, F_RDSK, rdsk::FILESYS_HEADER_LIST, F_FSHD as u32);
        // rdb_Reserved1[6] — six longwords no field of this crate names.
        for i in 0..6 {
            put32(&mut d, bs, F_RDSK, 40 + i * 4, 0xFFFF_FFFF);
        }
        put32(&mut d, bs, F_RDSK, rdsk::CYLINDERS, 10);
        put32(&mut d, bs, F_RDSK, rdsk::SECTORS, 32);
        put32(&mut d, bs, F_RDSK, rdsk::HEADS, 1);
        put32(&mut d, bs, F_RDSK, rdsk::INTERLEAVE, 2);
        put32(&mut d, bs, F_RDSK, rdsk::PARK, 10);
        put32(&mut d, bs, F_RDSK, rdsk::WRITE_PRE_COMP, 4);
        put32(&mut d, bs, F_RDSK, rdsk::REDUCED_WRITE, 5);
        put32(&mut d, bs, F_RDSK, rdsk::STEP_RATE, 7);
        put32(&mut d, bs, F_RDSK, rdsk::RDB_BLOCKS_LO, 0);
        put32(&mut d, bs, F_RDSK, rdsk::RDB_BLOCKS_HI, F_BLOCKS_HI);
        put32(&mut d, bs, F_RDSK, rdsk::LO_CYLINDER, 2);
        put32(&mut d, bs, F_RDSK, rdsk::HI_CYLINDER, 9);
        put32(&mut d, bs, F_RDSK, rdsk::CYL_BLOCKS, 32);
        put32(&mut d, bs, F_RDSK, rdsk::AUTO_PARK_SECONDS, 30);
        put32(&mut d, bs, F_RDSK, rdsk::HIGH_RDSK_BLOCK, F_HIGH_RDSK);
        // rdb_Reserved4, and bytes past the identification strings that
        // the 64-longword checksum does not even cover.
        put32(&mut d, bs, F_RDSK, 156, 0xA5A5_A5A5);
        put32(&mut d, bs, F_RDSK, 300, 0x5A5A_5A5A);
        put_padded(&mut d, bs, F_RDSK, rdsk::DISK_VENDOR, "MAXTOR  ");
        put_padded(&mut d, bs, F_RDSK, rdsk::DISK_PRODUCT, "LXT213S");
        put_padded(&mut d, bs, F_RDSK, rdsk::DISK_REVISION, "4.10");
        put_padded(&mut d, bs, F_RDSK, rdsk::CONTROLLER_VENDOR, "GVP");
        put_padded(&mut d, bs, F_RDSK, rdsk::CONTROLLER_PRODUCT, "SERIES II");
        put_padded(&mut d, bs, F_RDSK, rdsk::CONTROLLER_REVISION, "4.15");
        seal(&mut d, bs, F_RDSK, 64);

        write_foreign_part(&mut d, F_PART0, F_PART1 as u32, "DH0", (2, 5), 0, 128);
        write_foreign_part(&mut d, F_PART1, CHAIN_END, "DH1", (6, 8), -3, 64);

        put32(&mut d, bs, F_FSHD, hdr::ID, id::FSHD);
        put32(&mut d, bs, F_FSHD, chain::NEXT, CHAIN_END);
        put32(&mut d, bs, F_FSHD, fshd::HOST_ID, 6);
        put32(&mut d, bs, F_FSHD, fshd::FLAGS, 0x0000_00FF);
        put32(&mut d, bs, F_FSHD, fshd::DOS_TYPE, 0x444F_5307);
        put32(&mut d, bs, F_FSHD, fshd::VERSION, (46 << 16) | 13);
        // An unusual mask: Task and Lock patched (which rdbtool never
        // does), Type and Handler not, and a bit above GlobalVec that
        // this crate models nowhere at all.
        put32(
            &mut d,
            bs,
            F_FSHD,
            fshd::PATCH_FLAGS,
            fshd_patch::TASK
                | fshd_patch::LOCK
                | fshd_patch::STARTUP
                | fshd_patch::SEG_LIST
                | fshd_patch::GLOBAL_VEC
                | (1 << 20),
        );
        for (i, v) in [
            0xAAAA_0000u32,
            0x0000_1111,
            0x0000_2222,
            0xBBBB_0000,
            0xCCCC_0000,
            0xDDDD_0000,
            0x0000_3333,
            F_LSEG as u32,
            0xFFFF_FFFF,
        ]
        .into_iter()
        .enumerate()
        {
            put32(&mut d, bs, F_FSHD, fshd::PATCHED + i * 4, v);
        }
        seal(&mut d, bs, F_FSHD, 64);

        for (i, block) in [F_LSEG, F_LSEG + 1].into_iter().enumerate() {
            put32(&mut d, bs, block, hdr::ID, id::LSEG);
            put32(&mut d, bs, block, hdr::HOST_ID, 6);
            put32(
                &mut d,
                bs,
                block,
                chain::NEXT,
                if i == 0 {
                    (F_LSEG + 1) as u32
                } else {
                    CHAIN_END
                },
            );
            let base = block * bs + lseg::LOAD_DATA;
            for (j, b) in d[base..block * bs + bs].iter_mut().enumerate() {
                *b = ((i * 97 + j) % 251) as u8;
            }
            // The second block's count stops well short of its payload,
            // as a driver whose last block is mostly slack would — so
            // the bytes past it are outside the checksum entirely and
            // only survive because the editor never rebuilds the block.
            seal(&mut d, bs, block, if i == 0 { 128 } else { 45 });
        }

        for (block, next, entries) in [
            (
                F_BADB,
                (F_BADB + 1) as u32,
                &[(700u32, 800u32), (701, 801)][..],
            ),
            (F_BADB + 1, CHAIN_END, &[(702, 802)][..]),
        ] {
            put32(&mut d, bs, block, hdr::ID, id::BADB);
            put32(&mut d, bs, block, hdr::HOST_ID, 6);
            put32(&mut d, bs, block, chain::NEXT, next);
            for (i, (bad, good)) in entries.iter().enumerate() {
                put32(&mut d, bs, block, badb::ENTRIES + i * 8, *bad);
                put32(&mut d, bs, block, badb::ENTRIES + i * 8 + 4, *good);
            }
            seal(
                &mut d,
                bs,
                block,
                (badb::HEADER_LONGS + entries.len() * 2) as u32,
            );
        }

        // Partition contents, so "the extents were not touched" is a
        // comparison against something rather than against zeros.
        for (i, b) in d[64 * bs..].iter_mut().enumerate() {
            *b = (i % 253) as u8;
        }

        d
    }

    /// One `PART` block of [`foreign_image`], with every field a
    /// field-by-field rewrite would lose.
    fn write_foreign_part(
        d: &mut [u8],
        block: usize,
        next: u32,
        name: &str,
        (low_cyl, high_cyl): (u32, u32),
        boot_pri: i32,
        summed_longs: u32,
    ) {
        let bs = 512;
        put32(d, bs, block, hdr::ID, id::PART);
        put32(d, bs, block, hdr::HOST_ID, 6);
        put32(d, bs, block, chain::NEXT, next);
        // Bootable, plus two bits nobody models — the ones AmiPart's
        // checkbox rebuild clears on an unrelated edit.
        put32(d, bs, block, part::FLAGS, 1 | (1 << 7) | (1 << 31));
        let name = name.as_bytes();
        d[block * bs + part::DRIVE_NAME] = name.len() as u8;
        d[block * bs + part::DRIVE_NAME + 1..block * bs + part::DRIVE_NAME + 1 + name.len()]
            .copy_from_slice(name);

        let e = part::ENVIRONMENT;
        let mut env = |i: usize, v: u32| put32(d, bs, block, e + i * 4, v);
        env(de::TABLE_SIZE, 21);
        env(de::SIZE_BLOCK, 128);
        env(de::SEC_ORG, 1);
        env(de::SURFACES, 1);
        env(de::SECTORS_PER_BLOCK, 1);
        env(de::BLOCKS_PER_TRACK, 32);
        env(de::RESERVED, 3);
        env(de::PRE_ALLOC, 5);
        env(de::INTERLEAVE, 1);
        env(de::LOW_CYL, low_cyl);
        env(de::HIGH_CYL, high_cyl);
        env(de::NUM_BUFFERS, 50);
        env(de::BUF_MEM_TYPE, 1);
        env(de::MAX_TRANSFER, 0x0001_FE00);
        env(de::MASK, 0xFFFF_FFFE);
        env(de::BOOT_PRI, boot_pri as u32);
        env(de::DOS_TYPE, 0x444F_5307);
        env(de::BAUD, 19200);
        env(de::CONTROL, 0x0000_CAFE);
        env(de::BOOT_BLOCKS, 4);
        // Two longwords past de_BootBlocks: inside de_TableSize, past
        // anything this crate names, and reachable only through
        // `envec_raw`.
        env(20, 0x1111_1111);
        env(21, 0x2222_2222);
        // Slack past the envec, which a foreign tool left behind.
        put32(d, bs, block, e + 22 * 4, 0xDEAD_BEEF);
        seal(d, bs, block, summed_longs);
    }

    fn foreign_disk() -> MemDisk {
        MemDisk::new(foreign_image())
    }

    /// Every byte offset at which two images differ.
    fn differing_offsets(a: &[u8], b: &[u8]) -> Vec<usize> {
        assert_eq!(a.len(), b.len());
        (0..a.len()).filter(|&i| a[i] != b[i]).collect()
    }

    /// The fixture is what it claims to be: everything a rewrite could
    /// drop is *there* before any test asserts it survived.
    #[test]
    fn the_foreign_fixture_carries_what_a_rewrite_would_drop() {
        let mut disk = foreign_disk();
        let editor = RdbEditor::open(&mut disk).unwrap();
        let rdb = editor.rdb();

        assert_eq!(rdb.rdsk_block, F_RDSK as u64);
        assert_eq!(rdb.drive_init, 0x0000_1234);
        assert_eq!(rdb.flags & (1 << 31), 1 << 31);
        assert_eq!(rdb.controller_product, "SERIES II");
        assert_eq!(rdb.bad_blocks.len(), 3);
        assert_eq!(rdb.badb_blocks, vec![F_BADB as u64, F_BADB as u64 + 1]);
        assert_eq!(rdb.filesystems.len(), 1);
        assert_eq!(rdb.filesystems[0].patch_flags & (1 << 20), 1 << 20);
        assert_eq!(rdb.filesystems[0].task, Some(0x0000_1111));

        let p = &rdb.partitions[0];
        assert_eq!(p.name, "DH0");
        assert_eq!(p.envec_raw.len(), 22);
        assert_eq!(p.envec_raw[20], 0x1111_1111);
        assert_eq!(p.baud, Some(19200));
        assert_eq!(p.boot_blocks, Some(4));
        assert!(rdb.validate().is_empty());
    }

    /// A commit with no edits writes the area back byte for byte.
    ///
    /// The strongest statement of the preserve-everything property
    /// there is: not "the fields we model came back", but "the disk is
    /// the same disk". `rdb_HighRDSKBlock` is the one field that is
    /// recomputed rather than preserved, and the fixture's stored value
    /// already agrees with its own blocks, so even that does not move.
    #[test]
    fn no_op_commit_is_byte_identical() {
        let before = foreign_image();
        let mut disk = MemDisk::new(before.clone());
        let editor = RdbEditor::open(&mut disk).unwrap();
        let report = editor.commit(&mut disk).unwrap();

        assert_eq!(differing_offsets(&before, &disk.data), Vec::<usize>::new());
        assert_eq!(report.rdsk_block, F_RDSK as u64);
        assert_eq!(report.high_rdsk_block, F_HIGH_RDSK);
        assert_eq!(report.rdb_blocks_hi, F_BLOCKS_HI);
        assert_eq!(report.part_blocks, vec![F_PART0 as u64, F_PART1 as u64]);
        assert!(report.blocks_zeroed.is_empty());
        // The RDSK is the last block written, always.
        assert_eq!(report.blocks_written.last(), Some(&(F_RDSK as u64)));
        // A non-standard `SummedLongs` is preserved, not normalised to
        // the 64 this crate writes on create.
        assert_eq!(be32(&disk.data[F_PART0 * 512..], hdr::SUMMED_LONGS), 128);
    }

    /// **The test of this chunk.** One field changes; every other byte
    /// on the disk — modelled, unmodelled, and outside the checksum —
    /// is exactly where it was.
    #[test]
    fn editing_one_field_changes_only_that_field() {
        let before = foreign_image();
        let mut disk = MemDisk::new(before.clone());
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.set_boot_priority(0, 7).unwrap();
        editor.commit(&mut disk).unwrap();

        // Only two longwords of one block may move: de_BootPri, and the
        // ChkSum that covers it.
        let boot_pri = F_PART0 * 512 + part::ENVIRONMENT + de::BOOT_PRI * 4;
        let chk_sum = F_PART0 * 512 + hdr::CHK_SUM;
        for off in differing_offsets(&before, &disk.data) {
            assert!(
                (boot_pri..boot_pri + 4).contains(&off) || (chk_sum..chk_sum + 4).contains(&off),
                "byte {off} changed, which no edit asked for"
            );
        }

        // And the model that comes back is the model that went in, save
        // for the one field.
        let mut fresh = MemDisk::new(before);
        let old = Rdb::parse(&mut fresh).unwrap();
        let new = Rdb::parse(&mut disk).unwrap();
        assert_eq!(new.partitions[0].boot_pri, 7);
        assert_eq!(old.partitions[0].envec_raw.len(), 22);
        let mut expected = old.clone();
        expected.partitions[0].boot_pri = 7;
        expected.partitions[0].envec_raw[de::BOOT_PRI] = 7;
        assert_eq!(new, expected);
    }

    /// Every metadata setter, applied at once and read back off the
    /// disk — including the three tail fields, which have to extend
    /// `de_TableSize` on a partition that stops at 16.
    #[test]
    fn metadata_edits_round_trip_through_the_disk() {
        let mut disk = MemDisk::new(one_partition_image(2));
        let mut editor = RdbEditor::open(&mut disk).unwrap();

        editor.set_name(0, "WORK").unwrap();
        editor.set_dos_type(0, 0x444F_5307).unwrap();
        editor.set_boot_priority(0, -5).unwrap();
        editor.set_bootable(0, false).unwrap();
        editor.set_automount(0, false).unwrap();
        editor.set_reserved(0, 4).unwrap();
        editor.set_pre_alloc(0, 6).unwrap();
        editor.set_interleave(0, 3).unwrap();
        editor.set_num_buffers(0, 300).unwrap();
        editor.set_buf_mem_type(0, 1).unwrap();
        editor.set_max_transfer(0, 0x0001_FE00).unwrap();
        editor.set_mask(0, 0x00FF_FFFE).unwrap();
        editor.set_baud(0, 9600).unwrap();
        editor.set_control(0, 0x0000_BEEF).unwrap();
        editor.set_boot_blocks(0, 2).unwrap();
        editor.commit(&mut disk).unwrap();

        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = &rdb.partitions[0];
        assert_eq!(p.name, "WORK");
        assert_eq!(p.dos_type, 0x444F_5307);
        assert_eq!(p.boot_pri, -5);
        assert!(!p.bootable);
        assert!(p.no_automount);
        assert_eq!(p.num_buffers, 300);
        assert_eq!(p.buf_mem_type, 1);
        assert_eq!(p.max_transfer, 0x0001_FE00);
        assert_eq!(p.mask, 0x00FF_FFFE);
        assert_eq!(p.baud, Some(9600));
        assert_eq!(p.control, Some(0x0000_BEEF));
        assert_eq!(p.boot_blocks, Some(2));
        assert_eq!(p.envec_raw[de::RESERVED], 4);
        assert_eq!(p.envec_raw[de::PRE_ALLOC], 6);
        assert_eq!(p.envec_raw[de::INTERLEAVE], 3);
        assert_eq!(p.envec_raw.len(), 20); // de_TableSize 19 now
        assert!(rdb.validate().is_empty());
        // The editor's own model said all of this before the commit did.
        assert_eq!(rdb.partitions, editor.partitions());
    }

    /// Setting one tail field extends `de_TableSize` exactly as far as
    /// it must, and the longwords that become readable on the way are
    /// zeroed rather than exposing whatever the block was carrying.
    #[test]
    fn envec_tail_setters_extend_table_size() {
        // The fixture plants values in all three tail longwords whatever
        // de_TableSize says, which is what makes "were they exposed or
        // zeroed" a question with an observable answer.
        let mut disk = MemDisk::new(one_partition_image_envec(2, 512, 16));
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        assert_eq!(editor.partitions()[0].baud, None);

        editor.set_control(0, 0x1234_5678).unwrap();
        let p = &editor.partitions()[0];
        assert_eq!(p.envec_raw[de::TABLE_SIZE], 18);
        assert_eq!(p.baud, Some(0), "an exposed field is zeroed, not slack");
        assert_eq!(p.control, Some(0x1234_5678));
        assert_eq!(p.boot_blocks, None, "and no further than it must");

        // Extending again leaves what is already there alone.
        editor.set_boot_blocks(0, 9).unwrap();
        let p = &editor.partitions()[0];
        assert_eq!(p.envec_raw[de::TABLE_SIZE], 19);
        assert_eq!(p.control, Some(0x1234_5678));
        assert_eq!(p.boot_blocks, Some(9));

        // And a field below de_TableSize never touches it.
        editor.set_mask(0, 7).unwrap();
        assert_eq!(editor.partitions()[0].envec_raw[de::TABLE_SIZE], 19);
    }

    /// A rename rewrites the 32-byte BCPL field and nothing else — a
    /// shorter name leaves none of the longer one behind, and the rest
    /// of the `PART` block is untouched.
    #[test]
    fn set_name_rewrites_only_the_name_field() {
        let before = foreign_image();
        let mut disk = MemDisk::new(before.clone());
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.set_name(0, "A").unwrap();
        editor.commit(&mut disk).unwrap();

        let name = F_PART0 * 512 + part::DRIVE_NAME;
        let chk_sum = F_PART0 * 512 + hdr::CHK_SUM;
        for off in differing_offsets(&before, &disk.data) {
            assert!(
                (name..name + 32).contains(&off) || (chk_sum..chk_sum + 4).contains(&off),
                "byte {off} changed, which the rename did not ask for"
            );
        }
        assert_eq!(Rdb::parse(&mut disk).unwrap().partitions[0].name, "A");
        // The old name's tail is gone rather than left in the padding.
        assert!(disk.data[name + 2..name + 32].iter().all(|&b| b == 0));
    }

    #[test]
    fn set_name_refuses_a_duplicate_or_unstorable_name() {
        let mut disk = foreign_disk();
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        assert_eq!(
            editor.set_name(0, "DH1"),
            Err(EditError::DuplicateName {
                name: String::from("DH1")
            })
        );
        assert_eq!(
            editor.set_name(0, ""),
            Err(EditError::InvalidName {
                name: String::new(),
                max: 31
            })
        );
        // Renaming a partition to what it is already called is not a
        // duplicate — it is a no-op the caller is entitled to make.
        editor.set_name(0, "DH0").unwrap();
        assert_eq!(editor.partitions()[0].name, "DH0");
    }

    #[test]
    fn editor_rejects_a_partition_index_that_is_not_there() {
        let mut disk = foreign_disk();
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        let missing = |index| Err(EditError::NoSuchPartition { index, count: 2 });
        assert_eq!(editor.set_dos_type(2, 0), missing(2));
        assert_eq!(editor.set_bootable(9, true), missing(9));
        assert_eq!(editor.set_mask(2, 0), missing(2));
        assert_eq!(editor.set_name(2, "NEW"), missing(2));
    }

    /// `pb_Flags` bits this crate does not model survive an edit to the
    /// ones it does — the failure AmiPart's checkbox rebuild produces.
    #[test]
    fn flag_edits_leave_unmodelled_bits_alone() {
        let mut disk = foreign_disk();
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.set_bootable(0, false).unwrap();
        editor.set_automount(0, false).unwrap();
        editor.commit(&mut disk).unwrap();

        let flags = be32(&disk.data[F_PART0 * 512..], part::FLAGS);
        assert_eq!(flags, (1 << 1) | (1 << 7) | (1 << 31));

        // And the escape hatch says exactly what it is told to.
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.set_flags_raw(0, 0x8000_0003).unwrap();
        editor.commit(&mut disk).unwrap();
        assert_eq!(be32(&disk.data[F_PART0 * 512..], part::FLAGS), 0x8000_0003);
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb.partitions[0].bootable && rdb.partitions[0].no_automount);
    }

    /// The identity setters write the strings *and* the flag bit that
    /// makes them mean anything.
    #[test]
    fn identity_setters_set_the_flag_that_gates_them() {
        let mut disk = MemDisk::new(one_partition_image(2));
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.set_rdb_flags(rdsk_defaults::FLAGS);
        editor
            .set_disk_identity("SEAGATE", "ST3120A", "1.02")
            .unwrap();
        editor
            .set_controller_identity("CBM", "A2091", "7.0")
            .unwrap();
        editor.commit(&mut disk).unwrap();

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.disk_vendor, "SEAGATE");
        assert_eq!(rdb.disk_product, "ST3120A");
        assert_eq!(rdb.disk_revision, "1.02");
        assert_eq!(rdb.controller_vendor, "CBM");
        assert_eq!(rdb.controller_product, "A2091");
        assert_eq!(rdb.controller_revision, "7.0");
        assert_eq!(rdb.flags & rdb_flags::DISK_ID, rdb_flags::DISK_ID);
        assert_eq!(rdb.flags & rdb_flags::CTRLR_ID, rdb_flags::CTRLR_ID);

        // A field that does not fit is refused, and refused before any
        // of the three is written.
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        assert_eq!(
            editor.set_disk_identity("SEAGATE", "ST3120A", "12345"),
            Err(EditError::IdentityTooLong {
                field: "rdb_DiskRevision",
                value: String::from("12345"),
                max: 4,
            })
        );
        assert_eq!(editor.rdb().disk_vendor, "SEAGATE");
    }

    #[test]
    fn set_rdb_flags_replaces_the_whole_word() {
        let mut disk = foreign_disk();
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.set_rdb_flags(rdb_flags::LAST | (1 << 30));
        editor.commit(&mut disk).unwrap();
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.flags, rdb_flags::LAST | (1 << 30));
    }

    /// A sink that records every LBA it is asked to write.
    struct TrackingSink {
        data: Vec<u8>,
        writes: Vec<u64>,
    }

    impl BlockSink for TrackingSink {
        type Error = ();
        fn block_size(&self) -> usize {
            512
        }
        fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
            self.writes.push(lba);
            let off = lba as usize * 512;
            self.data[off..off + 512].copy_from_slice(buf);
            Ok(())
        }
        fn block_count(&self) -> Option<u64> {
            Some(self.data.len() as u64 / 512)
        }
    }

    /// The never-touch guarantee, from the outside: a commit writes only
    /// inside `0..=rdb_RDBBlocksHi`, and the partitions' contents come
    /// out byte for byte.
    #[test]
    fn commit_writes_only_inside_the_rdb_area() {
        let before = foreign_image();
        let mut disk = MemDisk::new(before.clone());
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.set_name(0, "SYS").unwrap();
        editor.set_boot_priority(1, 20).unwrap();
        editor.set_dos_type(1, 0x444F_5300).unwrap();

        let mut sink = TrackingSink {
            data: before.clone(),
            writes: Vec::new(),
        };
        let report = editor.commit(&mut sink).unwrap();

        assert!(!sink.writes.is_empty());
        for &lba in &sink.writes {
            assert!(
                lba <= F_BLOCKS_HI as u64,
                "block {lba} is outside the RDB area lease"
            );
        }
        assert_eq!(sink.writes, report.blocks_written);

        // Both partitions' extents, byte for byte. The first starts at
        // cylinder 2, i.e. block 64, which is where the fixture's
        // partition contents begin.
        assert_eq!(&sink.data[64 * 512..], &before[64 * 512..]);
    }

    /// Crash shape, exhaustively: cut the commit off after every
    /// possible number of writes and re-parse what is on the disk.
    ///
    /// The RDB is *always* readable, always self-consistent, and always
    /// carries both partitions with their old or their new metadata —
    /// never a chain leading into garbage. That holds because every
    /// block is sealed before it is written and no block is ever
    /// overwritten by a *different* structure, so an interrupted commit
    /// leaves a mixture of old and new blocks on a chain whose shape did
    /// not change.
    #[test]
    fn commit_truncated_at_every_write_leaves_a_readable_rdb() {
        /// A sink that fails on the *n*th write.
        struct FlakySink {
            data: Vec<u8>,
            writes: usize,
            fail_after: usize,
        }
        impl BlockSink for FlakySink {
            type Error = ();
            fn block_size(&self) -> usize {
                512
            }
            fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
                if self.writes == self.fail_after {
                    return Err(());
                }
                self.writes += 1;
                let off = lba as usize * 512;
                self.data[off..off + 512].copy_from_slice(buf);
                Ok(())
            }
            fn block_count(&self) -> Option<u64> {
                Some(self.data.len() as u64 / 512)
            }
        }

        let before = foreign_image();
        let mut disk = MemDisk::new(before.clone());
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.set_name(0, "SYS").unwrap();
        editor.set_boot_priority(0, 9).unwrap();

        let total = editor
            .commit(&mut TrackingSink {
                data: before.clone(),
                writes: Vec::new(),
            })
            .unwrap()
            .blocks_written
            .len();
        assert!(total > 1);

        for fail_after in 0..=total {
            let mut sink = FlakySink {
                data: before.clone(),
                writes: 0,
                fail_after,
            };
            let result = editor.commit(&mut sink);
            if fail_after < total {
                assert_eq!(result.unwrap_err(), CommitError::Io(()));
            } else {
                result.unwrap();
            }

            let mut disk = MemDisk::new(sink.data);
            let rdb = Rdb::parse(&mut disk).unwrap_or_else(|e| {
                panic!("truncated at {fail_after} left no readable RDB: {e:?}")
            });
            assert!(rdb.validate().is_empty(), "truncated at {fail_after}");
            assert!(rdb.validate_seg_lists(&mut disk).unwrap().is_empty());
            assert_eq!(rdb.partitions.len(), 2);
            assert_eq!(rdb.partitions[1].name, "DH1");
            assert_eq!(rdb.bad_blocks.len(), 3);
            assert_eq!(rdb.drive_init, 0x0000_1234);
            // The one partition being edited is either its old self or
            // its new one, never a half-written mixture: one block, one
            // write.
            let p = &rdb.partitions[0];
            assert!(
                (p.name == "DH0" && p.boot_pri == 0) || (p.name == "SYS" && p.boot_pri == 9),
                "truncated at {fail_after} left partition 0 as {} / {}",
                p.name,
                p.boot_pri
            );
            // The driver still reassembles, whatever was interrupted.
            let fs = &rdb.filesystems[0];
            assert_eq!(rdb.load_filesystem(fs, &mut disk).unwrap().len(), 2 * 492);
        }
    }

    /// A structure lying outside the area — the
    /// damaged-by-construction case — is moved *into* it, into blocks
    /// the old layout does not use, and its old blocks are left exactly
    /// as they were: they are outside the lease, so they are not ours to
    /// tidy up.
    #[test]
    fn commit_relocates_a_chain_block_from_outside_the_area() {
        let mut image = foreign_image();
        let bs = 512;
        // Move the BADB chain to blocks 20 and 21, above RDBBlocksHi.
        for (from, to) in [(F_BADB, 20usize), (F_BADB + 1, 21)] {
            let (src, dst) = (from * bs, to * bs);
            let block: Vec<u8> = image[src..src + bs].to_vec();
            image[dst..dst + bs].copy_from_slice(&block);
            image[src..src + bs].fill(0);
        }
        put32(&mut image, bs, 20, chain::NEXT, 21);
        seal(&mut image, bs, 20, (badb::HEADER_LONGS + 4) as u32);
        put32(&mut image, bs, F_RDSK, rdsk::BAD_BLOCK_LIST, 20);
        seal(&mut image, bs, F_RDSK, 64);

        let mut disk = MemDisk::new(image.clone());
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.validate().len(), 2, "both BADB blocks are outside");

        let editor = RdbEditor::open(&mut disk).unwrap();
        let report = editor.commit(&mut disk).unwrap();
        assert!(report.blocks_zeroed.is_empty());
        for &lba in &report.blocks_written {
            assert!(lba <= F_BLOCKS_HI as u64);
        }

        // Relocated into the lowest blocks the old layout left free.
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.badb_blocks, vec![0, 2]);
        assert_eq!(rdb.bad_blocks.len(), 3);
        assert_eq!(
            rdb.bad_blocks[2],
            BadBlockEntry {
                bad: 702,
                good: 802
            }
        );
        assert!(rdb.validate().is_empty());
        // Nothing outside the lease was touched, the vacated blocks
        // above it included.
        assert_eq!(&disk.data[16 * bs..], &image[16 * bs..]);
    }

    /// An area too small for the layout is refused before a byte is
    /// written, naming the shortfall — and pointing at
    /// `expand_rdb_area`, which is the way out.
    #[test]
    fn commit_refuses_an_area_that_cannot_hold_the_layout() {
        let bs = 512;
        let mut image = foreign_image();
        put32(&mut image, bs, F_RDSK, rdsk::RDB_BLOCKS_HI, 6);
        seal(&mut image, bs, F_RDSK, 64);

        let mut disk = MemDisk::new(image.clone());
        let editor = RdbEditor::open(&mut disk).unwrap();
        let err = editor.commit(&mut disk).unwrap_err();
        assert_eq!(
            err,
            CommitError::RdbAreaTooSmall {
                needed: 8,
                available: 7,
                lo: 0,
                hi: 6,
            }
        );
        assert_eq!(disk.data, image, "a refused commit writes nothing");
    }

    #[test]
    fn commit_refuses_an_inverted_area_and_an_unreachable_rdsk() {
        let bs = 512;
        for (lo, hi, expected) in [
            (9u32, 4u32, CommitError::RdbAreaInvalid { lo: 9, hi: 4 }),
            (
                0,
                0,
                CommitError::RdskOutsideRdbArea {
                    rdsk_block: F_RDSK as u64,
                    hi: 0,
                },
            ),
        ] {
            let mut image = foreign_image();
            put32(&mut image, bs, F_RDSK, rdsk::RDB_BLOCKS_LO, lo);
            put32(&mut image, bs, F_RDSK, rdsk::RDB_BLOCKS_HI, hi);
            seal(&mut image, bs, F_RDSK, 64);
            let mut disk = MemDisk::new(image.clone());
            let editor = RdbEditor::open(&mut disk).unwrap();
            assert_eq!(editor.commit(&mut disk).unwrap_err(), expected);
            assert_eq!(disk.data, image);
        }
    }

    #[test]
    fn commit_refuses_a_sink_of_the_wrong_block_size() {
        let mut disk = foreign_disk();
        let editor = RdbEditor::open(&mut disk).unwrap();
        let mut wrong = MemDisk {
            data: vec![0u8; 320 * 4096],
            block_size: 4096,
        };
        assert_eq!(
            editor.commit(&mut wrong).unwrap_err(),
            CommitError::BlockSizeMismatch {
                rdb: 512,
                sink: 4096
            }
        );
        let mut bad = MemDisk {
            data: vec![0u8; 4096],
            block_size: 768,
        };
        assert_eq!(
            editor.commit(&mut bad).unwrap_err(),
            CommitError::UnsupportedBlockSize { block_size: 768 }
        );
    }

    // ---- milestone 3: the area levers ------------------------------

    /// Growing the area is permitted exactly when the blocks being
    /// claimed belong to no partition, and refused with the blocking
    /// partition and the cylinder it would have to move to.
    ///
    /// The fixture's area is 0..=15 and its first partition starts at
    /// block 64 (cylinder 2 of 32 blocks), so blocks 16..=63 are the
    /// space an expansion may take.
    #[test]
    fn expand_rdb_area_takes_only_blocks_no_partition_owns() {
        let mut disk = MemDisk::new(foreign_image());
        let mut editor = RdbEditor::open(&mut disk).unwrap();

        // Never backwards, and never by a commit's own decision.
        assert_eq!(
            editor.expand_rdb_area(14).unwrap_err(),
            EditError::RdbAreaWouldShrink {
                hi: F_BLOCKS_HI,
                new_hi: 14
            }
        );
        // Equal is a no-op that succeeds.
        editor.expand_rdb_area(F_BLOCKS_HI).unwrap();
        assert_eq!(editor.rdb().rdb_blocks_hi, F_BLOCKS_HI);

        // One block into DH0's first cylinder, which starts at block 64.
        assert_eq!(
            editor.expand_rdb_area(64).unwrap_err(),
            EditError::RdbAreaBlocked {
                index: 0,
                name: String::from("DH0"),
                new_hi: 64,
                // Its own 32-block cylinders: block 65 upward is
                // cylinder 3, so cylinder 2 is where it may no longer be.
                move_to_cylinder: 3,
            }
        );
        // And past the end of the 320-block disk.
        assert_eq!(
            editor.expand_rdb_area(320).unwrap_err(),
            EditError::ClaimsBlocksPastEndOfDisk {
                last_block: 320,
                disk_blocks: 320,
            }
        );
        assert_eq!(
            editor.rdb().rdb_blocks_hi,
            F_BLOCKS_HI,
            "a refused expansion changes nothing"
        );

        // The last block that is nobody's: 63, one below DH0.
        editor.expand_rdb_area(63).unwrap();
        assert_eq!(editor.rdb().rdb_blocks_hi, 63);
        editor.commit(&mut disk).unwrap();

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.rdb_blocks_hi, 63);
        // The lease grew; the high-water mark did not, because the
        // expansion made room rather than using any of it.
        assert_eq!(rdb.high_rdsk_block, F_HIGH_RDSK);
        assert!(rdb.validate().is_empty());
        assert_eq!(rdb.lo_cylinder, 2, "the other lever was not touched");
    }

    /// The expansion is what makes a previously impossible add possible:
    /// the same `add_filesystem` that was `RdbAreaTooSmall` succeeds
    /// after it, and its blocks land in the newly claimed space.
    #[test]
    fn an_add_that_did_not_fit_succeeds_after_expanding_the_area() {
        // 8 blocks of the fixture's 0..=15 area are free, so a driver
        // needing an FSHD plus twelve LSEG blocks cannot fit.
        let driver: Vec<u8> = (0..12 * 492).map(|i| (i % 251) as u8).collect();

        let before = foreign_image();
        let mut disk = MemDisk::new(before.clone());
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor
            .add_filesystem(FileSystemSpec::new(0x444F_5301, driver.clone()))
            .unwrap();
        assert!(matches!(
            editor.commit(&mut disk).unwrap_err(),
            CommitError::RdbAreaTooSmall { .. }
        ));
        assert_eq!(disk.data, before, "a refused commit writes nothing");

        editor.expand_rdb_area(63).unwrap();
        let report = editor.commit(&mut disk).unwrap();
        assert!(
            report
                .blocks_written
                .iter()
                .any(|&b| b > F_BLOCKS_HI as u64),
            "the new blocks should be using the space the expansion claimed"
        );
        assert!(report.blocks_written.iter().all(|&b| b <= 63));

        let mut disk = MemDisk::new(disk.data);
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb.validate().is_empty());
        assert!(rdb.validate_seg_lists(&mut disk).unwrap().is_empty());
        assert_eq!(rdb.filesystems.len(), 2);
        let fs = &rdb.filesystems[1];
        assert_eq!(fs.dos_type, 0x444F_5301);
        assert_eq!(rdb.load_filesystem(fs, &mut disk).unwrap(), driver);
        // The partition contents are still exactly where they were: the
        // expansion stopped one block short of them.
        assert_eq!(&disk.data[64 * 512..], &before[64 * 512..]);
    }

    /// An expansion is a claim about the disk's size, so the sink gets
    /// to refuse it too — before a byte is written.
    #[test]
    fn commit_refuses_an_expanded_area_the_sink_cannot_hold() {
        let before = foreign_image();
        let mut disk = MemDisk::new(before.clone());
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.expand_rdb_area(63).unwrap();

        // A sink smaller than the ceiling the expansion claimed.
        let mut small = MemDisk::new(before[..32 * 512].to_vec());
        assert_eq!(
            editor.commit(&mut small).unwrap_err(),
            CommitError::RdbAreaPastEndOfDisk {
                hi: 63,
                block_count: 32,
            }
        );
        assert_eq!(&small.data[..], &before[..32 * 512]);

        // An editor that did *not* move the ceiling is not
        // re-litigated: it writes nowhere it was not already entitled
        // to, and the same undersized sink takes its commit.
        let editor = RdbEditor::open(&mut disk).unwrap();
        let mut small = MemDisk::new(before[..32 * 512].to_vec());
        editor.commit(&mut small).unwrap();
    }

    /// `rdb_LoCylinder` is the second lever: raised freely above the
    /// last partition, refused when one starts below it, and lowered
    /// without complaint.
    #[test]
    fn set_lo_cylinder_refuses_to_swallow_a_partition() {
        let mut disk = MemDisk::new(foreign_image());
        let mut editor = RdbEditor::open(&mut disk).unwrap();

        assert_eq!(
            editor.set_lo_cylinder(3).unwrap_err(),
            EditError::LoCylinderBlocked {
                index: 0,
                name: String::from("DH0"),
                low_cyl: 2,
                lo_cylinder: 3,
            }
        );
        assert_eq!(editor.rdb().lo_cylinder, 2);

        // Lowering hands cylinders back and passes the predicate.
        editor.set_lo_cylinder(1).unwrap();
        assert_eq!(editor.rdb().lo_cylinder, 1);

        // Delete DH0 and the boundary may move up over its cylinders.
        editor.remove_partition(0).unwrap();
        editor.set_lo_cylinder(6).unwrap();
        editor.commit(&mut disk).unwrap();

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.lo_cylinder, 6);
        assert_eq!(rdb.partitions.len(), 1);
        assert!(rdb.validate().is_empty());
    }

    /// `set_geometry_cylinders` is AmiPart's `INIT NEWGEO`: the disk got
    /// bigger, `rdb_Heads`/`rdb_Sectors`/`rdb_LoCylinder` stay, and
    /// shrinking under a partition — or under the RDB area — is refused.
    #[test]
    fn set_geometry_cylinders_grows_the_disk_and_refuses_to_cut_a_partition() {
        // 640 blocks: twice the geometry the fixture declares, so the
        // "cloned onto a larger medium" case is a real one here.
        let mut image = foreign_image();
        image.resize(640 * 512, 0);
        let mut disk = MemDisk::new(image);
        let mut editor = RdbEditor::open(&mut disk).unwrap();

        // DH1 ends at cylinder 8, so 8 cylinders is one too few.
        assert_eq!(
            editor.set_geometry_cylinders(8).unwrap_err(),
            EditError::CylindersBelowPartition {
                index: 1,
                name: String::from("DH1"),
                high_cyl: 8,
                cylinders: 8,
            }
        );
        // Nothing at all is refused by the partition check while there
        // are partitions, and by the area check once there are not:
        // a disk of no blocks cannot hold an RDB area that always has
        // at least one.
        assert_eq!(
            editor.set_geometry_cylinders(0).unwrap_err(),
            EditError::CylindersBelowPartition {
                index: 0,
                name: String::from("DH0"),
                high_cyl: 5,
                cylinders: 0,
            }
        );
        {
            let mut bare = RdbEditor::open(&mut disk).unwrap();
            bare.remove_partition(1).unwrap();
            bare.remove_partition(0).unwrap();
            assert_eq!(
                bare.set_geometry_cylinders(0).unwrap_err(),
                EditError::ClaimsBlocksPastEndOfDisk {
                    last_block: F_BLOCKS_HI as u64,
                    disk_blocks: 0,
                }
            );
        }
        // And more disk than the source reported is refused as well.
        assert_eq!(
            editor.set_geometry_cylinders(21).unwrap_err(),
            EditError::ClaimsBlocksPastEndOfDisk {
                last_block: 671,
                disk_blocks: 640,
            }
        );
        assert_eq!(editor.rdb().cylinders, 10);

        editor.set_geometry_cylinders(20).unwrap();
        assert_eq!(editor.rdb().cylinders, 20);
        assert_eq!(editor.rdb().hi_cylinder, 19);
        assert_eq!(editor.rdb().heads, 1);
        assert_eq!(editor.rdb().sectors, 32);
        assert_eq!(editor.rdb().lo_cylinder, 2);
        // Left alone on purpose, where AmiPart rewrites them.
        assert_eq!(editor.rdb().park, 10);
        assert_eq!(editor.rdb().write_pre_comp, 4);

        // The new cylinders are usable: a partition can be placed there.
        let index = editor
            .add_partition(PartitionSpec::by_size(5 * 32 * 512).named("NEW"))
            .unwrap();
        assert_eq!(editor.partitions()[index].low_cyl, 9);
        assert_eq!(editor.partitions()[index].high_cyl, 13);
        editor.commit(&mut disk).unwrap();

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.cylinders, 20);
        assert_eq!(rdb.hi_cylinder, 19);
        assert_eq!(rdb.partitions.len(), 3);
        assert_eq!(rdb.partitions[2].name, "NEW");
        assert!(rdb.validate().is_empty());
    }

    /// Crash shape over an *expanding* commit — the truncation test
    /// above, run over the case that writes above the published ceiling.
    ///
    /// The subtlety this pins: a truncated expanding commit can leave
    /// blocks written above the **old** `rdb_RDBBlocksHi`, which the old
    /// `RDSK` still on disk does not claim. That is harmless and it is
    /// the price of the operation. Those blocks are referenced by
    /// nothing (the old chains live entirely below the old ceiling),
    /// owned by nothing (the expansion proved no partition holds them),
    /// and are either overwritten by the next successful commit or left
    /// as unreferenced bytes in reserved space. The property that
    /// matters — the RDB parses, validates clean, and is the old one or
    /// the new one and never a mixture — holds throughout.
    #[test]
    fn an_expanding_commit_truncated_at_every_write_leaves_a_readable_rdb() {
        struct FlakySink {
            data: Vec<u8>,
            writes: usize,
            fail_after: usize,
        }
        impl BlockSink for FlakySink {
            type Error = ();
            fn block_size(&self) -> usize {
                512
            }
            fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
                if self.writes == self.fail_after {
                    return Err(());
                }
                self.writes += 1;
                let off = lba as usize * 512;
                self.data[off..off + 512].copy_from_slice(buf);
                Ok(())
            }
            fn block_count(&self) -> Option<u64> {
                Some(self.data.len() as u64 / 512)
            }
        }

        let driver: Vec<u8> = (0..12 * 492).map(|i| (i % 251) as u8).collect();
        let before = foreign_image();
        let mut disk = MemDisk::new(before.clone());
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.expand_rdb_area(63).unwrap();
        editor
            .add_filesystem(FileSystemSpec::new(0x444F_5301, driver.clone()))
            .unwrap();

        let total = editor
            .commit(&mut TrackingSink {
                data: before.clone(),
                writes: Vec::new(),
            })
            .unwrap()
            .blocks_written
            .len();
        assert!(total > 13);

        for fail_after in 0..=total {
            let mut sink = FlakySink {
                data: before.clone(),
                writes: 0,
                fail_after,
            };
            let result = editor.commit(&mut sink);
            if fail_after < total {
                assert_eq!(result.unwrap_err(), CommitError::Io(()));
            } else {
                result.unwrap();
            }

            let mut disk = MemDisk::new(sink.data);
            let rdb = Rdb::parse(&mut disk).unwrap_or_else(|e| {
                panic!("truncated at {fail_after} left no readable RDB: {e:?}")
            });
            assert_eq!(rdb.partitions.len(), 2);
            assert_eq!(rdb.bad_blocks.len(), 3);
            assert_eq!(rdb.drive_init, 0x0000_1234);

            // Every issue either side reports is the *documented*
            // one and nothing else: a chained block sitting in the
            // region the expansion claimed but the `RDSK` on disk has
            // not published yet. It is unreferenced by anything the
            // old table needs and owned by no partition.
            let issues: Vec<ValidationIssue> = rdb
                .validate()
                .into_iter()
                .chain(rdb.validate_seg_lists(&mut disk).unwrap())
                .collect();
            for issue in &issues {
                match issue {
                    ValidationIssue::BlockOutsideRdbArea { lba, hi, .. }
                        if *hi == F_BLOCKS_HI as u64 && *lba > F_BLOCKS_HI as u64 && *lba <= 63 => {
                    }
                    other => panic!("truncated at {fail_after}: unexpected {other:?}"),
                }
            }
            if rdb.rdb_blocks_hi == 63 {
                assert!(
                    issues.is_empty(),
                    "the published area covers its own blocks"
                );
            }

            // Whatever was interrupted, every driver on the chain
            // reassembles — the old one always, the new one whenever
            // its `FSHD` has landed.
            assert_eq!(
                rdb.load_filesystem(&rdb.filesystems[0], &mut disk)
                    .unwrap()
                    .len(),
                2 * 492
            );
            if let Some(added) = rdb.filesystems.get(1) {
                assert_eq!(rdb.load_filesystem(added, &mut disk).unwrap(), driver);
            }
            // Nothing above the new ceiling was touched, in particular
            // no partition's contents.
            assert_eq!(&disk.data[64 * 512..], &before[64 * 512..]);
        }
    }

    /// The editor works at 4 KB device blocks as it does at 512 —
    /// nothing in the write path may reintroduce the 512-byte
    /// assumptions the survey found in AmiPart (§7.5).
    #[test]
    fn editor_round_trips_a_4k_block_image() {
        let before = one_partition_image_bs(2, 4096);
        let mut disk = MemDisk {
            data: before.clone(),
            block_size: 4096,
        };
        let editor = RdbEditor::open(&mut disk).unwrap();
        editor.commit(&mut disk).unwrap();
        assert_eq!(disk.data, before);

        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.set_boot_priority(0, 4).unwrap();
        editor.commit(&mut disk).unwrap();
        assert_eq!(Rdb::parse(&mut disk).unwrap().partitions[0].boot_pri, 4);
    }

    /// An image the builder made, edited and read back: the create and
    /// mutate paths agree about the same disk.
    #[test]
    fn an_image_this_crate_built_survives_an_edit() {
        let mut disk = MemDisk {
            data: vec![0u8; TEN_MIB_BLOCKS * 512],
            block_size: 512,
        };
        let driver: Vec<u8> = (0..2000).map(|i| (i % 251) as u8).collect();
        RdbBuilder::for_size(TEN_MIB, 512)
            .unwrap()
            .partition(PartitionSpec::by_size(2 * 1024 * 1024))
            .partition(PartitionSpec::by_size(2 * 1024 * 1024).named("WORK"))
            .filesystem(FileSystemSpec::new(0x444F_5307, driver.clone()))
            .build(&mut disk)
            .unwrap();
        let built = disk.data.clone();

        let editor = RdbEditor::open(&mut disk).unwrap();
        editor.commit(&mut disk).unwrap();
        assert_eq!(disk.data, built, "a no-op commit changes nothing");

        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.set_bootable(1, true).unwrap();
        editor.set_boot_priority(1, 3).unwrap();
        editor.commit(&mut disk).unwrap();

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb.partitions[1].bootable);
        assert_eq!(rdb.partitions[1].boot_pri, 3);
        assert!(rdb.validate().is_empty());
        assert!(rdb.validate_seg_lists(&mut disk).unwrap().is_empty());
        let loaded = rdb.load_filesystem(&rdb.filesystems[0], &mut disk).unwrap();
        assert_eq!(&loaded[..driver.len()], &driver[..]);
    }

    // ---- milestone 3: structural edits -----------------------------

    /// **The fidelity test of this chunk**: a partition is *added* to a
    /// foreign image and every pre-existing structure comes back byte
    /// for byte — the whole block, every unmodelled longword of it,
    /// only ever moved and re-chained, never rebuilt.
    ///
    /// Adding to the tail of the `PART` chain changes what the last
    /// block points at, and a block whose pointers change may not be
    /// rewritten where the still-published `RDSK` walks through it (see
    /// [`RdbEditor::plan`]), so the chain is *relocated* into blocks the
    /// old layout was not using and the old blocks are zeroed after the
    /// flip. What must not change is the content: each `PART` block
    /// arrives at its new home identical apart from `pb_Next` and the
    /// checksum that covers it, and the structures whose chains did not
    /// change — the `FSHD`, its `LSEG`s, the `BADB`s — do not move at
    /// all.
    #[test]
    fn adding_a_partition_leaves_every_other_structure_byte_identical() {
        let before = foreign_image();
        let mut disk = MemDisk::new(before.clone());
        let mut editor = RdbEditor::open(&mut disk).unwrap();

        // One cylinder, the one the fixture leaves free.
        let index = editor
            .add_partition(PartitionSpec::by_size(32 * 512).named("EXTRA").bootable(2))
            .unwrap();
        assert_eq!(index, 2);
        assert_eq!(editor.partitions()[2].low_cyl, 9);
        assert_eq!(editor.partitions()[2].high_cyl, 9);
        // Not placed until the commit says so.
        assert_eq!(editor.partitions()[2].part_block, UNPLACED_BLOCK);

        let report = editor.commit(&mut disk).unwrap();
        // The whole chain relocates, into the lowest blocks *neither*
        // layout uses — 1 is the `RDSK`, 3 and 5 are the blocks being
        // vacated — and the vacated pair is zeroed after the flip.
        assert_eq!(report.part_blocks, vec![0, 2, 4]);
        assert_eq!(report.high_rdsk_block, F_HIGH_RDSK);
        assert_eq!(report.rdb_blocks_hi, F_BLOCKS_HI);
        // Sorted, so `F_PART1` (3) before `F_PART0` (5).
        assert_eq!(report.blocks_zeroed, vec![F_PART1 as u64, F_PART0 as u64]);

        // Each moved block is its old self, apart from the one pointer
        // that had to change and the checksum over it.
        for (from, to) in [(F_PART0, 0usize), (F_PART1, 2)] {
            for off in 0..512 {
                if (chain::NEXT..chain::NEXT + 4).contains(&off)
                    || (hdr::CHK_SUM..hdr::CHK_SUM + 4).contains(&off)
                {
                    continue;
                }
                assert_eq!(
                    before[from * 512 + off],
                    disk.data[to * 512 + off],
                    "byte {off} of the PART block moved from {from} to {to} changed"
                );
            }
        }
        // Everything whose chain did not change stayed exactly put.
        for block in [F_FSHD, F_LSEG, F_LSEG + 1, F_BADB, F_BADB + 1] {
            assert_eq!(
                &disk.data[block * 512..(block + 1) * 512],
                &before[block * 512..(block + 1) * 512],
                "block {block} moved, which adding a partition did not ask for"
            );
        }
        // And nothing outside the RDB area was touched at all.
        assert_eq!(
            &disk.data[(F_BLOCKS_HI as usize + 1) * 512..],
            &before[(F_BLOCKS_HI as usize + 1) * 512..]
        );

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb.validate().is_empty());
        assert_eq!(rdb.partitions.len(), 3);
        let p = &rdb.partitions[2];
        assert_eq!(p.part_block, 4);
        assert_eq!(p.name, "EXTRA");
        assert_eq!((p.low_cyl, p.high_cyl), (9, 9));
        assert_eq!(p.start_lba, 9 * 32);
        assert_eq!(p.block_len, 32);
        assert!(p.bootable);
        assert_eq!(p.boot_pri, 2);
        // Filled from the same defaults a *created* partition gets, off
        // the same geometry: this is `fill_part_fields`, once.
        assert_eq!(p.dos_type, envec_defaults::DOS_TYPE);
        assert_eq!(p.num_buffers, envec_defaults::NUM_BUFFERS);
        assert_eq!(p.size_block_longs, envec_defaults::size_block_longs(512));
        assert_eq!(p.cylinder_blocks, 32);
        assert_eq!(p.envec_raw.len(), envec_defaults::TABLE_SIZE as usize + 1);
    }

    /// An added partition takes the first `DH`*n* free, avoiding the
    /// names already on the disk — the builder's rule, on an existing
    /// table.
    #[test]
    fn an_added_partition_is_named_around_the_existing_ones() {
        let mut disk = foreign_disk();
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor
            .add_partition(PartitionSpec::by_cylinders(9, 9))
            .unwrap();
        // DH0 and DH1 are taken by the fixture.
        assert_eq!(editor.partitions()[2].name, "DH2");
    }

    /// An image whose partition's cylinder is *not* the drive's, which
    /// the format allows: `de_Surfaces * de_BlocksPerTrack` is a per
    /// partition number and may disagree with `rdb_Heads *
    /// rdb_Sectors`.
    ///
    /// The drive is 10 cylinders of 32 blocks — 320 blocks — and DH0's
    /// own cylinder is 64 blocks, so its cylinder 4 ends where the
    /// drive's cylinder 9 does. Comparing the two numbers as if they
    /// were the same unit is the bug these fixtures exist to catch.
    fn divergent_geometry_disk() -> MemDisk {
        let bs = 512;
        let mut d = one_partition_image(2);
        let e = part::ENVIRONMENT;
        put32(&mut d, bs, 3, e + de::SURFACES * 4, 2);
        put32(&mut d, bs, 3, e + de::BLOCKS_PER_TRACK * 4, 32);
        put32(&mut d, bs, 3, e + de::LOW_CYL * 4, 1);
        put32(&mut d, bs, 3, e + de::HIGH_CYL * 4, 2);
        seal(&mut d, bs, 3, 64);

        let mut disk = MemDisk::new(d);
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.partitions[0].cylinder_blocks, 64);
        assert_eq!(rdb.partitions[0].start_lba, 64);
        assert_eq!(rdb.partitions[0].block_len, 128);
        assert!(rdb.validate().is_empty(), "the fixture is a clean layout");
        disk
    }

    /// **End of disk is a question about blocks, not cylinders.** A
    /// partition whose cylinder is twice the drive's runs off the
    /// medium at *its* cylinder 5, less than half the drive's cylinder
    /// count — and an extent check that compared `de_HighCyl` against
    /// `rdb_Cylinders - 1` waved it through.
    #[test]
    fn an_extent_past_the_medium_is_refused_in_the_partitions_own_cylinders() {
        let mut disk = divergent_geometry_disk();
        let mut editor = RdbEditor::open(&mut disk).unwrap();

        // Cylinders 1..=4 of 64 blocks each end at block 320, which is
        // exactly the medium's last block.
        editor.resize_partition(0, 4).unwrap();
        assert_eq!(editor.partitions()[0].start_lba, 64);
        assert_eq!(editor.partitions()[0].block_len, 4 * 64);

        // One more of its cylinders is 64 blocks past the end, though
        // the drive still has cylinders 6..=9 by its own reckoning.
        assert_eq!(
            editor.resize_partition(0, 5).unwrap_err(),
            EditError::PastEndOfDisk {
                high_cyl: 5,
                last_cylinder: 4,
            }
        );
        assert_eq!(
            editor.set_extent(0, 4, 9).unwrap_err(),
            EditError::PastEndOfDisk {
                high_cyl: 9,
                last_cylinder: 4,
            }
        );
        // And an *added* partition goes through the same check, in its
        // own cylinders — which here are the drive's 32-block ones.
        assert_eq!(
            editor
                .add_partition(PartitionSpec::by_cylinders(10, 10))
                .unwrap_err(),
            EditError::PastEndOfDisk {
                high_cyl: 10,
                last_cylinder: 9,
            }
        );
        // The drive's own geometry is unchanged by any of it.
        assert_eq!(editor.partitions()[0].high_cyl, 4);
    }

    /// The same unit confusion on the shrink guard: `INIT NEWGEO`
    /// downwards must not truncate a partition, and whether it does is
    /// decided by the partition's last *block* against the medium's.
    #[test]
    fn shrinking_the_geometry_is_refused_in_blocks_not_cylinders() {
        let mut disk = divergent_geometry_disk();
        let mut editor = RdbEditor::open(&mut disk).unwrap();

        // DH0 ends at block 192; six drive cylinders of 32 blocks are
        // exactly that, so this fits and the one below it does not —
        // even though DH0's `de_HighCyl` is 2, far below either.
        assert_eq!(
            editor.set_geometry_cylinders(5).unwrap_err(),
            EditError::CylindersBelowPartition {
                index: 0,
                name: String::from("DH0"),
                high_cyl: 2,
                cylinders: 5,
            }
        );
        editor.set_geometry_cylinders(6).unwrap();
        assert_eq!(editor.rdb().cylinders, 6);
        assert_eq!(editor.rdb().hi_cylinder, 5);
    }

    /// A geometry whose cylinder is larger than the address space must
    /// refuse a sized placement, not overflow computing how big it is —
    /// a debug panic, and in release a wrap to zero and then a division
    /// by it.
    #[test]
    fn a_by_size_add_refuses_an_absurd_cylinder_instead_of_panicking() {
        let bs = 512;
        let mut d = one_partition_image(2);
        put32(&mut d, bs, 2, rdsk::HEADS, 1 << 28);
        put32(&mut d, bs, 2, rdsk::SECTORS, 1 << 28);
        seal(&mut d, bs, 2, 64);

        let mut disk = MemDisk::new(d);
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        assert_eq!(
            editor
                .add_partition(PartitionSpec::by_size(1 << 40))
                .unwrap_err(),
            EditError::PartitionTooSmall {
                bytes: 1 << 40,
                cylinder_bytes: u64::MAX,
            }
        );
    }

    /// Delete: the block is unchained *and zeroed*, the chain that is
    /// left is intact, `rdb_HighRDSKBlock` is recomputed, the lease is
    /// not shrunk — and the partition's *contents* are exactly where
    /// they were, because removing a table entry is not erasing a
    /// filesystem.
    #[test]
    fn removing_a_partition_zeroes_its_block_and_not_its_contents() {
        let mut before = foreign_image();
        // A stamp inside DH0's extent (cylinders 2..=5, blocks 64..192),
        // so "untouched" is a comparison against something distinctive.
        for (i, b) in before[70 * 512..71 * 512].iter_mut().enumerate() {
            *b = (i % 199) as u8 ^ 0x5A;
        }
        let stamp = before[70 * 512..71 * 512].to_vec();

        let mut disk = MemDisk::new(before.clone());
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.remove_partition(0).unwrap();
        assert_eq!(editor.partitions().len(), 1);
        let report = editor.commit(&mut disk).unwrap();

        assert_eq!(report.part_blocks, vec![F_PART1 as u64]);
        assert_eq!(report.blocks_zeroed, vec![F_PART0 as u64]);
        // The lease is never shrunk; the high-water mark is recomputed.
        assert_eq!(report.rdb_blocks_hi, F_BLOCKS_HI);
        assert_eq!(report.high_rdsk_block, F_HIGH_RDSK);
        // Zeroed *after* the RDSK, so the block that publishes the new
        // table lands before the old one stops being readable.
        let rdsk_at = report
            .blocks_written
            .iter()
            .position(|&b| b == F_RDSK as u64)
            .unwrap();
        let zeroed_at = report
            .blocks_written
            .iter()
            .position(|&b| b == F_PART0 as u64)
            .unwrap();
        assert!(zeroed_at > rdsk_at);

        assert_eq!(
            &disk.data[F_PART0 * 512..(F_PART0 + 1) * 512],
            &vec![0u8; 512][..],
            "the vacated PART block is still on the disk"
        );

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb.validate().is_empty());
        assert_eq!(rdb.partitions.len(), 1);
        assert_eq!(rdb.partitions[0].name, "DH1");
        assert_eq!(rdb.partitions[0].part_block, F_PART1 as u64);
        assert_eq!(rdb.rdb_blocks_hi, F_BLOCKS_HI);
        assert_eq!(rdb.high_rdsk_block, F_HIGH_RDSK);
        // Everything the deleted partition held is exactly where it was.
        assert_eq!(&disk.data[70 * 512..71 * 512], &stamp[..]);
        assert_eq!(&disk.data[64 * 512..], &before[64 * 512..]);
    }

    /// **Free-block management, proved.** The block a delete frees is
    /// zeroed, and the next add takes it back.
    ///
    /// Two commits rather than one on purpose: a hole left by an
    /// *earlier* edit is the case the allocator exists for, and it is
    /// the case where reuse is unambiguously free of cost. Within a
    /// single commit the allocator prefers a block neither layout uses,
    /// so that the old chains stay walkable until the `RDSK` flip; a
    /// vacated block is reused straight away only when the area has
    /// nothing else, which `add_refuses_what_it_cannot_place` covers
    /// from the other end.
    #[test]
    fn a_block_freed_by_a_delete_is_reused_by_a_later_add() {
        let mut disk = MemDisk {
            data: vec![0u8; TEN_MIB_BLOCKS * 512],
            block_size: 512,
        };
        RdbBuilder::for_size(TEN_MIB, 512)
            .unwrap()
            .partition(PartitionSpec::by_size(2 * 1024 * 1024))
            .partition(PartitionSpec::by_size(2 * 1024 * 1024))
            .build(&mut disk)
            .unwrap();
        // Contiguous by construction: RDSK 0, PART 1, PART 2.
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.partitions[0].part_block, 1);
        assert_eq!(rdb.partitions[1].part_block, 2);

        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.remove_partition(0).unwrap();
        let report = editor.commit(&mut disk).unwrap();
        assert_eq!(report.blocks_zeroed, vec![1]);
        assert_eq!(&disk.data[512..1024], &vec![0u8; 512][..]);

        // The hole is the lowest free block in the area, so the commit
        // takes it — no compaction, no growth, no scan of the disk: the
        // layout is what the chains say and the gaps are usable.
        //
        // *Which* structure takes it is the chain-shape rule's answer,
        // not the add's: chaining a second partition on changes the
        // survivor's `pb_Next`, and a block the published `RDSK` still
        // walks through may not be rewritten with a new pointer, so the
        // survivor relocates into the hole and the new partition takes
        // the next free block. Block 2, vacated by that move, is zeroed
        // after the flip and is the hole the *next* edit will use.
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor
            .add_partition(PartitionSpec::by_size(2 * 1024 * 1024).named("NEW"))
            .unwrap();
        let report = editor.commit(&mut disk).unwrap();
        assert_eq!(report.part_blocks, vec![1, 3]);
        assert_eq!(report.blocks_zeroed, vec![2]);

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb.validate().is_empty());
        assert_eq!(rdb.partitions.len(), 2);
        assert_eq!(rdb.partitions[0].part_block, 1);
        assert_eq!(rdb.partitions[1].name, "NEW");
        assert_eq!(rdb.partitions[1].part_block, 3);
    }

    /// Every way an add can be refused, and the sink is untouched in
    /// each — including the one that is only answerable at commit time,
    /// the area having no block left to put the `PART` on.
    #[test]
    fn add_refuses_what_it_cannot_place() {
        let mut disk = foreign_disk();
        let mut editor = RdbEditor::open(&mut disk).unwrap();

        // Straight onto DH1's cylinders (6..=8, blocks 192..=287).
        assert_eq!(
            editor
                .add_partition(PartitionSpec::by_cylinders(7, 9))
                .unwrap_err(),
            EditError::PartitionsOverlap {
                index: 1,
                name: String::from("DH1"),
                start: 7 * 32,
                len: 2 * 32,
            }
        );
        assert_eq!(
            editor
                .add_partition(PartitionSpec::by_cylinders(9, 9).named("DH1"))
                .unwrap_err(),
            EditError::DuplicateName {
                name: String::from("DH1")
            }
        );
        assert_eq!(
            editor
                .add_partition(PartitionSpec::by_cylinders(0, 1))
                .unwrap_err(),
            EditError::OverlapsRdbArea {
                low_cyl: 0,
                lo_cylinder: 2,
            }
        );
        assert_eq!(
            editor
                .add_partition(PartitionSpec::by_cylinders(9, 12))
                .unwrap_err(),
            EditError::PastEndOfDisk {
                high_cyl: 12,
                last_cylinder: 9,
            }
        );
        assert_eq!(
            editor
                .add_partition(PartitionSpec::by_cylinders(9, 8))
                .unwrap_err(),
            EditError::CylindersInverted {
                low_cyl: 9,
                high_cyl: 8,
            }
        );
        assert_eq!(
            editor
                .add_partition(PartitionSpec::by_size(100))
                .unwrap_err(),
            EditError::PartitionTooSmall {
                bytes: 100,
                cylinder_bytes: 32 * 512,
            }
        );
        // One free cylinder, and two asked for.
        assert_eq!(
            editor
                .add_partition(PartitionSpec::by_size(2 * 32 * 512))
                .unwrap_err(),
            EditError::NoRoomForPartition {
                cylinders: 2,
                largest_gap: 1,
            }
        );
        // A refused add leaves the editor as it was, every time.
        assert_eq!(editor.partitions().len(), 2);

        // And the area-is-full case, which only a commit can answer —
        // refused before a byte is written, naming the shortfall and
        // pointing at `expand_rdb_area`.
        let mut small = MemDisk {
            data: vec![0u8; TEN_MIB_BLOCKS * 512],
            block_size: 512,
        };
        RdbBuilder::for_size(TEN_MIB, 512)
            .unwrap()
            .reserved_blocks(2)
            .partition(PartitionSpec::by_size(2 * 1024 * 1024))
            .build(&mut small)
            .unwrap();
        let built = small.data.clone();
        let mut editor = RdbEditor::open(&mut small).unwrap();
        editor
            .add_partition(PartitionSpec::by_size(2 * 1024 * 1024))
            .unwrap();
        assert_eq!(
            editor.commit(&mut small).unwrap_err(),
            CommitError::RdbAreaTooSmall {
                needed: 3,
                available: 2,
                lo: 0,
                hi: 1,
            }
        );
        assert_eq!(small.data, built, "a refused commit writes nothing");
    }

    /// Resize is a table-entry edit: it grows into a gap, shrinks
    /// destructively, and refuses to grow into a neighbour, into the RDB
    /// area or past the end of the disk.
    #[test]
    fn resize_moves_the_table_entry_and_refuses_an_overlap() {
        let mut disk = MemDisk {
            data: vec![0u8; TEN_MIB_BLOCKS * 512],
            block_size: 512,
        };
        // 640 cylinders of 32 blocks; the RDB area takes cylinder 0.
        RdbBuilder::for_size(TEN_MIB, 512)
            .unwrap()
            .partition(PartitionSpec::by_cylinders(1, 100))
            .partition(PartitionSpec::by_cylinders(200, 300))
            .build(&mut disk)
            .unwrap();

        let mut editor = RdbEditor::open(&mut disk).unwrap();
        // Into the gap: fine.
        editor.resize_partition(0, 150).unwrap();
        assert_eq!(editor.partitions()[0].high_cyl, 150);
        assert_eq!(editor.partitions()[0].block_len, 150 * 32);
        // Into the neighbour: refused, and the entry is left as it was.
        assert_eq!(
            editor.resize_partition(0, 250).unwrap_err(),
            EditError::PartitionsOverlap {
                index: 1,
                name: String::from("DH1"),
                start: 200 * 32,
                len: 51 * 32,
            }
        );
        assert_eq!(editor.partitions()[0].high_cyl, 150);
        // Past the end of the disk: refused.
        assert_eq!(
            editor.resize_partition(1, 640).unwrap_err(),
            EditError::PastEndOfDisk {
                high_cyl: 640,
                last_cylinder: 639,
            }
        );
        // Into the RDB area: refused.
        assert_eq!(
            editor.set_extent(0, 0, 150).unwrap_err(),
            EditError::OverlapsRdbArea {
                low_cyl: 0,
                lo_cylinder: 1,
            }
        );
        // Shrinking is allowed, and destructive to whatever is inside —
        // which the docs say loudly and the format cannot prevent.
        editor.resize_partition(0, 50).unwrap();
        editor.set_extent(1, 400, 500).unwrap();
        editor.commit(&mut disk).unwrap();

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb.validate().is_empty());
        assert_eq!(
            (rdb.partitions[0].low_cyl, rdb.partitions[0].high_cyl),
            (1, 50)
        );
        assert_eq!(
            (rdb.partitions[1].low_cyl, rdb.partitions[1].high_cyl),
            (400, 500)
        );
        assert_eq!(rdb.partitions[1].start_lba, 400 * 32);
        // Nothing but the two extents moved: the blocks stayed put.
        assert_eq!(rdb.partitions[0].part_block, 1);
        assert_eq!(rdb.partitions[1].part_block, 2);
    }

    #[test]
    fn resize_reports_an_index_that_is_not_there() {
        let mut disk = foreign_disk();
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        assert_eq!(
            editor.set_extent(5, 2, 3).unwrap_err(),
            EditError::NoSuchPartition { index: 5, count: 2 }
        );
        assert_eq!(
            editor.resize_partition(5, 3).unwrap_err(),
            EditError::NoSuchPartition { index: 5, count: 2 }
        );
        assert_eq!(
            editor.remove_partition(5).unwrap_err(),
            EditError::NoSuchPartition { index: 5, count: 2 }
        );
    }

    /// A filesystem added, read back, and removed again — with every
    /// block of the vacated `LSEG` chain zeroed, and the partitions that
    /// were relying on it reported rather than rewritten.
    #[test]
    fn filesystem_add_and_remove_round_trip() {
        let mut disk = MemDisk {
            data: vec![0u8; TEN_MIB_BLOCKS * 512],
            block_size: 512,
        };
        let first = fake_driver(2000);
        RdbBuilder::for_size(TEN_MIB, 512)
            .unwrap()
            .partition(PartitionSpec::by_size(2 * 1024 * 1024).dos_type(0x444F_5307))
            .filesystem(FileSystemSpec::new(0x444F_5307, first.clone()))
            .build(&mut disk)
            .unwrap();

        let second = fake_driver(1500);
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        let index = editor
            .add_filesystem(FileSystemSpec::new(0x444F_5300, second.clone()).version(45, 1))
            .unwrap();
        assert_eq!(index, 1);
        // Unplaced until the commit decides.
        assert_eq!(editor.rdb().filesystems[1].fshd_block, UNPLACED_BLOCK);
        assert_eq!(
            editor.rdb().filesystems[1].seg_list_blocks,
            UNPLACED_BLOCK as u32
        );
        let report = editor.commit(&mut disk).unwrap();
        assert_eq!(report.fshd_blocks.len(), 2);

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb.validate().is_empty());
        assert!(rdb.validate_seg_lists(&mut disk).unwrap().is_empty());
        assert_eq!(rdb.filesystems.len(), 2);
        let added = &rdb.filesystems[1];
        assert_eq!(added.dos_type, 0x444F_5300);
        assert_eq!(added.version_major(), 45);
        assert_eq!(added.version_minor(), 1);
        assert_eq!(added.global_vec, Some(fshd_defaults::GLOBAL_VEC));
        assert_eq!(
            added.patch_flags,
            fshd_patch::SEG_LIST | fshd_patch::GLOBAL_VEC
        );
        let loaded = rdb.load_filesystem(added, &mut disk).unwrap();
        assert_eq!(&loaded[..second.len()], &second[..]);
        // The one that was already there is untouched, bytes included.
        let loaded = rdb.load_filesystem(&rdb.filesystems[0], &mut disk).unwrap();
        assert_eq!(&loaded[..first.len()], &first[..]);

        // Now remove the *first* one, whose LSEG chain is five blocks.
        let vacated: Vec<u64> = {
            let fs = &rdb.filesystems[0];
            let mut blocks = vec![fs.fshd_block];
            let mut next = fs.seg_list_blocks;
            while next != CHAIN_END {
                blocks.push(next as u64);
                let mut buf = vec![0u8; 512];
                disk.read_block(next as u64, &mut buf).unwrap();
                next = be32(&buf, chain::NEXT);
            }
            blocks
        };
        assert_eq!(vacated.len(), 6);

        let dos_types: Vec<u32> = rdb.partitions.iter().map(|p| p.dos_type).collect();
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        let affected = editor.remove_filesystem(0).unwrap();
        assert_eq!(affected, vec![0], "the partition relying on it, reported");
        let report = editor.commit(&mut disk).unwrap();
        for lba in &vacated {
            assert!(report.blocks_zeroed.contains(lba), "block {lba} not zeroed");
            let off = *lba as usize * 512;
            assert_eq!(
                &disk.data[off..off + 512],
                &vec![0u8; 512][..],
                "block {lba} still holds its old contents"
            );
        }

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb.validate().is_empty());
        assert!(rdb.validate_seg_lists(&mut disk).unwrap().is_empty());
        assert_eq!(rdb.filesystems.len(), 1);
        assert_eq!(rdb.filesystems[0].dos_type, 0x444F_5300);
        // Removing a driver says nothing about the partitions: their
        // dostype is exactly what it was, which is the caller's call to
        // make with the list `remove_filesystem` handed back.
        assert_eq!(
            rdb.partitions
                .iter()
                .map(|p| p.dos_type)
                .collect::<Vec<_>>(),
            dos_types
        );
    }

    /// `replace_filesystem` is the operation AmiPart's `ADDFS`
    /// documents and does not perform — and the index queries refuse an
    /// index that is not there.
    #[test]
    fn replace_filesystem_swaps_the_driver_in_place() {
        let mut disk = MemDisk {
            data: vec![0u8; TEN_MIB_BLOCKS * 512],
            block_size: 512,
        };
        RdbBuilder::for_size(TEN_MIB, 512)
            .unwrap()
            .partition(PartitionSpec::by_size(2 * 1024 * 1024).dos_type(0x444F_5307))
            .filesystem(FileSystemSpec::new(0x444F_5307, fake_driver(2000)))
            .build(&mut disk)
            .unwrap();

        let newer = fake_driver(900);
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        assert_eq!(editor.partitions_using_filesystem(0).unwrap(), vec![0]);
        assert_eq!(
            editor.replace_filesystem(3, FileSystemSpec::new(0, Vec::new())),
            Err(EditError::NoSuchFileSystem { index: 3, count: 1 })
        );
        assert_eq!(
            editor.remove_filesystem(3).unwrap_err(),
            EditError::NoSuchFileSystem { index: 3, count: 1 }
        );
        editor
            .replace_filesystem(
                0,
                FileSystemSpec::new(0x444F_5307, newer.clone()).version(46, 2),
            )
            .unwrap();
        editor.commit(&mut disk).unwrap();

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb.validate().is_empty());
        assert!(rdb.validate_seg_lists(&mut disk).unwrap().is_empty());
        assert_eq!(rdb.filesystems.len(), 1);
        assert_eq!(rdb.filesystems[0].version_major(), 46);
        let loaded = rdb.load_filesystem(&rdb.filesystems[0], &mut disk).unwrap();
        assert_eq!(&loaded[..newer.len()], &newer[..]);
        // Two blocks of the old five-block chain are no longer needed.
        assert_eq!(loaded.len(), 2 * 492);
    }

    /// The `BADB` chain, written by us rather than merely preserved:
    /// replaced, spread across two blocks when it needs them, and
    /// removed again.
    #[test]
    fn bad_block_list_round_trips_through_the_editor() {
        let mut disk = foreign_disk();
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        assert_eq!(editor.rdb().bad_blocks.len(), 3);

        // 62 entries at 61 per 512-byte block: two blocks, the second
        // holding one entry, so the per-block SummedLongs is exercised
        // on a partial block as well as a full one.
        let entries: Vec<BadBlockEntry> = (0..62)
            .map(|i| BadBlockEntry {
                bad: 1000 + i,
                good: 2000 + i,
            })
            .collect();
        editor.set_bad_blocks(entries.clone());
        assert_eq!(editor.rdb().bad_blocks, entries);
        let report = editor.commit(&mut disk).unwrap();
        // The old chain's blocks were vacated and are back in the pool,
        // so one of them is reused rather than left as litter.
        assert!(report
            .blocks_zeroed
            .iter()
            .all(|b| *b <= F_BLOCKS_HI as u64));

        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb.validate().is_empty());
        assert_eq!(rdb.bad_blocks, entries);
        assert_eq!(rdb.badb_blocks.len(), 2);
        let first = rdb.badb_blocks[0] as usize;
        assert_eq!(
            be32(&disk.data[first * 512..], hdr::SUMMED_LONGS),
            (badb::HEADER_LONGS + 61 * 2) as u32
        );
        let second = rdb.badb_blocks[1] as usize;
        assert_eq!(
            be32(&disk.data[second * 512..], hdr::SUMMED_LONGS),
            (badb::HEADER_LONGS + 2) as u32
        );

        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.remove_bad_blocks();
        let report = editor.commit(&mut disk).unwrap();
        assert_eq!(report.blocks_zeroed.len(), 2);
        for lba in &report.blocks_zeroed {
            let off = *lba as usize * 512;
            assert_eq!(&disk.data[off..off + 512], &vec![0u8; 512][..]);
        }
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert!(rdb.validate().is_empty());
        assert_eq!(rdb.bad_block_list, CHAIN_END);
        assert!(rdb.bad_blocks.is_empty());
        assert!(rdb.badb_blocks.is_empty());
    }

    /// Crash shape over a *structural* commit — a delete and an add in
    /// one — cut off after every possible number of writes.
    ///
    /// The RDB always parses, always validates clean, its driver always
    /// reassembles — and the table it carries is **either exactly the
    /// old one or exactly the new one**, at every single truncation
    /// point. Not "the old one with the new partition already appended",
    /// which is what this test used to allow and what
    /// [`RdbEditor::plan`]'s chain-shape rule now rules out: such a
    /// mixture is a checksum-valid table listing a deleted partition
    /// beside the one that replaced it, and the two can overlap.
    ///
    /// The whole point of writing the `RDSK` last is that the flip is
    /// what publishes the change; this asserts that nothing published it
    /// early.
    #[test]
    fn commit_truncated_at_every_write_survives_a_structural_edit() {
        struct FlakySink {
            data: Vec<u8>,
            writes: usize,
            fail_after: usize,
        }
        impl BlockSink for FlakySink {
            type Error = ();
            fn block_size(&self) -> usize {
                512
            }
            fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
                if self.writes == self.fail_after {
                    return Err(());
                }
                self.writes += 1;
                let off = lba as usize * 512;
                self.data[off..off + 512].copy_from_slice(buf);
                Ok(())
            }
            fn block_count(&self) -> Option<u64> {
                Some(self.data.len() as u64 / 512)
            }
        }

        let mut disk = MemDisk {
            data: vec![0u8; TEN_MIB_BLOCKS * 512],
            block_size: 512,
        };
        let driver = fake_driver(2000);
        RdbBuilder::for_size(TEN_MIB, 512)
            .unwrap()
            .partition(PartitionSpec::by_cylinders(1, 100).named("DOOMED"))
            .partition(PartitionSpec::by_cylinders(200, 300).named("KEPT"))
            .filesystem(FileSystemSpec::new(0x444F_5307, driver.clone()))
            .build(&mut disk)
            .unwrap();
        let before = disk.data.clone();

        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.remove_partition(0).unwrap();
        editor
            .add_partition(PartitionSpec::by_cylinders(400, 500).named("FRESH"))
            .unwrap();

        let total = editor
            .commit(&mut TrackingSink {
                data: before.clone(),
                writes: Vec::new(),
            })
            .unwrap()
            .blocks_written
            .len();
        assert!(total > 2);

        for fail_after in 0..=total {
            let mut sink = FlakySink {
                data: before.clone(),
                writes: 0,
                fail_after,
            };
            let result = editor.commit(&mut sink);
            if fail_after < total {
                assert_eq!(result.unwrap_err(), CommitError::Io(()));
            } else {
                result.unwrap();
            }

            let mut disk = MemDisk::new(sink.data);
            let rdb = Rdb::parse(&mut disk).unwrap_or_else(|e| {
                panic!("truncated at {fail_after} left no readable RDB: {e:?}")
            });
            assert!(rdb.validate().is_empty(), "truncated at {fail_after}");
            assert!(rdb.validate_seg_lists(&mut disk).unwrap().is_empty());
            let fs = &rdb.filesystems[0];
            let loaded = rdb.load_filesystem(fs, &mut disk).unwrap();
            assert_eq!(&loaded[..driver.len()], &driver[..]);
            // Old or new, whole, with nothing of the other in it.
            let table: Vec<(&str, u32, u32)> = rdb
                .partitions
                .iter()
                .map(|p| (p.name.as_str(), p.low_cyl, p.high_cyl))
                .collect();
            let old = vec![("DOOMED", 1, 100), ("KEPT", 200, 300)];
            let new = vec![("KEPT", 200, 300), ("FRESH", 400, 500)];
            assert!(
                table == old || table == new,
                "truncated at {fail_after} left a table that is neither the old \
                 one nor the new one: {table:?}"
            );
        }

        // And the finished article is the new table exactly.
        let mut disk = MemDisk::new(before);
        editor.commit(&mut disk).unwrap();
        let rdb = Rdb::parse(&mut disk).unwrap();
        let names: Vec<&str> = rdb.partitions.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["KEPT", "FRESH"]);
    }

    /// The failure the chain-shape rule exists for, in its destructive
    /// form: the new partition takes the *deleted* one's cylinders.
    ///
    /// A commit that rewrote a kept block in place with a new `pb_Next`
    /// would leave a window in which the published `RDSK` walks the old
    /// head into the new structure — a checksum-valid table carrying
    /// the deleted partition *and* the one that replaced it, claiming
    /// the same blocks, each filesystem free to destroy the other. Every
    /// prefix of the commit must parse as exactly one of the two tables,
    /// and must validate clean: an overlap in the result is the mixture,
    /// caught.
    #[test]
    fn commit_never_publishes_a_table_mixing_a_delete_with_its_replacement() {
        struct FlakySink {
            data: Vec<u8>,
            writes: usize,
            fail_after: usize,
        }
        impl BlockSink for FlakySink {
            type Error = ();
            fn block_size(&self) -> usize {
                512
            }
            fn write_block(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
                if self.writes == self.fail_after {
                    return Err(());
                }
                self.writes += 1;
                let off = lba as usize * 512;
                self.data[off..off + 512].copy_from_slice(buf);
                Ok(())
            }
            fn block_count(&self) -> Option<u64> {
                Some(self.data.len() as u64 / 512)
            }
        }

        let mut disk = MemDisk {
            data: vec![0u8; TEN_MIB_BLOCKS * 512],
            block_size: 512,
        };
        RdbBuilder::for_size(TEN_MIB, 512)
            .unwrap()
            .partition(PartitionSpec::by_cylinders(1, 100).named("DOOMED"))
            .partition(PartitionSpec::by_cylinders(200, 300).named("KEPT"))
            .build(&mut disk)
            .unwrap();
        let before = disk.data.clone();

        // The head of the chain goes, and the new partition lands on
        // cylinders the old one still claims.
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.remove_partition(0).unwrap();
        editor
            .add_partition(PartitionSpec::by_cylinders(50, 150).named("REUSED"))
            .unwrap();

        let total = editor
            .commit(&mut TrackingSink {
                data: before.clone(),
                writes: Vec::new(),
            })
            .unwrap()
            .blocks_written
            .len();

        for fail_after in 0..=total {
            let mut sink = FlakySink {
                data: before.clone(),
                writes: 0,
                fail_after,
            };
            let _ = editor.commit(&mut sink);

            let mut disk = MemDisk::new(sink.data);
            let rdb = Rdb::parse(&mut disk).unwrap_or_else(|e| {
                panic!("truncated at {fail_after} left no readable RDB: {e:?}")
            });
            let table: Vec<(&str, u32, u32)> = rdb
                .partitions
                .iter()
                .map(|p| (p.name.as_str(), p.low_cyl, p.high_cyl))
                .collect();
            let old = vec![("DOOMED", 1, 100), ("KEPT", 200, 300)];
            let new = vec![("KEPT", 200, 300), ("REUSED", 50, 150)];
            assert!(
                table == old || table == new,
                "truncated at {fail_after} published a mixture: {table:?}"
            );
            assert!(
                rdb.validate().is_empty(),
                "truncated at {fail_after}: {:?}",
                rdb.validate()
            );
        }
    }

    // ---- milestone 3: AmiPart as a second differential oracle -------
    //
    // A second independent implementation of the *mutation* path, and
    // one whose source we may legally read (MIT) when the two disagree
    // — which is what makes it worth having beside the `rdbtool`
    // oracle, whose GPL we run but never consult.
    //
    // **The comparison is semantic, not a byte diff**, for the reason
    // `docs/amipart-survey.md` §7.6 gives: AmiPart regenerates the whole
    // RDB area from its own model on every write, so its output differs
    // from ours *by construction*. Expected and deliberately not
    // compared: chain order (it sorts partitions by `de_LowCyl`, we
    // preserve the order the table had), `rdb_RDBBlocksHi` (it shrinks
    // the area to `rdb_HighRDSKBlock`, we treat it as a lease and only
    // ever grow it), `de_TableSize` (it forces 19, we preserve),
    // `rdb_BadBlockList`/`rdb_DriveInit`/the controller identity strings
    // (it zeroes them, we preserve), the dead geometry fields, and the
    // `RDSK`'s location. What *is* compared is what both tools claim to
    // own: every partition's name, extent, dostype and the `pb_Flags`
    // bits the read side names (bootable, automount), and every
    // filesystem's dostype, version and driver bytes.
    //
    // **Not wired into CI.** AmiPart is a macOS-local oracle for now:
    // its host build is a plain `gcc` target with no packaging, and
    // building it on the CI runner would mean pinning a git revision of
    // a third-party repository plus the small patch set below. The
    // `rdbtool` differential stays the CI gate; this one is run by hand
    // (see PLAN.md for the exact recipe).

    /// Is the AmiPart differential switched on? It is not a build
    /// dependency and not on any CI runner, so it is opt-in.
    #[cfg(feature = "std")]
    fn amipart_enabled() -> bool {
        std::env::var_os("AMIGA_RDB_AMIPART").is_some()
    }

    /// The AmiPart host binary: `AMIGA_RDB_AMIPART_BIN` if set,
    /// otherwise whatever `amipart` is on `PATH`.
    #[cfg(feature = "std")]
    fn amipart_bin() -> std::ffi::OsString {
        std::env::var_os("AMIGA_RDB_AMIPART_BIN")
            .unwrap_or_else(|| std::ffi::OsString::from("amipart"))
    }

    /// Run the AmiPart host CLI over `image` and hand back its stdout.
    ///
    /// `FORCE` is always passed: every write command asks a Y/N question
    /// otherwise, and a test that hung waiting on stdin would be worse
    /// than one that failed.
    ///
    /// It runs *in the image's directory* and is handed the bare file
    /// name, because `IMAGE=` is capped at 58 characters
    /// (`src/cli.c:resolve_target`) — an AmigaDOS-sized limit carried
    /// into the host build, and one that a macOS `$TMPDIR` path blows
    /// through on its own.
    #[cfg(feature = "std")]
    fn amipart(image: &std::path::Path, args: &[&str]) -> String {
        let mut arg = std::ffi::OsString::from("IMAGE=");
        arg.push(image.file_name().expect("image has a file name"));
        let out = std::process::Command::new(amipart_bin())
            .current_dir(image.parent().expect("image has a directory"))
            .arg(arg)
            .args(args)
            .arg("FORCE")
            .output()
            .expect("run amipart");
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "amipart {args:?} failed: {stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        stdout
    }

    /// AmiPart's image geometry is fixed at 16 heads by 63 sectors of
    /// 512 bytes, with `cylinders = filesize / (512 * 16 * 63)` — so a
    /// shared fixture has to be a whole number of *its* cylinders or the
    /// two tools would be comparing different disks.
    #[cfg(feature = "std")]
    const AMIPART_CYL_BYTES: usize = 512 * 16 * 63;
    #[cfg(feature = "std")]
    const AMIPART_CYLINDERS: usize = 40;

    /// A fresh image with an AmiPart-written RDB and one AmiPart-written
    /// partition, at `scratch(name)`.
    ///
    /// AmiPart's own `CREATE` cannot make the file — its host shim stubs
    /// `SetFileSize` out — so the zeros are ours and only the RDB is
    /// its.
    #[cfg(feature = "std")]
    fn amipart_image(name: &str) -> std::path::PathBuf {
        let path = scratch(name);
        std::fs::write(&path, vec![0u8; AMIPART_CYLINDERS * AMIPART_CYL_BYTES])
            .expect("write blank image");
        amipart(&path, &["INIT", "NEW"]);
        amipart(
            &path,
            &[
                "ADDPART",
                "NAME=DH0",
                "LOW=1",
                "HIGH=20",
                "TYPE=DOS3",
                "BOOTABLE",
                "BOOTPRI=3",
            ],
        );
        path
    }

    /// The fields both tools claim to own, pulled out of a parsed image.
    ///
    /// Partitions are **sorted by `de_LowCyl`**, which normalises away
    /// the one divergence that is pure bookkeeping: AmiPart writes the
    /// `PART` chain in cylinder order, this crate preserves the order
    /// the table already had. Everything else in the tuple is a real
    /// claim about the disk that a disagreement would be a bug in.
    #[cfg(feature = "std")]
    #[allow(clippy::type_complexity)]
    fn semantic_view(
        image: &std::path::Path,
    ) -> (
        Vec<(String, u32, u32, u64, u64, u32, bool, bool, i32)>,
        Vec<(u32, u16, u16, Vec<u8>)>,
    ) {
        let mut disk = MemDisk::new(std::fs::read(image).expect("read image"));
        let rdb = Rdb::parse(&mut disk).expect("parse image");
        assert!(
            rdb.validate().is_empty(),
            "{image:?} does not validate: {:?}",
            rdb.validate()
        );
        assert!(rdb
            .validate_seg_lists(&mut disk)
            .expect("walk seg lists")
            .is_empty());

        let mut parts: Vec<(String, u32, u32, u64, u64, u32, bool, bool, i32)> = rdb
            .partitions
            .iter()
            .map(|p| {
                (
                    p.name.clone(),
                    p.low_cyl,
                    p.high_cyl,
                    p.start_lba,
                    p.block_len,
                    p.dos_type,
                    p.bootable,
                    p.no_automount,
                    p.boot_pri,
                )
            })
            .collect();
        parts.sort_by_key(|p| p.1);

        let filesystems = rdb
            .filesystems
            .iter()
            .map(|f| {
                (
                    f.dos_type,
                    f.version_major(),
                    f.version_minor(),
                    rdb.load_filesystem(f, &mut disk).expect("load driver"),
                )
            })
            .collect();
        (parts, filesystems)
    }

    /// The same partition added by both tools to copies of one image,
    /// compared on the fields both of them own.
    #[cfg(feature = "std")]
    #[test]
    fn amipart_and_this_crate_agree_on_an_added_partition() {
        if !amipart_enabled() {
            return;
        }
        let base = amipart_image("amipart-addpart-base.hdf");
        let theirs = scratch("amipart-addpart-theirs.hdf");
        let ours = scratch("amipart-addpart-ours.hdf");
        std::fs::copy(&base, &theirs).expect("copy");
        std::fs::copy(&base, &ours).expect("copy");

        amipart(
            &theirs,
            &["ADDPART", "NAME=WORK", "LOW=21", "HIGH=39", "TYPE=PFS3"],
        );

        let mut disk = MemDisk::new(std::fs::read(&ours).expect("read"));
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        // AmiPart shrinks `rdb_RDBBlocksHi` to `rdb_HighRDSKBlock` on
        // every write (survey §1c), so the area it leaves behind is
        // exactly full and the add would be `RdbAreaTooSmall`. This is
        // the expansion lever earning its keep on a real foreign image
        // rather than on a fixture — and the divergence in
        // `rdb_RDBBlocksHi` afterwards is expected, not compared.
        assert_eq!(editor.rdb().rdb_blocks_hi, editor.rdb().high_rdsk_block);
        editor.expand_rdb_area(15).unwrap();
        editor
            .add_partition(
                PartitionSpec::by_cylinders(21, 39)
                    .named("WORK")
                    // PFS\3, which is what AmiPart's `TYPE=PFS3` means.
                    .dos_type(0x5046_5303),
            )
            .unwrap();
        editor.commit(&mut disk).unwrap();
        std::fs::write(&ours, &disk.data).expect("write");

        let (their_parts, their_fs) = semantic_view(&theirs);
        let (our_parts, our_fs) = semantic_view(&ours);
        assert_eq!(their_parts, our_parts);
        assert_eq!(their_fs, our_fs);
        assert_eq!(our_parts.len(), 2);
        assert_eq!(our_parts[1].0, "WORK");

        for p in [&base, &theirs, &ours] {
            let _ = std::fs::remove_file(p);
        }
    }

    /// The same filesystem driver added by both tools, compared on the
    /// `FSHD` fields and on the driver bytes an `LSEG` walk gives back.
    ///
    /// The driver bytes are the sharp end: they prove the chain, the
    /// payload split and the block order agree with a second
    /// implementation, in a way no field-by-field assertion could.
    #[cfg(feature = "std")]
    #[test]
    fn amipart_and_this_crate_agree_on_an_added_filesystem() {
        if !amipart_enabled() {
            return;
        }
        // Not a multiple of the 492-byte payload, so the partial last
        // block is part of what is being compared.
        let driver = fake_driver(2564);
        let driver_path = scratch("amipart-driver.bin");
        std::fs::write(&driver_path, &driver).expect("write driver");

        let base = amipart_image("amipart-addfs-base.hdf");
        let theirs = scratch("amipart-addfs-theirs.hdf");
        let ours = scratch("amipart-addfs-ours.hdf");
        std::fs::copy(&base, &theirs).expect("copy");
        std::fs::copy(&base, &ours).expect("copy");

        // A bare name, since `amipart` runs in this directory.
        let file_arg = alloc::format!(
            "FILE={}",
            driver_path.file_name().unwrap().to_string_lossy()
        );
        amipart(
            &theirs,
            &[
                "ADDFS",
                "TYPE=DOS7",
                &file_arg,
                // AmiPart's VERSION is the packed 32-bit word, not
                // `major.minor`: 45.13.
                "VERSION=0x002D000D",
            ],
        );

        let mut disk = MemDisk::new(std::fs::read(&ours).expect("read"));
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        editor.expand_rdb_area(63).unwrap();
        editor
            .add_filesystem(FileSystemSpec::new(0x444F_5307, driver.clone()).version(45, 13))
            .unwrap();
        editor.commit(&mut disk).unwrap();
        std::fs::write(&ours, &disk.data).expect("write");

        let (their_parts, their_fs) = semantic_view(&theirs);
        let (our_parts, our_fs) = semantic_view(&ours);
        assert_eq!(their_parts, our_parts);
        assert_eq!(their_fs.len(), 1);
        assert_eq!(our_fs.len(), 1);
        assert_eq!(
            (their_fs[0].0, their_fs[0].1, their_fs[0].2),
            (0x444F_5307, 45, 13)
        );
        // dostype, version and the driver bytes, all three.
        assert_eq!(our_fs, their_fs);
        // And the bytes really are the driver, not two tools agreeing on
        // the same mistake: `LSEG` records no byte count, so both pad to
        // a block and the prefix is what was asked for.
        assert_eq!(&our_fs[0].3[..driver.len()], &driver[..]);

        for p in [&base, &theirs, &ours, &driver_path] {
            let _ = std::fs::remove_file(p);
        }
    }

    /// This crate opens an image AmiPart wrote and a no-op commit
    /// preserves it **byte for byte**.
    ///
    /// The other direction of the preserve-unmodelled-fields property,
    /// against a foreign writer rather than a fixture: whatever AmiPart
    /// put in its `RDSK`, `PART` and `FSHD` blocks — including the
    /// `de_TableSize` of 19 it forces and every field this crate does
    /// not model — comes back unchanged.
    #[cfg(feature = "std")]
    #[test]
    fn this_crate_preserves_an_image_amipart_wrote() {
        if !amipart_enabled() {
            return;
        }
        let driver = fake_driver(1500);
        let driver_path = scratch("amipart-preserve-driver.bin");
        std::fs::write(&driver_path, &driver).expect("write driver");

        let path = amipart_image("amipart-preserve.hdf");
        amipart(
            &path,
            &["ADDPART", "NAME=WORK", "LOW=21", "HIGH=39", "TYPE=PFS3"],
        );
        // A bare name, since `amipart` runs in this directory.
        let file_arg = alloc::format!(
            "FILE={}",
            driver_path.file_name().unwrap().to_string_lossy()
        );
        amipart(&path, &["ADDFS", "TYPE=DOS7", &file_arg]);

        let before = std::fs::read(&path).expect("read");
        let mut disk = MemDisk::new(before.clone());
        let editor = RdbEditor::open(&mut disk).unwrap();
        let report = editor.commit(&mut disk).unwrap();
        assert_eq!(
            disk.data, before,
            "a no-op commit changed an AmiPart-written image"
        );
        // Every write landed inside the area AmiPart declared, which is
        // exactly full: minimal motion moved nothing.
        for &lba in &report.blocks_written {
            assert!(lba <= report.rdb_blocks_hi as u64);
        }
        assert!(report.blocks_zeroed.is_empty());

        for p in [&path, &driver_path] {
            let _ = std::fs::remove_file(p);
        }
    }

    /// Both editor error types render one line fit to show a user, in
    /// `no_std` as much as `std` — the contract every other error type
    /// in this crate holds to.
    #[test]
    fn editor_errors_display_as_one_useful_line() {
        let edits: [(EditError, &str); 17] = [
            (
                EditError::NoSuchPartition { index: 3, count: 2 },
                "there is no partition 3: the RDB has 2 of them",
            ),
            (
                EditError::InvalidName {
                    name: String::from(""),
                    max: 31,
                },
                "drive name \"\" does not fit pb_DriveName: 1..=31 characters are available",
            ),
            (
                EditError::DuplicateName {
                    name: String::from("DH0"),
                },
                "another partition is already named \"DH0\"",
            ),
            (
                EditError::IdentityTooLong {
                    field: "rdb_DiskRevision",
                    value: String::from("12345"),
                    max: 4,
                },
                "rdb_DiskRevision holds 4 characters, which \"12345\" exceeds",
            ),
            (
                EditError::NoSuchFileSystem { index: 2, count: 1 },
                "there is no filesystem 2: the RDB has 1 of them",
            ),
            (
                EditError::CylindersInverted {
                    low_cyl: 9,
                    high_cyl: 4,
                },
                "the cylinder range runs backwards: LowCyl 9 is above HighCyl 4",
            ),
            (
                EditError::PastEndOfDisk {
                    high_cyl: 700,
                    last_cylinder: 639,
                },
                "cylinder 700 is past 639, the last one the disk has",
            ),
            (
                EditError::OverlapsRdbArea {
                    low_cyl: 0,
                    lo_cylinder: 2,
                },
                "cylinder 0 is inside the RDB area: the first cylinder \
                 available to partitions is 2",
            ),
            (
                EditError::PartitionsOverlap {
                    index: 1,
                    name: String::from("DH1"),
                    start: 224,
                    len: 64,
                },
                "the extent overlaps partition 1 (\"DH1\") on 64 blocks from block 224",
            ),
            (
                EditError::PartitionTooSmall {
                    bytes: 100,
                    cylinder_bytes: 16384,
                },
                "100 bytes is less than the 16384-byte cylinder that is the \
                 smallest partition",
            ),
            (
                EditError::NoRoomForPartition {
                    cylinders: 2,
                    largest_gap: 1,
                },
                "no free run of 2 cylinders: the largest gap is 1",
            ),
            (
                EditError::UnusableGeometry {
                    heads: 0,
                    sectors: 32,
                },
                "a cylinder of 0 heads by 32 sectors holds no blocks",
            ),
            (
                EditError::RdbAreaWouldShrink { hi: 63, new_hi: 15 },
                "RDBBlocksHi 63 is never shrunk: 15 is below it",
            ),
            (
                EditError::RdbAreaBlocked {
                    index: 0,
                    name: String::from("DH0"),
                    new_hi: 64,
                    move_to_cylinder: 3,
                },
                "partition 0 (\"DH0\") is inside the blocks an RDBBlocksHi of 64 \
                 would claim: move it to cylinder 3 or above first",
            ),
            (
                EditError::LoCylinderBlocked {
                    index: 0,
                    name: String::from("DH0"),
                    low_cyl: 2,
                    lo_cylinder: 3,
                },
                "partition 0 (\"DH0\") starts at cylinder 2, below the \
                 LoCylinder 3 asked for",
            ),
            (
                EditError::CylindersBelowPartition {
                    index: 1,
                    name: String::from("DH1"),
                    high_cyl: 8,
                    cylinders: 6,
                },
                "a disk of 6 cylinders does not reach cylinder 8, \
                 where partition 1 (\"DH1\") ends",
            ),
            (
                EditError::ClaimsBlocksPastEndOfDisk {
                    last_block: 320,
                    disk_blocks: 320,
                },
                "block 320 is past the end of a disk of 320 blocks",
            ),
        ];
        for (e, expected) in edits {
            assert_eq!(alloc::format!("{e}"), expected);
        }

        let commits: [(CommitError<&str>, &str); 7] = [
            (
                CommitError::Io("device is read-only"),
                "writing a block failed: device is read-only",
            ),
            (
                CommitError::BlockSizeMismatch {
                    rdb: 512,
                    sink: 4096,
                },
                "the RDB is in 512-byte blocks but the sink writes 4096-byte blocks",
            ),
            (
                CommitError::RdbAreaInvalid { lo: 9, hi: 4 },
                "RDB area is empty or inverted: RDBBlocksLo 9 is above RDBBlocksHi 4",
            ),
            (
                CommitError::RdskOutsideRdbArea {
                    rdsk_block: 3,
                    hi: 1,
                },
                "the RDSK block at 3 is above RDBBlocksHi 1, \
                 outside the area this edit may write",
            ),
            (
                CommitError::RdbAreaTooSmall {
                    needed: 8,
                    available: 7,
                    lo: 0,
                    hi: 6,
                },
                "the RDB area 0..=6 holds 7 blocks but the new layout needs 8; \
                 grow it with expand_rdb_area",
            ),
            (
                CommitError::RdbAreaPastEndOfDisk {
                    hi: 400,
                    block_count: 320,
                },
                "the expanded RDB area ends at block 400, past the 320 blocks the sink has",
            ),
            (
                CommitError::OutsideRdbArea { lba: 99, hi: 15 },
                "refusing to write block 99, which is outside the RDB area 0..=15",
            ),
        ];
        for (e, expected) in commits {
            assert_eq!(alloc::format!("{e}"), expected);
        }
    }

    // ---- chain walking: cost and bounds -----------------------------

    /// A `PART` block whose extent starts at block 2⁵⁵ — the one value
    /// that makes `lba * 512` wrap to exactly zero.
    ///
    /// `de_Surfaces * de_BlocksPerTrack` is 2³⁰ and `de_LowCyl` is 2²⁵,
    /// both perfectly ordinary-looking longwords, and the extent is one
    /// cylinder so it is not empty and not inverted: nothing about the
    /// block says "hostile" until its LBA meets a multiply.
    fn wrapping_start_lba_image() -> Vec<u8> {
        let bs = 512;
        let mut d = one_partition_image(2);
        let e = part::ENVIRONMENT;
        put32(&mut d, bs, 3, e + de::SURFACES * 4, 1 << 15);
        put32(&mut d, bs, 3, e + de::BLOCKS_PER_TRACK * 4, 1 << 15);
        put32(&mut d, bs, 3, e + de::LOW_CYL * 4, 1 << 25);
        put32(&mut d, bs, 3, e + de::HIGH_CYL * 4, 1 << 25);
        seal(&mut d, bs, 3, 64);
        d
    }

    /// `SeekBlockSource` used to compute its byte offset with an
    /// unchecked `lba * block_size`. In a debug build that panicked; in
    /// a release build it *wrapped*, and 2⁵⁵ × 512 is zero — so a read
    /// of a block the disk does not have returned `Ok` with the `RDSK`
    /// in the buffer, and a write of one would have landed on the `RDSK`.
    #[cfg(feature = "std")]
    #[test]
    fn seek_block_source_refuses_an_lba_that_wraps_the_byte_offset() {
        use std::io::Cursor;

        let mut image = one_partition_image(2);
        // A recognisable block 0, which is where `lba * 512` wraps to.
        image[..512].iter_mut().for_each(|b| *b = 0x5A);
        let block0 = image[..512].to_vec();
        {
            let mut disk = SeekBlockSource::with_block_size(Cursor::new(&mut image), 512).unwrap();

            let mut buf = vec![0u8; 512];
            let err = BlockSource::read_block(&mut disk, 1 << 55, &mut buf).unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
            assert_ne!(buf, block0, "the read answered with block 0's contents");
            assert!(buf.iter().all(|&b| b == 0));

            let err = BlockSink::write_block(&mut disk, 1 << 55, &vec![0xAB; 512]).unwrap_err();
            assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        }
        assert_eq!(image[..512], block0[..], "the write landed on block 0");
    }

    /// The path that gets a hostile LBA to a real source: a partition
    /// whose `start_lba` came off the image, viewed through
    /// `PartitionSource`, which forwards `start_lba + lba` to the
    /// parent. Block 0 of *this* partition is block 2⁵⁵ of the disk.
    #[cfg(feature = "std")]
    #[test]
    fn partition_source_refuses_a_wrapping_start_over_a_seek_source() {
        use std::io::Cursor;

        let image = wrapping_start_lba_image();
        let rdsk = image[1024..1536].to_vec();
        let mut disk = SeekBlockSource::with_block_size(Cursor::new(image), 512).unwrap();
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = &rdb.partitions[0];
        assert_eq!(p.start_lba, 1 << 55);
        assert_eq!(p.block_len, 1 << 30);

        let mut view = PartitionSource::new(&mut disk, p);
        let mut buf = vec![0u8; 512];
        let err = view.read_block(0, &mut buf).unwrap_err();
        // Caught by the adapter, on the parent's own block count, before
        // the parent ever sees the LBA.
        assert!(matches!(
            err,
            PartitionSourceError::BeyondParent {
                lba: 0,
                parent_lba: 0x0080_0000_0000_0000,
                block_count: 320,
            }
        ));
        assert_ne!(buf, rdsk, "the read answered with block 0's contents");
    }

    /// The same guard for a parent that is not a `SeekBlockSource`: the
    /// adapter refuses an out-of-range parent LBA itself rather than
    /// trusting whatever the parent does with one.
    #[test]
    fn partition_source_refuses_a_block_past_the_parent() {
        let mut disk = MemDisk::new(wrapping_start_lba_image());
        let rdb = Rdb::parse(&mut disk).unwrap();
        let p = rdb.partitions[0].clone();
        let mut view = PartitionSource::new(&mut disk, &p);
        let mut buf = vec![0u8; 512];
        assert!(matches!(
            view.read_block(1, &mut buf),
            Err(PartitionSourceError::BeyondParent { .. })
        ));
    }

    /// A `LSEG` chain 10 000 blocks long — a size the *image* chooses,
    /// which is the point. The visited set used to be a `Vec` scanned
    /// linearly per hop, so the walk cost grew with the square of a
    /// number an attacker writes into a block; a driver chain of tens of
    /// thousands of blocks is also perfectly legitimate.
    #[test]
    fn a_long_lseg_chain_walks_in_linear_time() {
        let bs = 512;
        const CHAIN: usize = 10_000;
        // RDSK at 2, PART at 3, FSHD at 4, LSEG from 5 upward.
        let mut d = vec![0u8; (5 + CHAIN + 1) * bs];
        d[..one_partition_image(2).len()].copy_from_slice(&one_partition_image(2));
        put32(&mut d, bs, 2, rdsk::FILESYS_HEADER_LIST, 4);
        // The area has to cover the chain, or every block of it is an
        // issue rather than a walk.
        put32(&mut d, bs, 2, rdsk::RDB_BLOCKS_HI, (5 + CHAIN) as u32);
        seal(&mut d, bs, 2, 64);
        write_fshd(&mut d, bs, 4, CHAIN_END, 0x444F_5303, 5);
        for i in 0..CHAIN {
            let block = 5 + i;
            put32(&mut d, bs, block, 0, id::LSEG);
            let next = if i + 1 == CHAIN {
                CHAIN_END
            } else {
                (block + 1) as u32
            };
            put32(&mut d, bs, block, chain::NEXT, next);
            seal(&mut d, bs, block, (bs / 4) as u32);
        }

        let mut disk = MemDisk::new(d);
        let rdb = Rdb::parse(&mut disk).unwrap();
        let driver = rdb.load_filesystem(&rdb.filesystems[0], &mut disk).unwrap();
        assert_eq!(driver.len(), CHAIN * LSEG_PAYLOAD);
        assert!(rdb.validate_seg_lists(&mut disk).unwrap().is_empty());
    }

    /// A source that reports no `block_count` cannot have its chain
    /// pointers range-checked, so nothing but [`MAX_CHAIN_BLOCKS`]
    /// bounds how long a chain it can be made to walk — and before the
    /// cap, nothing did: the walk ran until the visited set exhausted
    /// memory.
    #[test]
    fn a_chain_on_a_countless_source_stops_at_the_hop_cap() {
        /// Every block is a valid `LSEG` pointing at the next one, for
        /// ever, and the source declines to say how many blocks it has.
        struct Endless;

        impl BlockSource for Endless {
            type Error = ();

            fn block_size(&self) -> usize {
                512
            }

            fn read_block(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
                buf.iter_mut().for_each(|b| *b = 0);
                buf[..4].copy_from_slice(&id::LSEG.to_be_bytes());
                buf[chain::NEXT..chain::NEXT + 4]
                    .copy_from_slice(&((lba as u32).wrapping_add(1)).to_be_bytes());
                // The shortest legal sum, so the test's cost is the walk
                // rather than 128 longwords of checksum per block.
                seal_checksum(&mut buf[..MIN_SUMMED_LONGS as usize * 4], MIN_SUMMED_LONGS)
                    .expect("seal");
                Ok(())
            }

            fn block_count(&self) -> Option<u64> {
                None
            }
        }

        let mut buf = vec![0u8; 512];
        let err = walk_chain(&mut Endless, 1, id::LSEG, &mut buf, |_, _| Ok(()));
        assert_eq!(
            err,
            Err(RdbError::ChainTooLong {
                limit: MAX_CHAIN_BLOCKS
            })
        );
    }

    // ---- shared LSEG chains -----------------------------------------

    /// Two `FSHD` blocks whose `fhb_SegListBlocks` name the *same*
    /// `LSEG` chain — the amplification shape: `k` filesystems retain
    /// `k` copies of one chain, and an edit to either rewrites both.
    fn shared_chain_image() -> Vec<u8> {
        let (mut d, _) = fs_image();
        let bs = 512;
        // A second FSHD at block 9, chained after the first, pointing at
        // the first's LSEG head.
        put32(&mut d, bs, FSHD_BLOCK, chain::NEXT, 9);
        seal(&mut d, bs, FSHD_BLOCK, 64);
        write_fshd(&mut d, bs, 9, CHAIN_END, 0x444F_5303, LSEG_BLOCK as u32);
        d
    }

    /// The editor holds every block of every chain, so aliased chains
    /// are both a memory amplification and an unrepresentable edit: a
    /// commit would write one driver's blocks twice, from two different
    /// buffers. Refused, with the block that gave it away.
    #[test]
    fn open_refuses_two_filesystems_sharing_an_lseg_chain() {
        let mut disk = MemDisk::new(shared_chain_image());
        // The parser is happy: it walks one chain at a time and reads
        // each of them correctly.
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.filesystems.len(), 2);
        assert_eq!(
            RdbEditor::open(&mut disk),
            Err(RdbError::SharedChain {
                lba: LSEG_BLOCK as u64
            })
        );
    }

    /// Validation reports the same damage rather than refusing it — and
    /// once per colliding filesystem, not once per shared block, since
    /// the image chooses how many of those there are.
    #[test]
    fn validate_seg_lists_reports_a_shared_lseg_chain() {
        let mut disk = MemDisk::new(shared_chain_image());
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(
            rdb.validate_seg_lists(&mut disk).unwrap(),
            vec![ValidationIssue::SharedLsegChain {
                index: 1,
                other: 0,
                lba: LSEG_BLOCK as u64,
            }]
        );
    }

    // ---- overlap sweep, and the model between edit and commit --------

    /// Three partitions that all claim the same blocks. Every pair is
    /// reported, `a` below `b` in each — the sweep replaced a pairwise
    /// scan and must not lose or reorder a pair.
    #[test]
    fn validate_reports_every_overlapping_pair() {
        let bs = 512;
        let mut d = one_partition_image(2);
        write_part(&mut d, bs, 3, 4, "DH0", 128, (2, 5), 16);
        write_part(&mut d, bs, 4, 5, "DH1", 128, (3, 6), 16);
        write_part(&mut d, bs, 5, CHAIN_END, "DH2", 128, (4, 7), 16);
        put32(&mut d, bs, 2, rdsk::HIGH_RDSK_BLOCK, 5);
        seal(&mut d, bs, 2, 64);

        let mut disk = MemDisk::new(d);
        let rdb = Rdb::parse(&mut disk).unwrap();
        let pairs: Vec<(usize, usize)> = rdb
            .validate()
            .into_iter()
            .filter_map(|i| match i {
                ValidationIssue::PartitionsOverlap { a, b, .. } => Some((a, b)),
                _ => None,
            })
            .collect();
        assert_eq!(pairs, vec![(0, 1), (0, 2), (1, 2)]);
    }

    /// `rdb()` is the edited model, and a model that still named a
    /// removed `FSHD` as its chain head was contradicting itself: a
    /// caller reading it between the edit and the commit saw a head
    /// block with no filesystem behind it.
    #[test]
    fn removing_a_filesystem_updates_the_chain_head_in_the_model() {
        let (image, _) = fs_image();
        let mut disk = MemDisk::new(image);
        let mut editor = RdbEditor::open(&mut disk).unwrap();
        assert_eq!(editor.rdb().filesys_header_list, FSHD_BLOCK as u32);

        editor.remove_filesystem(0).unwrap();
        assert!(editor.rdb().filesystems.is_empty());
        assert_eq!(editor.rdb().filesys_header_list, CHAIN_END);

        // And an added one reads as unplaced — CHAIN_END until a commit
        // gives it a block — rather than as the removed one's block.
        editor
            .add_filesystem(FileSystemSpec::new(0x444F_5303, vec![0xAB; 100]))
            .unwrap();
        assert_eq!(editor.rdb().filesys_header_list, CHAIN_END);
        editor.commit(&mut disk).unwrap();
        let rdb = Rdb::parse(&mut disk).unwrap();
        assert_eq!(rdb.filesystems.len(), 1);
        assert_ne!(rdb.filesys_header_list, CHAIN_END);
    }
}
