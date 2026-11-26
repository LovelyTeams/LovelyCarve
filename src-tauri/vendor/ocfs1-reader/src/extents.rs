use crate::{
    format::Allocation,
    le32, le64,
    object::{Entry, Extent},
    require, Header, SECTOR_BYTES,
};
use lamfold::{BlockSource, Result};
use std::collections::HashSet;

const SECTOR: u64 = SECTOR_BYTES as u64;
const MAX_GROUPS: usize = 65_536;

pub(crate) fn resolve(
    source: &mut impl BlockSource,
    header: &Header,
    bitmap: &mut Allocation,
    node: u64,
    file: &Entry,
) -> Result<Vec<Extent>> {
    let mut spans = Vec::new();
    if let Some(depth) = file.depth {
        let mut pending = Vec::new();
        for &branch in file.extents.iter().rev() {
            pending.push((node, depth, branch));
        }
        let mut groups = HashSet::new();
        let mut previous_next = None;
        let mut previous_leaf = 0;
        while let Some((parent, level, branch)) = pending.pop() {
            require(
                branch.physical.is_multiple_of(SECTOR)
                    && groups.len() < MAX_GROUPS
                    && groups.insert(branch.physical),
                "ocfs1: extent tree cycle, limit or invalid address",
            )?;
            bitmap.check(source, header, branch.physical, SECTOR)?;
            let mut raw = [0; SECTOR_BYTES];
            source.read_at(branch.physical, &mut raw)?;
            let (signature, record_type) = if level == 0 {
                (b"EXTDAT1\0", 1)
            } else {
                (b"EXTHDR2\0", 2)
            };
            require(
                raw.starts_with(signature)
                    && le32(&raw, 20)? == record_type
                    && le64(&raw, 32)? == branch.physical
                    && le64(&raw, 64)? == parent,
                "ocfs1: invalid extent group identity",
            )?;
            let count = le32(&raw, 8)? as usize;
            require(
                (1..=18).contains(&count),
                "ocfs1: invalid extent group count",
            )?;
            let records = Extent::parse(&raw, 72, count)?;
            let group_end = check_logical(&records, branch.logical)?;
            require(
                group_end == branch.end()?,
                "ocfs1: group does not cover parent range",
            )?;
            if level == 0 {
                require(
                    previous_next.is_none_or(|next| next == branch.physical),
                    "ocfs1: extent leaf links disagree with tree",
                )?;
                previous_leaf = branch.physical;
                previous_next = Some(le64(&raw, 40)?);
                spans.extend(records);
            } else {
                require(
                    le32(&raw, 24)? == u32::from(level - 1),
                    "ocfs1: extent group level",
                )?;
                pending.extend(
                    records
                        .into_iter()
                        .rev()
                        .map(|child| (branch.physical, level - 1, child)),
                );
            }
        }
        require(
            previous_leaf != 0 && previous_leaf == file.last_leaf && previous_next == Some(0),
            "ocfs1: extent tree last leaf",
        )?;
    } else {
        require(
            file.last_leaf == 0,
            "ocfs1: unexpected external extent pointer",
        )?;
        spans.extend_from_slice(&file.extents);
    }
    let end = check_logical(&spans, 0)?;
    require(
        end == file.allocation && end >= file.metadata.size,
        "ocfs1: file extent coverage",
    )?;
    let mut physical = Vec::with_capacity(spans.len());
    for span in &spans {
        require(
            span.physical.is_multiple_of(SECTOR),
            "ocfs1: unaligned physical extent",
        )?;
        bitmap.check(source, header, span.physical, span.bytes)?;
        physical.push((span.physical, span.physical + span.bytes));
    }
    physical.sort_unstable();
    require(
        physical.windows(2).all(|pair| pair[0].1 <= pair[1].0),
        "ocfs1: overlapping physical extents",
    )?;
    Ok(spans)
}

fn check_logical(extents: &[Extent], start: u64) -> Result<u64> {
    let mut end = start;
    for span in extents {
        require(
            span.logical == end && span.bytes > 0 && span.bytes.is_multiple_of(SECTOR),
            "ocfs1: logical extent coverage or alignment",
        )?;
        end = span.end()?;
    }
    Ok(end)
}
