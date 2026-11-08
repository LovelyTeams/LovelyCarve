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
    io::{BufRead, Seek, SeekFrom},
};

use bincode_next::{de::read::Reader, Decode};

use super::{
    compat::{FileType, ENOENT},
    da_btree::hashname,
    definitions::*,
    dir3::{Dir2DataEntry, Dir2DataHdr, Dir2DataUnused, Dir2LeafEntry, Dir3, Dir3DataHdr},
    sb::Sb,
    utils::{decode, get_file_type, FileKind},
};

#[derive(Debug, Decode)]
pub struct Dir2BlockTail {
    count: u32,
    _stale: u32,
}

impl Dir2BlockTail {
    /// On-disk size in bytes
    pub const SIZE: usize = 8;
}

#[derive(Debug)]
pub struct Dir2BlockDisk {
    pub leaf: Vec<Dir2LeafEntry>,
    tail: Dir2BlockTail,
    raw: Vec<u8>,
    /// Start of directory entries within the directory block
    data_offset: usize,
}

impl Dir2BlockDisk {
    pub fn new<T>(buf_reader: &mut T, offset: u64, size: u32) -> Dir2BlockDisk
    where
        T: BufRead + Seek,
    {
        buf_reader.seek(SeekFrom::Start(offset)).unwrap();
        let mut raw = vec![0u8; size as usize];
        buf_reader.read_exact(&mut raw).unwrap();

        let magic: u32 = decode(&raw[..]).unwrap().0;
        let data_offset = match magic {
            XFS_DIR2_BLOCK_MAGIC => {
                let hdr: Dir2DataHdr = decode(&raw[..]).unwrap().0;
                assert_eq!(hdr.magic, XFS_DIR2_BLOCK_MAGIC);
                Dir2DataHdr::SIZE as usize
            }
            XFS_DIR3_BLOCK_MAGIC => {
                let hdr: Dir3DataHdr = decode(&raw[..]).unwrap().0;
                assert_eq!(hdr.hdr.magic, XFS_DIR3_BLOCK_MAGIC);
                Dir3DataHdr::SIZE as usize
            }
            _ => panic!("Unknown magic number for block directory {magic:#x}"),
        };

        let tail_offset = (size as usize) - Dir2BlockTail::SIZE;
        let tail: Dir2BlockTail = decode(&raw[tail_offset..]).unwrap().0;

        let mut leaf_offset = tail_offset - Dir2LeafEntry::SIZE * tail.count as usize;

        let mut leaf = Vec::with_capacity(tail.count as usize);
        for _i in 0..tail.count {
            leaf.push(decode(&raw[leaf_offset..]).unwrap().0);
            leaf_offset += Dir2LeafEntry::SIZE;
        }

        Dir2BlockDisk {
            leaf,
            tail,
            raw,
            data_offset,
        }
    }

    /// get the length of the raw data region
    fn get_data_len(&self, directory_block_size: u32) -> u64 {
        directory_block_size as u64
            - Dir2BlockTail::SIZE as u64
            - Dir2LeafEntry::SIZE as u64 * (self.tail.count as u64)
    }
}

#[derive(Debug)]
pub struct Dir2Block {
    ents: Vec<Dir2LeafEntry>,
    raw: Box<[u8]>,
    /// Start of directory entries within the directory block
    data_offset: usize,
}

impl Dir2Block {
    pub fn new<T: BufRead + Seek>(
        buf_reader: &mut T,
        superblock: &Sb,
        start_block: XfsFsblock,
    ) -> Dir2Block {
        let offset = superblock.fsb_to_offset(start_block);
        let dir_blk_size = superblock.sb_blocksize << superblock.sb_dirblklog;

        let dir_disk = Dir2BlockDisk::new(buf_reader.by_ref(), offset, dir_blk_size);

        let data_len = dir_disk.get_data_len(dir_blk_size);
        assert!(data_len as usize <= dir_disk.raw.len());
        let mut raw = dir_disk.raw;
        raw.truncate(data_len as usize);

        Dir2Block {
            raw: raw.into(),
            ents: dir_disk.leaf,
            data_offset: dir_disk.data_offset,
        }
    }

    fn get_addresses(&self, hash: XfsDahash) -> impl Iterator<Item = usize> + '_ {
        let i = self.ents.partition_point(|ent| ent.hashval < hash);
        let l = self.ents.len();
        let j = (i..l).find(|x| self.ents[*x].hashval > hash).unwrap_or(l);
        self.ents[i..j]
            .iter()
            .map(|ent| (ent.address << 3) as usize)
    }
}

impl Dir3 for Dir2Block {
    fn lookup<R: Reader + BufRead + Seek>(
        &self,
        _buf_reader: &mut R,
        _sb: &Sb,
        name: &[u8],
    ) -> Result<u64, i32> {
        let hash = hashname(name);

        for offset in self.get_addresses(hash) {
            assert!(offset < self.raw.len());
            let entry: Dir2DataEntry = decode(&self.raw[offset..]).unwrap().0;
            if entry.name.as_slice() == name {
                return Ok(entry.inumber);
            }
        }
        Err(ENOENT)
    }

    /// Read the next dirent from a Directory
    fn next<R: Reader + BufRead + Seek>(
        &self,
        _buf_reader: &mut R,
        sb: &Sb,
        offset: i64,
    ) -> Result<(XfsIno, i64, Option<FileType>, Vec<u8>), i32> {
        let mut offset: usize = offset.try_into().unwrap();
        assert!(offset < self.raw.len());
        let mut next = offset == 0;

        if offset == 0 {
            offset += self.data_offset;
        }

        while offset < self.raw.len() {
            let freetag: u16 = decode(&self.raw[offset..]).unwrap().0;
            if freetag == 0xffff {
                let (_, length) = decode::<Dir2DataUnused>(&self.raw[offset..]).unwrap();
                offset += length;
            } else if !next {
                let length = Dir2DataEntry::get_length(sb, &self.raw[offset..]);
                offset += length as usize;
                next = true;
            } else {
                let (entry, _l) = decode::<Dir2DataEntry>(&self.raw[offset..]).unwrap();
                let kind = match entry.ftype {
                    Some(ftype) => Some(get_file_type(FileKind::Type(ftype))?),
                    None => None,
                };
                let name = entry.name;
                let entry_offset = entry.tag as u64;
                return Ok((entry.inumber, entry_offset as i64, kind, name));
            }
        }
        Err(ENOENT)
    }
}
