/*
 * BSD 2-Clause License
 *
 * Copyright (c) 2024, Axcient
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
    ops::{Deref, Range},
};

use bincode_next::{
    de::{read::Reader, Decoder},
    error::DecodeError,
    Decode,
};
use tracing::error;

use super::{
    bmbt_rec::Bmx,
    btree::{BmbtKey, BmdrBlock, Btree, BtreeRoot, XfsBmbtPtr},
    compat::{FileType, EIO, ENOENT, ENXIO, SEEK_DATA},
    da_btree::{hashname, XfsDa3Blkinfo, XfsDa3Intnode, XfsDaBlkinfo},
    definitions::*,
    dir3::{Dir2DataEntry, Dir2DataHdr, Dir2DataUnused, Dir3, Dir3DataHdr, XfsDir2Dataptr},
    sb::Sb,
    utils::{decode, get_file_type, FileKind},
    volume::SUPERBLOCK,
};

/// All of the different ways that a directory can store its data fork.
// TODO: combine this code with file_extent_list and file_btree
#[derive(Debug)]
enum Dfork {
    /// Extent list.  Used for Leaf and Node directories.
    Bmx(Bmx),
    /// Btree.  Used only for BTree directories.
    Btree(BtreeRoot),
}

impl Dfork {
    fn lseek<R>(&self, buf_reader: &mut R, offset: u64, whence: i32) -> Result<u64, i32>
    where
        R: BufRead + Reader + Seek,
    {
        match self {
            Dfork::Bmx(bmx) => bmx.lseek(offset, whence),
            Dfork::Btree(btree_root) => btree_root.lseek(buf_reader, offset, whence),
        }
    }

    fn map_dblock<R: Reader + BufRead + Seek>(
        &self,
        buf_reader: &mut R,
        dblock: XfsDablk,
    ) -> Result<XfsFsblock, i32> {
        match self {
            Dfork::Bmx(bmx) => bmx.map_dblock(dblock).ok_or(ENOENT),
            Dfork::Btree(root) => root.map_block(buf_reader, dblock.into())?.0.ok_or(ENOENT),
        }
    }
}

#[derive(Debug, Decode)]
struct Dir2LeafHdr {
    info: XfsDaBlkinfo,
    pub count: u16,
    _stale: u16,
}

#[derive(Debug, Decode)]
struct Dir3LeafHdr {
    pub info: XfsDa3Blkinfo,
    pub count: u16,
    _stale: u16,
    _pad: u32,
}

#[derive(Clone, Copy, Debug, Decode, Default)]
struct Dir2LeafEntry {
    pub hashval: XfsDahash,
    pub address: XfsDir2Dataptr,
}

#[derive(Debug)]
struct Dir2LeafNDisk {
    forw: u32,
    pub ents: Vec<Dir2LeafEntry>,
}

impl Dir2LeafNDisk {
    /// Return the range of entry indices that include the given hash
    pub fn get_address_range(&self, hash: XfsDahash) -> Range<usize> {
        let l = self.ents.len();
        let i = self.ents.partition_point(|ent| ent.hashval < hash);
        let j = (i..l).find(|x| self.ents[*x].hashval > hash).unwrap_or(l);
        i..j
    }
}

impl<Context> Decode<Context> for Dir2LeafNDisk {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let magic: u16 = decode(&decoder.reader().peek_read(10).unwrap()[8..])?.0;
        let (count, forw) = match magic {
            XFS_DIR2_LEAF1_MAGIC | XFS_DIR2_LEAFN_MAGIC => {
                let hdr: Dir2LeafHdr = Decode::decode(decoder)?;
                (hdr.count, hdr.info.forw)
            }
            XFS_DIR3_LEAF1_MAGIC | XFS_DIR3_LEAFN_MAGIC => {
                let hdr: Dir3LeafHdr = Decode::decode(decoder)?;
                (hdr.count, hdr.info.forw)
            }
            _ => panic!("Unexpected magic {magic:#x}"),
        };
        let mut ents = Vec::<Dir2LeafEntry>::new();
        for _i in 0..count {
            let leaf_entry: Dir2LeafEntry = Decode::decode(decoder)?;
            ents.push(leaf_entry);
        }

        Ok(Dir2LeafNDisk { forw, ents })
    }
}

/// Stores the "leaf" info (the hash => address map) for Leaf, Node, and Btree directories.  But
/// does not store the freeindex info.
#[derive(Debug)]
enum Leaf {
    LeafN(Dir2LeafNDisk),
    /// A Btree.  Used for BTree directories and Node directories with > 1 Leaf blocks
    Btree(XfsDa3Intnode),
}

impl Leaf {
    fn open(raw: &[u8]) -> Self {
        let magic: u16 = decode(&raw[8..]).unwrap().0;
        let config = bincode_next::config::standard()
            .with_big_endian()
            .with_fixed_int_encoding();
        let reader = bincode_next::de::read::SliceReader::new(raw);
        let mut decoder = bincode_next::de::DecoderImpl::new(reader, config, ());
        match magic {
            XFS_DA_NODE_MAGIC | XFS_DA3_NODE_MAGIC => {
                let leaf_btree = XfsDa3Intnode::decode(&mut decoder)
                    .map_err(|_| EIO)
                    .unwrap();
                assert!(
                    leaf_btree.magic == XFS_DA3_NODE_MAGIC || leaf_btree.magic == XFS_DA_NODE_MAGIC
                );
                Self::Btree(leaf_btree)
            }
            XFS_DIR2_LEAFN_MAGIC | XFS_DIR3_LEAFN_MAGIC => {
                Self::LeafN(Dir2LeafNDisk::decode(&mut decoder).unwrap())
            }
            XFS_DIR2_LEAF1_MAGIC | XFS_DIR3_LEAF1_MAGIC => {
                Self::LeafN(Dir2LeafNDisk::decode(&mut decoder).unwrap())
            }
            magic => panic!("Bad magic in Leaf block! {magic:#x}"),
        }
    }

    fn lookup_leaf_blk<R>(
        self,
        buf_reader: &mut R,
        sb: &Sb,
        dir: &Dir2Lf,
        hash: u32,
    ) -> Result<Dir2LeafNDisk, i32>
    where
        R: BufRead + Reader + Seek,
    {
        match self {
            Leaf::LeafN(leafn) => Ok(leafn),
            Leaf::Btree(btree) => {
                let dablk: XfsDablk =
                    btree.lookup(buf_reader.by_ref(), sb, hash, |block, br| {
                        dir.dfork.map_dblock(br, block).unwrap()
                    })?;
                let raw = dir.read_dblock(buf_reader.by_ref(), sb, dablk)?;
                Ok(decode(&raw).unwrap().0)
            }
        }
    }
}

/// Iterates through all dirents with a given hash, for NodeLike directories
#[derive(Debug)]
struct NodeLikeAddressIterator<'a, R: Reader + BufRead + Seek + 'a> {
    dir: &'a Dir2Lf,
    hash: XfsDahash,
    leaf: Dir2LeafNDisk,
    leaf_range: Range<usize>,
    brrc: &'a RefCell<&'a mut R>,
}

impl<'a, R: Reader + BufRead + Seek + 'a> NodeLikeAddressIterator<'a, R> {
    pub fn new(
        dir: &'a Dir2Lf,
        brrc: &'a RefCell<&'a mut R>,
        hash: XfsDahash,
    ) -> Result<Self, i32> {
        let sb = SUPERBLOCK.get().unwrap();
        let dblock = sb.get_dir3_leaf_offset();
        let mut buf_reader = brrc.borrow_mut();
        let leaf_btree = {
            let raw = dir.read_dblock(buf_reader.by_ref(), &sb, dblock)?;
            Leaf::open(raw.deref())
        };
        let leaf = leaf_btree.lookup_leaf_blk(buf_reader.by_ref(), &sb, dir, hash)?;

        let leaf_range = leaf.get_address_range(hash);

        Ok(Self {
            dir,
            hash,
            leaf,
            leaf_range,
            brrc,
        })
    }
}

impl<'a, R: Reader + BufRead + Seek + 'a> Iterator for NodeLikeAddressIterator<'a, R> {
    type Item = XfsDir2Dataptr;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.leaf_range.is_empty() {
                if self
                    .leaf
                    .ents
                    .last()
                    .map(|e| e.hashval == self.hash)
                    .unwrap_or(false)
                {
                    // There was a probably hash collision in the directory.  This happens
                    // frequently, since the hash is only 32 bits.  Tragically, the colliding
                    // entries were located in different leaf blocks.
                    // Traverse the forw pointer
                    let forw = self.leaf.forw;
                    let mut buf_reader = self.brrc.borrow_mut();
                    let sb = SUPERBLOCK.get().unwrap();
                    let raw = match self.dir.read_dblock(buf_reader.by_ref(), &sb, forw) {
                        Ok(raw) => raw,
                        Err(e) => {
                            // It would be nice to print inode number here
                            error!("Cannot read dblock {}: {}", forw, e);
                            return None;
                        }
                    };
                    self.leaf = decode(raw.deref()).unwrap().0;
                    self.leaf_range = self.leaf.get_address_range(self.hash);
                } else {
                    return None;
                }
            } else {
                let i = self.leaf_range.start;
                self.leaf_range.start += 1;
                let ent = self.leaf.ents[i];
                debug_assert_eq!(ent.hashval, self.hash);
                return Some(ent.address << 3);
            }
        }
    }
}

/// "Long form" directories.  This structure represents every directory type that isn't short form
/// or Block.  As described XFS Algorithms and Data Structures, that includes "Leaf", "Node", and
/// "BTree" directories. All of these directory types store their data on disk in the same format,
/// but differ in their metadata storage.
#[derive(Debug)]
pub struct Dir2Lf {
    /// Maps directory block numbers to FS block numbers for this directory
    dfork: Dfork,

    /// A cache of directory blocks, indexed by directory block number
    blocks: RefCell<BTreeMap<XfsDablk, Vec<u8>>>,
}

impl Dir2Lf {
    pub fn from_bmx(bmx: Bmx) -> Self {
        let dfork = Dfork::Bmx(bmx);
        let blocks = Default::default();
        Dir2Lf { dfork, blocks }
    }

    pub fn from_btree(bmbt: BmdrBlock, keys: Vec<BmbtKey>, pointers: Vec<XfsBmbtPtr>) -> Self {
        let root = BtreeRoot::new(bmbt, keys, pointers);
        let dfork = Dfork::Btree(root);
        let blocks = Default::default();
        Dir2Lf { dfork, blocks }
    }

    fn get_addresses<'a, R>(
        &'a self,
        buf_reader: &'a RefCell<&'a mut R>,
        hash: XfsDahash,
    ) -> Box<dyn Iterator<Item = XfsDir2Dataptr> + 'a>
    where
        R: Reader + BufRead + Seek + 'a,
    {
        if let Ok(ai) = NodeLikeAddressIterator::new(self, buf_reader, hash) {
            Box::new(ai)
        } else {
            Box::new(std::iter::empty())
        }
    }

    fn read_dblock<'a, R>(
        &'a self,
        mut buf_reader: R,
        sb: &Sb,
        dblock: XfsDablk,
    ) -> Result<impl Deref<Target = [u8]> + 'a, i32>
    where
        R: Reader + BufRead + Seek,
    {
        let mut cache_guard = self.blocks.borrow_mut();
        let entry = cache_guard.entry(dblock);
        if matches!(entry, Entry::Vacant(_)) {
            let fsblock = self.dfork.map_dblock(buf_reader.by_ref(), dblock)?;
            let buf = self.read_fsblock(buf_reader.by_ref(), sb, fsblock)?;
            entry.or_insert(buf);
        }
        // Annoyingly, there's no function to downgrade a RefMut into a Ref.
        drop(cache_guard);
        let cache_guard = self.blocks.borrow();
        Ok(Ref::map(cache_guard, |v| &v[&dblock][..]))
    }

    // NB: this code could be combined with File::read_sectors.  However, the latter must contend
    // with much larger extents, and with reads of partial sectors.
    fn read_fsblock<R>(
        &self,
        mut buf_reader: R,
        sb: &Sb,
        fsblock: XfsFsblock,
    ) -> Result<Vec<u8>, i32>
    where
        R: Reader + BufRead + Seek,
    {
        let dblksize: usize = 1 << (sb.sb_blocklog + sb.sb_dirblklog);

        let mut buf = vec![0; dblksize];
        buf_reader
            .seek(SeekFrom::Start(sb.fsb_to_offset(fsblock)))
            .unwrap();
        buf_reader.read_exact(&mut buf).unwrap();
        Ok(buf)
    }
}

impl Dir3 for Dir2Lf {
    fn lookup<R: Reader + BufRead + Seek>(
        &self,
        buf_reader: &mut R,
        sb: &Sb,
        name: &[u8],
    ) -> Result<u64, i32> {
        let hash = hashname(name);

        let brrc = RefCell::new(buf_reader);
        for address in self.get_addresses(&brrc, hash) {
            let blk_offset =
                (address & ((1u32 << (sb.sb_dirblklog + sb.sb_blocklog)) - 1)) as usize;
            let dblock = (address >> sb.sb_blocklog) & !((1u32 << sb.sb_dirblklog) - 1);
            let mut guard = brrc.borrow_mut();
            let raw = self.read_dblock(guard.by_ref(), sb, dblock)?;
            let entry: Dir2DataEntry = decode(&raw[blk_offset..]).unwrap().0;
            if entry.name.as_slice() == name {
                return Ok(entry.inumber);
            }
        }
        Err(ENOENT)
    }

    fn next<R: Reader + BufRead + Seek>(
        &self,
        buf_reader: &mut R,
        sb: &Sb,
        offset: i64,
    ) -> Result<(XfsIno, i64, Option<FileType>, Vec<u8>), i32> {
        let dblksize: u64 = 1 << (sb.sb_blocklog + sb.sb_dirblklog);
        let dblkmask: u64 = dblksize - 1;
        let mut offset: u64 = offset.try_into().unwrap();
        let mut next = offset == 0;

        loop {
            // Skip any holes in the directory
            let newoffset = self
                .dfork
                .lseek(buf_reader.by_ref(), offset, SEEK_DATA)
                .map_err(|e| if e == ENXIO { ENOENT } else { e })?;
            if newoffset >= u64::from(sb.get_dir3_leaf_offset()) << sb.sb_blocklog {
                return Err(ENOENT);
            }
            offset = newoffset;

            // Byte offset within this directory block
            let dir_block_offset = offset & ((1 << (sb.sb_dirblklog + sb.sb_blocklog)) - 1);
            // Offset of this directory block within the directory
            let doffset = offset - dir_block_offset;

            let dblock = ((offset >> sb.sb_blocklog) & !((1u64 << sb.sb_dirblklog) - 1))
                .try_into()
                .unwrap();
            let raw = self.read_dblock(buf_reader.by_ref(), sb, dblock)?;

            let mut blk_offset = if offset & dblkmask > 0 {
                (offset & dblkmask) as usize
            } else {
                let magic: u32 = decode(&raw[..]).unwrap().0;
                match magic {
                    XFS_DIR2_BLOCK_MAGIC | XFS_DIR2_DATA_MAGIC => Dir2DataHdr::SIZE as usize,
                    XFS_DIR3_BLOCK_MAGIC | XFS_DIR3_DATA_MAGIC => Dir3DataHdr::SIZE as usize,
                    _ => panic!("Unknown magic number for block directory {magic:#x}"),
                }
            };
            while blk_offset < raw.len() {
                let freetag: u16 = decode(&raw[blk_offset..]).unwrap().0;
                if freetag == 0xffff {
                    let (_, length) = decode::<Dir2DataUnused>(&raw[blk_offset..]).unwrap();
                    offset += length as u64;
                    blk_offset += length;
                } else if !next {
                    let length = Dir2DataEntry::get_length(sb, &raw[blk_offset..]);
                    blk_offset += length as usize;
                    offset += length as u64;
                    next = true;
                } else {
                    let (entry, _l) = decode::<Dir2DataEntry>(&raw[blk_offset..]).unwrap();
                    let kind = match entry.ftype {
                        Some(ftype) => Some(get_file_type(FileKind::Type(ftype))?),
                        None => None,
                    };
                    let name = entry.name;
                    let entry_offset = doffset + entry.tag as u64;
                    return Ok((entry.inumber, entry_offset as i64, kind, name));
                }
            }
        }
    }
}
