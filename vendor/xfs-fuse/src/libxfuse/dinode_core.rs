/*
 * BSD 2-Clause License
 *
 * Copyright (c) 2021, Khaled Emara
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions are met:
 *
 * 1. Redistributions of source code must retain the above copyright notice, this
 *    list of conditions and the following disclaimer.
 *
 * 2. Redistributions in binary form must reproduce the above copyright notice,
 *    this list of conditions and the following disclaimer in the documentation
 *    and/or other materials provided with the distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
 * AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
 * IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
 * DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
 * FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
 * DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
 * SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
 * CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
 * OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
 * OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
 */
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bincode_next::{de::Decoder, error::DecodeError, impl_borrow_decode, Decode};
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;

use super::{
    btree::{BmbtKey, BmdrBlock},
    compat::{FileAttr, S_IFMT},
    definitions::*,
    utils::{get_file_type, FileKind, Uuid},
};

/// Mask matching libc's `S_IFMT` but truncated to `u16` (the width of
/// `DinodeCore::di_mode` on disk).
const S_IFMT_U16: u16 = S_IFMT as u16;

#[derive(Debug, FromPrimitive)]
#[cfg_attr(test, derive(Default))]
pub enum XfsDinodeFmt {
    Dev,
    Local,
    #[cfg_attr(test, default)]
    Extents,
    Btree,
    Uuid,
    Rmap,
}

impl<Ctx> bincode_next::Decode<Ctx> for XfsDinodeFmt {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let discriminant: u8 = Decode::decode(decoder)?;
        XfsDinodeFmt::from_u8(discriminant).ok_or(DecodeError::Other("unknown dinode fmt"))
    }
}
impl_borrow_decode!(XfsDinodeFmt);

#[derive(Debug, Decode, Default)]
pub struct XfsTimestamp {
    pub t_sec: i32,
    pub t_nsec: u32,
}

#[allow(dead_code)]
mod constants {
    pub const XFS_DIFLAG_REALTIME: u16 = 1 << 0;
    pub const XFS_DIFLAG_PREALLOC: u16 = 1 << 1;
    pub const XFS_DIFLAG_NEWRTBM: u16 = 1 << 2;
    pub const XFS_DIFLAG_IMMUTABLE: u16 = 1 << 3;
    pub const XFS_DIFLAG_APPEND: u16 = 1 << 4;
    pub const XFS_DIFLAG_SYNC: u16 = 1 << 5;
    pub const XFS_DIFLAG_NOATIME: u16 = 1 << 6;
    pub const XFS_DIFLAG_NODUMP: u16 = 1 << 7;
    pub const XFS_DIFLAG_RTINHERIT: u16 = 1 << 8;
    pub const XFS_DIFLAG_PROJINHERIT: u16 = 1 << 9;
    pub const XFS_DIFLAG_NOSYMLINKS: u16 = 1 << 10;
    pub const XFS_DIFLAG_EXTSIZE: u16 = 1 << 11;
    pub const XFS_DIFLAG_EXTSZINHERIT: u16 = 1 << 12;
    pub const XFS_DIFLAG_NODEFRAG: u16 = 1 << 13;
    pub const XFS_DIFLAG_FILESTREAMS: u16 = 1 << 14;

    pub const XFS_DIFLAG2_BIGTIME: u64 = 1 << 3;
    pub const XFS_DIFLAG2_NREXT64: u64 = 1 << 4;
}

#[derive(Debug)]
#[cfg_attr(test, derive(Default))]
pub struct DinodeCore {
    //_di_magic: u16,
    pub di_mode: u16,
    pub di_version: i8,
    pub di_format: XfsDinodeFmt,
    //_di_onlink: u16,
    pub di_uid: u32,
    pub di_gid: u32,
    pub di_nlink: u32,
    //_di_projid: u16,
    //_di_projid_hi: u16,
    //_di_pad: [u8; 6],
    //_di_flushiter: u16,
    pub di_atime: XfsTimestamp,
    pub di_mtime: XfsTimestamp,
    pub di_ctime: XfsTimestamp,
    pub di_size: XfsFsize,
    pub di_nblocks: XfsRfsblock,
    //_di_extsize: XfsExtlen,
    /// Number of extents in the data fork
    pub nextents: u64,
    /// Number of extents in the attr fork
    pub anextents: u32,
    pub di_forkoff: u8,
    pub di_aformat: XfsDinodeFmt,
    //_di_dmevmask: u32,
    //_di_dmstate: u16,
    pub di_flags: u16,
    pub di_gen: u32,

    //_di_next_unlinked: u32,

    /* Version 5 file system (inode version 3) fields start here */
    //_di_crc: u32,
    //_di_changecount: u64,
    //_di_lsn: u64,
    pub di_flags2: u64,
    //_di_cowextsize: u32,
    //_di_pad2: [u8; 12],
    pub di_crtime: XfsTimestamp,
    pub di_ino: u64,
    //_di_uuid: Uuid,
}

impl DinodeCore {
    /// Compute the gap in bytes between the end of the keys and the start of the pointers, for
    /// BTree-formatted inodes only, for the data fork.
    pub const fn dfork_btree_ptr_gap(&self, inode_size: usize, bb_numrecs: u16) -> usize {
        debug_assert!(matches!(self.di_format, XfsDinodeFmt::Btree));
        // The XFS Algorithms and Data Structures document contains an error here.  It says that
        // the array of xfs_bmbt_ptr_t values immediately follows the array of xfs_bmbt_key_t
        // values, and the size of both arrays is specified by bb_numrecs.  HOWEVER, there is
        // actually a gap.  The space from the end of bmbt to the beginning of the attribute fork
        // is split in half.  Half for keys and half for pointers.  The remaining space is padded
        // with zeros.  The beginning of the attribute fork is given as di_forkoff *
        // 8 bytes from the start of the literal area, which is where BmdrBlock is located.
        let space = if self.di_forkoff == 0 {
            (inode_size - self.literal_area_offset()) / 2
        } else {
            let space = (self.di_forkoff as usize) * 8 / 2;
            // Round up to a multiple of 8
            let rem = space % 8;
            if rem == 0 {
                space
            } else {
                space + 8 - rem
            }
        };
        let gap = space - BmdrBlock::SIZE - bb_numrecs as usize * BmbtKey::SIZE;
        // Round down to a multiple of 8
        gap - gap % 8
    }

    /// Compute the gap in bytes between the end of the keys and the start of the pointers, for
    /// BTree-formatted inodes only, for the attr fork.
    pub const fn afork_btree_ptr_gap(&self, inode_size: usize, bb_numrecs: u16) -> usize {
        debug_assert!(matches!(self.di_aformat, XfsDinodeFmt::Btree));
        debug_assert!(self.di_forkoff != 0);
        // The XFS Algorithms and Data Structures document, section 15.4, isn't really specific
        // about where the pointers are located.  They appear to be halfway between the start of
        // the attribute fork and the end of the inode, modulo some rounding.
        let mut already = BmdrBlock::SIZE + bb_numrecs as usize * BmbtKey::SIZE;
        if !already.is_multiple_of(8) {
            already += 8 - already % 8
        }
        let mut attr_fork_ofs = self.literal_area_offset() + self.di_forkoff as usize * 8;
        attr_fork_ofs -= attr_fork_ofs % 8;
        let mut ptr_ofs = (inode_size - attr_fork_ofs) / 2 + attr_fork_ofs;
        if !ptr_ofs.is_multiple_of(8) {
            ptr_ofs += 8 - ptr_ofs % 8
        }
        ptr_ofs - attr_fork_ofs - already
    }

    pub const fn literal_area_offset(&self) -> usize {
        match self.di_version {
            1..=2 => 0x64,
            3 => 0xb0,
            _ => unreachable!(),
        }
    }

    /// Is the inode's data located on a real-time device?
    pub fn is_realtime(&self) -> bool {
        (self.di_flags & constants::XFS_DIFLAG_REALTIME) != 0
    }

    pub fn stat(&self, ino: XfsIno) -> Result<FileAttr, i32> {
        let kind = get_file_type(FileKind::Mode(self.di_mode))?;
        // Special case for ino 1.  FUSE requires / to have inode 1, but XFS
        // does not.
        if self.di_version >= 3 {
            assert!(ino == 1 || ino == self.di_ino);
        }
        Ok(FileAttr {
            ino,
            size: u64::try_from(self.di_size).map_err(|_| super::compat::EIO)?,
            blocks: self.di_nblocks,
            atime: self.timestamp(&self.di_atime)?,
            mtime: self.timestamp(&self.di_mtime)?,
            ctime: self.timestamp(&self.di_ctime)?,
            crtime: if self.di_version >= 3 {
                Some(self.timestamp(&self.di_crtime)?)
            } else {
                None
            },
            mode: self.di_mode,
            inode_version: self.di_version,
            raw_atime: self.raw_timestamp(&self.di_atime)?,
            raw_mtime: self.raw_timestamp(&self.di_mtime)?,
            raw_ctime: self.raw_timestamp(&self.di_ctime)?,
            raw_crtime: if self.di_version >= 3 {
                Some(self.raw_timestamp(&self.di_crtime)?)
            } else {
                None
            },
            kind,
            perm: self.di_mode & !S_IFMT_U16,
            nlink: self.di_nlink,
            uid: self.di_uid,
            gid: self.di_gid,
            rdev: 0,
            blksize: 0,
            flags: 0,
        })
    }

    fn raw_timestamp(&self, ts: &XfsTimestamp) -> Result<(i64, u32), i32> {
        if self.di_version >= 3 && self.di_flags2 & constants::XFS_DIFLAG2_BIGTIME != 0 {
            let value = (u64::from(ts.t_sec as u32) << 32) | u64::from(ts.t_nsec);
            Ok((
                (value / 1_000_000_000) as i64 - 2_147_483_648,
                (value % 1_000_000_000) as u32,
            ))
        } else {
            if ts.t_nsec >= 1_000_000_000 {
                return Err(super::compat::EIO);
            }
            Ok((i64::from(ts.t_sec), ts.t_nsec))
        }
    }

    fn timestamp(&self, ts: &XfsTimestamp) -> Result<SystemTime, i32> {
        if self.di_version >= 3 && (self.di_flags2 & constants::XFS_DIFLAG2_BIGTIME != 0) {
            // XXX this could be made a const if the Rust const_trait_impl
            // feature stabilizes.
            let classic_epoch: SystemTime = UNIX_EPOCH - Duration::from_secs(i32::MAX as u64 + 1);

            classic_epoch
                .checked_add(Duration::from_nanos(
                    u64::from(ts.t_sec as u32) * (1u64 << 32) + u64::from(ts.t_nsec),
                ))
                .ok_or(super::compat::EIO)
        } else {
            if ts.t_nsec >= 1_000_000_000 {
                return Err(super::compat::EIO);
            }
            let seconds = i64::from(ts.t_sec);
            let base = if seconds < 0 {
                UNIX_EPOCH.checked_sub(Duration::from_secs(seconds.unsigned_abs()))
            } else {
                UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64))
            };
            base.and_then(|v| v.checked_add(Duration::from_nanos(u64::from(ts.t_nsec))))
                .ok_or(super::compat::EIO)
        }
    }
}

impl<Ctx> Decode<Ctx> for DinodeCore {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let mut di_flags2 = 0;
        let mut di_crtime: XfsTimestamp = Default::default();
        let mut di_ino = 0;
        let nextents;
        let anextents;

        let di_magic: u16 = Decode::decode(decoder)?;
        if di_magic != XFS_DINODE_MAGIC {
            return Err(DecodeError::Other("invalid inode magic"));
        }
        let di_mode: u16 = Decode::decode(decoder)?;
        let di_version: i8 = Decode::decode(decoder)?;
        if !(1..=3).contains(&di_version) {
            return Err(DecodeError::Other("unsupported inode version"));
        }
        let di_format: XfsDinodeFmt = Decode::decode(decoder)?;
        let di_onlink: u16 = Decode::decode(decoder)?;
        let di_uid: u32 = Decode::decode(decoder)?;
        let di_gid: u32 = Decode::decode(decoder)?;
        let di_nlink: u32 = Decode::decode(decoder)?;
        let nlink: u32 = if di_version == 1 {
            di_onlink.into()
        } else {
            di_nlink
        };
        let _di_projid: u16 = Decode::decode(decoder)?;
        let _di_projid_hi: u16 = Decode::decode(decoder)?;
        let di_maybe_big_nextents: u64 = Decode::decode(decoder)?;
        let di_atime: XfsTimestamp = Decode::decode(decoder)?;
        let di_mtime: XfsTimestamp = Decode::decode(decoder)?;
        let di_ctime: XfsTimestamp = Decode::decode(decoder)?;
        let di_size: XfsFsize = Decode::decode(decoder)?;
        let di_nblocks: XfsRfsblock = Decode::decode(decoder)?;
        let _di_extsize: XfsExtlen = Decode::decode(decoder)?;
        let di_maybe_big_anextents: u32 = Decode::decode(decoder)?;
        let di_maybe_anextents: u16 = Decode::decode(decoder)?;
        let di_forkoff: u8 = Decode::decode(decoder)?;
        let di_aformat: XfsDinodeFmt = Decode::decode(decoder)?;
        let _di_dmevmask: u32 = Decode::decode(decoder)?;
        let _di_dmstate: u16 = Decode::decode(decoder)?;
        let di_flags: u16 = Decode::decode(decoder)?;
        let di_gen: u32 = Decode::decode(decoder)?;
        let _di_next_unlinked: u32 = Decode::decode(decoder)?;
        if di_version >= 3 {
            let _di_crc: u32 = Decode::decode(decoder)?;
            let _di_changecount: u64 = Decode::decode(decoder)?;
            let _di_lsn: u64 = Decode::decode(decoder)?;
            di_flags2 = Decode::decode(decoder)?;
            let _di_cowextsize: u32 = Decode::decode(decoder)?;
            let _di_pad2: [u8; 12] = Decode::decode(decoder)?;
            di_crtime = Decode::decode(decoder)?;
            di_ino = Decode::decode(decoder)?;
            let _di_uuid: Uuid = Decode::decode(decoder)?;
            if di_flags2 & constants::XFS_DIFLAG2_NREXT64 != 0 {
                nextents = di_maybe_big_nextents;
                anextents = di_maybe_big_anextents;
            } else {
                nextents = u64::from(di_maybe_big_anextents);
                anextents = u32::from(di_maybe_anextents);
            }
        } else {
            nextents = u64::from(di_maybe_big_anextents);
            anextents = u32::from(di_maybe_anextents);
        }

        Ok(DinodeCore {
            di_mode,
            di_version,
            di_format,
            di_uid,
            di_gid,
            di_nlink: nlink,
            di_atime,
            di_mtime,
            di_ctime,
            di_size,
            di_nblocks,
            nextents,
            anextents,
            di_forkoff,
            di_aformat,
            di_flags,
            di_gen,
            di_flags2,
            di_crtime,
            di_ino,
        })
    }
}
impl_borrow_decode!(DinodeCore);

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[test]
    fn legacy_inode_links_and_realtime_flags_decode_from_disk() {
        let mut bytes = [0u8; 100];
        bytes[..2].copy_from_slice(&XFS_DINODE_MAGIC.to_be_bytes());
        bytes[2..4].copy_from_slice(&0x81a4u16.to_be_bytes());
        bytes[4] = 1;
        bytes[5] = 2; // Extents.
        bytes[6..8].copy_from_slice(&5u16.to_be_bytes());
        bytes[16..20].copy_from_slice(&17u32.to_be_bytes());
        bytes[83] = 2;
        bytes[90..92].copy_from_slice(&constants::XFS_DIFLAG_REALTIME.to_be_bytes());
        let config = bincode_next::config::standard()
            .with_fixed_int_encoding()
            .with_big_endian();
        let (legacy, consumed) =
            bincode_next::decode_from_slice::<DinodeCore, _>(&bytes, config).unwrap();
        assert_eq!(
            (
                legacy.di_nlink,
                legacy.literal_area_offset(),
                consumed,
                legacy.is_realtime()
            ),
            (5, 100, 100, true)
        );
        bytes[4] = 2;
        let (modern, _) = bincode_next::decode_from_slice::<DinodeCore, _>(&bytes, config).unwrap();
        assert_eq!(modern.di_nlink, 17);
        bytes[4] = 4;
        assert!(bincode_next::decode_from_slice::<DinodeCore, _>(&bytes, config).is_err());
    }

    /// Test the afork_btree_ptr_gap function against data from real live file systems.  The XFS
    /// Algorithms & Data Structures book does not accurately document this gap.
    #[rstest]
    #[case(512, 24, 3, 1, 56)]
    #[case(512, 24, 3, 5, 24)]
    #[case(256, 15, 2, 1, 8)]
    fn afork_btree_ptr_gap(
        #[case] inode_size: usize,
        #[case] di_forkoff: u8,
        #[case] di_version: i8,
        #[case] bb_numrecs: u16,
        #[case] gap: usize,
    ) {
        let dic = DinodeCore {
            di_forkoff,
            di_version,
            di_aformat: XfsDinodeFmt::Btree,
            ..Default::default()
        };
        assert_eq!(dic.afork_btree_ptr_gap(inode_size, bb_numrecs), gap);
    }

    /// Test the dfork_btree_ptr_gap function against data from real live file systems.  The XFS
    /// Algorithms & Data Structures book does not accurately document this gap.
    #[rstest]
    #[case(512, 0, 3, 1, 152)]
    #[case(512, 0, 3, 3, 136)]
    #[case(512, 24, 3, 1, 80)]
    #[case(512, 0, 3, 2, 144)]
    #[case(512, 24, 3, 9, 16)]
    #[case(512, 37, 3, 2, 128)]
    #[case(256, 0, 2, 2, 56)]
    #[case(256, 0, 2, 1, 64)]
    #[case(256, 15, 2, 1, 48)]
    #[case(2048, 0, 3, 1, 920)]
    #[case(2048, 0, 3, 3, 904)]
    #[case(1024, 0, 3, 7, 360)]
    fn dfork_btree_ptr_gap(
        #[case] inode_size: usize,
        #[case] di_forkoff: u8,
        #[case] di_version: i8,
        #[case] bb_numrecs: u16,
        #[case] gap: usize,
    ) {
        let dic = DinodeCore {
            di_forkoff,
            di_version,
            di_format: XfsDinodeFmt::Btree,
            ..Default::default()
        };
        assert_eq!(dic.dfork_btree_ptr_gap(inode_size, bb_numrecs), gap);
    }
}
