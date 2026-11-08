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

use bincode_next::{
    de::{read::Reader, Decoder},
    error::DecodeError,
    impl_borrow_decode, Decode,
};

use super::{
    attr_leaf::AttrLeaf,
    attr_node::AttrNode,
    bmbt_rec::Bmx,
    compat::ENOATTR,
    da_btree::{XfsDa3Blkinfo, XfsDa3Intnode, XfsDaBlkinfo},
    definitions::{
        XfsDablk, XfsFsblock, XFS_ATTR3_LEAF_MAGIC, XFS_ATTR_LEAF_MAGIC, XFS_DA3_NODE_MAGIC,
        XFS_DA_NODE_MAGIC,
    },
    sb::Sb,
    utils,
    volume::SUPERBLOCK,
};

#[allow(dead_code)]
mod constants {
    pub const XFS_ATTR_LOCAL_BIT: u8 = 0;
    pub const XFS_ATTR_ROOT_BIT: u8 = 1;
    pub const XFS_ATTR_SECURE_BIT: u8 = 2;
    pub const XFS_ATTR_INCOMPLETE_BIT: u8 = 7;
    pub const XFS_ATTR_LOCAL: u8 = 1 << XFS_ATTR_LOCAL_BIT;
    pub const XFS_ATTR_ROOT: u8 = 1 << XFS_ATTR_ROOT_BIT;
    pub const XFS_ATTR_SECURE: u8 = 1 << XFS_ATTR_SECURE_BIT;
    pub const XFS_ATTR_INCOMPLETE: u8 = 1 << XFS_ATTR_INCOMPLETE_BIT;
    pub const XFS_ATTR_NSP_ONDISK_MASK: u8 = XFS_ATTR_ROOT | XFS_ATTR_SECURE;
}

pub const fn get_namespace_from_flags(flags: u8) -> &'static [u8] {
    if flags & constants::XFS_ATTR_SECURE != 0 {
        b"secure."
    } else if flags & constants::XFS_ATTR_ROOT != 0 {
        b"trusted."
    } else {
        b"user."
    }
}

pub const fn get_namespace_size_from_flags(flags: u8) -> u32 {
    get_namespace_from_flags(flags).len() as u32
}

#[derive(Debug, Decode)]
pub struct AttrLeafMap {
    _base: u16,
    _size: u16,
}

#[derive(Debug)]
pub struct AttrLeafHdr {
    pub forw: u32,
    pub count: u16,
}

impl<Ctx> Decode<Ctx> for AttrLeafHdr {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let magic: u16 = utils::decode(&decoder.reader().peek_read(10).unwrap()[8..])?.0;
        let forw = match magic {
            XFS_ATTR_LEAF_MAGIC => {
                let info: XfsDaBlkinfo = Decode::decode(decoder)?;
                info.forw
            }
            XFS_ATTR3_LEAF_MAGIC => {
                let info: XfsDa3Blkinfo = Decode::decode(decoder)?;
                info.forw
            }
            _ => panic!("Unexpected magic value {magic:#x}"),
        };
        let count = Decode::decode(decoder)?;
        let _usedbytes: u16 = Decode::decode(decoder)?;
        let _firstused: u16 = Decode::decode(decoder)?;
        let _holes: u8 = Decode::decode(decoder)?;
        let _pad1: u8 = Decode::decode(decoder)?;
        let _freemap: [AttrLeafMap; 3] = Decode::decode(decoder)?;
        if magic == XFS_ATTR3_LEAF_MAGIC {
            let _pad2: u32 = Decode::decode(decoder)?;
        }

        Ok(Self { forw, count })
    }
}
impl_borrow_decode!(AttrLeafHdr);

#[derive(Debug, Decode)]
pub struct AttrLeafEntry {
    pub hashval: u32,
    pub nameidx: u16,
    pub flags: u8,
    _pad2: u8,
}

#[derive(Debug)]
pub struct AttrLeafNameLocal {
    pub namelen: u8,
    pub nameval: Vec<u8>,
}

impl<Ctx> Decode<Ctx> for AttrLeafNameLocal {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let valuelen: u16 = Decode::decode(decoder)?;
        let namelen: u8 = Decode::decode(decoder)?;
        let mut nameval = vec![0u8; usize::from(namelen) + usize::from(valuelen)];
        decoder.reader().read(&mut nameval[..])?;

        Ok(Self { namelen, nameval })
    }
}

#[derive(Debug)]
pub enum AttrLeafName {
    Local(AttrLeafNameLocal),
    Remote(AttrLeafNameRemote),
}

impl AttrLeafName {
    fn name(&self) -> &[u8] {
        match self {
            AttrLeafName::Local(local) => &local.nameval[0..usize::from(local.namelen)],
            AttrLeafName::Remote(remote) => &remote.name[0..usize::from(remote.namelen)],
        }
    }

    fn namelen(&self) -> u8 {
        match self {
            AttrLeafName::Local(local) => local.namelen,
            AttrLeafName::Remote(remote) => remote.namelen,
        }
    }

    fn value<F, R>(&mut self, buf_reader: &mut R, map_dblock: F) -> &[u8]
    where
        R: BufRead + Reader + Seek,
        F: Fn(XfsDablk, &mut R) -> XfsFsblock,
    {
        match self {
            AttrLeafName::Local(local) => &local.nameval[local.namelen as usize..],
            AttrLeafName::Remote(remote) => remote.value(buf_reader.by_ref(), map_dblock),
        }
    }
}

#[derive(Debug)]
pub struct AttrLeafblock {
    pub hdr: AttrLeafHdr,
    // TODO: in-memory, combine AttrLeafEntry and AttrLeafName into a struct, so we'll only need a
    // single Vec
    pub entries: Vec<AttrLeafEntry>,
    pub names: Vec<AttrLeafName>,
}

impl AttrLeafblock {
    pub fn get_total_size(&self) -> u32 {
        let mut total: u32 = 0;

        for (entry, name) in std::iter::zip(self.entries.iter(), self.names.iter()) {
            total += get_namespace_size_from_flags(entry.flags) + u32::from(name.namelen()) + 1;
        }

        total
    }

    pub fn list(&self, list: &mut Vec<u8>) {
        for (entry, name_entry) in std::iter::zip(self.entries.iter(), self.names.iter()) {
            list.extend_from_slice(get_namespace_from_flags(entry.flags));
            list.extend_from_slice(name_entry.name());
            list.push(0)
        }
    }

    pub fn get<R: BufRead + Reader + Seek, F: Fn(XfsDablk, &mut R) -> XfsFsblock>(
        &mut self,
        buf_reader: &mut R,
        hash: u32,
        map_logical_block_to_fs_block: F,
    ) -> Result<&[u8], i32> {
        match self
            .entries
            .binary_search_by_key(&hash, |entry| entry.hashval)
        {
            Ok(i) => Ok(self.names[i].value(buf_reader, map_logical_block_to_fs_block)),
            Err(_) => Err(ENOATTR),
        }
    }
}

impl<Ctx> Decode<Ctx> for AttrLeafblock {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let blocksize = SUPERBLOCK.get().unwrap().sb_blocksize as usize;
        let mut raw = vec![0u8; blocksize];
        decoder.reader().read(&mut raw[..])?;

        let config = decoder.config();
        let sl = bincode_next::de::read::SliceReader::new(&raw);
        let mut sldecoder = bincode_next::de::DecoderImpl::new(sl, *config, ());
        let hdr: AttrLeafHdr = Decode::decode(&mut sldecoder)?;

        let mut entries = Vec::<AttrLeafEntry>::with_capacity(hdr.count.into());
        for _i in 0..entries.capacity() {
            entries.push(Decode::decode(&mut sldecoder)?);
        }

        let mut names = Vec::with_capacity(entries.len());
        for e in entries.iter() {
            let ofs = usize::from(e.nameidx);
            if e.flags & constants::XFS_ATTR_LOCAL != 0 {
                let local = bincode_next::decode_from_slice(&raw[ofs..], *config)?.0;
                names.push(AttrLeafName::Local(local));
            } else {
                let remote = bincode_next::decode_from_slice(&raw[ofs..], *config)?.0;
                names.push(AttrLeafName::Remote(remote));
            }
        }

        Ok(AttrLeafblock {
            hdr,
            entries,
            names,
        })
    }
}

#[derive(Debug)]
pub struct AttrLeafNameRemote {
    pub valueblk: u32,
    pub valuelen: u32,
    pub namelen: u8,
    pub name: Vec<u8>,
    pub value: Vec<u8>,
}

impl AttrLeafNameRemote {
    fn value<R, F>(&mut self, buf_reader: &mut R, map_dblock: F) -> &[u8]
    where
        R: BufRead + Reader + Seek,
        F: Fn(XfsDablk, &mut R) -> XfsFsblock,
    {
        if self.value.len() < self.valuelen as usize {
            let sb = SUPERBLOCK.get().unwrap();
            self.value.reserve(self.valuelen as usize);
            let mut valueblk = self.valueblk;
            let mut valuelen: i64 = self.valuelen.into();

            while valuelen > 0 {
                let blk_num = map_dblock(valueblk, buf_reader.by_ref());
                buf_reader
                    .seek(SeekFrom::Start(sb.fsb_to_offset(blk_num)))
                    .unwrap();
                let hdr: AttrRmtHdr = utils::decode_from(buf_reader.by_ref()).unwrap();
                let oldlen = self.value.len();
                self.value.resize(oldlen + hdr.rm_bytes as usize, 0);
                buf_reader.read_exact(&mut self.value[oldlen..]).unwrap();
                valuelen -= i64::from(hdr.rm_bytes);
                valueblk += 1;
            }
        }
        &self.value[..]
    }
}

impl<Ctx> Decode<Ctx> for AttrLeafNameRemote {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let valueblk = Decode::decode(decoder)?;
        let valuelen = Decode::decode(decoder)?;
        let namelen: u8 = Decode::decode(decoder)?;
        let mut name = vec![0u8; usize::from(namelen)];
        decoder.reader().read(&mut name[..])?;
        let value = Default::default();

        Ok(Self {
            valueblk,
            valuelen,
            namelen,
            name,
            value,
        })
    }
}

#[derive(Debug, Decode)]
struct AttrRmtHdr {
    _rm_magic: u32,
    _rm_offset: u32,
    rm_bytes: u32,
    _rm_crc: u32,
    _rm_uuid: utils::Uuid,
    _rm_owner: u64,
    _rm_blkno: u64,
    _rm_lsn: u64,
}

#[enum_dispatch::enum_dispatch]
pub trait Attr {
    fn get_total_size<R: BufRead + Reader + Seek>(
        &mut self,
        buf_reader: &mut R,
        super_block: &Sb,
    ) -> u32;

    fn list<R: BufRead + Reader + Seek>(&mut self, buf_reader: &mut R, super_block: &Sb)
        -> Vec<u8>;

    fn get<R>(&mut self, buf_reader: &mut R, super_block: &Sb, name: &[u8]) -> Result<Vec<u8>, i32>
    where
        R: BufRead + Reader + Seek;
}

/// Open an attribute block, whose type may be unknown until its contents are examined.
pub fn open<R: Reader + BufRead + Seek>(
    buf_reader: &mut R,
    superblock: &Sb,
    bmx: Bmx,
) -> Attributes {
    if let Some(rec) = bmx.first() {
        let ofs = superblock.fsb_to_offset(rec.br_startblock);
        buf_reader.seek(SeekFrom::Start(ofs)).unwrap();
        let mut raw = vec![0u8; superblock.sb_blocksize as usize];
        buf_reader.read_exact(&mut raw).unwrap();
        // What follows is either a xfs_da_blkinfo or a xfs_da3_blkinfo.  The first three fields
        // are the same.
        let magic: u16 = utils::decode(&raw[8..]).unwrap().0;

        match magic {
            XFS_ATTR_LEAF_MAGIC | XFS_ATTR3_LEAF_MAGIC => {
                let leaf: AttrLeafblock = utils::decode(&raw).unwrap().0;
                Attributes::Leaf(AttrLeaf {
                    bmx,
                    leaf,
                    total_size: -1,
                })
            }
            XFS_DA_NODE_MAGIC | XFS_DA3_NODE_MAGIC => {
                let node: XfsDa3Intnode = utils::decode(&raw).unwrap().0;
                Attributes::Node(AttrNode::new(bmx, node))
            }
            magic => {
                panic!(
                    "bad magic!  expected either {XFS_ATTR3_LEAF_MAGIC:#x} or \
                     {XFS_DA3_NODE_MAGIC:#x} but found {magic:#x}"
                );
            }
        }
    } else {
        panic!("Extent records missing!");
    }
}

#[derive(Debug)]
#[enum_dispatch::enum_dispatch(Attr)]
pub enum Attributes {
    Sf(crate::libxfuse::attr_shortform::AttrShortform),
    Leaf(AttrLeaf),
    Node(AttrNode),
    Btree(crate::libxfuse::attr_bptree::AttrBtree),
}
