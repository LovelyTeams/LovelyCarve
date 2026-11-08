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
    Decode,
};

use super::{
    attr::Attributes,
    attr_bptree::AttrBtree,
    attr_shortform::AttrShortform,
    bmbt_rec::{BmbtRec, Bmx},
    btree::{BmbtKey, BmdrBlock, BtreeRoot, XfsBmbtPtr},
    compat::{EIO, ENOENT, S_IFBLK, S_IFCHR, S_IFDIR, S_IFIFO, S_IFLNK, S_IFMT, S_IFREG, S_IFSOCK},
    definitions::*,
    dinode_core::{DinodeCore, XfsDinodeFmt},
    dir3::Directory,
    dir3_block::Dir2Block,
    dir3_lf::Dir2Lf,
    dir3_sf::Dir2Sf,
    file::File,
    file_btree::FileBtree,
    file_extent_list::FileExtentList,
    sb::Sb,
    symlink_extent::SymlinkExtents,
};

#[derive(Debug)]
pub enum DiU {
    Blk,
    Bmbt((BmdrBlock, Vec<BmbtKey>, Vec<XfsBmbtPtr>)),
    Bmx(Vec<BmbtRec>),
    Chr,
    Dir2Sf(Dir2Sf),
    LegacyLocal(Vec<u8>),
    Fifo,
    Socket,
    Symlink(Vec<u8>),
}

#[derive(Debug)]
pub enum DiA {
    Attrsf(AttrShortform),
    Abmx(Vec<BmbtRec>),
    Abmbt((BmdrBlock, Vec<BmbtKey>, Vec<XfsBmbtPtr>)),
}

#[derive(Debug)]
pub struct Dinode {
    pub di_core: DinodeCore,
    pub di_u: DiU,
    pub di_a: Option<DiA>,
    /// Cache of this inode's directory object, if any.
    directory: Option<Directory>,
    /// Cache of this inode's attribute object, if any
    attributes: Option<Attributes>,
}

impl Dinode {
    pub fn from<R: bincode_next::de::read::Reader + BufRead + Seek>(
        buf_reader: &mut R,
        superblock: &Sb,
        inode_number: XfsIno,
    ) -> Result<Dinode, i32> {
        let off = superblock.inode_offset(inode_number)?;

        buf_reader.seek(SeekFrom::Start(off)).map_err(|_| EIO)?;
        let mut raw = vec![0u8; superblock.inode_size()];
        buf_reader.read_exact(&mut raw).map_err(|_| EIO)?;
        let config = bincode_next::config::standard()
            .with_big_endian()
            .with_fixed_int_encoding();
        if raw.get(4) == Some(&3) {
            super::checked::crc(&raw, 100)?;
            if super::checked::u64be(&raw, 152)? != inode_number
                || super::checked::bytes(&raw, 160, 16)? != superblock.metadata_uuid.as_bytes()
            {
                return Err(EIO);
            }
        }
        let reader = bincode_next::de::read::SliceReader::new(&raw[..]);
        let mut decoder = bincode_next::de::DecoderImpl::new(reader, config, ());

        let di_core = DinodeCore::decode(&mut decoder).map_err(|_| EIO)?;

        let literal_start = di_core.literal_area_offset();
        let data_end = if di_core.di_forkoff == 0 {
            raw.len()
        } else {
            literal_start + di_core.di_forkoff as usize * 8
        };
        if di_core.di_size < 0 || data_end > raw.len() || data_end < literal_start {
            return Err(EIO);
        }
        let reader = bincode_next::de::read::SliceReader::new(&raw[literal_start..data_end]);
        let mut decoder = bincode_next::de::DecoderImpl::new(reader, config, ());
        if matches!(di_core.di_format, XfsDinodeFmt::Extents)
            && di_core.nextents > ((data_end - literal_start) / 16) as u64
        {
            return Err(EIO);
        }
        let di_u: Option<DiU>;
        match (di_core.di_mode as u32) & S_IFMT {
            S_IFREG => match di_core.di_format {
                XfsDinodeFmt::Extents => {
                    let mut bmx = Vec::<BmbtRec>::new();
                    for _i in 0..di_core.nextents {
                        bmx.push(BmbtRec::decode(&mut decoder).map_err(|_| EIO)?)
                    }
                    di_u = Some(DiU::Bmx(bmx));
                }
                XfsDinodeFmt::Btree => {
                    di_u = Some(DiU::Bmbt(decode_btree_fork(&raw[literal_start..data_end])?));
                }
                _ => return Err(EIO),
            },
            S_IFDIR => match di_core.di_format {
                XfsDinodeFmt::Local => {
                    if !superblock.has_dirv2() {
                        let length = usize::try_from(di_core.di_size).map_err(|_| EIO)?;
                        di_u = Some(DiU::LegacyLocal(
                            super::checked::bytes(&raw, literal_start, length)?.to_vec(),
                        ));
                    } else {
                        let mut dir_sf = Dir2Sf::decode(&mut decoder).map_err(|_| EIO)?;
                        dir_sf.set_ino(inode_number);
                        di_u = Some(DiU::Dir2Sf(dir_sf));
                    }
                }
                XfsDinodeFmt::Extents => {
                    let mut bmx = Vec::<BmbtRec>::new();
                    for _i in 0..di_core.nextents {
                        bmx.push(BmbtRec::decode(&mut decoder).map_err(|_| EIO)?)
                    }
                    di_u = Some(DiU::Bmx(bmx));
                }
                XfsDinodeFmt::Btree => {
                    di_u = Some(DiU::Bmbt(decode_btree_fork(&raw[literal_start..data_end])?));
                }
                _ => return Err(EIO),
            },
            S_IFLNK => match di_core.di_format {
                XfsDinodeFmt::Local => {
                    if di_core.di_size > 1024 || di_core.di_size as usize > data_end - literal_start
                    {
                        return Err(EIO);
                    }
                    let mut data = vec![0u8; di_core.di_size as usize];
                    decoder.reader().read(&mut data[..]).map_err(|_| EIO)?;
                    di_u = Some(DiU::Symlink(data))
                }
                XfsDinodeFmt::Extents => {
                    let mut bmx = Vec::<BmbtRec>::new();
                    for _i in 0..di_core.nextents {
                        bmx.push(BmbtRec::decode(&mut decoder).map_err(|_| EIO)?);
                    }
                    di_u = Some(DiU::Bmx(bmx));
                }
                _ => return Err(EIO),
            },
            S_IFBLK => di_u = Some(DiU::Blk),
            S_IFCHR => di_u = Some(DiU::Chr),
            S_IFIFO => di_u = Some(DiU::Fifo),
            S_IFSOCK => di_u = Some(DiU::Socket),
            _ => return Err(ENOENT),
        }

        let di_a: Option<DiA>;
        if di_core.di_forkoff != 0 {
            let attr_fork_ofs = di_core.literal_area_offset() + di_core.di_forkoff as usize * 8;
            if attr_fork_ofs > raw.len() {
                return Err(EIO);
            }
            let config = bincode_next::config::standard()
                .with_big_endian()
                .with_fixed_int_encoding();
            let reader = bincode_next::de::read::SliceReader::new(&raw[attr_fork_ofs..]);
            let mut decoder = bincode_next::de::DecoderImpl::new(reader, config, ());

            match di_core.di_aformat {
                XfsDinodeFmt::Local => {
                    let attr_shortform = AttrShortform::decode(&mut decoder).map_err(|_| EIO)?;
                    di_a = Some(DiA::Attrsf(attr_shortform));
                }
                XfsDinodeFmt::Extents => {
                    let mut bmx = Vec::<BmbtRec>::new();
                    for _i in 0..di_core.anextents {
                        bmx.push(BmbtRec::decode(&mut decoder).map_err(|_| EIO)?);
                    }
                    di_a = Some(DiA::Abmx(bmx));
                }
                XfsDinodeFmt::Btree => {
                    di_a = Some(DiA::Abmbt(decode_btree_fork(&raw[attr_fork_ofs..])?));
                }
                _ => return Err(EIO),
            }
        } else {
            di_a = None;
        }

        Ok(Dinode {
            di_core,
            di_u: di_u.ok_or(EIO)?,
            di_a,
            directory: None,
            attributes: None,
        })
    }

    pub fn get_dir<R: bincode_next::de::read::Reader + BufRead + Seek>(
        &mut self,
        buf_reader: &mut R,
        sb: &Sb,
    ) -> &Directory {
        if self.directory.is_none() {
            let directory = match &self.di_u {
                DiU::Dir2Sf(dir) => Directory::Sf(dir.clone()),
                DiU::Bmx(bmbtv) => {
                    if bmbtv.len() == 1 {
                        Directory::Block(Dir2Block::new(
                            buf_reader.by_ref(),
                            sb,
                            bmbtv[0].br_startblock,
                        ))
                    } else {
                        let bmx = Bmx::new(bmbtv);
                        Directory::Lf(Dir2Lf::from_bmx(bmx))
                    }
                }
                DiU::Bmbt((bmbt, keys, pointers)) => Directory::Lf(Dir2Lf::from_btree(
                    bmbt.clone(),
                    keys.clone(),
                    pointers.clone(),
                )),
                _ => {
                    panic!("Unsupported dir format!");
                }
            };
            self.directory = Some(directory);
        }
        self.directory.as_ref().unwrap()
    }

    pub fn get_file<R: bincode_next::de::read::Reader + BufRead + Seek>(
        &self,
        _buf_reader: &mut R,
    ) -> Box<dyn File<R>> {
        match &self.di_u {
            DiU::Bmx(bmx) => Box::new(FileExtentList {
                bmx: Bmx::new(bmx),
                size: self.di_core.di_size,
            }),
            DiU::Bmbt((bmdr, keys, pointers)) => Box::new(FileBtree {
                btree: BtreeRoot::new(bmdr.clone(), keys.clone(), pointers.clone()),
                size: self.di_core.di_size,
            }),
            _ => {
                panic!("Unsupported file format!");
            }
        }
    }

    pub fn get_link_data<R>(&self, buf_reader: &mut R, superblock: &Sb) -> Vec<u8>
    where
        R: BufRead + Reader + Seek,
    {
        match &self.di_u {
            DiU::Symlink(data) => {
                let mut bytes = data.clone();
                if let Some(pos) = bytes.iter().position(|b| *b == 0) {
                    bytes.truncate(pos);
                }
                bytes
            }
            DiU::Bmx(bmbtv) => {
                SymlinkExtents::get_target(buf_reader.by_ref(), &Bmx::new(bmbtv), superblock)
            }
            _ => {
                panic!("Unsupported link format!");
            }
        }
    }

    pub fn get_attrs<R: Reader + BufRead + Seek>(
        &mut self,
        buf_reader: &mut R,
        superblock: &Sb,
    ) -> &mut Option<Attributes> {
        if self.attributes.is_none() {
            self.attributes = match &self.di_a {
                Some(DiA::Attrsf(attr)) => Some(Attributes::Sf(attr.clone())),
                Some(DiA::Abmx(bmbtv)) => {
                    if self.di_core.anextents > 0 {
                        Some(crate::libxfuse::attr::open(
                            buf_reader.by_ref(),
                            superblock,
                            Bmx::new(bmbtv),
                        ))
                    } else {
                        None
                    }
                }
                Some(DiA::Abmbt((bmdr, keys, pointers))) => {
                    let btree_root = BtreeRoot::new(bmdr.clone(), keys.clone(), pointers.clone());
                    Some(Attributes::Btree(AttrBtree::new(
                        buf_reader.by_ref(),
                        superblock,
                        btree_root,
                    )))
                }
                None => None,
            };
        }
        &mut self.attributes
    }
}

/// In-inode bmap arrays use the fork capacity, not the live record count or alignment guesses.
fn decode_btree_fork(raw: &[u8]) -> Result<(BmdrBlock, Vec<BmbtKey>, Vec<XfsBmbtPtr>), i32> {
    use super::checked::*;
    let header = BmdrBlock {
        bb_level: u16be(raw, 0)?,
        bb_numrecs: u16be(raw, 2)?,
    };
    let capacity = raw.len().checked_sub(4).ok_or(EIO)? / 16;
    if header.bb_level == 0
        || header.bb_level > 32
        || header.bb_numrecs == 0
        || header.bb_numrecs as usize > capacity
    {
        return Err(EIO);
    }
    let mut keys = Vec::new();
    let mut pointers = Vec::new();
    for index in 0..header.bb_numrecs as usize {
        keys.push(BmbtKey {
            br_startoff: u64be(raw, 4 + index * 8)?,
        });
        pointers.push(u64be(raw, 4 + capacity * 8 + index * 8)?);
    }
    if keys
        .windows(2)
        .any(|pair| pair[0].br_startoff >= pair[1].br_startoff)
    {
        return Err(EIO);
    }
    Ok((header, keys, pointers))
}
