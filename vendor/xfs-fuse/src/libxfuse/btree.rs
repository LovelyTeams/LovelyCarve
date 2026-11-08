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
    cell::RefCell,
    collections::{btree_map::Entry, BTreeMap},
    io::{prelude::*, SeekFrom},
    marker::PhantomData,
};

use bincode_next::{
    de::{read::Reader, Decoder},
    error::DecodeError,
    Decode,
};
use num_traits::{PrimInt, Unsigned};

use super::{
    bmbt_rec::Bmx,
    compat::{EDESTADDRREQ, ENXIO, SEEK_HOLE},
    definitions::{XfsFileoff, XfsFsblock, XFS_BMAP_CRC_MAGIC, XFS_BMAP_MAGIC},
    utils::{decode, decode_from, Uuid},
    volume::SUPERBLOCK,
};

#[derive(Clone, Copy, Debug)]
pub struct BtreeBlockHdr<T: PrimInt + Unsigned> {
    bb_magic: u32,
    pub bb_level: u16,
    pub bb_numrecs: u16,
    //_bb_leftsib: T,
    //_bb_rightsib: T,
    _phantom: PhantomData<T>,
    // Below fields are for V5 file systems only
    //_bb_blkno: u64,
    //_bb_lsn: u64,
    //_bb_uuid: Uuid,
    //_bb_owner: u64,
    //_bb_crc: u32,
    //_bb_pad: u32,
}

impl<T: Decode<Ctx> + PrimInt + Unsigned, Ctx> Decode<Ctx> for BtreeBlockHdr<T> {
    fn decode<D: Decoder<Context = Ctx>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let bb_magic: u32 = Decode::decode(decoder)?;
        let bb_level = Decode::decode(decoder)?;
        let bb_numrecs = Decode::decode(decoder)?;
        let _bb_leftsib: T = Decode::decode(decoder)?;
        let _bb_rightsib: T = Decode::decode(decoder)?;
        match bb_magic {
            XFS_BMAP_MAGIC => {}
            XFS_BMAP_CRC_MAGIC => {
                let _bb_blkno: u64 = Decode::decode(decoder)?;
                let _bb_lsn: u64 = Decode::decode(decoder)?;
                let bb_uuid: Uuid = Decode::decode(decoder)?;
                let super_block = SUPERBLOCK.get().unwrap();
                assert_eq!(bb_uuid, super_block.metadata_uuid);
                let _bb_owner: u64 = Decode::decode(decoder)?;
                let _bb_crc: u32 = Decode::decode(decoder)?;
                let _bb_pad: u32 = Decode::decode(decoder)?;
            }
            _ => panic!("Unexpected magic value {bb_magic:#x}"),
        };
        Ok(BtreeBlockHdr {
            bb_magic,
            bb_level,
            bb_numrecs,
            _phantom: PhantomData,
        })
    }
}

#[derive(Debug, Clone, Decode)]
pub struct BmdrBlock {
    pub bb_level: u16,
    pub bb_numrecs: u16,
}

impl BmdrBlock {
    pub const SIZE: usize = 4;
}

#[derive(Debug, Clone, Decode)]
pub struct BmbtKey {
    pub br_startoff: XfsFileoff,
}

impl BmbtKey {
    pub const SIZE: usize = 8;
}

pub type XfsBmbtPtr = XfsFsblock;
pub type XfsBmdrPtr = XfsFsblock;
pub type XfsBmbtLblock = BtreeBlockHdr<u64>;

trait BtreePriv {
    fn keys(&self) -> &[BmbtKey];
    fn level(&self) -> u16;
    fn block_cache(&self) -> &RefCell<BlockCache>;
    fn ptrs(&self) -> &[XfsBmbtPtr];
}

/// Methods that are common to both BtreeRoot and BtreeIntermediate
#[allow(private_bounds)]
pub trait Btree: BtreePriv {
    /// Return the extent, if any, that contains the given block within the file.
    /// Return its starting position as an FSblock, and its length in file system block units.
    /// If a hole's length extents to EoF, return None for length.
    fn map_block<R: bincode_next::de::read::Reader + BufRead + Seek>(
        &self,
        buf_reader: &mut R,
        logical_block: XfsFileoff,
    ) -> Result<(Option<XfsFsblock>, Option<u64>), i32> {
        let super_block = SUPERBLOCK.get().unwrap();
        let pp = self
            .keys()
            .partition_point(|k| k.br_startoff <= logical_block);
        // If there's a hole at the start, we should still descend into the leftmost child.
        // BtreeLeaf::get_extent will calculate the hole's size.
        let idx = pp.saturating_sub(1);

        let mut guard = self.block_cache().borrow_mut();
        match &mut *guard {
            BlockCache::Intermediate(bci) => {
                assert!(self.level() > 1);

                let entry = bci.entry(idx);
                match entry {
                    Entry::Vacant(ve) => {
                        let offset = super_block.fsb_to_offset(self.ptrs()[idx]);
                        buf_reader
                            .seek(SeekFrom::Start(offset))
                            .map_err(|e| e.raw_os_error().unwrap())?;
                        let bti: BtreeIntermediate =
                            decode_from(buf_reader.by_ref()).map_err(|_| EDESTADDRREQ)?;
                        ve.insert(bti).map_block(buf_reader, logical_block)
                    }
                    Entry::Occupied(oe) => {
                        let v: &BtreeIntermediate = oe.get();
                        v.map_block(buf_reader, logical_block)
                    }
                }
            }
            BlockCache::Leaf(bcl) => {
                assert!(self.level() <= 1);

                let entry = bcl.entry(idx);
                match entry {
                    Entry::Vacant(ve) => {
                        let offset = super_block.fsb_to_offset(self.ptrs()[idx]);
                        buf_reader
                            .seek(SeekFrom::Start(offset))
                            .map_err(|e| e.raw_os_error().unwrap())?;
                        let btl: BtreeLeaf =
                            decode_from(buf_reader.by_ref()).map_err(|_| EDESTADDRREQ)?;
                        Ok(ve.insert(btl).get_extent(logical_block))
                    }
                    Entry::Occupied(oe) => {
                        let v: &BtreeLeaf = oe.get();
                        Ok(v.get_extent(logical_block))
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
enum BlockCache {
    Intermediate(BTreeMap<usize, BtreeIntermediate>),
    Leaf(BTreeMap<usize, BtreeLeaf>),
}

impl BlockCache {
    fn new(level: u16) -> Self {
        if level > 1 {
            BlockCache::Intermediate(Default::default())
        } else {
            BlockCache::Leaf(Default::default())
        }
    }
}

/// A root BTree in an extent list.
///
/// This is actually part of the inode, not a separate disk block.  Note that root and intermediate
/// nodes are stored differently on disk: root btrees are stored in the inode, whereas intermediate
/// btrees are stored in a BMA3 block.
#[derive(Debug)]
pub struct BtreeRoot {
    pub bmdr: BmdrBlock,
    pub keys: Vec<BmbtKey>,
    pub ptrs: Vec<XfsBmdrPtr>,
    /// A cache of the object's extents, indexed by block number
    blocks: RefCell<BlockCache>,
}

impl BtreeRoot {
    pub fn lseek<R>(&self, buf_reader: &mut R, offset: u64, whence: i32) -> Result<u64, i32>
    where
        R: BufRead + Reader + Seek,
    {
        let sb = SUPERBLOCK.get().unwrap();

        let mut dblock = offset >> sb.sb_blocklog;
        match self.map_block(buf_reader.by_ref(), dblock)? {
            (None, Some(len)) => {
                // A hole, followed by data
                if whence == SEEK_HOLE {
                    Ok(offset)
                } else {
                    // It should be impossible to have two hole extents in a row.  But
                    // double-check.
                    debug_assert!(self
                        .map_block(buf_reader.by_ref(), dblock + len)
                        .unwrap()
                        .0
                        .is_some());
                    Ok(offset + (len << sb.sb_blocklog))
                }
            }
            (Some(_), None) => {
                unreachable!(
                    "Btree::map_block should never return None for the length of a data region"
                );
            }
            (Some(_), Some(len)) => {
                // In a data region
                if whence == SEEK_HOLE {
                    // Scan for the next hole
                    dblock += len;
                    loop {
                        match self.map_block(buf_reader.by_ref(), dblock)? {
                            (Some(_fsblock), Some(len)) => {
                                dblock += len;
                            }
                            (Some(_fsblock), None) => {
                                unreachable!(
                                    "Btree::map_block should never return None for the length of \
                                     a data region"
                                );
                            }
                            (None, _) => {
                                return Ok(dblock << sb.sb_blocklog);
                            }
                        }
                    }
                } else {
                    Ok(offset)
                }
            }
            (None, None) => {
                // A hole that extends to EOF
                if whence == SEEK_HOLE {
                    Ok(offset)
                } else {
                    Err(ENXIO)
                }
            }
        }
    }

    pub fn new(bmdr: BmdrBlock, keys: Vec<BmbtKey>, ptrs: Vec<XfsBmdrPtr>) -> Self {
        let blocks = RefCell::new(BlockCache::new(bmdr.bb_level));
        Self {
            bmdr,
            keys,
            ptrs,
            blocks,
        }
    }
}

impl BtreePriv for BtreeRoot {
    fn block_cache(&self) -> &RefCell<BlockCache> {
        &self.blocks
    }

    fn keys(&self) -> &[BmbtKey] {
        &self.keys
    }

    fn level(&self) -> u16 {
        self.bmdr.bb_level
    }

    fn ptrs(&self) -> &[XfsBmdrPtr] {
        &self.ptrs
    }
}

impl Btree for BtreeRoot {}

/// An intermediate Btree.
#[derive(Debug)]
struct BtreeIntermediate {
    hdr: XfsBmbtLblock,
    keys: Vec<BmbtKey>,
    ptrs: Vec<XfsBmbtPtr>,
    /// A cache of the object's extents, indexed by block number
    blocks: RefCell<BlockCache>,
}

impl BtreePriv for BtreeIntermediate {
    fn block_cache(&self) -> &RefCell<BlockCache> {
        &self.blocks
    }

    fn keys(&self) -> &[BmbtKey] {
        &self.keys
    }

    fn level(&self) -> u16 {
        self.hdr.bb_level
    }

    fn ptrs(&self) -> &[XfsBmbtPtr] {
        &self.ptrs
    }
}

impl Btree for BtreeIntermediate {}

impl<Ctx> Decode<Ctx> for BtreeIntermediate {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let blocksize = SUPERBLOCK.get().unwrap().sb_blocksize as usize;
        let mut raw = vec![0u8; blocksize];
        decoder.reader().read(&mut raw)?;
        let (hdr, mut ofs) = decode::<XfsBmbtLblock>(&raw)?;
        assert!(hdr.bb_level > 0);

        let mut keys = Vec::with_capacity(usize::from(hdr.bb_numrecs));
        for _ in 0..hdr.bb_numrecs {
            let (key, keylen) = decode(&raw[ofs..])?;
            ofs += keylen;
            keys.push(key);
        }

        // The XFS Algorithms & Data Structures document section
        // 16.2 says that the pointers start at offset 0x808 within the block.  But for V5 file
        // systems it looks to me like they really start at offset 0x820.
        ofs = match hdr.bb_magic {
            XFS_BMAP_MAGIC => blocksize / 2 + 0x08,
            XFS_BMAP_CRC_MAGIC => blocksize / 2 + 0x20,
            _ => unreachable!(),
        };
        let mut ptrs = Vec::with_capacity(usize::from(hdr.bb_numrecs));
        for _ in 0..hdr.bb_numrecs {
            let (ptr, ptrlen) = decode(&raw[ofs..])?;
            ofs += ptrlen;
            ptrs.push(ptr);
        }

        let blocks = RefCell::new(BlockCache::new(hdr.bb_level));
        Ok(Self {
            hdr,
            keys,
            ptrs,
            blocks,
        })
    }
}

/// A Leaf Btree.
#[derive(Debug)]
struct BtreeLeaf {
    // hdr: XfsBmbtLblock,
    bmx: Bmx,
}

impl BtreeLeaf {
    /// Return the extent, if any, that contains the given block within the file.
    /// Return its starting position as an FSblock, and its length in file system block units.
    /// If a hole's length extends to EoF, return None for length.
    pub fn get_extent(&self, dblock: XfsFileoff) -> (Option<XfsFsblock>, Option<u64>) {
        self.bmx.get_extent(dblock)
    }
}

impl<Ctx> Decode<Ctx> for BtreeLeaf {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let hdr: XfsBmbtLblock = Decode::decode(decoder)?;
        assert_eq!(hdr.bb_level, 0);

        let bmx = Bmx::from((0..hdr.bb_numrecs).map(|_| Decode::decode(decoder).unwrap()));

        Ok(Self { bmx })
    }
}
