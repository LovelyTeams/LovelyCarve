//!
//! Datastructures used by the ZFS Intent Log
//!
use nom::{number::complete as number, IResult};

use crate::spa::{BlockPtr, ZioBlockTail};

#[derive(Debug, Clone)]
pub struct ZilHeader {
    pub claim_txg: u64,
    pub replay_seq: u64,
    pub log: BlockPtr,
    pub claim_block_seq: u64,
    pub flags: u64,
    pub claim_lr_seq: u64,
}

impl ZilHeader {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (claim_txg, replay_seq, log, claim_block_seq, flags, claim_lr_seq, _pad)) =
            nom::sequence::tuple((
                number::le_u64,
                number::le_u64,
                BlockPtr::parse,
                number::le_u64,
                number::le_u64,
                number::le_u64,
                nom::bytes::complete::take(3 * 8usize),
            ))(input)?;
        Ok((
            input,
            Self {
                claim_txg,
                replay_seq,
                log,
                claim_block_seq,
                flags,
                claim_lr_seq,
            },
        ))
    }
}

#[derive(Debug, Clone)]
pub struct ZilTrailer {
    pub next_blk: BlockPtr,
    pub nused: u64,
    pub bt: ZioBlockTail,
}

impl ZilTrailer {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (next_blk, nused, bt)) =
            nom::sequence::tuple((BlockPtr::parse, number::le_u64, ZioBlockTail::parse))(input)?;
        Ok((
            input,
            Self {
                next_blk,
                nused,
                bt,
            },
        ))
    }
}

#[derive(Debug, Clone)]
pub struct ZilLogRecord {
    pub txtype: u64,
    pub reclen: u64,
    pub txg: u64,
    pub seq: u64,
}

impl ZilLogRecord {
    pub fn parse(input: &[u8]) -> IResult<&[u8], Self> {
        let (input, (txtype, reclen, txg, seq)) = nom::sequence::tuple((
            number::le_u64,
            number::le_u64,
            number::le_u64,
            number::le_u64,
        ))(input)?;
        Ok((
            input,
            Self {
                txtype,
                reclen,
                txg,
                seq,
            },
        ))
    }
}
