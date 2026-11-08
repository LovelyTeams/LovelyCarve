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

use bincode_next::{de::Decoder, error::DecodeError, Decode};

use super::{
    compat::{ENXIO, SEEK_HOLE},
    definitions::*,
    volume::SUPERBLOCK,
};

#[derive(Debug, Clone, Copy)]
pub struct BmbtRec {
    pub br_startoff: XfsFileoff,
    pub br_startblock: XfsFsblock,
    pub br_blockcount: XfsFilblks,
    /// If set, indicates that the extent has been preallocated but has not yet been written
    /// (unwritten extent)
    pub br_flag: bool,
}

impl<Ctx> Decode<Ctx> for BmbtRec {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let br: u128 = Decode::decode(decoder)?;

        let br_blockcount = (br & ((1 << 21) - 1)) as u64;
        let br = br >> 21;

        let br_startblock = (br & ((1 << 52) - 1)) as u64;
        let br = br >> 52;

        let br_startoff = (br & ((1 << 54) - 1)) as u64;
        let br_flag = (br >> 54) != 0;

        Ok(BmbtRec {
            br_startoff,
            br_startblock,
            br_blockcount,
            br_flag,
        })
    }
}

/// An ordered list of [`BmbtRec`].
#[derive(Debug, Clone)]
pub struct Bmx(Vec<BmbtRec>);

impl Bmx {
    pub fn new<'a, I>(bmx: I) -> Self
    where
        I: IntoIterator<Item = &'a BmbtRec>,
    {
        // Filter out preallocated but unwritten extents.  This makes the lseek implementation much
        // easier than if we try to consider the br_flag field in the lseek method itself.
        let bmx = bmx
            .into_iter()
            .filter(|rec| !rec.br_flag)
            .cloned()
            .collect();
        Self(bmx)
    }

    /// Return the extent, if any, that contains the given block within the file.
    /// Return its starting position as an FSblock, and its length in file system block units.
    /// If a hole's length extends to EoF, return None for length.
    pub fn get_extent(&self, dblock: XfsFileoff) -> (Option<XfsFsblock>, Option<u64>) {
        match self.0.partition_point(|entry| entry.br_startoff <= dblock) {
            0 => {
                // A hole at the beginning of the file
                let len = self.0.first().map(|b| b.br_startoff - dblock);
                (None, len)
            }
            i => {
                let entry = &self.0[i - 1];
                let skip = dblock - entry.br_startoff;
                if entry.br_startoff + entry.br_blockcount > dblock {
                    assert!(!entry.br_flag);
                    (
                        Some(entry.br_startblock + skip),
                        Some(entry.br_blockcount - skip),
                    )
                } else {
                    // It's a hole
                    let len = self
                        .0
                        .get(i)
                        .map(|e| e.br_startoff - entry.br_startoff - skip);
                    (None, len)
                }
            }
        }
    }

    pub fn first(&self) -> Option<&BmbtRec> {
        self.0.first()
    }

    pub fn lseek(&self, offset: u64, whence: i32) -> Result<u64, i32> {
        let sb = SUPERBLOCK.get().unwrap();

        let dblock = offset >> sb.sb_blocklog;
        match self.0.partition_point(|entry| entry.br_startoff <= dblock) {
            0 => {
                // A hole at the beginning of the file
                if whence == SEEK_HOLE {
                    Ok(offset)
                } else {
                    self.first()
                        .map(|b| b.br_startoff << sb.sb_blocklog)
                        .ok_or(ENXIO)
                }
            }
            i => {
                let cur_entry = &self.0[i - 1];
                let br_end = cur_entry.br_startoff + cur_entry.br_blockcount;
                if dblock < br_end {
                    // In a data region
                    if whence == SEEK_HOLE {
                        // Scan for the next hole
                        for j in (i - 1)..self.0.len() - 1 {
                            let before = &self.0[j];
                            let after = &self.0[j + 1];
                            let br_end = before.br_startoff + before.br_blockcount;
                            if after.br_startoff > br_end {
                                return Ok(br_end << sb.sb_blocklog);
                            }
                        }
                        // Reached EOF without finding another hole.  Return the virtual hole at
                        // EOF
                        let entry = self.0.last().unwrap();
                        let br_end = entry.br_startoff + entry.br_blockcount;
                        Ok(br_end << sb.sb_blocklog)
                    } else {
                        Ok(offset)
                    }
                } else {
                    // In a hole
                    if whence == SEEK_HOLE {
                        Ok(offset)
                    } else {
                        match self.0.get(i) {
                            Some(next_entry) => Ok(next_entry.br_startoff << sb.sb_blocklog),
                            None => Err(ENXIO),
                        }
                    }
                }
            }
        }
    }

    pub fn map_dblock(&self, dblock: XfsDablk) -> Option<XfsFsblock> {
        let dblock = XfsFileoff::from(dblock);
        let i = self.0.partition_point(|rec| rec.br_startoff <= dblock);
        if i == 0 {
            return None;
        }
        let rec = &self.0[i - 1];
        if rec.br_startoff > dblock || rec.br_startoff + rec.br_blockcount <= dblock {
            None
        } else {
            Some(rec.br_startblock + dblock - rec.br_startoff)
        }
    }
}

impl<I: IntoIterator<Item = BmbtRec>> From<I> for Bmx {
    // The same as Bmx::new, but with an owned iterator
    fn from(i: I) -> Self {
        // Filter out preallocated but unwritten extents.  This makes the lseek implementation much
        // easier than if we try to consider the br_flag field in the lseek method itself.
        let bmx = i.into_iter().filter(|rec| !rec.br_flag).collect();
        Self(bmx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_dblock() {
        let bmx = Bmx::new(&[
            BmbtRec {
                br_startoff: 0,
                br_startblock: 20,
                br_blockcount: 2,
                br_flag: false,
            },
            BmbtRec {
                br_startoff: 2,
                br_startblock: 30,
                br_blockcount: 3,
                br_flag: false,
            },
            BmbtRec {
                br_startoff: 5,
                br_startblock: 40,
                br_blockcount: 2,
                br_flag: false,
            },
        ]);

        assert_eq!(bmx.map_dblock(6), Some(41));
    }
}
