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
    cell::{Ref, RefCell},
    collections::{btree_map::Entry, BTreeMap},
    io::{BufRead, Seek, SeekFrom},
};

use bincode_next::{
    de::{read::Reader, Decoder},
    error::DecodeError,
    impl_borrow_decode, Decode,
};
use byteorder::{BigEndian, ReadBytesExt};

use super::{compat::ENOENT, definitions::*, sb::Sb, utils, utils::Uuid, volume::SUPERBLOCK};

pub fn hashname(name: &[u8]) -> XfsDahash {
    let mut namelen = name.len();
    let mut hash: XfsDahash = 0;

    let mut i: usize = 0;
    while namelen >= 4 {
        hash = ((name[i] as u32) << 21)
            ^ ((name[i + 1] as u32) << 14)
            ^ ((name[i + 2] as u32) << 7)
            ^ (name[i + 3] as u32)
            ^ hash.rotate_left(7 * 4);

        namelen -= 4;
        i += 4;
    }

    match namelen {
        3 => {
            ((name[i] as u32) << 14)
                ^ ((name[i + 1] as u32) << 7)
                ^ (name[i + 2] as u32)
                ^ hash.rotate_left(7 * 3)
        }
        2 => ((name[i] as u32) << 7) ^ (name[i + 1] as u32) ^ hash.rotate_left(7 * 2),
        1 => (name[i] as u32) ^ hash.rotate_left(7),
        _ => hash,
    }
}

#[derive(Debug, Decode)]
pub struct XfsDaBlkinfo {
    pub forw: u32,
    _back: u32,
    _magic: u16,
    _pad: u16,
}

#[derive(Debug)]
pub struct XfsDa3Blkinfo {
    pub forw: u32,
    // _back: u32
    pub magic: u16,
    // _pad: u16
    // _crc: u32
    // _blkno: u64
    // _lsn: u64
    // uuid: Uuid
    // _owner: u64
}

impl<Ctx> Decode<Ctx> for XfsDa3Blkinfo {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let forw = Decode::decode(decoder)?;
        let _back: u32 = Decode::decode(decoder)?;
        let magic = Decode::decode(decoder)?;
        let _pad: u16 = Decode::decode(decoder)?;
        let _crc: u32 = Decode::decode(decoder)?;
        let _blkno: u64 = Decode::decode(decoder)?;
        let _lsn: u64 = Decode::decode(decoder)?;
        let uuid: Uuid = Decode::decode(decoder)?;
        let _owner: u64 = Decode::decode(decoder)?;
        assert_eq!(
            uuid,
            SUPERBLOCK.get().unwrap().metadata_uuid,
            "UUID mismatch!"
        );

        Ok(XfsDa3Blkinfo { forw, magic })
    }
}
impl_borrow_decode!(XfsDa3Blkinfo);

#[derive(Debug, Decode)]
struct XfsDaNodeHdr {
    _info: XfsDaBlkinfo,
    pub count: u16,
    pub level: u16,
}

#[derive(Debug)]
struct XfsDa3NodeHdr {
    // info: XfsDa3Blkinfo,
    pub count: u16,
    pub level: u16,
    // _pad32: u32
}

impl<Ctx> Decode<Ctx> for XfsDa3NodeHdr {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let info: XfsDa3Blkinfo = Decode::decode(decoder)?;
        if info.magic != XFS_DA3_NODE_MAGIC {
            return Err(DecodeError::Other("bad magic"));
        }
        let count = Decode::decode(decoder)?;
        let level = Decode::decode(decoder)?;
        let _pad32: u32 = Decode::decode(decoder)?;
        Ok(XfsDa3NodeHdr { count, level })
    }
}
impl_borrow_decode!(XfsDa3NodeHdr);

#[derive(Debug, Decode)]
pub struct XfsDa3NodeEntry {
    pub hashval: XfsDahash,
    pub before: XfsDablk,
}

impl XfsDa3NodeEntry {
    pub fn from<R: BufRead>(buf_reader: &mut R) -> XfsDa3NodeEntry {
        let hashval = buf_reader.read_u32::<BigEndian>().unwrap();
        let before = buf_reader.read_u32::<BigEndian>().unwrap();

        XfsDa3NodeEntry { hashval, before }
    }
}

/// A BTree Interior node.  Could be either an xfs_da_intnode or xfs_da3_intnode, depending on file
/// system verison.
#[derive(Debug)]
pub struct XfsDa3Intnode {
    pub magic: u16,
    level: u16,
    //hdr: XfsDa3NodeHdr,
    pub btree: Vec<XfsDa3NodeEntry>,
    children: RefCell<BTreeMap<XfsDablk, Self>>,
}

impl XfsDa3Intnode {
    pub fn from<R: BufRead + Reader + Seek>(buf_reader: &mut R) -> XfsDa3Intnode {
        let magic: u16 = utils::decode(&buf_reader.peek_read(10).unwrap()[8..])
            .unwrap()
            .0;
        let (count, level) = match magic {
            XFS_DA_NODE_MAGIC => {
                let hdr: XfsDaNodeHdr = utils::decode_from(buf_reader.by_ref()).unwrap();
                (hdr.count, hdr.level)
            }
            XFS_DA3_NODE_MAGIC => {
                let hdr: XfsDa3NodeHdr = utils::decode_from(buf_reader.by_ref()).unwrap();
                (hdr.count, hdr.level)
            }
            _ => panic!("Bad magic in XfsDa3Intnode! {magic:#x}"),
        };

        let mut btree = Vec::<XfsDa3NodeEntry>::new();
        for _i in 0..count {
            btree.push(XfsDa3NodeEntry::from(buf_reader.by_ref()))
        }
        let children = Default::default();

        XfsDa3Intnode {
            magic,
            level,
            btree,
            children,
        }
    }

    pub fn lookup<R: BufRead + Reader + Seek, F: Fn(XfsDablk, &mut R) -> XfsFsblock>(
        &self,
        buf_reader: &mut R,
        super_block: &Sb,
        hash: u32,
        map_dblock: F,
    ) -> Result<XfsDablk, i32> {
        let pidx = self.btree.partition_point(|k| k.hashval < hash);
        if pidx >= self.btree.len() {
            return Err(ENOENT);
        }
        let before = self.btree[pidx].before;

        if self.level == 1 {
            Ok(before)
        } else {
            assert!(self.level > 1);

            let node = self.read_child(buf_reader.by_ref(), super_block, before, &map_dblock)?;
            node.lookup(buf_reader.by_ref(), super_block, hash, map_dblock)
        }
    }

    pub fn first_block<R, F>(&self, buf_reader: &mut R, super_block: &Sb, map_dblock: F) -> XfsDablk
    where
        R: BufRead + Reader + Seek,
        F: Fn(XfsDablk, &mut R) -> XfsFsblock,
    {
        if self.level == 1 {
            self.btree.first().unwrap().before
        } else {
            let before = self.btree.first().unwrap().before;
            let node = self
                .read_child(buf_reader.by_ref(), super_block, before, &map_dblock)
                .unwrap();
            node.first_block(buf_reader.by_ref(), super_block, map_dblock)
        }
    }

    fn read_child<'a, R, F>(
        &'a self,
        buf_reader: &mut R,
        super_block: &Sb,
        dblock: XfsDablk,
        map_dblock: &F,
    ) -> Result<impl std::ops::Deref<Target = Self> + 'a, i32>
    where
        R: BufRead + Reader + Seek,
        F: Fn(XfsDablk, &mut R) -> XfsFsblock,
    {
        let mut cache_guard = self.children.borrow_mut();
        let entry = cache_guard.entry(dblock);
        if matches!(entry, Entry::Vacant(_)) {
            let fsblock = map_dblock(dblock, buf_reader.by_ref());
            let offset = super_block.fsb_to_offset(fsblock);
            buf_reader.seek(SeekFrom::Start(offset)).unwrap();
            buf_reader.fill_buf().unwrap();
            let node = XfsDa3Intnode::from(buf_reader.by_ref());
            entry.or_insert(node);
        }
        // Annoyingly, there's no function to downgrade a RefMut into a Ref.
        drop(cache_guard);
        let cache_guard = self.children.borrow();
        Ok(Ref::map(cache_guard, |v| &v[&dblock]))
    }
}

impl<Ctx> Decode<Ctx> for XfsDa3Intnode {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let magic: u16 = utils::decode(&decoder.reader().peek_read(10).unwrap()[8..])?.0;
        let (count, level) = match magic {
            XFS_DA_NODE_MAGIC => {
                let hdr: XfsDaNodeHdr = Decode::decode(decoder)?;
                (hdr.count, hdr.level)
            }
            XFS_DA3_NODE_MAGIC => {
                let hdr: XfsDa3NodeHdr = Decode::decode(decoder)?;
                (hdr.count, hdr.level)
            }
            _ => panic!("Bad magic in XfsDa3Intnode! {magic:#x}"),
        };
        let mut btree = Vec::<XfsDa3NodeEntry>::new();
        for _i in 0..count {
            btree.push(Decode::decode(decoder)?);
        }
        let children = Default::default();

        Ok(XfsDa3Intnode {
            magic,
            level,
            btree,
            children,
        })
    }
}

/// Not really a "test" per se.  Instead it finds hash collisions to use in other tests.
#[test]
#[ignore = "Not a real test"]
fn hashname_collisions() {
    use std::collections::HashMap;

    let way = 4;
    let want = 10;
    let mut allnames: HashMap<XfsDahash, Vec<Vec<u8>>> = HashMap::new();
    let mut collisions = 0;
    for i in 0u64.. {
        let name = format!("{i:x}").into_bytes();
        let hash = hashname(&name);
        if let Some(mut v) = allnames.insert(hash, vec![name.clone()]) {
            v.push(name);
            if v.len() >= way {
                println!("{way}-way hash collision found");
                collisions += 1;
            }
            allnames.insert(hash, v);
        }
        if collisions >= want {
            break;
        }
    }
}
