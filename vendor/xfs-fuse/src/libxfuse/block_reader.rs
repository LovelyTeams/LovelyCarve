/*
 * BSD 2-Clause License
 *
 * Copyright (c) 2024, Benjamin Stürz
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

//! Block-aligned buffered reader generic over any `Read + Seek` source.
//!
//! Upstream this struct wrapped `std::fs::File` directly and pulled the device
//! sector size from `MetadataExt::blksize()` (Unix) / a FreeBSD `DIOCGSECTORSIZE`
//! ioctl. Both paths are gone — the lovelycarve XFS adapter feeds in a partition
//! cursor backed by `DiskImage` (raw mmap, EWF, VMDK), so the host-IO source is
//! plain `Read + Seek` and the sector size comes from the caller.

use std::io::{self, BufRead, Read, Result as IoResult, Seek, SeekFrom};

use bincode_next::{de::read::Reader, error::DecodeError};

use super::compat::EINVAL;

/// Default sector size handed to a freshly-opened `BlockReader`. 512 bytes
/// matches the XFS minimum and historical disk geometry; callers reset it to
/// the filesystem block size via `set_bufsize` once the superblock is parsed.
pub const DEFAULT_SECTOR_SIZE: usize = 512;

#[derive(Debug)]
pub struct BlockReader<R: Read + Seek> {
    inner: R,
    block: Vec<u8>,
    idx: usize,
    /// Absolute floor on read granularity. Each refill of the internal buffer
    /// reads `block.len()` bytes — which must always be a multiple of this.
    sectorsize: usize,
}

impl<R: Read + Seek> BlockReader<R> {
    /// Wrap an arbitrary `Read + Seek` source. The buffer starts empty (idx ==
    /// block.len()) so the first read triggers a refill. Sector size defaults
    /// to 512 bytes; bump it via `set_bufsize` after parsing the superblock.
    pub fn new(inner: R) -> Self {
        Self::with_sector_size(inner, DEFAULT_SECTOR_SIZE)
    }

    pub fn with_sector_size(inner: R, sectorsize: usize) -> Self {
        let sectorsize = sectorsize.max(1);
        let block = vec![0u8; sectorsize];
        Self {
            inner,
            block,
            idx: sectorsize,
            sectorsize,
        }
    }

    fn refill(&mut self) -> IoResult<()> {
        self.inner.read_exact(&mut self.block)?;
        self.idx = 0;
        Ok(())
    }

    fn buffered(&self) -> usize {
        self.block.len() - self.idx
    }

    fn refill_if_empty(&mut self) -> IoResult<()> {
        if self.buffered() == 0 {
            self.refill()?;
        }
        Ok(())
    }

    /// The current size of the buffer
    pub fn bufsize(&self) -> usize {
        self.block.len()
    }

    /// Change the reader's bufsize.  It will be rounded up to a multiple of
    /// the sectorsize. After this operation, the buffer should be considered
    /// undefined until the next absolute Seek operation.
    pub fn set_bufsize(&mut self, bufsize: usize) {
        let bufsize = bufsize.max(self.sectorsize);
        let remainder = bufsize % self.sectorsize;
        let bufsize = if remainder > 0 {
            bufsize + self.sectorsize - remainder
        } else {
            bufsize
        };
        self.block.resize(bufsize, 0u8);
        self.idx = bufsize;
    }
}

impl<R: Read + Seek> Read for BlockReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.refill_if_empty()?;
        let num = buf.len().min(self.buffered());
        let buf = &mut buf[0..num];
        buf.copy_from_slice(&self.block[self.idx..(self.idx + num)]);
        self.idx += num;
        Ok(num)
    }
}

impl<R: Read + Seek> BufRead for BlockReader<R> {
    fn fill_buf(&mut self) -> IoResult<&[u8]> {
        self.refill_if_empty()?;
        Ok(&self.block[self.idx..])
    }

    fn consume(&mut self, amt: usize) {
        assert!(amt <= self.buffered());
        self.idx += amt;
    }
}

impl<R: Read + Seek> Seek for BlockReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> IoResult<u64> {
        let bs = self.bufsize() as u64;
        match pos {
            SeekFrom::Start(pos) => {
                let real = self.inner.seek(SeekFrom::Start(pos / bs * bs))?;
                let rem = pos - real;
                assert!(rem < bs);

                self.refill()?;
                self.idx = rem as usize;

                Ok(real + rem)
            }
            SeekFrom::Current(offset) => {
                let real = self.inner.stream_position()?;
                let cur = real - self.block.len() as u64 + self.idx as u64;
                let newidx = offset + self.idx as i64;
                if newidx >= 0 && newidx < self.bufsize() as i64 {
                    // The data is already buffered; just adjust the pointer
                    self.idx = newidx as usize;
                    Ok(real - self.block.len() as u64 + newidx as u64)
                } else if cur as i64 + offset < 0 {
                    Err(io::Error::from_raw_os_error(EINVAL))
                } else {
                    self.seek(SeekFrom::Start((cur as i64 + offset) as u64))
                }
            }
            SeekFrom::End(_) => todo!("SeekFrom::End()"),
        }
    }
}

impl<R: Read + Seek> Reader for BlockReader<R> {
    fn read(&mut self, bytes: &mut [u8]) -> Result<(), DecodeError> {
        self.read_exact(bytes).map_err(|inner| DecodeError::Io {
            inner,
            additional: bytes.len(),
        })
    }

    fn peek_read(&mut self, n: usize) -> Option<&[u8]> {
        self.block[self.idx..].get(..n)
    }

    fn consume(&mut self, n: usize) {
        <Self as std::io::BufRead>::consume(self, n);
    }
}
