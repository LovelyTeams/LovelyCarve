//! ZFS subsystem hierarchy
//! ```text
//! SPA
//!  |
//!  v
//! DMU--\
//!  | \  \
//!  |  v  \
//!  |  ZAP \
//!  |  / | |
//!  v v  v v
//!  DSL-->ZPL
//! ```

// `#![feature(backtrace)]` was needed against nightly; the vendored build no
// longer carries a `Backtrace` field on `ZfsError`, so the `Backtrace` import
// is gone too.
use std::fs::File;
use std::io::Error as IoError;
use std::io::ErrorKind as IoErrorKind;
use std::io::Result as IoResult;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use thiserror::Error;

use nom::number::complete as number;

pub mod dmu;
pub mod dsl;
pub mod spa;
pub mod zap;
pub mod zil;
pub mod zpl;

pub mod compression;
pub mod fletcher;

use crate::compression::decompress_lz4;
use crate::fletcher::{Fletcher2, Fletcher4};

use crate::dmu::{DNodePhys, ObjsetPhys};
use crate::dsl::{DatasetPhys, DirPhys};
use crate::spa::{BlockPtr, BlockPtrKind, ChecksumType, CompressionType, Label, DVA};
use crate::zap::{
    zap_hash, zap_hash_idx, zap_leaf_hash_numentries, ZapBlock, ZapLeafPhys, ZapResult, ZapStr,
    ZapString,
};
use crate::zpl::{DirEntry, DirEntryType, SAAttr, SAAttrPhys, SABuf, SAValue};

#[derive(Debug)]
pub struct DefaultDrive {
    file: File,
}

impl DefaultDrive {
    pub fn new<P: AsRef<Path>>(path: P) -> IoResult<Self> {
        let f = File::open(path)?;
        Ok(Self { file: f })
    }
    pub fn read(&self, address: u64, len: u64) -> IoResult<Vec<u8>> {
        let mut buf = vec![0; len as usize];
        (&self.file).seek(SeekFrom::Start(address))?;
        (&self.file).read_exact(&mut buf)?;
        Ok(buf)
    }
}

pub trait RawDevice {
    type Block: AsRef<[u8]>;
    /// Read the requested amount, given in bytes
    fn read_raw(&self, addr: u64, size: u64) -> IoResult<Self::Block>;
}

impl RawDevice for DefaultDrive {
    type Block = Vec<u8>;
    fn read_raw(&self, addr: u64, size: u64) -> IoResult<Self::Block> {
        self.read(addr, size)
    }
}

impl DefaultSPA for DefaultDrive {}
impl DefaultDMU for DefaultDrive {}
impl DefaultZAP for DefaultDrive {}
impl DefaultDSL for DefaultDrive {}
impl DefaultZPL for DefaultDrive {}

#[derive(Debug, Error)]
#[error("{source}")]
pub struct ZfsError {
    // The upstream nightly build also carried a `backtrace: Backtrace` field;
    // on stable thiserror 1.x it requires a `#[backtrace]` hookup the upstream
    // code does not provide. We drop it — our callers only need the message.
    #[from]
    pub source: ZfsErrorKind,
}

impl From<IoError> for ZfsError {
    fn from(e: IoError) -> Self {
        ZfsErrorKind::from(e).into()
    }
}

impl Into<IoError> for ZfsError {
    fn into(self) -> IoError {
        use ZfsErrorKind::*;
        let kind = match self.source {
            Checksum | Parse(_) | UnsupportedFeature | Invalid => IoErrorKind::InvalidData,
            NotFound => IoErrorKind::NotFound,
            Io(ref e) => e.kind(),
        };
        IoError::new(kind, self)
    }
}

impl<T> From<nom::Err<(T, nom::error::ErrorKind)>> for ZfsError {
    fn from(e: nom::Err<(T, nom::error::ErrorKind)>) -> Self {
        ZfsErrorKind::from(e).into()
    }
}

#[derive(Debug, Error)]
pub enum ZfsErrorKind {
    #[error("Checksum validation error")]
    Checksum,
    #[error("Data parse error {0:?}")]
    Parse(nom::error::ErrorKind),
    #[error("Invalid data error")]
    Invalid,
    #[error("Unsupported feature present")]
    UnsupportedFeature,
    #[error("Required data not found")]
    NotFound,
    #[error("IO Error {0}")]
    Io(IoError),
}

impl From<IoError> for ZfsErrorKind {
    fn from(e: IoError) -> Self {
        Self::Io(e)
    }
}

impl<T> From<nom::Err<(T, nom::error::ErrorKind)>> for ZfsErrorKind {
    fn from(e: nom::Err<(T, nom::error::ErrorKind)>) -> Self {
        Self::Parse(match e {
            nom::Err::Incomplete(_) => nom::error::ErrorKind::Complete,
            nom::Err::Error(e) => e.1,
            nom::Err::Failure(f) => f.1,
        })
    }
}

// TODO: better name
#[derive(Debug)]
pub enum RawOrNah<B> {
    Raw(B),
    Nah(Vec<u8>),
}

impl<B: AsRef<[u8]>> AsRef<[u8]> for RawOrNah<B> {
    fn as_ref(&self) -> &[u8] {
        match self {
            RawOrNah::Raw(b) => b.as_ref(),
            RawOrNah::Nah(b) => b.as_ref(),
        }
    }
}

pub trait Device {
    type Block: AsRef<[u8]>;
    /// Offset and length in units of 512bytes
    fn read_offset(&self, vdev: u32, offset: u64, length: u32) -> IoResult<Self::Block>;
    fn get_label(&self, label_num: u8) -> Result<Option<Label>, ZfsError>;
}

impl<T, B> Device for T
where
    T: RawDevice<Block = B>,
    B: AsRef<[u8]>,
{
    type Block = B;
    /// Length in units of 512bytes
    fn read_offset(&self, vdev: u32, offset: u64, length: u32) -> IoResult<Self::Block> {
        // offset is stored in units of 512, but read_raw takes units of bytes
        // this could overflow offset, but I don't have any 2^55 byte drives to test on
        if vdev != 0 {
            return Err(IoErrorKind::NotFound.into());
        }
        self.read_raw((offset * 512) + 0x400000, length as u64 * 512)
    }
    fn get_label(&self, label_num: u8) -> Result<Option<Label>, ZfsError> {
        if label_num < 2 {
            Ok(Some(
                Label::parse(
                    self.read_raw(label_num as u64 * 256 * 1024, 256 * 1024)?
                        .as_ref(),
                )?
                .1,
            ))
        } else {
            Err(ZfsErrorKind::UnsupportedFeature.into())
        }
    }
}

pub trait SPA {
    type Block: AsRef<[u8]>;
    fn get(&self, ptr: &BlockPtr) -> Result<RawOrNah<Self::Block>, ZfsError> {
        if ptr.encryption
            || !(ptr.compression_type == CompressionType::LZ4
                || ptr.compression_type == CompressionType::Off
                || ptr.compression_type == CompressionType::On)
        {
            return Err(ZfsErrorKind::UnsupportedFeature)?;
        }
        match &ptr.embedded {
            BlockPtrKind::Ptr(ptrdata) => {
                let r: Result<Vec<ZfsError>, RawOrNah<Self::Block>> = ptrdata
                    .addresses
                    .iter()
                    .filter(|dva| dva.asize != 0)
                    .map(|dva| {
                        let block = self.read(*dva, ptr.physical_size as u32 + 1)?;
                        let data_size = (ptr.physical_size as usize + 1) * 512;
                        let data = &block.as_ref()[..data_size];
                        // check checksums
                        match ptrdata.checksum_type {
                            ChecksumType::ZILog | ChecksumType::Fletcher2 => {
                                // fletcher2
                                let (_input, cksum) = Fletcher2::parse(data)?;
                                if ptrdata.checksum == cksum.into() {
                                    Ok(())
                                } else {
                                    Err(ZfsErrorKind::Checksum)
                                }
                            }
                            ChecksumType::On | ChecksumType::Fletcher4 => {
                                // fletcher4
                                let (_input, cksum) = Fletcher4::parse(data)?;
                                if ptrdata.checksum == cksum.clone().into() {
                                    Ok(())
                                } else {
                                    Err(ZfsErrorKind::Checksum)
                                }
                            }
                            ChecksumType::Label
                            | ChecksumType::GangHeader
                            | ChecksumType::SHA256 => {
                                // SHA256
                                Err(ZfsErrorKind::UnsupportedFeature)
                            }
                            ChecksumType::Off => Ok(()), // do nothing
                            _ => {
                                // unsupported
                                Err(ZfsErrorKind::UnsupportedFeature)
                            }
                        }?;
                        // decompress
                        Ok(match ptr.compression_type {
                            CompressionType::Off => block,
                            CompressionType::LZ4 | CompressionType::On => {
                                RawOrNah::Nah(decompress_lz4(data)?.1)
                            }
                            _ => Err(ZfsErrorKind::UnsupportedFeature)?,
                        })
                    })
                    .map(|r| match r {
                        Ok(o) => Err(o),
                        Err(e) => Ok(e),
                    })
                    .collect();
                match r.map(|mut v| v.pop().unwrap()) {
                    Ok(e) => Err(e),
                    Err(o) => Ok(o),
                }
            }
            BlockPtrKind::Data {
                data: block,
                embedded_type: _,
            } => Ok(RawOrNah::Nah(match ptr.compression_type {
                CompressionType::Off => block.clone(),
                CompressionType::LZ4 | CompressionType::On => decompress_lz4(block)?.1,
                _ => Err(ZfsErrorKind::UnsupportedFeature)?,
            })),
        }
    }
    fn lookup_block(
        &self,
        block: &BlockPtr,
        block_id: u64,
        level: u8,
        level_shift: u8,
        data_block_size: u32,
    ) -> Result<RawOrNah<Self::Block>, ZfsError> {
        let block = self.get(block)?;
        if level != 0 {
            let idx = get_level_index(block_id, level - 1, level_shift);
            let (_input, block_pointer) = BlockPtr::parse(&block.as_ref()[idx as usize * 128..])?;
            self.lookup_block(
                &block_pointer,
                block_id,
                level - 1,
                level_shift,
                data_block_size,
            )
        } else {
            Ok(block)
        }
    }
    fn read(&self, addr: DVA, psize: u32) -> Result<RawOrNah<Self::Block>, ZfsError>;
}

/// Marker trait that we want the default SPA implementation
pub trait DefaultSPA {}

impl<T, B: AsRef<[u8]>> SPA for T
where
    T: Device<Block = B> + DefaultSPA,
{
    type Block = B;
    fn read(&self, addr: DVA, psize: u32) -> Result<RawOrNah<Self::Block>, ZfsError> {
        if addr.gang {
            return Err(ZfsErrorKind::UnsupportedFeature.into());
        }
        Ok(RawOrNah::Raw(self.read_offset(
            addr.vdev,
            addr.offset,
            psize,
        )?))
    }
}

pub trait DMU {
    type Block: AsRef<[u8]>;
    fn get_objset(&self, ptr: &BlockPtr) -> Result<ObjsetPhys, ZfsError>;
    fn get_dnode(&self, objset: &ObjsetPhys, dnode_id: u64) -> Result<DNodePhys, ZfsError> {
        let dnode_per_block = objset.metadnode.header.datablkszsec as u64; // dnodes are 512 bytes so this just works
        let block_id = dnode_id / dnode_per_block;
        let dnode_id = dnode_id % dnode_per_block;
        let block = self.read_block(&objset.metadnode, block_id)?;
        let input = &block.as_ref()[dnode_id as usize * 512..];
        let (_input, dnode) = DNodePhys::parse(&input)?;
        Ok(dnode)
    }
    fn read_block(&self, dnode: &DNodePhys, block_id: u64) -> Result<Self::Block, ZfsError>;
}

const fn get_level_index(id: u64, level: u8, level_shift: u8) -> u64 {
    (id >> (level as u64 * level_shift as u64)) % (1 << (level_shift as u64))
}

/// Marker trait that we want the default DMU implementation
pub trait DefaultDMU {}

impl<T, B: AsRef<[u8]>> DMU for T
where
    T: SPA<Block = B> + DefaultDMU,
{
    type Block = RawOrNah<B>;
    fn get_objset(&self, ptr: &BlockPtr) -> Result<ObjsetPhys, ZfsError> {
        Ok(ObjsetPhys::parse(self.get(ptr)?.as_ref())?.1)
    }
    fn read_block(&self, dnode: &DNodePhys, block_id: u64) -> Result<Self::Block, ZfsError> {
        if block_id > dnode.header.max_block_id {
            return Err(ZfsErrorKind::NotFound.into());
        }
        let level_shift = dnode.header.indirect_block_shift - 7;
        let data_block_size = dnode.header.datablkszsec as u32 * 512;
        let idx = get_level_index(block_id, dnode.header.levels - 1, level_shift);
        self.lookup_block(
            &dnode.block_pointers[idx as usize],
            block_id,
            dnode.header.levels - 1,
            level_shift,
            data_block_size,
        )
    }
}

pub trait ZAP {
    fn lookup_zap(&self, zap_obj: &DNodePhys, key: &ZapStr) -> Result<Option<ZapResult>, ZfsError>;
    fn list_zap(&self, zap_dnode: &DNodePhys) -> Result<Vec<(ZapString, ZapResult)>, ZfsError>;
}

/// Marker trait that we want the default ZAP implementation
pub trait DefaultZAP {}

impl<T, B: AsRef<[u8]>> ZAP for T
where
    T: DMU<Block = B> + DefaultZAP,
{
    fn lookup_zap(
        &self,
        zap_dnode: &DNodePhys,
        key: &ZapStr,
    ) -> Result<Option<ZapResult>, ZfsError> {
        let input = self.read_block(zap_dnode, 0)?;
        let block_size = zap_dnode.header.datablkszsec as usize * 512;
        let leaf_block_shift = zap_leaf_hash_numentries(block_size).trailing_zeros();
        let (_input, zap_header) = ZapBlock::parse(input.as_ref(), block_size)?;
        match zap_header {
            ZapBlock::MicroZap(micro) => Ok(micro.lookup(key).map(|k| ZapResult::U64(vec![k]))),
            ZapBlock::FatHeader(zap_header) => {
                let hash = zap_hash(zap_header.salt, key);
                let idx = zap_hash_idx(hash, zap_header.ptrtbl.shift as u8);
                let mut leaf_block_num = match if zap_header.ptrtbl.blk == 0 {
                    // use embedded pointer table
                    zap_header.leafs[idx as usize]
                } else {
                    let block_idx = (idx) / (block_size as u64 / 8);
                    let idx = idx as usize % (block_size / 8);
                    if block_idx > zap_header.ptrtbl.numblks {
                        return Err(ZfsErrorKind::Invalid.into());
                    }
                    let tbl = self.read_block(zap_dnode, zap_header.ptrtbl.blk + block_idx)?;
                    let (_, num) = number::le_u64(&tbl.as_ref()[idx * 8..])?;
                    num
                    // use external pointer table
                } {
                    0 => return Ok(None), // Entry not found
                    p => p,
                };
                // loop through the chained leaf blocks
                loop {
                    let input = self.read_block(zap_dnode, leaf_block_num)?;
                    let (_input, leaf_block) = ZapLeafPhys::parse(input.as_ref(), block_size)?;
                    match leaf_block.lookup(key, hash, leaf_block_shift) {
                        Some(v) => return Ok(Some(v)),
                        None => {}
                    }
                    if leaf_block.hdr.next == 0 {
                        return Ok(None);
                    }
                    leaf_block_num = leaf_block.hdr.next;
                }
            }
            ZapBlock::FatLeaf(_) => Err(ZfsErrorKind::Invalid.into()),
        }
    }
    fn list_zap(&self, zap_dnode: &DNodePhys) -> Result<Vec<(ZapString, ZapResult)>, ZfsError> {
        let input = self.read_block(zap_dnode, 0)?;
        let block_size = zap_dnode.header.datablkszsec as usize * 512;
        let (_input, zap_header) = ZapBlock::parse(input.as_ref(), block_size)?;
        match zap_header {
            ZapBlock::MicroZap(micro) => Ok(micro
                .get_entries()
                .into_iter()
                .map(|(key, val)| (key, ZapResult::U64(vec![val])))
                .collect()),
            ZapBlock::FatHeader(zap_header) => {
                let mut block_list: Vec<u64> = if zap_header.ptrtbl.blk == 0 {
                    zap_header
                        .leafs
                        .iter()
                        .filter(|p| **p != 0)
                        .map(|i| *i)
                        .collect()
                } else {
                    let mut out = vec![];
                    for i in 0..zap_header.ptrtbl.numblks {
                        let block = self.read_block(zap_dnode, zap_header.ptrtbl.blk + i)?;
                        let nums = nom::multi::many0(number::le_u64)(block.as_ref())?.1;
                        out.extend(nums.into_iter().filter(|p| *p != 0));
                    }
                    out
                };
                block_list.sort_unstable();
                block_list.dedup();
                let mut out = vec![];
                for blocknum in block_list {
                    let input = self.read_block(zap_dnode, blocknum)?;
                    let (_input, leaf_block) = ZapLeafPhys::parse(input.as_ref(), block_size)?;
                    out.extend(leaf_block.get_entries())
                }
                Ok(out)
            }
            ZapBlock::FatLeaf(_) => Err(ZfsErrorKind::Invalid.into()),
        }
    }
}

pub trait DSL {
    fn get_dir(&self, objset: &ObjsetPhys, dir_id: u64) -> Result<DirPhys, ZfsError>;
    fn get_dataset(&self, objset: &ObjsetPhys, ds_id: u64) -> Result<DatasetPhys, ZfsError>;
}

/// Marker trait that we want the default DSL implementation
pub trait DefaultDSL {}

impl<B, T> DSL for T
where
    T: DMU<Block = B> + ZAP + DefaultDSL,
    B: AsRef<[u8]>,
{
    fn get_dir(&self, objset: &ObjsetPhys, dir_id: u64) -> Result<DirPhys, ZfsError> {
        let dnode = self.get_dnode(objset, dir_id)?;
        Ok(DirPhys::parse(&dnode.bonus)?.1)
    }
    fn get_dataset(&self, objset: &ObjsetPhys, ds_id: u64) -> Result<DatasetPhys, ZfsError> {
        let dnode = self.get_dnode(objset, ds_id)?;
        Ok(DatasetPhys::parse(&dnode.bonus)?.1)
    }
}

pub trait ZPL {
    fn lookup_dir_entry(&self, dir: &DNodePhys, key: &ZapStr)
        -> Result<Option<DirEntry>, ZfsError>;
    fn list_dir_entries(&self, dir: &DNodePhys) -> Result<Vec<(ZapString, DirEntry)>, ZfsError>;
    fn lookup_sa_layout(
        &self,
        objset: &ObjsetPhys,
        sa_root_zap: &DNodePhys,
        layout_id: u16,
    ) -> Result<Vec<SAAttr>, ZfsError>;
}

/// Marker trait that we want the default ZPL implementation
pub trait DefaultZPL {}

impl<T, B: AsRef<[u8]>> ZPL for T
where
    T: ZAP + DMU<Block = B> + DefaultZPL,
{
    fn lookup_dir_entry(
        &self,
        dir: &DNodePhys,
        key: &ZapStr,
    ) -> Result<Option<DirEntry>, ZfsError> {
        match self.lookup_zap(dir, key)? {
            Some(ZapResult::U64(r)) if r.len() == 1 => Ok(Some(DirEntry(r[0]))),
            _ => Ok(None),
        }
    }
    fn list_dir_entries(&self, dir: &DNodePhys) -> Result<Vec<(ZapString, DirEntry)>, ZfsError> {
        let entries = self.list_zap(dir)?;
        Ok(entries
            .into_iter()
            .filter_map(|(name, e)| match e {
                ZapResult::U64(r) if r.len() == 1 => Some((name, DirEntry(r[0]))),
                _ => None,
            })
            .collect())
    }
    fn lookup_sa_layout(
        &self,
        objset: &ObjsetPhys,
        sa_root_zap: &DNodePhys,
        layout_id: u16,
    ) -> Result<Vec<SAAttr>, ZfsError> {
        let registry_id = match self.lookup_zap(
            sa_root_zap,
            &ZapString::from_byte_slice(b"REGISTRY").unwrap(),
        )? {
            Some(ZapResult::U64(r)) if r.len() == 1 => Ok(r[0]),
            _ => Err(ZfsErrorKind::Invalid),
        }?;

        let layouts_id = match self.lookup_zap(
            sa_root_zap,
            &ZapString::from_byte_slice(b"LAYOUTS").unwrap(),
        )? {
            Some(ZapResult::U64(r)) if r.len() == 1 => Ok(r[0]),
            _ => Err(ZfsErrorKind::Invalid),
        }?;

        let layouts = self.get_dnode(objset, layouts_id)?;
        let registry = self.get_dnode(objset, registry_id)?;
        let layout = match self
            .lookup_zap(
                &layouts,
                &ZapString::from_byte_slice(format!("{}", layout_id).as_bytes()).unwrap(),
            )?
            .ok_or(ZfsErrorKind::NotFound)?
        {
            ZapResult::U16(a) => Ok(a),
            _ => Err(ZfsErrorKind::Invalid),
        }?;
        let mut registry: Vec<_> = self
            .list_zap(&registry)?
            .into_iter()
            .filter_map(|(k, v)| match v {
                ZapResult::U64(v) if v.len() == 1 => Some((SAAttrPhys(v[0]), k)),
                _ => None,
            })
            .collect();
        registry.sort_unstable_by_key(|a| a.0.get_attr_num());
        layout
            .into_iter()
            .map(|id| {
                let idx = registry
                    .binary_search_by_key(&id, |x| x.0.get_attr_num())
                    .map_err(|_| ZfsErrorKind::NotFound)?;
                Ok(SAAttr::new(
                    registry[idx].1.clone(),
                    registry[idx].0.clone(),
                ))
            })
            .collect()
    }
}

#[derive(Debug)]
pub struct ZfsDrive<Z> {
    inner: Z,
    label: Label,
}

impl<Z> ZfsDrive<Z> {
    pub fn new_with_label(inner: Z, label: Label) -> Self {
        Self { inner, label }
    }
    pub fn inner(&self) -> &Z {
        &self.inner
    }
}

impl<Z: DMU + SPA> ZfsDrive<Z> {
    pub fn get_most_recent_mos<'a>(&'a self) -> Result<ZfsMetaObjectSet<'a, Z>, ZfsError> {
        let mut ubers = self.label.uberblocks.clone();
        ubers.sort_unstable_by_key(|u| u.txg);
        let uber = ubers.pop().ok_or(ZfsErrorKind::Invalid)?;
        self.get_object_set(&uber.rootbp)?
            .as_mos()
            .ok_or(ZfsErrorKind::Invalid.into())
    }

    pub fn get_object_set<'drive>(
        &'drive self,
        ptr: &BlockPtr,
    ) -> Result<ZfsObjectSet<'drive, Z>, ZfsError> {
        let block = self.inner.get(ptr)?;
        let (_input, objset) = ObjsetPhys::parse(block.as_ref())?;
        Ok(ZfsObjectSet::new(self, objset))
    }
}

impl<Z: DMU + DSL + ZPL + Device> ZfsDrive<Z> {
    pub fn new(inner: Z) -> Result<Self, ZfsError> {
        let label = inner.get_label(0)?.ok_or(ZfsErrorKind::Invalid)?; // TODO: try checking other labels
        Ok(Self { inner, label })
    }
}

#[derive(Debug)]
pub struct ZfsObjectSet<'drive, Z> {
    drive: &'drive ZfsDrive<Z>,
    pub os: ObjsetPhys,
}

impl<'drive, Z: DMU> ZfsObjectSet<'drive, Z> {
    pub fn new(drive: &'drive ZfsDrive<Z>, os: ObjsetPhys) -> Self {
        Self { drive, os }
    }
    pub fn get_dnode(&self, obj_num: u64) -> Result<DNodePhys, ZfsError> {
        self.drive.inner.get_dnode(&self.os, obj_num)
    }
    pub fn as_mos(self) -> Option<ZfsMetaObjectSet<'drive, Z>> {
        if self.os.os_type == dmu::OsType::META {
            Some(ZfsMetaObjectSet { os: self })
        } else {
            None
        }
    }
    pub fn as_zvol(self) -> Option<ZfsZVolObjectSet<'drive, Z>> {
        if self.os.os_type == dmu::OsType::ZFS {
            Some(ZfsZVolObjectSet { os: self })
        } else {
            None
        }
    }
}
impl<'drive, Z: DMU + ZAP> ZfsObjectSet<'drive, Z> {
    pub fn as_zpl(self) -> Option<ZfsZPLObjectSet<'drive, Z>> {
        if self.os.os_type == dmu::OsType::ZFS {
            Some(ZfsZPLObjectSet::new(self))
        } else {
            None
        }
    }
}

#[derive(Debug)]
pub struct ZfsMetaObjectSet<'drive, Z> {
    os: ZfsObjectSet<'drive, Z>,
}

impl<'drive, Z: DSL + DMU + ZAP> ZfsMetaObjectSet<'drive, Z> {
    pub fn get_root_dir<'mos>(&'mos self) -> Result<ZfsDslDir<'drive, 'mos, Z>, ZfsError> {
        let mos_config = self.os.get_dnode(1)?;
        let root_obj_num = match self
            .os
            .drive
            .inner
            .lookup_zap(
                &mos_config,
                &ZapString::from_byte_slice(b"root_dataset").unwrap(),
            )?
            .ok_or(ZfsErrorKind::NotFound)?
        {
            ZapResult::U64(a) if a.len() == 1 => Ok(a[0]),
            _ => Err(ZfsErrorKind::Invalid),
        }?;
        self.get_dsl_dir(root_obj_num)
    }

    pub fn get_dsl_dir<'mos>(
        &'mos self,
        obj_num: u64,
    ) -> Result<ZfsDslDir<'drive, 'mos, Z>, ZfsError> {
        let dir_obj = self.os.drive.inner.get_dir(&self.os.os, obj_num)?;
        Ok(ZfsDslDir::new(self, dir_obj))
    }

    pub fn get_dsl_dataset<'mos>(
        &'mos self,
        obj_num: u64,
    ) -> Result<ZfsDslDataset<'drive, 'mos, Z>, ZfsError> {
        let ds_obj = self.os.drive.inner.get_dataset(&self.os.os, obj_num)?;
        Ok(ZfsDslDataset::new(self, ds_obj))
    }
}
impl<'drive, Z> ZfsMetaObjectSet<'drive, Z> {
    pub fn as_os(&self) -> &ZfsObjectSet<'drive, Z> {
        &self.os
    }
}

#[derive(Debug)]
pub struct ZfsDslDir<'drive, 'mos, Z> {
    mos: &'mos ZfsMetaObjectSet<'drive, Z>,
    dsl_dir: DirPhys,
}

impl<'drive, 'mos, Z: DSL + DMU + ZAP> ZfsDslDir<'drive, 'mos, Z> {
    pub fn new(
        mos: &'mos ZfsMetaObjectSet<'drive, Z>,
        dsl_dir: DirPhys,
    ) -> ZfsDslDir<'drive, 'mos, Z> {
        Self { mos, dsl_dir }
    }
    pub fn get_head_dataset(&self) -> Result<ZfsDslDataset<'drive, 'mos, Z>, ZfsError> {
        self.mos.get_dsl_dataset(self.dsl_dir.head_dataset_obj)
    }
}

impl<'drive, 'mos, Z: DMU + ZAP + DSL> ZfsDslDir<'drive, 'mos, Z> {
    pub fn children(&self) -> Result<Vec<(ZapString, ZfsDslDir<'drive, 'mos, Z>)>, ZfsError> {
        let child_zap = self.mos.os.get_dnode(self.dsl_dir.child_dir_zapobj)?;
        let children = self.mos.os.drive.inner.list_zap(&child_zap)?;
        children
            .into_iter()
            .map(|(name, val)| {
                let objnum = match val {
                    ZapResult::U64(a) if a.len() == 1 => Ok(a[0]),
                    _ => Err(ZfsErrorKind::Invalid),
                }?;
                self.mos.get_dsl_dir(objnum).map(|dir| (name, dir))
            })
            .collect()
    }
    pub fn get_child(
        &self,
        child: &ZapStr,
    ) -> Result<Option<ZfsDslDir<'drive, 'mos, Z>>, ZfsError> {
        let child_zap = self.mos.os.get_dnode(self.dsl_dir.child_dir_zapobj)?;
        let child = match self.mos.os.drive.inner.lookup_zap(&child_zap, child)? {
            Some(ZapResult::U64(a)) if a.len() == 1 => Ok(a[0]),
            Some(_) => Err(ZfsErrorKind::Invalid),
            None => return Ok(None),
        }?;
        self.mos.get_dsl_dir(child).map(Some)
    }
}

#[derive(Debug)]
pub struct ZfsDslDataset<'drive, 'mos, Z> {
    mos: &'mos ZfsMetaObjectSet<'drive, Z>,
    dsl_ds: DatasetPhys,
}

impl<'drive, 'mos, Z> ZfsDslDataset<'drive, 'mos, Z> {
    pub fn new(
        mos: &'mos ZfsMetaObjectSet<'drive, Z>,
        dsl_ds: DatasetPhys,
    ) -> ZfsDslDataset<'drive, 'mos, Z> {
        Self { mos, dsl_ds }
    }
}

impl<'drive, 'mos, Z: SPA + DMU> ZfsDslDataset<'drive, 'mos, Z> {
    pub fn get_object_set(&self) -> Result<ZfsObjectSet<'drive, Z>, ZfsError> {
        self.mos.os.drive.get_object_set(&self.dsl_ds.bp)
    }
}

#[derive(Debug)]
pub struct ZfsZPLObjectSet<'drive, Z> {
    os: ZfsObjectSet<'drive, Z>,
    registry: Option<Result<ZfsSARegistry<'drive, Z>, ZfsError>>, // this is a stupid way to do this
}

impl<'drive, Z: DMU + ZAP> ZfsZPLObjectSet<'drive, Z> {
    pub fn new(os: ZfsObjectSet<'drive, Z>) -> Self {
        let mut fs = ZfsZPLObjectSet {
            os: os,
            registry: None,
        };
        let registry = ZfsSARegistry::new(&fs);
        fs.registry = Some(registry);
        fs
    }
}
impl<'drive, Z: DMU + ZAP + ZPL> ZfsZPLObjectSet<'drive, Z> {
    pub fn get_root<'fs>(&'fs self) -> Result<ZfsZPLDir<'drive, 'fs, Z>, ZfsError> {
        let master_node = self.os.get_dnode(1)?;
        let root_obj_num = match self
            .os
            .drive
            .inner
            .lookup_zap(&master_node, &ZapString::from_byte_slice(b"ROOT").unwrap())?
            .ok_or(ZfsErrorKind::NotFound)?
        {
            ZapResult::U64(a) if a.len() == 1 => Ok(a[0]),
            _ => Err(ZfsErrorKind::Invalid),
        }?;
        self.get_dir(root_obj_num)
    }
}
impl<'drive, Z: DMU + ZPL> ZfsZPLObjectSet<'drive, Z> {
    pub fn get_dir<'fs>(&'fs self, obj_num: u64) -> Result<ZfsZPLDir<'drive, 'fs, Z>, ZfsError> {
        let dir_obj = self.os.get_dnode(obj_num)?;
        Ok(ZfsZPLDir::new(self, dir_obj))
    }

    // TODO: remove option after implementing other object types
    pub fn get_dir_entry<'fs>(
        &'fs self,
        entry: DirEntry,
    ) -> Result<Option<ZfsDirEntry<'drive, 'fs, Z>>, ZfsError> {
        let dnode = self.os.get_dnode(entry.get_objnum())?;
        match entry.get_type() {
            DirEntryType::Directory => Ok(Some(ZfsDirEntry::Dir(ZfsZPLDir::new(self, dnode)))),
            DirEntryType::RegularFile => Ok(Some(ZfsDirEntry::File(ZfsZPLFile::new(self, dnode)))),
            _ => Ok(None), // TODO: implement other types
        }
    }
}
impl<'drive, Z> ZfsZPLObjectSet<'drive, Z> {
    pub fn as_os(&self) -> &ZfsObjectSet<'drive, Z> {
        &self.os
    }
    pub fn get_registry(&self) -> Option<&ZfsSARegistry<'drive, Z>> {
        self.registry.as_ref().map(|r| r.as_ref().ok()).flatten() // this is cursed
    }
}

#[derive(Debug)]
pub struct ZfsSARegistry<'drive, Z> {
    drive: &'drive ZfsDrive<Z>,
    registry: DNodePhys,
    layout: DNodePhys,
}

impl<'drive, Z: DMU + ZAP> ZfsSARegistry<'drive, Z> {
    pub fn new(fs: &ZfsZPLObjectSet<'drive, Z>) -> Result<Self, ZfsError> {
        let drive = fs.as_os().drive;
        let master_node = fs.as_os().get_dnode(1)?;
        let zap_index = match drive.inner.lookup_zap(
            &master_node,
            &ZapString::from_byte_slice(b"SA_ATTRS").unwrap(),
        )? {
            Some(ZapResult::U64(r)) if r.len() == 1 => Ok(r[0]),
            _ => Err(ZfsErrorKind::Invalid),
        }?;
        let zap_index = fs.as_os().get_dnode(zap_index)?;
        let registry_id = match drive.inner.lookup_zap(
            &zap_index,
            &ZapString::from_byte_slice(b"REGISTRY").unwrap(),
        )? {
            Some(ZapResult::U64(r)) if r.len() == 1 => Ok(r[0]),
            _ => Err(ZfsErrorKind::Invalid),
        }?;

        let layout_id = match drive
            .inner
            .lookup_zap(&zap_index, &ZapString::from_byte_slice(b"LAYOUTS").unwrap())?
        {
            Some(ZapResult::U64(r)) if r.len() == 1 => Ok(r[0]),
            _ => Err(ZfsErrorKind::Invalid),
        }?;

        let layout = fs.as_os().get_dnode(layout_id)?;
        let registry = fs.as_os().get_dnode(registry_id)?;
        Ok(Self {
            drive,
            registry,
            layout,
        })
    }
}
impl<'drive, Z: ZAP> ZfsSARegistry<'drive, Z> {
    pub fn lookup_layout(&self, layout_id: u16) -> Result<Vec<SAAttr>, ZfsError> {
        let layout = self.lookup_raw_layout(layout_id)?;
        layout
            .into_iter()
            .map(|id| {
                self.lookup_attr(id)
                    .and_then(|o| o.ok_or(ZfsErrorKind::NotFound.into()))
            })
            .collect()
    }
    fn lookup_raw_layout(&self, layout_id: u16) -> Result<Vec<u16>, ZfsError> {
        match self
            .drive
            .inner
            .lookup_zap(
                &self.layout,
                &ZapString::from_byte_slice(format!("{}", layout_id).as_bytes()).unwrap(),
            )?
            .ok_or(ZfsErrorKind::NotFound)?
        {
            ZapResult::U16(a) => Ok(a),
            _ => Err(ZfsErrorKind::Invalid.into()),
        }
    }
    fn lookup_attr(&self, attr_id: u16) -> Result<Option<SAAttr>, ZfsError> {
        Ok(self
            .drive
            .inner
            .list_zap(&self.registry)?
            .into_iter()
            .filter_map(|(k, v)| match v {
                ZapResult::U64(v) if v.len() == 1 => Some((SAAttrPhys(v[0]), k)),
                _ => None,
            })
            .find(|(a, _)| a.get_attr_num() == attr_id)
            .map(|(attr, name)| SAAttr::new(name, attr)))
    }
}

#[derive(Debug)]
pub struct ZfsZPLDir<'drive, 'fs, Z> {
    fs: &'fs ZfsZPLObjectSet<'drive, Z>,
    dir: DNodePhys,
}

impl<'drive, 'fs, Z: ZPL + DMU> ZfsZPLDir<'drive, 'fs, Z> {
    pub fn new(fs: &'fs ZfsZPLObjectSet<'drive, Z>, dir: DNodePhys) -> Self {
        Self { fs, dir }
    }
    pub fn children(&self) -> Result<Vec<(ZapString, ZfsDirEntry<'drive, 'fs, Z>)>, ZfsError> {
        let children = self.fs.os.drive.inner.list_dir_entries(&self.dir)?;
        children
            .into_iter()
            .map(|(name, entry)| Ok(self.fs.get_dir_entry(entry)?.map(|e| (name, e))))
            .filter_map(|r| r.transpose())
            .collect()
    }
    pub fn get_child(
        &self,
        child: &ZapStr,
    ) -> Result<Option<ZfsDirEntry<'drive, 'fs, Z>>, ZfsError> {
        let child = match self.fs.os.drive.inner.lookup_dir_entry(&self.dir, child)? {
            Some(c) => c,
            None => return Ok(None),
        };
        self.fs.get_dir_entry(child)
    }
}

#[derive(Debug)]
pub struct ZfsZPLFile<'drive, 'fs, Z> {
    fs: &'fs ZfsZPLObjectSet<'drive, Z>,
    file: DNodePhys,
}

impl<'drive, 'fs, Z> ZfsZPLFile<'drive, 'fs, Z> {
    pub fn new(fs: &'fs ZfsZPLObjectSet<'drive, Z>, file: DNodePhys) -> Self {
        Self { fs, file }
    }
}
impl<'drive, 'fs, Z: ZAP> ZfsZPLFile<'drive, 'fs, Z> {
    pub fn list_attributes(&self) -> Result<Vec<(SAAttr, SAValue)>, ZfsError> {
        let (_input, buf) = SABuf::parse(&self.file.bonus)?;
        let registry = self.fs.get_registry().ok_or(ZfsErrorKind::NotFound)?;
        let layout = registry.lookup_layout(buf.layout)?;
        buf.parse_attrs(layout)
    }
    pub fn reader<'file>(&'file self) -> Result<ZfsZPLFileReader<'drive, 'fs, 'file, Z>, ZfsError> {
        ZfsZPLFileReader::new(self)
    }
}

impl<'drive, 'fs, B: AsRef<[u8]>, Z: DMU<Block = B>> ZfsZPLFile<'drive, 'fs, Z> {
    pub fn read_block(&self, block_idx: u64) -> Result<B, ZfsError> {
        self.fs
            .as_os()
            .drive
            .inner
            .read_block(&self.file, block_idx)
    }
}

#[derive(Debug)]
pub struct ZfsZPLFileReader<'drive, 'fs, 'file, Z> {
    file: &'file ZfsZPLFile<'drive, 'fs, Z>,
    buf: Vec<u8>,
    buf_pos: usize,
    block_num: u64,
    file_size: u64,
}

impl<'drive, 'fs, 'file, Z: ZAP> ZfsZPLFileReader<'drive, 'fs, 'file, Z> {
    pub fn new(file: &'file ZfsZPLFile<'drive, 'fs, Z>) -> Result<Self, ZfsError> {
        let attr_name = ZapString::from_byte_slice(b"ZPL_SIZE").unwrap();
        let file_size = match file
            .list_attributes()?
            .into_iter()
            .find(|(a, _v)| &*a.name == &*attr_name)
            .ok_or(ZfsErrorKind::NotFound)?
            .1
        {
            SAValue::U64(a) if a.len() == 1 => Ok(a[0]),
            _ => Err(ZfsErrorKind::Invalid),
        }?;
        Ok(Self {
            file: file,
            buf: Vec::with_capacity(file.file.header.datablkszsec as usize * 512),
            buf_pos: 0,
            block_num: 0,
            file_size,
        })
    }
}

impl<'drive, 'fs, 'file, Z: DMU> std::io::Read for ZfsZPLFileReader<'drive, 'fs, 'file, Z> {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        if self.buf_pos >= self.buf.len() {
            if self.block_num > self.file.file.header.max_block_id {
                return Ok(0);
            }
            let block = self
                .file
                .read_block(self.block_num)
                .map_err(Into::<IoError>::into)?;
            self.block_num += 1;
            self.buf.clear();
            self.buf_pos = 0;
            self.buf.extend_from_slice(block.as_ref());
            let bytes_read = self.block_num * self.file.file.header.datablkszsec as u64 * 512;
            let last_chunk = (self.block_num - 1) * self.file.file.header.datablkszsec as u64 * 512;
            if bytes_read > self.file_size {
                self.buf.truncate((self.file_size - last_chunk) as usize);
            }
        }
        let bytes_copied = buf.len().min(self.buf.len() - self.buf_pos);
        buf[..bytes_copied].copy_from_slice(&self.buf[self.buf_pos..(self.buf_pos + bytes_copied)]);
        self.buf_pos += bytes_copied;
        Ok(bytes_copied)
    }
}

#[derive(Debug)]
pub enum ZfsDirEntry<'drive, 'fs, Z> {
    Dir(ZfsZPLDir<'drive, 'fs, Z>),
    File(ZfsZPLFile<'drive, 'fs, Z>),
}

#[derive(Debug)]
pub struct ZfsZVolObjectSet<'drive, Z> {
    #[allow(dead_code)]
    os: ZfsObjectSet<'drive, Z>,
}
