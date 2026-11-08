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
use super::{
    checked::*,
    compat::{EINVAL, EIO},
};
use std::io::{prelude::*, SeekFrom};

use bitflags::bitflags;

use super::{definitions::*, utils::Uuid};

#[allow(dead_code)]
mod constants {
    pub const XFS_SB_VERSION_ATTRBIT: u16 = 0x0010;
    pub const XFS_SB_VERSION_NLINKBIT: u16 = 0x0020;
    pub const XFS_SB_VERSION_QUOTABIT: u16 = 0x0040;
    pub const XFS_SB_VERSION_ALIGNBIT: u16 = 0x0080;
    pub const XFS_SB_VERSION_DALIGNBIT: u16 = 0x0100;
    pub const XFS_SB_VERSION_SHAREDBIT: u16 = 0x0200;
    pub const XFS_SB_VERSION_LOGV2BIT: u16 = 0x0400;
    pub const XFS_SB_VERSION_SECTORBIT: u16 = 0x0800;
    pub const XFS_SB_VERSION_EXTFLGBIT: u16 = 0x1000;
    pub const XFS_SB_VERSION_DIRV2BIT: u16 = 0x2000;
    pub const XFS_SB_VERSION_MOREBITSBIT: u16 = 0x8000;

    pub const XFS_UQUOTA_ACCT: u16 = 0x0001;
    pub const XFS_UQUOTA_ENFD: u16 = 0x0002;
    pub const XFS_UQUOTA_CHKD: u16 = 0x0004;
    pub const XFS_PQUOTA_ACCT: u16 = 0x0008;
    pub const XFS_OQUOTA_ENFD: u16 = 0x0010;
    pub const XFS_OQUOTA_CHKD: u16 = 0x0020;
    pub const XFS_GQUOTA_ACCT: u16 = 0x0040;
    pub const XFS_GQUOTA_ENFD: u16 = 0x0080;
    pub const XFS_GQUOTA_CHKD: u16 = 0x0100;
    pub const XFS_PQUOTA_ENFD: u16 = 0x0200;
    pub const XFS_PQUOTA_CHKD: u16 = 0x0400;

    pub const XFS_SBF_READONLY: u8 = 0x01;

    pub const XFS_SB_VERSION2_LAZYSBCOUNTBIT: u32 = 0x00000002;
    pub const XFS_SB_VERSION2_ATTR2BIT: u32 = 0x00000008;
    pub const XFS_SB_VERSION2_PARENTBIT: u32 = 0x00000010;
    pub const XFS_SB_VERSION2_PROJID32BIT: u32 = 0x00000080;
    pub const XFS_SB_VERSION2_CRCBIT: u32 = 0x00000100;
    pub const XFS_SB_VERSION2_FTYPE: u32 = 0x00000200;

    pub const XFS_SB_FEAT_INCOMPAT_FTYPE: u32 = 0x00000001;
    pub const XFS_SB_FEAT_INCOMPAT_SPINODES: u32 = 0x00000002;
    pub const XFS_SB_FEAT_INCOMPAT_META_UUID: u32 = 0x00000004;
    pub const XFS_SB_FEAT_INCOMPAT_BIGTIME: u32 = 0x00000008;
    pub const XFS_SB_FEAT_INCOMPAT_NEEDSREPAIR: u32 = 0x00000010;
    pub const XFS_SB_FEAT_INCOMPAT_NREXT64: u32 = 0x00000020;
    pub const XFS_SB_FEAT_INCOMPAT_EXCHRANGE: u32 = 0x00000040;
    pub const XFS_SB_FEAT_INCOMPAT_PARENT: u32 = 0x00000080;
    pub const XFS_SB_FEAT_INCOMPAT_METADIR: u32 = 0x00000100;
    pub const XFS_SB_FEAT_INCOMPAT_ZONED: u32 = 0x00000200;
    pub const XFS_SB_FEAT_INCOMPAT_ZONE_GAPS: u32 = 0x00000400;
}

bitflags! {
    #[repr(transparent)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SbFeatures2: u32 {
        const LazySbCount = constants::XFS_SB_VERSION2_LAZYSBCOUNTBIT;
        const Attr2 = constants::XFS_SB_VERSION2_ATTR2BIT;
        const Parent = constants::XFS_SB_VERSION2_PARENTBIT;
        const ProjId32 = constants::XFS_SB_VERSION2_PROJID32BIT;
        const Crc = constants::XFS_SB_VERSION2_CRCBIT;
        const Ftype = constants::XFS_SB_VERSION2_FTYPE;
        const _ = !0;
    }
}

impl SbFeatures2 {
    pub const fn crc(&self) -> bool {
        self.contains(SbFeatures2::Crc)
    }

    pub const fn ftype(&self) -> bool {
        self.contains(SbFeatures2::Ftype)
    }
}

bitflags! {
    #[repr(transparent)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SbFeaturesIncompat: u32 {
        const Ftype = constants::XFS_SB_FEAT_INCOMPAT_FTYPE;
        const SpInodes = constants::XFS_SB_FEAT_INCOMPAT_SPINODES;
        const MetaUuid = constants::XFS_SB_FEAT_INCOMPAT_META_UUID;
        const Bigtime = constants::XFS_SB_FEAT_INCOMPAT_BIGTIME;
        const NeedsRepair = constants::XFS_SB_FEAT_INCOMPAT_NEEDSREPAIR;
        const NrExt64 = constants::XFS_SB_FEAT_INCOMPAT_NREXT64;
        const Exchrange = constants::XFS_SB_FEAT_INCOMPAT_EXCHRANGE;
        const Parent = constants::XFS_SB_FEAT_INCOMPAT_PARENT;
        const Metadir = constants::XFS_SB_FEAT_INCOMPAT_METADIR;
        const Zoned = constants::XFS_SB_FEAT_INCOMPAT_ZONED;
        const ZONE_GAPS = constants::XFS_SB_FEAT_INCOMPAT_ZONE_GAPS;
    }
}

impl SbFeaturesIncompat {
    pub const fn ftype(&self) -> bool {
        self.contains(SbFeaturesIncompat::Ftype)
    }

    // AFAICT, read-only implementations don't need to care.
    //pub const fn sparse_inodes(&self) -> bool {
    //    self.contains(SbFeaturesIncompat::SpInodes)
    //}

    pub const fn meta_uuid(&self) -> bool {
        self.contains(SbFeaturesIncompat::MetaUuid)
    }

    // This is redundant with information in DinodeCore.di_flags2
    //pub const fn bigtime(&self) -> bool {
    //    self.contains(SbFeaturesIncompat::Bigtime)
    //}

    pub const fn needs_repair(&self) -> bool {
        self.contains(SbFeaturesIncompat::NeedsRepair)
    }

    // This is redundant with information in DinodeCore.di_flags2
    //pub const fn large_extent_counters(&self) -> bool {
    //    self.contains(SbFeaturesIncompat::NrExt64)
    //}

    // I don't think we need to care about this, unless se support log intents
    //pub const fn exchrange(&self) -> bool{
    //    self.contains(SbFeaturesIncompat::Exchrange)
    //}

    pub const fn metadir(&self) -> bool {
        self.contains(SbFeaturesIncompat::Metadir)
    }

    pub const fn zoned(&self) -> bool {
        self.contains(SbFeaturesIncompat::Zoned)
    }

    // This feature will never be enabled if zoned is not, and we don't support zoned, so we don't
    // need to check for it explicitly.
    //pub const fn zone_gaps(&self) -> bool {
    //  self.contains(SbFeaturesIncompat::ZoneGaps)
    //}
}

bitflags! {
    #[repr(transparent)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct SbFeaturesLogIncompat: u32 {}
}

#[derive(Clone, Copy, Debug)]
pub struct Sb {
    // sb_magicnum: u32,
    pub sb_blocksize: u32,
    pub sb_dblocks: XfsRfsblock,
    pub sb_rblocks: XfsRfsblock,
    // sb_rextents: XfsRtblock,
    pub sb_uuid: Uuid,
    // sb_logstart: XfsFsblock,
    pub sb_rootino: XfsIno,
    // sb_rbmino: XfsIno,
    // sb_rsumino: XfsIno,
    // sb_rextsize: XfsAgblock,
    pub sb_agblocks: XfsAgblock,
    pub sb_agcount: XfsAgnumber,
    // sb_rbmblocks: XfsExtlen,
    pub sb_logblocks: XfsExtlen,
    sb_versionnum: u16,
    // sb_sectsize: u16,
    sb_inodesize: u16,
    // sb_inopblock: u16,
    pub label: [u8; 12],
    pub metadata_uuid: Uuid,
    pub features_compat: u32,
    pub features_ro_compat: u32,
    pub features_log_incompat: u32,
    pub sb_blocklog: u8,
    // sb_sectlog: u8,
    pub sb_inodelog: u8,
    pub sb_inopblog: u8,
    pub sb_agblklog: u8,
    // sb_rextslog: u8,
    // sb_inprogress: u8,
    // sb_imax_pct: u8,
    pub sb_icount: u64,
    pub sb_ifree: u64,
    pub sb_fdblocks: u64,
    // sb_frextents: u64,
    // sb_uquotino: XfsIno,
    // sb_gquotino: XfsIno,
    // sb_qflags: u16,
    // sb_flags: u8,
    // sb_shared_vn: u8,
    // sb_inoalignmt: XfsExtlen,
    // sb_unit: u32,
    // sb_width: u32,
    pub sb_dirblklog: u8,
    // sb_logsectlog: u8,
    // sb_logsectsize: u16,
    // sb_logsunit: u32,
    sb_features2: SbFeatures2,
    // sb_bad_features2: u32,
    // sb_features_compat: u32,
    // sb_features_ro_compat: u32,
    sb_features_incompat: SbFeaturesIncompat,
    // sb_features_log_incompat: u32,
}

impl Sb {
    const BBSHIFT: u8 = 9;

    /// Parse and validate the superblock without unwinding on bad evidence.
    pub fn read<T: BufRead + Seek>(reader: &mut T) -> Result<Sb, i32> {
        reader.seek(SeekFrom::Start(0)).map_err(|_| EIO)?;
        let mut raw = vec![0; 512];
        reader.read_exact(&mut raw).map_err(|_| EIO)?;
        if u32be(&raw, 0)? != XFS_SB_MAGIC {
            return Err(EIO);
        }
        let version = u16be(&raw, 100)?;
        if version & constants::XFS_SB_VERSION_SHAREDBIT != 0 || !(1..=5).contains(&(version & 15))
        {
            return Err(EINVAL);
        }
        let sector_size = u16be(&raw, 102)? as usize;
        if !sector_size.is_power_of_two() || !(512..=32768).contains(&sector_size) {
            return Err(EIO);
        }
        if version & 15 == 5 {
            raw.resize(sector_size, 0);
            reader.read_exact(&mut raw[512..]).map_err(|_| EIO)?;
            crc(&raw, 224)?;
        }
        let block_size = u32be(&raw, 4)?;
        let inode_size = u16be(&raw, 104)?;
        let block_log = raw[120];
        let inode_log = raw[122];
        let inop_log = raw[123];
        let ag_log = raw[124];
        let ag_blocks = u32be(&raw, 84)?;
        let ag_count = u32be(&raw, 88)?;
        let data_blocks = u64be(&raw, 8)?;
        if !block_size.is_power_of_two()
            || !(512..=65536).contains(&block_size)
            || block_size < sector_size as u32
            || block_log != block_size.trailing_zeros() as u8
            || !inode_size.is_power_of_two()
            || !(256..=2048).contains(&inode_size)
            || inode_size as u32 > block_size
            || inode_log != inode_size.trailing_zeros() as u8
            || inop_log != block_log - inode_log
            || u16be(&raw, 106)? as u32 != block_size / inode_size as u32
            || ag_blocks == 0
            || ag_count == 0
            || ag_log
                != ag_blocks
                    .checked_next_power_of_two()
                    .ok_or(EIO)?
                    .trailing_zeros() as u8
            || data_blocks == 0
            || data_blocks > u64::from(ag_blocks) * u64::from(ag_count)
            || data_blocks <= u64::from(ag_blocks) * u64::from(ag_count - 1)
            || raw[192] > 6
            || u32::from(block_log) + u32::from(raw[192]) > 16
            || raw[121] != sector_size.trailing_zeros() as u8
        {
            return Err(EIO);
        }
        let uuid = Uuid::from_u128(u128::from_be_bytes(
            bytes(&raw, 32, 16)?.try_into().map_err(|_| EIO)?,
        ));
        // Legacy superblocks have no feature words; never interpret their padding as V5 flags.
        let features2 = if version & 15 == 5 || version & constants::XFS_SB_VERSION_MOREBITSBIT != 0
        {
            u32be(&raw, 200)?
        } else {
            0
        };
        let incompat = if version & 15 == 5 {
            u32be(&raw, 216)?
        } else {
            0
        };
        let features_incompat = SbFeaturesIncompat::from_bits(incompat).ok_or(EINVAL)?;
        if features_incompat.intersects(
            SbFeaturesIncompat::NeedsRepair
                | SbFeaturesIncompat::Metadir
                | SbFeaturesIncompat::Zoned
                | SbFeaturesIncompat::ZONE_GAPS,
        ) {
            return Err(EINVAL);
        }
        let metadata_uuid = if features_incompat.meta_uuid() {
            Uuid::from_u128(u128::from_be_bytes(
                bytes(&raw, 248, 16)?.try_into().map_err(|_| EIO)?,
            ))
        } else {
            uuid
        };
        let superblock = Sb {
            sb_blocksize: block_size,
            sb_dblocks: data_blocks,
            sb_rblocks: u64be(&raw, 16)?,
            sb_uuid: uuid,
            metadata_uuid,
            label: bytes(&raw, 108, 12)?.try_into().map_err(|_| EIO)?,
            features_compat: if version & 15 == 5 {
                u32be(&raw, 208)?
            } else {
                0
            },
            features_ro_compat: if version & 15 == 5 {
                u32be(&raw, 212)?
            } else {
                0
            },
            features_log_incompat: if version & 15 == 5 {
                u32be(&raw, 220)?
            } else {
                0
            },
            sb_rootino: u64be(&raw, 56)?,
            sb_agblocks: ag_blocks,
            sb_agcount: ag_count,
            sb_logblocks: u32be(&raw, 96)?,
            sb_versionnum: version,
            sb_inodesize: inode_size,
            sb_blocklog: block_log,
            sb_inodelog: inode_log,
            sb_inopblog: inop_log,
            sb_agblklog: ag_log,
            sb_icount: u64be(&raw, 128)?,
            sb_ifree: u64be(&raw, 136)?,
            sb_fdblocks: u64be(&raw, 144)?,
            sb_dirblklog: raw[192],
            sb_features2: SbFeatures2::from_bits_retain(features2),
            sb_features_incompat: features_incompat,
        };
        superblock.inode_offset(superblock.sb_rootino)?;
        Ok(superblock)
    }

    pub fn feature_flags(&self) -> u16 {
        self.sb_versionnum & !15
    }
    pub fn incompat_flags(&self) -> u32 {
        self.sb_features_incompat.bits()
    }
    pub fn has_dirv2(&self) -> bool {
        self.version() == 5 || self.sb_versionnum & constants::XFS_SB_VERSION_DIRV2BIT != 0
    }
    pub(crate) fn inode_offset(&self, inode: u64) -> Result<u64, i32> {
        let ag = inode >> (self.sb_agblklog + self.sb_inopblog);
        let block = (inode >> self.sb_inopblog) & ((1u64 << self.sb_agblklog) - 1);
        let slot = inode & ((1u64 << self.sb_inopblog) - 1);
        Ok(self.extent_offset((ag << self.sb_agblklog) | block, 1)? + (slot << self.sb_inodelog))
    }
    pub(crate) fn extent_offset(&self, start: u64, count: u64) -> Result<u64, i32> {
        let ag = start >> self.sb_agblklog;
        let block = start & ((1u64 << self.sb_agblklog) - 1);
        let physical = ag
            .checked_mul(u64::from(self.sb_agblocks))
            .and_then(|v| v.checked_add(block))
            .ok_or(EIO)?;
        if count == 0
            || ag >= u64::from(self.sb_agcount)
            || block.checked_add(count).ok_or(EIO)? > u64::from(self.sb_agblocks)
            || physical.checked_add(count).ok_or(EIO)? > self.sb_dblocks
        {
            return Err(EIO);
        }
        physical
            .checked_mul(u64::from(self.sb_blocksize))
            .ok_or(EIO)
    }

    #[inline]
    pub fn get_dir3_leaf_offset(&self) -> XfsDablk {
        1 << (35 - self.sb_blocklog)
    }

    /// Get the size of an inode in bytes
    pub fn inode_size(&self) -> usize {
        self.sb_inodesize.into()
    }

    /// Given a file system block number, calculate its disk address in units of 512B blocks
    fn fsb_to_daddr(&self, fsbno: XfsFsblock) -> u64 {
        let blkbb_log = self.sb_blocklog - Self::BBSHIFT;
        let agno = fsbno >> self.sb_agblklog;
        let agbno = fsbno & ((1 << self.sb_agblklog) - 1);
        (agno * u64::from(self.sb_agblocks) + agbno) << blkbb_log
    }

    /// Calculate the disk address for a given file system block number, if it's stored on a
    /// real-time device.  Real-time devices don't have allocation groups.
    fn fsb_to_daddr_rt(&self, fsbno: XfsFsblock) -> u64 {
        fsbno << (self.sb_blocklog - Self::BBSHIFT)
    }

    /// Given a file system block number, calculate its disk byte offset
    pub fn fsb_to_offset(&self, fsbno: XfsFsblock) -> u64 {
        self.fsb_to_daddr(fsbno) << Self::BBSHIFT
    }

    /// Given a realtime device file system block number, calculate its disk byte offset
    pub fn fsb_to_offset_rt(&self, fsbno: XfsFsblock) -> u64 {
        self.fsb_to_daddr_rt(fsbno) << Self::BBSHIFT
    }

    /// Does this file system record file type in its directory inodes?
    pub fn has_ftype(&self) -> bool {
        // Though it isn't documented, it seems that the ftype bit was originally part of the
        // sb_features2 field, and then later moved to the sb_features_incompat field.
        self.sb_features2.ftype() || self.sb_features_incompat.ftype()
    }

    /// Return the file system version (usually 4 or 5)
    pub fn version(&self) -> u16 {
        self.sb_versionnum & 0xF
    }
}
