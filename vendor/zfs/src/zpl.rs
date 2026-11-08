//!
//! Datastructures used by the ZFS Posix Layer
//!
use std::convert::{TryFrom, TryInto};

use nom::{number::complete as number, IResult};

use enum_repr_derive::TryFrom;

use crate::zap::ZapString;
use crate::{ZfsError, ZfsErrorKind};

/// Turns out this structure isn't used anymore since ZPL version 5
// -_-
#[derive(Debug, Clone)]
pub struct ZNodePhys {
    pub atime: [u64; 2],
    pub mtime: [u64; 2],
    pub ctime: [u64; 2],
    pub crtime: [u64; 2],
    pub gen: u64,
    pub mode: u64,
    pub parent: u64,
    pub links: u64,
    pub xattr: u64,
    pub rdev: u64,
    pub flags: u64,
    pub uid: u64,
    pub gid: u64,
    pub acl: ZNodeAcl,
}

impl ZNodePhys {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (
            input,
            (
                at0,
                at1,
                mt0,
                mt1,
                ct0,
                ct1,
                crt0,
                crt1,
                gen,
                mode,
                parent,
                links,
                xattr,
                rdev,
                flags,
                uid,
                gid,
                _pad,
                acl,
            ),
        ) = nom::sequence::tuple((
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
            nom::bytes::complete::take(8 * 4usize),
            ZNodeAcl::parse,
        ))(input)?;
        Ok((
            input,
            Self {
                atime: [at0, at1],
                mtime: [mt0, mt1],
                ctime: [ct0, ct1],
                crtime: [crt0, crt1],
                gen,
                mode,
                parent,
                links,
                xattr,
                rdev,
                flags,
                uid,
                gid,
                acl,
            },
        ))
    }
}

pub const ACE_SLOT_CNT: usize = 6;

/// Access control list in the old format
#[derive(Debug, Clone)]
pub struct ZNodeAcl {
    pub extern_obj: u64,
    pub count: u32,
    pub version: u16,
    pub ace_data: [Ace; ACE_SLOT_CNT],
}

impl ZNodeAcl {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (extern_obj, count, version, _pad, ace_data)) = nom::sequence::tuple((
            number::le_u64,
            number::le_u32,
            number::le_u16,
            nom::bytes::complete::take(2usize),
            nom::multi::count(Ace::parse, ACE_SLOT_CNT),
        ))(input)?;
        let ace_data: &[_; ACE_SLOT_CNT] = (*ace_data).try_into().unwrap();
        Ok((
            input,
            Self {
                extern_obj,
                count,
                version,
                ace_data: ace_data.clone(),
            },
        ))
    }
}

/// Acess control list entry
#[derive(Debug, Clone)]
pub struct Ace {
    pub who: u64,
    pub access_mask: u32,
    pub flags: u16,
    pub kind: u16,
}

impl Ace {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (who, access_mask, flags, kind)) = nom::sequence::tuple((
            number::le_u64,
            number::le_u32,
            number::le_u16,
            number::le_u16,
        ))(input)?;
        Ok((
            input,
            Ace {
                who,
                access_mask,
                flags,
                kind,
            },
        ))
    }
}

/// A directory entry containing an object id pointing to the child object, and the type of the
/// object
#[derive(Debug)]
pub struct DirEntry(pub u64);

impl DirEntry {
    pub fn get_type(&self) -> DirEntryType {
        let ty = (self.0 >> 60) as u8;
        DirEntryTypeInternal::try_from(ty)
            .map(|ty| ty.into())
            .unwrap_or_else(|_| DirEntryType::Invalid(ty))
    }
    pub fn get_objnum(&self) -> u64 {
        self.0 & ((1 << 48) - 1)
    }
    pub fn new(kind: DirEntryType, obj_num: u64) -> Self {
        let ty: u8 = kind.into();
        Self(((ty as u64) << 60) | (obj_num & ((1 << 48) - 1)))
    }
}

/// The types of directory entries
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum DirEntryType {
    NotSpecified,
    FIFO,
    CharacterDevice,
    Directory,
    BlockDevice,
    RegularFile,
    SymLink,
    Socket,
    Door,
    EventPort,
    Invalid(u8),
}

impl From<DirEntryTypeInternal> for DirEntryType {
    fn from(i: DirEntryTypeInternal) -> Self {
        use DirEntryType::*;
        use DirEntryTypeInternal as DI;
        match i {
            DI::NotSpecified => NotSpecified,
            DI::FIFO => FIFO,
            DI::CharacterDevice => CharacterDevice,
            DI::Directory => Directory,
            DI::BlockDevice => BlockDevice,
            DI::RegularFile => RegularFile,
            DI::SymLink => SymLink,
            DI::Socket => Socket,
            DI::Door => Door,
            DI::EventPort => EventPort,
        }
    }
}

impl Into<u8> for DirEntryType {
    fn into(self) -> u8 {
        use DirEntryType::*;
        match self {
            NotSpecified => 0,
            FIFO => 1,
            CharacterDevice => 2,
            Directory => 4,
            BlockDevice => 6,
            RegularFile => 8,
            SymLink => 10,
            Socket => 12,
            Door => 13,
            EventPort => 14,
            Invalid(i) => i,
        }
    }
}

#[derive(TryFrom)]
#[repr(u8)]
enum DirEntryTypeInternal {
    NotSpecified = 0,
    FIFO = 1,
    CharacterDevice = 2,
    // invalid
    Directory = 4,
    // invalid
    BlockDevice = 6,
    // invalid
    RegularFile = 8,
    // invalid
    SymLink = 10,
    // invalid
    Socket = 12,
    // invalid
    Door = 13,
    EventPort = 14,
    // invalid
}

/// A system attribute type
#[derive(Debug)]
pub struct SAAttr {
    pub name: ZapString,
    /// Length of 0 means this attribute is of variable size (!!!)
    pub length: u16,
    pub byteswap: SAByteswapType,
}

impl SAAttr {
    pub fn new(name: ZapString, phys: SAAttrPhys) -> Self {
        SAAttr {
            name: name,
            length: phys.get_len(),
            byteswap: phys.get_byteswap().unwrap(),
        }
    }
}

/// How system attributes are stored in the SA registry
#[derive(Debug, Clone)]
pub struct SAAttrPhys(pub u64);
impl SAAttrPhys {
    pub fn get_len(&self) -> u16 {
        ((self.0 >> 24) & ((1 << 16) - 1)) as u16
    }
    pub fn get_byteswap(&self) -> Option<SAByteswapType> {
        (((self.0 >> 16) & ((1 << 8) - 1)) as u8).try_into().ok()
    }
    pub fn get_attr_num(&self) -> u16 {
        (self.0 & ((1 << 16) - 1)) as u16
    }
}

/// How to byteswap the value of an attribute
#[derive(Debug, TryFrom)]
#[repr(u8)]
pub enum SAByteswapType {
    U64Array = 0,
    U32Array = 1,
    U16Array = 2,
    U8Array = 3,
    ACL = 4,
}

/// The type of the data stored in the system attribute
#[derive(Debug)]
pub enum SAValue {
    U64(Vec<u64>),
    U32(Vec<u32>),
    U16(Vec<u16>),
    U8(Vec<u8>),
    Acl(Vec<Ace>), // TODO
}

impl SAValue {
    pub fn parse<'a>(attr: &SAAttr, len: u16, input: &'a [u8]) -> IResult<&'a [u8], Self> {
        use SAByteswapType::*;
        use SAValue::*;
        let parser = |data| match attr.byteswap {
            U64Array => nom::combinator::map(nom::multi::many0(number::le_u64), U64)(data),
            U32Array => nom::combinator::map(nom::multi::many0(number::le_u32), U32)(data),
            U16Array => nom::combinator::map(nom::multi::many0(number::le_u16), U16)(data),
            U8Array => nom::combinator::map(nom::multi::many0(number::le_u8), U8)(data),
            ACL => nom::combinator::map(nom::multi::many0(Ace::parse), Acl)(data),
        };
        nom::combinator::map_parser(nom::bytes::complete::take(len), parser)(input)
    }
}

pub const SA_MAGIC: u32 = 0x2F505A;

/// SA data from a bonus or spill buffer
#[derive(Debug)]
pub struct SABuf {
    //magic: u32,
    //header_size: u8,
    // don't really need these outside of parsing, lets try leaving them out
    pub layout: u16,
    pub lengths: Vec<u16>,
    pub data: Vec<u8>,
}

impl SABuf {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (_magic, size_layout)) = nom::sequence::tuple((
            nom::combinator::verify(number::le_u32, |magic| *magic == SA_MAGIC),
            number::le_u16,
        ))(input)?; // read 4 + 2 bytes
        let header_size = ((size_layout >> 10) & ((1 << 6) - 1)) * 8;
        let layout = (size_layout) & ((1 << 10) - 1);
        let length_size = header_size - (4 + 2); // subtract the bytes we have already read
        let (input, lengths) = nom::combinator::map_parser(
            nom::bytes::complete::take(length_size),
            nom::multi::many0(number::le_u16),
        )(input)?;
        let (input, data) = nom::combinator::rest(input)?;
        Ok((
            input,
            Self {
                // magic,
                // header_size,
                layout,
                lengths,
                data: data.to_owned(),
            },
        ))
    }
    pub fn parse_attrs(&self, layout: Vec<SAAttr>) -> Result<Vec<(SAAttr, SAValue)>, ZfsError> {
        let attr_count = layout.len();
        layout
            .into_iter()
            .scan(0, |var_length_idx, attr| {
                // find out all the variable lengths first
                if attr.length == 0 {
                    Some(
                        self.lengths
                            .get(*var_length_idx as usize)
                            .ok_or(ZfsErrorKind::Invalid.into())
                            .map(|len| (attr, *len)),
                    )
                } else {
                    let len = attr.length;
                    Some(Ok((attr, len)))
                }
            })
            .try_fold(
                (&self.data[..], Vec::with_capacity(attr_count)),
                |(input, mut out), res| match res {
                    Ok((attr, len)) => {
                        let (input, val) = SAValue::parse(&attr, len, input)?;
                        out.push((attr, val));
                        Ok((input, out))
                    }
                    Err(e) => Err(e),
                },
            )
            .map(|(_input, out)| out)
    }
}
