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
    convert::TryInto,
    io::{BufRead, Seek},
};

use bincode_next::de::read::Reader;

use super::{
    attr::{Attr, AttrLeafblock},
    bmbt_rec::Bmx,
    da_btree::hashname,
    sb::Sb,
};

#[derive(Debug)]
pub struct AttrLeaf {
    pub bmx: Bmx,
    pub leaf: AttrLeafblock,
    pub total_size: i64,
}

impl Attr for AttrLeaf {
    fn get_total_size<R: BufRead + Reader + Seek>(
        &mut self,
        _buf_reader: &mut R,
        _super_block: &Sb,
    ) -> u32 {
        if self.total_size != -1 {
            self.total_size.try_into().unwrap()
        } else {
            self.total_size = i64::from(self.leaf.get_total_size());
            self.total_size as u32
        }
    }

    fn list<R: BufRead + Reader + Seek>(
        &mut self,
        buf_reader: &mut R,
        super_block: &Sb,
    ) -> Vec<u8> {
        let mut list: Vec<u8> =
            Vec::with_capacity(self.get_total_size(buf_reader.by_ref(), super_block) as usize);

        self.leaf.list(&mut list);

        list
    }

    fn get<R>(&mut self, buf_reader: &mut R, _super_block: &Sb, name: &[u8]) -> Result<Vec<u8>, i32>
    where
        R: BufRead + Reader + Seek,
    {
        let hash = hashname(name);

        let bmx = &self.bmx;
        self.leaf
            .get(buf_reader.by_ref(), hash, |block, _| {
                bmx.map_dblock(block)
                    .expect("holes are not allowed in attr forks")
            })
            .map(Vec::from)
    }
}
