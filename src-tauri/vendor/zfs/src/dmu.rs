//!
//! Datastructures used by the Data Management Unit
//!
use std::convert::TryFrom;

use nom::{number::complete as number, IResult};

use enum_repr_derive::TryFrom;

use crate::{spa::BlockPtr, ZfsError, ZfsErrorKind};

pub const DNODE_SLOT_SIZE: usize = 512;
const MAX_DNODE_SLOTS: usize = 32;
const DNODE_HEADER_SIZE: usize = 64;
const BLOCK_PTR_SIZE: usize = 128;
const HAS_SPILL: u8 = 1 << 2;
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
    pub flags: u8,
    pub extra_slots: u8,
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
        let (input, flags) = number::le_u8(input)?;
        let (input, (datablkszsec, bonus_len)) =
            nom::sequence::tuple((number::le_u16, number::le_u16))(input)?;
        let (input, extra_slots) = number::le_u8(input)?;
        let (input, _pad) = nom::bytes::complete::take(3usize)(input)?;
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
                flags,
                extra_slots,
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
    pub spill: Option<BlockPtr>,
}

impl DNodePhys {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (_, header) = DNodePhysHeader::parse(input)?;
        let bad_layout = || nom::Err::Error((input, nom::error::ErrorKind::Verify));
        let length = slot_span(input).ok_or_else(bad_layout)? * DNODE_SLOT_SIZE;
        let (record, rest) = input.split_at_checked(length).ok_or_else(bad_layout)?;
        let (body, spill) = if header.flags & HAS_SPILL != 0 {
            let (body, spill) = record.split_at(length - BLOCK_PTR_SIZE);
            (body, Some(BlockPtr::parse(spill)?.1))
        } else {
            (record, None)
        };
        if header.num_block_ptr > 3 {
            return Err(bad_layout());
        }
        let (bonus_storage, block_pointers) = nom::multi::count(
            BlockPtr::parse,
            usize::from(header.num_block_ptr),
        )(&body[DNODE_HEADER_SIZE..])?;
        let bonus = bonus_storage
            .get(..usize::from(header.bonus_len))
            .ok_or_else(bad_layout)?
            .to_vec();
        Ok((
            rest,
            Self {
                header,
                block_pointers,
                bonus,
                spill,
            },
        ))
    }

    pub(crate) fn at_slot(block: &[u8], index: usize) -> Result<Self, ZfsError> {
        let mut slots = block;
        let mut offset = 0;
        while offset <= index {
            let count = slot_span(slots).ok_or(ZfsErrorKind::Invalid)?;
            let length = count * DNODE_SLOT_SIZE;
            let (record, rest) = slots
                .split_at_checked(length)
                .ok_or(ZfsErrorKind::Invalid)?;
            if offset == index && record[0] != 0 {
                return Ok(Self::parse(record)?.1);
            }
            slots = rest;
            offset += count;
        }
        Err(ZfsErrorKind::NotFound.into())
    }
}

fn slot_span(input: &[u8]) -> Option<usize> {
    let count = if *input.first()? == 0 {
        1
    } else {
        usize::from(*input.get(12)?) + 1
    };
    (count <= MAX_DNODE_SLOTS).then_some(count)
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
            nom::sequence::tuple((
                nom::combinator::map_parser(
                    nom::bytes::complete::take(DNODE_SLOT_SIZE),
                    DNodePhys::parse,
                ),
                ZilHeader::parse,
                OsType::parse,
            )),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_all_slots_of_a_large_dnode_and_retains_the_following_object() {
        let mut block = [0u8; 4 * DNODE_SLOT_SIZE];
        block[0] = 19;
        block[3] = 1;
        block[10..12].copy_from_slice(&700u16.to_le_bytes());
        block[12] = 1;
        block[192..892].fill(0x5a);
        block[1024] = 20;
        let first = DNodePhys::at_slot(&block, 0).unwrap();
        assert_eq!(first.header.extra_slots, 1);
        assert_eq!(first.bonus, vec![0x5a; 700]);
        assert!(matches!(
            DNodePhys::at_slot(&block, 1).unwrap_err().source,
            ZfsErrorKind::NotFound
        ));
        assert_eq!(DNodePhys::at_slot(&block, 2).unwrap().header.kind, 20);
        assert!(DNodePhys::parse(&block[..DNODE_SLOT_SIZE]).is_err());
    }

    #[test]
    fn does_not_include_the_last_spill_pointer_in_the_bonus_buffer() {
        let mut record = [0u8; 2 * DNODE_SLOT_SIZE];
        record[0] = 19;
        record[3] = 1;
        record[7] = HAS_SPILL;
        record[12] = 1;
        record[10..12].copy_from_slice(&704u16.to_le_bytes());
        let (_, dnode) = DNodePhys::parse(&record).unwrap();
        assert_eq!(dnode.bonus.len(), 704);
        assert!(dnode.spill.is_some());
        record[10..12].copy_from_slice(&705u16.to_le_bytes());
        assert!(DNodePhys::parse(&record).is_err());
    }
}
