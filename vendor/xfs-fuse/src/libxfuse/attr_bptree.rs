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
    convert::TryInto,
    io::{BufRead, Seek, SeekFrom},
};

use bincode_next::de::read::Reader;

use super::{
    attr::{Attr, AttrLeafblock},
    btree::{Btree, BtreeRoot},
    compat::{ENOATTR, ENOENT},
    da_btree::{hashname, XfsDa3Intnode},
    definitions::{
        XfsDablk, XfsFsblock, XFS_ATTR3_LEAF_MAGIC, XFS_ATTR_LEAF_MAGIC, XFS_DA3_NODE_MAGIC,
        XFS_DA_NODE_MAGIC,
    },
    sb::Sb,
    utils,
};

/// According to XFS Algorithms & Data Structures, a BTree attribute fork will always contain an
/// xfs_da_intnode_t or xfs_da3_intnode_t in its first attribute block.  However, sometimes it
/// contains a xfs_attr3_leafblock instead.  I think that can happen when a BTree-formatted
/// attribute fork shrinks enough that it no longer requires more than one extent to hold all
/// attributes.
#[derive(Debug)]
enum AttrBtreeBlock0 {
    Node(XfsDa3Intnode),
    Leaf,
}

impl AttrBtreeBlock0 {
    fn first_block<R, F>(&self, buf_reader: &mut R, super_block: &Sb, map_dblock: F) -> XfsDablk
    where
        R: BufRead + Reader + Seek,
        F: Fn(XfsDablk, &mut R) -> XfsFsblock,
    {
        match self {
            AttrBtreeBlock0::Node(node) => node.first_block(buf_reader, super_block, map_dblock),
            AttrBtreeBlock0::Leaf => 0,
        }
    }

    fn lookup<R: BufRead + Reader + Seek, F: Fn(XfsDablk, &mut R) -> XfsFsblock>(
        &self,
        buf_reader: &mut R,
        super_block: &Sb,
        hash: u32,
        map_dblock: F,
    ) -> Result<XfsDablk, i32> {
        match self {
            AttrBtreeBlock0::Node(node) => node.lookup(buf_reader, super_block, hash, map_dblock),
            AttrBtreeBlock0::Leaf => Ok(0),
        }
    }

    fn new<R: BufRead + Reader + Seek>(buf_reader: &mut R) -> Self {
        buf_reader.fill_buf().unwrap();
        let magic: u16 = utils::decode(&buf_reader.peek_read(10).unwrap()[8..])
            .unwrap()
            .0;
        match magic {
            XFS_DA_NODE_MAGIC | XFS_DA3_NODE_MAGIC => {
                AttrBtreeBlock0::Node(XfsDa3Intnode::from(buf_reader))
            }
            XFS_ATTR_LEAF_MAGIC | XFS_ATTR3_LEAF_MAGIC => AttrBtreeBlock0::Leaf,
            _ => panic!("Unexpected magic value {magic:#x}"),
        }
    }
}

#[derive(Debug)]
pub struct AttrBtree {
    btree: BtreeRoot,
    total_size: i64,
    node: AttrBtreeBlock0,
    /// A cache of leaf blocks, indexed by directory block number
    leaves: RefCell<BTreeMap<XfsDablk, AttrLeafblock>>,
}

impl AttrBtree {
    pub fn new<R>(buf_reader: &mut R, sb: &Sb, btree: BtreeRoot) -> Self
    where
        R: bincode_next::de::read::Reader + BufRead + Seek,
    {
        let fsblk = btree.map_block(buf_reader.by_ref(), 0).unwrap().0.unwrap();
        buf_reader
            .seek(SeekFrom::Start(sb.fsb_to_offset(fsblk)))
            .unwrap();

        buf_reader.fill_buf().unwrap();
        let node = AttrBtreeBlock0::new(buf_reader.by_ref());

        Self {
            btree,
            total_size: -1,
            node,
            leaves: Default::default(),
        }
    }

    // Attribute blocks always have the same size, so we don't need to return the extent length.
    // They also need to return a different errno.
    fn map_dblock<R: bincode_next::de::read::Reader + BufRead + Seek>(
        &self,
        buf_reader: &mut R,
        logical_block: XfsDablk,
    ) -> Result<XfsFsblock, i32> {
        self.btree
            .map_block(buf_reader, logical_block.into())?
            .0
            .ok_or(ENOATTR)
    }

    /// Read the AttrLeafblock located at the given directory block number
    fn read_leaf<'a, R>(
        &'a self,
        buf_reader: &mut R,
        sb: &Sb,
        dblock: XfsDablk,
    ) -> Result<impl std::ops::DerefMut<Target = AttrLeafblock> + 'a, i32>
    where
        R: Reader + BufRead + Seek,
    {
        let mut cache_guard = self.leaves.borrow_mut();
        let entry = cache_guard.entry(dblock);
        if matches!(entry, Entry::Vacant(_)) {
            let fsblock = self.map_dblock(buf_reader.by_ref(), dblock)?;
            let leaf_offset = sb.fsb_to_offset(fsblock);
            buf_reader.seek(SeekFrom::Start(leaf_offset)).unwrap();
            let leaf: AttrLeafblock = utils::decode_from(buf_reader.by_ref()).unwrap();
            entry.or_insert(leaf);
        }
        Ok(std::cell::RefMut::map(cache_guard, |v| {
            v.get_mut(&dblock).unwrap()
        }))
    }
}

impl Attr for AttrBtree {
    fn get_total_size<R: Reader + BufRead + Seek>(
        &mut self,
        buf_reader: &mut R,
        super_block: &Sb,
    ) -> u32 {
        if self.total_size == -1 {
            let mut total_size: u32 = 0;

            // Now read the first leaf block of the btree
            let mut dablk =
                self.node
                    .first_block(buf_reader.by_ref(), super_block, |block, reader| {
                        self.map_dblock(reader.by_ref(), block).unwrap()
                    });
            loop {
                let leaf = self
                    .read_leaf(buf_reader.by_ref(), super_block, dablk)
                    .unwrap();
                total_size += leaf.get_total_size();
                dablk = leaf.hdr.forw;
                if dablk == 0 {
                    break;
                }
            }

            self.total_size = i64::from(total_size);
        }

        self.total_size.try_into().unwrap()
    }

    fn list<R: Reader + BufRead + Seek>(
        &mut self,
        buf_reader: &mut R,
        super_block: &Sb,
    ) -> Vec<u8> {
        let mut list: Vec<u8> =
            Vec::with_capacity(self.get_total_size(buf_reader.by_ref(), super_block) as usize);

        let mut dablk = self
            .node
            .first_block(buf_reader.by_ref(), super_block, |block, reader| {
                self.map_dblock(reader.by_ref(), block).unwrap()
            });
        loop {
            let leaf = self
                .read_leaf(buf_reader.by_ref(), super_block, dablk)
                .unwrap();
            (*leaf).list(&mut list);
            dablk = leaf.hdr.forw;
            if dablk == 0 {
                break;
            }
        }

        list
    }

    fn get<R>(&mut self, buf_reader: &mut R, super_block: &Sb, name: &[u8]) -> Result<Vec<u8>, i32>
    where
        R: Reader + BufRead + Seek,
    {
        let hash = hashname(name);

        let dablk = self
            .node
            .lookup(buf_reader.by_ref(), super_block, hash, |block, reader| {
                self.map_dblock(reader.by_ref(), block).unwrap()
            })
            .map_err(|e| if e == ENOENT { ENOATTR } else { e })?;
        let mut leaf = self.read_leaf(buf_reader.by_ref(), super_block, dablk)?;

        leaf.get(buf_reader.by_ref(), hash, |block, reader| {
            self.map_dblock(reader.by_ref(), block).unwrap()
        })
        .map(Vec::from)
    }
}
