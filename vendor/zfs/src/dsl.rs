//!
//! Datastructures used by the Dataset and Snapshot Layer
//!
use nom::{number::complete as number, IResult};

use crate::spa::BlockPtr;

/// A DSL dataset contains a `BlockPtr` referencing an Object Set
#[derive(Debug, Clone)]
pub struct DatasetPhys {
    pub dir_obj: u64,
    pub prev_snap_obj: u64,
    pub prev_snap_obj_transaction: u64,
    pub next_snap_obj: u64,
    pub snapnames_zapobj: u64,
    pub num_children: u64,
    pub creation_time: u64,
    pub creation_txg: u64,
    pub deadlist_obj: u64,
    pub referenced_bytes: u64,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub unique_bytes: u64,
    pub fsid_guid: u64,
    pub guid: u64,
    pub flags: u64,
    /// `BlockPtr` to the object set
    pub bp: BlockPtr,
    pub next_clones_obj: u64,
    /// ZAP object containing properties
    pub props_obj: u64,
    pub userrefs_obj: u64,
}

impl DatasetPhys {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (
            input,
            (
                dir_obj,
                prev_snap_obj,
                prev_snap_obj_transaction,
                next_snap_obj,
                snapnames_zapobj,
                num_children,
                creation_time,
                creation_txg,
                deadlist_obj,
                referenced_bytes,
                compressed_bytes,
                uncompressed_bytes,
                unique_bytes,
                fsid_guid,
                guid,
                flags,
                bp,
                next_clones_obj,
                props_obj,
                userrefs_obj,
            ),
        ) = nom::combinator::map_parser(
            nom::bytes::complete::take(320usize),
            nom::sequence::tuple((
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
                BlockPtr::parse,
                number::le_u64,
                number::le_u64,
                number::le_u64,
            )),
        )(input)?;
        Ok((
            input,
            Self {
                dir_obj,
                prev_snap_obj,
                prev_snap_obj_transaction,
                next_snap_obj,
                snapnames_zapobj,
                num_children,
                creation_time,
                creation_txg,
                deadlist_obj,
                referenced_bytes,
                compressed_bytes,
                uncompressed_bytes,
                unique_bytes,
                fsid_guid,
                guid,
                flags,
                bp,
                next_clones_obj,
                props_obj,
                userrefs_obj,
            },
        ))
    }
}

/// A DSL Dir points to a dataset and child DSL Dirs
#[derive(Debug, Clone)]
pub struct DirPhys {
    pub creation_time: u64,
    /// The dataset for this directory
    pub head_dataset_obj: u64,
    pub parent_obj: u64,
    pub clone_parent_obj: u64,
    /// A ZAP object containing children of this directory
    pub child_dir_zapobj: u64,
    pub used_bytes: u64,
    pub compressed_bytes: u64,
    pub uncompressed_bytes: u64,
    pub quota: u64,
    pub reserved: u64,
    /// ZAP object containing properties
    pub props_zapobj: u64,
    pub deleg_zapobj: u64,
    pub flags: u64,
    pub used_breakdown: [u64; 5],
    pub clones: u64,
}

impl DirPhys {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (
            input,
            (
                creation_time,
                head_dataset_obj,
                parent_obj,
                clone_parent_obj,
                child_dir_zapobj,
                used_bytes,
                compressed_bytes,
                uncompressed_bytes,
                quota,
                reserved,
                props_zapobj,
                deleg_zapobj,
                flags,
                b1,
                b2,
                b3,
                b4,
                b5,
                clones,
            ),
        ) = nom::combinator::map_parser(
            nom::bytes::complete::take(256usize),
            nom::sequence::tuple((
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
                number::le_u64,
                number::le_u64,
            )),
        )(input)?;
        Ok((
            input,
            Self {
                creation_time,
                head_dataset_obj,
                parent_obj,
                clone_parent_obj,
                child_dir_zapobj,
                used_bytes,
                compressed_bytes,
                uncompressed_bytes,
                quota,
                reserved,
                props_zapobj,
                deleg_zapobj,
                flags,
                used_breakdown: [b1, b2, b3, b4, b5],
                clones,
            },
        ))
    }
}
