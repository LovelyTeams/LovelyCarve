//!
//! Datastructures used by the Data Management Unit
//!
use std::convert::TryFrom;

use nom::{number::complete as number, IResult};

use enum_repr_derive::TryFrom;

use crate::spa::BlockPtr;
use crate::zil::ZilHeader;

/// Metadata stored in the DNode header
#[derive(Debug, Clone)]
pub struct DNodePhysHeader {
    /// What type of DNode is this
    pub kind: u8,
    /// How big are the indirect blocks, log base 2 (size = `2^indirect_block_shift`), in bytes
    pub indirect_block_shift: u8,
    /// How many `BlockPtr`s do we traverse before we hit data blocks
    pub levels: u8,
    /// How many block pointers are stored in this DNode, range from 1 to 3
    pub num_block_ptr: u8,
    /// The type of the data stored in the bonus buffer
    pub bonus_type: u8,
    /// The checksum algorithm used for the data pointed at by this DNode
    pub checksum: u8,
    /// The type of compression used for the data pointed at by this DNode
    pub compress: u8,
    /// Size of a datablock, stored as number of 512 byte sectors.
    /// `Size in bytes = 512 * datablkszsec`
    pub datablkszsec: u16, // idk what this one is
    /// How big the bonus buffer is
    pub bonus_len: u16,
    /// ID of the largest data block pointed to by this DNode
    pub max_block_id: u64,
    /// Sum of the asize values of all direct and indirect block pointers referenced by this DNode
    pub sec_phys: u64,
}

impl DNodePhysHeader {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (
            input,
            (kind, indirect_block_shift, levels, num_block_ptr, bonus_type, checksum, compress),
        ) = nom::sequence::tuple((
            number::le_u8,
            number::le_u8,
            number::le_u8,
            number::le_u8,
            number::le_u8,
            number::le_u8,
            number::le_u8,
        ))(input)?;
        let (input, _pad) = nom::bytes::complete::take(1usize)(input)?;
        let (input, (datablkszsec, bonus_len)) =
            nom::sequence::tuple((number::le_u16, number::le_u16))(input)?;
        let (input, _pad) = nom::bytes::complete::take(4usize)(input)?;
        let (input, (max_block_id, sec_phys)) =
            nom::sequence::tuple((number::le_u64, number::le_u64))(input)?;
        let (input, _pad) = nom::bytes::complete::take(4 * 8usize)(input)?;
        Ok((
            input,
            Self {
                kind,
                indirect_block_shift,
                levels,
                num_block_ptr,
                bonus_type,
                checksum,
                compress,
                datablkszsec,
                bonus_len,
                max_block_id,
                sec_phys,
            },
        ))
    }
}

/// Every ZFS object on disk is represented by a DNode. The DNode can contain metadata about the
/// object, stored in the `bonus` buffer, or block pointers to areas on disk containing the
/// object's data, or both.
#[derive(Debug, Clone)]
pub struct DNodePhys {
    pub header: DNodePhysHeader,
    pub block_pointers: Vec<BlockPtr>,
    pub bonus: Vec<u8>,
}

impl DNodePhys {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        nom::combinator::map_parser(nom::bytes::complete::take(512usize), |input| {
            let (input, header) = DNodePhysHeader::parse(input)?;
            let (input, block_pointers) =
                nom::multi::count(BlockPtr::parse, header.num_block_ptr as usize)(input)?;
            let (input, bonus) = nom::bytes::complete::take(header.bonus_len as usize)(input)?;
            Ok((
                input,
                Self {
                    header,
                    block_pointers,
                    bonus: bonus.to_vec(),
                },
            ))
        })(input)
    }
}

/// An object set is the foundational datastructure used in ZFS, everything other object exists
/// as a member of an objectset, known as a DNode. DNodes may contain metadata and/or a pointer
/// to blocks on disk containing data
#[derive(Debug, Clone)]
pub struct ObjsetPhys {
    /// The metadnode's block pointers point to an array of DNodes, the members of the object set
    pub metadnode: DNodePhys,
    pub os_zil_header: ZilHeader,
    /// What type of object set is this
    pub os_type: OsType,
}

impl ObjsetPhys {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (metadnode, os_zil_header, os_type)) = nom::combinator::map_parser(
            nom::bytes::complete::take(1024usize),
            nom::sequence::tuple((DNodePhys::parse, ZilHeader::parse, OsType::parse)),
        )(input)?;
        Ok((
            input,
            Self {
                metadnode,
                os_zil_header,
                os_type,
            },
        ))
    }
}

/// The type of the ObjectSet
#[derive(Debug, Clone, TryFrom, PartialEq, Eq)]
#[repr(u64)]
pub enum OsType {
    NONE = 0,
    META = 1,
    ZFS = 2,
    ZVOL = 3,
}

impl OsType {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        nom::combinator::map_res(number::le_u64, |x| OsType::try_from(x))(input)
    }
}
