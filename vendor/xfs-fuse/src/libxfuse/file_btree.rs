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

use bincode_next::de::read::Reader;

use super::{
    btree::{Btree, BtreeRoot},
    definitions::{XfsFileoff, XfsFsblock, XfsFsize},
    file::File,
    volume::SUPERBLOCK,
};

#[derive(Debug)]
pub struct FileBtree {
    pub btree: BtreeRoot,
    pub size: XfsFsize,
}

impl<R: BufRead + Reader + Seek> File<R> for FileBtree {
    fn get_extent(&self, buf_reader: &mut R, block: XfsFileoff) -> (Option<XfsFsblock>, u64) {
        let sb = SUPERBLOCK.get().unwrap();
        let (start, len) = self.btree.map_block(buf_reader.by_ref(), block).unwrap();
        let len = len.unwrap_or((self.size as u64).div_ceil(sb.sb_blocksize.into()) - block);
        (start, len)
    }

    fn lseek(&self, buf_reader: &mut R, offset: u64, whence: i32) -> Result<u64, i32> {
        self.btree.lseek(buf_reader, offset, whence)
    }

    fn size(&self) -> XfsFsize {
        self.size
    }
}
