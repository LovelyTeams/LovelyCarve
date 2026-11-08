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
use std::io::{BufRead, Seek};

use bincode_next::{
    de::{read::Reader, Decoder},
    error::DecodeError,
    Decode,
};

use super::{
    compat::FileType,
    definitions::*,
    sb::Sb,
    utils::{decode, Uuid},
    volume::SUPERBLOCK,
};

type XfsDir2DataOff = u16;
/// Block address of a directory entry, in eight byte units.
pub type XfsDir2Dataptr = u32;

#[allow(dead_code)]
mod constants {
    pub const XFS_DIR2_DATA_FD_COUNT: usize = 3;

    pub const XFS_DIR3_FT_UNKNOWN: u8 = 0;
    pub const XFS_DIR3_FT_REG_FILE: u8 = 1;
    pub const XFS_DIR3_FT_DIR: u8 = 2;
    pub const XFS_DIR3_FT_CHRDEV: u8 = 3;
    pub const XFS_DIR3_FT_BLKDEV: u8 = 4;
    pub const XFS_DIR3_FT_FIFO: u8 = 5;
    pub const XFS_DIR3_FT_SOCK: u8 = 6;
    pub const XFS_DIR3_FT_SYMLINK: u8 = 7;
    pub const XFS_DIR3_FT_WHT: u8 = 8;
}
pub use constants::*;

#[derive(Debug, Decode)]
pub struct Dir3BlkHdr {
    pub magic: u32,
    _crc: u32,
    _blkno: u64,
    _lsn: u64,
    _uuid: Uuid,
    _owner: u64,
}

impl Dir3BlkHdr {
    pub const SIZE: u64 = 48;
}

#[derive(Debug, Decode, Clone, Copy)]
struct Dir2DataFree {
    _offset: XfsDir2DataOff,
    _length: XfsDir2DataOff,
}

impl Dir2DataFree {
    pub const SIZE: u64 = 4;
}

#[derive(Debug, Decode)]
pub struct Dir2DataHdr {
    pub magic: u32,
    _best_free: [Dir2DataFree; constants::XFS_DIR2_DATA_FD_COUNT],
}

impl Dir2DataHdr {
    pub const SIZE: u64 = 4 + constants::XFS_DIR2_DATA_FD_COUNT as u64 * Dir2DataFree::SIZE;
}

#[derive(Debug, Decode)]
pub struct Dir3DataHdr {
    pub hdr: Dir3BlkHdr,
    _best_free: [Dir2DataFree; constants::XFS_DIR2_DATA_FD_COUNT],
    _pad: u32,
}

impl Dir3DataHdr {
    pub const SIZE: u64 =
        Dir3BlkHdr::SIZE + constants::XFS_DIR2_DATA_FD_COUNT as u64 * Dir2DataFree::SIZE + 4;
}

#[derive(Debug)]
pub struct Dir2DataEntry {
    pub inumber: XfsIno,
    pub name: Vec<u8>,
    pub ftype: Option<u8>,
    pub tag: XfsDir2DataOff,
}

impl Dir2DataEntry {
    pub fn get_length(sb: &Sb, raw: &[u8]) -> i64 {
        let namelen: u8 = decode(&raw[8..]).unwrap().0;
        if sb.has_ftype() {
            ((namelen as i64 + 19) / 8) * 8
        } else {
            ((namelen as i64 + 18) / 8) * 8
        }
    }
}

impl<Ctx> Decode<Ctx> for Dir2DataEntry {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let inumber = Decode::decode(decoder)?;
        let sb = SUPERBLOCK.get().unwrap();
        let namelen: u8 = Decode::decode(decoder)?;
        let mut namebytes = vec![0u8; namelen.into()];
        decoder.reader().read(&mut namebytes[..])?;
        let name = namebytes;
        let ftype: Option<u8> = if sb.has_ftype() {
            Some(Decode::decode(decoder)?)
        } else {
            None
        };
        // Pad up to 1 less than a multiple of 8 bytes
        let pad: usize = if sb.has_ftype() {
            // current offset is 9 + 1 + namelen + 1
            4 - namelen as i16
        } else {
            // current offset is 9 + 1 + namelen
            5 - namelen as i16
        }
        .rem_euclid(8)
        .try_into()
        .unwrap();
        decoder.reader().consume(pad);
        let tag = Decode::decode(decoder)?;
        Ok(Dir2DataEntry {
            inumber,
            name,
            ftype,
            tag,
        })
    }
}

#[derive(Debug)]
pub struct Dir2DataUnused {
    _freetag: u16,
    _length: XfsDir2DataOff,
    _tag: XfsDir2DataOff,
}

impl<Ctx> Decode<Ctx> for Dir2DataUnused {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let _freetag = Decode::decode(decoder)?;
        let length = Decode::decode(decoder)?;
        decoder.reader().consume(length as usize - 6);
        let _tag = Decode::decode(decoder)?;
        Ok(Dir2DataUnused {
            _freetag,
            _length: length,
            _tag,
        })
    }
}

#[derive(Clone, Copy, Debug, Decode, Default)]
pub struct Dir2LeafEntry {
    pub hashval: XfsDahash,
    pub address: XfsDir2Dataptr,
}

impl Dir2LeafEntry {
    /// On-disk size in bytes
    pub const SIZE: usize = 8;
}

#[enum_dispatch::enum_dispatch]
pub trait Dir3 {
    fn lookup<R: Reader + BufRead + Seek>(
        &self,
        buf_reader: &mut R,
        sb: &Sb,
        name: &[u8],
    ) -> Result<u64, i32>;

    /// Read the next dirent from a Directory
    fn next<R: Reader + BufRead + Seek>(
        &self,
        buf_reader: &mut R,
        sb: &Sb,
        offset: i64,
    ) -> Result<(XfsIno, i64, Option<FileType>, Vec<u8>), i32>;
}

#[derive(Debug)]
#[enum_dispatch::enum_dispatch(Dir3)]
pub enum Directory {
    Sf(super::dir3_sf::Dir2Sf),
    Block(super::dir3_block::Dir2Block),
    Lf(super::dir3_lf::Dir2Lf),
}
