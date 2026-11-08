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
use std::io::{BufRead, Seek, SeekFrom};

use bincode_next::{de::read::Reader, Decode};

use super::{
    bmbt_rec::Bmx,
    definitions::XFS_SYMLINK_MAGIC,
    sb::Sb,
    utils::{decode_from, Uuid},
};

/// Decode targets by inode length; V5 sl_offset is a logical offset, not a seek delta.
pub(crate) fn read<R: BufRead + Reader + Seek>(
    reader: &mut R,
    sb: &Sb,
    inode: &super::dinode::Dinode,
    owner: u64,
) -> Result<Vec<u8>, i32> {
    use super::{checked::*, compat::EIO, dinode::DiU};
    let length = usize::try_from(inode.di_core.di_size).map_err(|_| EIO)?;
    if length == 0 || length > 1024 {
        return Err(EIO);
    }
    if let DiU::Symlink(data) = &inode.di_u {
        if data.len() != length || data.contains(&0) {
            return Err(EIO);
        }
        return Ok(data.clone());
    }
    let records = super::logical::data_extents(reader, sb, inode)?;
    if records.iter().any(|r| r.br_flag) {
        return Err(EIO);
    }
    let mut output = Vec::with_capacity(length);
    let mut logical_block = 0;
    while output.len() < length {
        let (raw, offset) = super::attributes::block(reader, sb, &records, logical_block)?;
        let (header, count) = if sb.version() == 5 {
            crc(&raw, 12)?;
            let count = u32be(&raw, 8)? as usize;
            if u32be(&raw, 0)? != XFS_SYMLINK_MAGIC
                || u32be(&raw, 4)? as usize != output.len()
                || count == 0
                || count > raw.len() - 56
                || count > length - output.len()
                || bytes(&raw, 16, 16)? != sb.metadata_uuid.as_bytes()
                || u64be(&raw, 32)? != owner
                || u64be(&raw, 40)? != offset / 512
            {
                return Err(EIO);
            }
            (56, count)
        } else {
            (0, raw.len().min(length - output.len()))
        };
        output.extend_from_slice(bytes(&raw, header, count)?);
        logical_block += 1;
    }
    if output.contains(&0) {
        return Err(EIO);
    }
    Ok(output)
}

#[derive(Clone, Copy, Debug, Decode)]
pub struct DsymlinkHdr {
    sl_magic: u32,
    sl_offset: u32,
    sl_bytes: u32,
    _sl_crc: u32,
    _sl_uuid: Uuid,
    _sl_owner: u64,
    _sl_blkno: u64,
    _sl_lsn: u64,
}

#[derive(Debug)]
pub struct SymlinkExtents;

impl SymlinkExtents {
    pub fn get_target<T: BufRead + Reader + Seek>(
        buf_reader: &mut T,
        bmx: &Bmx,
        superblock: &Sb,
    ) -> Vec<u8> {
        let mut data = Vec::<u8>::with_capacity(1024);

        let mut dblock = 0;
        loop {
            let (ofsb, oblocks) = bmx.get_extent(dblock);
            if ofsb.is_none() {
                break;
            }
            let fsb = ofsb.unwrap();
            let blocks = oblocks.unwrap();
            buf_reader
                .seek(SeekFrom::Start(superblock.fsb_to_offset(fsb)))
                .unwrap();

            let bytes = match superblock.version() {
                5 => {
                    let hdr: DsymlinkHdr = decode_from(buf_reader.by_ref()).unwrap();
                    assert_eq!(XFS_SYMLINK_MAGIC, hdr.sl_magic);

                    buf_reader
                        .seek(SeekFrom::Current(hdr.sl_offset as i64))
                        .unwrap();
                    hdr.sl_bytes as usize
                }
                4 => {
                    // Version 4 file systems do not have the DsymlinkHdr
                    (blocks as usize) << superblock.sb_blocklog
                }
                _ => unimplemented!(),
            };

            let oldlen = data.len();
            data.resize(oldlen + bytes, 0);
            buf_reader.read_exact(&mut data[oldlen..]).unwrap();
            dblock += blocks;
        }

        // Trim at the first NUL — V4 file systems do not store an explicit
        // length, so the on-disk buffer is padded with zeros.
        if let Some(pos) = data.iter().position(|b| *b == 0) {
            data.truncate(pos);
        }
        data
    }
}
