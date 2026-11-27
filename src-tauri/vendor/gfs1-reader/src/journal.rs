use lamfold::{BlockSource, FoldError, Result};

use crate::allocation::Allocation;
use crate::format::{Header, meta, require, u32be, u64be};

pub(crate) const INDEX_RECORD: usize = 80;
pub(crate) const INDEX_LIMIT: usize = 256;
const HEAD_BYTES: usize = 128;
const MAX_HEADERS: u64 = 1_048_576;

pub(crate) fn validate(
    source: &mut impl BlockSource,
    header: &Header,
    allocation: &Allocation,
    index: &[u8],
) -> Result<()> {
    require(
        !index.is_empty()
            && index.len().is_multiple_of(INDEX_RECORD)
            && index.len() / INDEX_RECORD <= INDEX_LIMIT,
        "invalid GFS1 journal index length",
    )?;
    let segment = u64::from(header.segment_blocks);
    let mut journals = Vec::with_capacity(index.len() / INDEX_RECORD);
    let mut work = 0;
    for record in index.as_chunks::<INDEX_RECORD>().0 {
        let start = u64be(record, 0)?;
        let segments = u64::from(u32be(record, 8)?);
        header.block_offset(start)?;
        work += segments;
        let end = start
            .checked_add(segments * segment)
            .ok_or(FoldError::Corrupt("GFS1 journal address overflow"))?;
        require(
            segments > 0
                && work <= MAX_HEADERS
                && end <= header.source_blocks
                && allocation.disjoint(start, end),
            "invalid or excessive GFS1 journal geometry",
        )?;
        journals.push((start, end));
    }
    journals.sort_unstable();
    require(
        journals.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "overlapping GFS1 journals",
    )?;
    for (start, end) in journals {
        let mut head = None;
        let mut raw = [0; 512];
        for block in (start..end).step_by(segment as usize) {
            source.read_at(header.block_offset(block)?, &mut raw)?;
            let first = &raw[..HEAD_BYTES];
            meta(first, 8, 800)?;
            require(
                first == &raw[512 - HEAD_BYTES..],
                "inconsistent GFS1 journal header copies",
            )?;
            let transaction = u64be(first, 40)?;
            let begin = u64be(first, 32)?;
            let clean = u32be(first, 24)? & 1 != 0;
            require(
                (start..end).contains(&begin) && (begin - start).is_multiple_of(segment),
                "GFS1 log pointer outside journal segments",
            )?;
            match head {
                None => head = Some((transaction, begin, clean, block == begin)),
                Some((newest, _, _, _)) if transaction > newest => {
                    head = Some((transaction, begin, clean, block == begin))
                }
                Some((newest, first_block, was_clean, seen)) if transaction == newest => {
                    require(
                        begin == first_block && clean == was_clean,
                        "inconsistent GFS1 journal transaction",
                    )?;
                    head = Some((transaction, begin, clean, seen || block == begin));
                }
                _ => {}
            }
        }
        let (_, _, clean, found) = head.ok_or(FoldError::Corrupt("missing GFS1 log header"))?;
        require(found, "GFS1 journal is missing the latest transaction head")?;
        if !clean {
            return Err(FoldError::Unsupported("GFS1 journal requires replay"));
        }
    }
    Ok(())
}
