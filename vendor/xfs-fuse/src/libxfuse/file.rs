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
use std::{
    cmp::min,
    io::{BufRead, Seek, SeekFrom},
};

use bincode_next::de::read::Reader;

use super::{
    definitions::{XfsFileoff, XfsFsblock, XfsFsize},
    volume::SUPERBLOCK,
};

pub trait File<R: BufRead + Reader + Seek> {
    /// Return the extent, if any, that contains the given data block within the file.
    /// Return its starting position as an FSblock, and its length in file system block units
    fn get_extent(&self, buf_reader: &mut R, block: XfsFileoff) -> (Option<XfsFsblock>, u64);

    /// Like lseek(2), but only works for SEEK_HOLE and SEEK_DATA
    fn lseek(&self, buf_reader: &mut R, offset: u64, whence: i32) -> Result<u64, i32>;

    /// Perform a sector-size aligned read of the file
    fn read_sectors(
        &self,
        buf_reader: &mut R,
        offset: i64,
        mut size: usize,
    ) -> Result<Vec<u8>, i32> {
        let sb = SUPERBLOCK.get().unwrap();
        debug_assert_eq!(
            offset & ((1i64 << sb.sb_blocklog) - 1),
            0,
            "fusefs did a non-sector-size aligned read.  offset={offset:?} size={size:?}"
        );
        debug_assert_eq!(
            size & ((1usize << sb.sb_blocklog) - 1),
            0,
            "fusefs did a non-sector-size aligned read.  offset={offset:?} size={size:?}"
        );

        let mut data = Vec::<u8>::with_capacity(size);

        let mut logical_block = u64::try_from(offset >> sb.sb_blocklog).unwrap();
        let mut block_offset: u64 = 0;

        while size > 0 {
            let (blk, blocks) = self.get_extent(buf_reader.by_ref(), logical_block);
            let z = usize::try_from(min(
                u64::try_from(size).unwrap(),
                (blocks << sb.sb_blocklog) - block_offset,
            ))
            .unwrap();

            let oldlen = data.len();
            data.resize(oldlen + z, 0u8);
            if let Some(blk) = blk {
                buf_reader
                    .seek(SeekFrom::Start(sb.fsb_to_offset(blk) + block_offset))
                    .map_err(|e| e.raw_os_error().unwrap())?;

                buf_reader
                    .read_exact(&mut data[oldlen..])
                    .map_err(|e| e.raw_os_error().unwrap())?;
            } else {
                // A hole
            }
            logical_block += blocks;
            size -= z;
            block_offset = 0;
        }

        Ok(data)
    }

    /// Return from a file.  Return a buffer containing the requested data, plus a number of bytes
    /// that the caller should ignore from the head of the vector.
    fn read(&self, buf_reader: &mut R, offset: i64, size: u32) -> Result<(Vec<u8>, usize), i32> {
        let sb = SUPERBLOCK.get().unwrap();
        let size = u32::try_from(i64::from(size).min(self.size() - offset)).unwrap();

        let block_offset = usize::try_from(offset & ((1i64 << sb.sb_blocklog) - 1)).unwrap();
        let size_with_leader = usize::try_from(size).unwrap() + block_offset;
        let size_remainder = size_with_leader & ((1 << sb.sb_blocklog) - 1);
        let actual_size = if size_remainder > 0 {
            size_with_leader + usize::try_from(sb.sb_blocksize).unwrap() - size_remainder
        } else {
            size_with_leader
        };
        let actual_offset = offset - i64::try_from(block_offset).unwrap();
        let mut v = self.read_sectors(buf_reader, actual_offset, actual_size)?;
        v.resize(size_with_leader, 0);
        Ok((v, block_offset))
    }

    fn size(&self) -> XfsFsize;
}
