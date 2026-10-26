//! Validate fixed 512-byte classic HFS B-trees, including the allocation map,
//! rooted index topology, leaf links, record bounds, key order and counts.
use crate::{be16, be32, names, require};
use lamfold::{FoldError, Result};
use std::{cmp::Ordering, collections::HashSet};

#[derive(Clone, Copy)]
pub(crate) enum TreeKind {
    Catalog,
    Extents,
}
pub(crate) struct Node<'a> {
    pub next: u32,
    pub previous: u32,
    pub kind: u8,
    pub height: u8,
    pub records: Vec<&'a [u8]>,
}
pub(crate) fn node(bytes: &[u8], number: u32) -> Result<Node<'_>> {
    let start = number as usize * 512;
    let bytes = bytes
        .get(start..start + 512)
        .ok_or(FoldError::Corrupt("hfs: B-tree node outside available map"))?;
    let count = usize::from(be16(bytes, 10)?);
    require(count <= 248, "hfs: B-tree record count")?;
    let table = 512 - 2 * (count + 1);
    let mut offsets = Vec::with_capacity(count + 1);
    for index in 0..=count {
        let offset = usize::from(be16(bytes, 510 - index * 2)?);
        require(
            offset >= 14 && offset <= table && offset.is_multiple_of(2),
            "hfs: B-tree record offset",
        )?;
        if let Some(previous) = offsets.last() {
            require(*previous < offset, "hfs: B-tree record offset order")?;
        }
        offsets.push(offset);
    }
    Ok(Node {
        next: be32(bytes, 0)?,
        previous: be32(bytes, 4)?,
        kind: bytes[8],
        height: bytes[9],
        records: offsets
            .windows(2)
            .map(|pair| &bytes[pair[0]..pair[1]])
            .collect(),
    })
}
#[derive(Clone)]
pub(crate) struct Header {
    pub depth: u16,
    pub root: u32,
    pub records: u32,
    pub first: u32,
    pub last: u32,
    pub nodes: u32,
    free: u32,
}
pub(crate) fn header(bytes: &[u8], kind: TreeKind, logical: usize) -> Result<Header> {
    let node = node(bytes, 0)?;
    require(
        node.kind == 1 && node.height == 0 && node.previous == 0 && node.records.len() == 3,
        "hfs: B-tree header node",
    )?;
    let record = node.records[0];
    require(
        record.len() >= 106 && be16(record, 18)? == 512,
        "hfs: B-tree header length/node size",
    )?;
    let max_key = match kind {
        TreeKind::Catalog => 37,
        TreeKind::Extents => 7,
    };
    require(
        be16(record, 20)? == max_key,
        "hfs: B-tree maximum key length",
    )?;
    let result = Header {
        depth: be16(record, 0)?,
        root: be32(record, 2)?,
        records: be32(record, 6)?,
        first: be32(record, 10)?,
        last: be32(record, 14)?,
        nodes: be32(record, 22)?,
        free: be32(record, 26)?,
    };
    require(
        result.nodes as u64 * 512 == logical as u64
            && result.nodes > 0
            && result.free < result.nodes
            && result.depth <= 32,
        "hfs: B-tree file size/depth/node count",
    )?;
    for number in [result.root, result.first, result.last] {
        require(number < result.nodes, "hfs: B-tree header node reference")?;
    }
    require(
        if result.records == 0 {
            result.depth == 0 && result.root == 0 && result.first == 0 && result.last == 0
        } else {
            result.depth > 0 && result.root > 0 && result.first > 0 && result.last > 0
        },
        "hfs: empty/nonempty tree header",
    )?;
    Ok(result)
}
pub(crate) fn split(record: &[u8], kind: TreeKind) -> Result<(&[u8], &[u8])> {
    let length = usize::from(
        *record
            .first()
            .ok_or(FoldError::Corrupt("hfs: empty keyed record"))?,
    );
    let padded = (length + 2) & !1;
    require(padded <= record.len(), "hfs: B-tree key length")?;
    let key = &record[1..1 + length];
    match kind {
        TreeKind::Extents => require(length == 7 && matches!(key[0], 0 | 0xff), "hfs: extent key")?,
        TreeKind::Catalog => {
            require((6..=37).contains(&length), "hfs: catalog key length")?;
            require(
                key[0] == 0 && key[5] <= 31 && 6 + usize::from(key[5]) <= length,
                "hfs: catalog key name",
            )?;
        }
    }
    Ok((key, &record[padded..]))
}
pub(crate) fn compare(a: &[u8], b: &[u8], kind: TreeKind) -> Result<Ordering> {
    Ok(match kind {
        TreeKind::Extents => be32(a, 1)?
            .cmp(&be32(b, 1)?)
            .then_with(|| a[0].cmp(&b[0]))
            .then(be16(a, 5)?.cmp(&be16(b, 5)?)),
        TreeKind::Catalog => be32(a, 1)?.cmp(&be32(b, 1)?).then_with(|| {
            names::compare(&a[6..6 + usize::from(a[5])], &b[6..6 + usize::from(b[5])])
        }),
    })
}
pub(crate) fn leaves(bytes: &[u8], kind: TreeKind) -> Result<Vec<(&[u8], &[u8])>> {
    let header = header(bytes, kind, bytes.len())?;
    let head_node = node(bytes, 0)?;
    let mut bitmap = head_node.records[2].to_vec();
    let mut maps = HashSet::from([0]);
    let mut current = head_node.next;
    let mut previous = 0;
    while current != 0 {
        require(
            current < header.nodes && maps.insert(current),
            "hfs: B-tree map chain cycle/reference",
        )?;
        let map = node(bytes, current)?;
        require(
            map.kind == 2 && map.height == 0 && map.previous == previous && map.records.len() == 1,
            "hfs: B-tree map node",
        )?;
        bitmap.extend_from_slice(map.records[0]);
        previous = current;
        current = map.next;
    }
    require(
        bitmap.len() * 8 >= header.nodes as usize,
        "hfs: short B-tree allocation map",
    )?;
    let allocated = |number: u32| bitmap[number as usize / 8] & (0x80 >> (number % 8)) != 0;
    require(
        (0..header.nodes)
            .filter(|number| !allocated(*number))
            .count()
            == header.free as usize,
        "hfs: B-tree free node count",
    )?;
    for number in &maps {
        require(
            allocated(*number),
            "hfs: unallocated B-tree map/header node",
        )?;
    }
    let mut seen = maps;
    let mut stack = if header.root == 0 {
        Vec::new()
    } else {
        vec![(header.root, header.depth as u8, None::<Vec<u8>>)]
    };
    let mut leaf_numbers = Vec::new();
    let mut records = Vec::new();
    while let Some((number, height, separator)) = stack.pop() {
        require(
            number < header.nodes && seen.insert(number) && allocated(number),
            "hfs: B-tree cycle/shared/unallocated child",
        )?;
        let current = node(bytes, number)?;
        require(
            current.height == height
                && !current.records.is_empty()
                && current.kind == if height == 1 { 0xff } else { 0 },
            "hfs: B-tree node kind/height",
        )?;
        let entries = current
            .records
            .iter()
            .map(|record| split(record, kind))
            .collect::<Result<Vec<_>>>()?;
        if let Some(separator) = separator {
            require(
                compare(&separator, entries[0].0, kind)? == Ordering::Equal,
                "hfs: index separator/child key mismatch",
            )?;
        }
        for pair in entries.windows(2) {
            require(
                compare(pair[0].0, pair[1].0, kind)? == Ordering::Less,
                "hfs: unordered/duplicate B-tree keys",
            )?;
        }
        if height == 1 {
            leaf_numbers.push(number);
            records.extend(entries);
        } else {
            for (key, value) in entries.into_iter().rev() {
                require(value.len() >= 4, "hfs: short index child pointer")?;
                stack.push((be32(value, 0)?, height - 1, Some(key.to_vec())));
            }
        }
    }
    require(
        records.len() == header.records as usize,
        "hfs: B-tree leaf record count",
    )?;
    if !leaf_numbers.is_empty() {
        require(
            leaf_numbers.first() == Some(&header.first)
                && leaf_numbers.last() == Some(&header.last),
            "hfs: first/last leaf mismatch",
        )?;
    }
    for (index, number) in leaf_numbers.iter().enumerate() {
        let leaf = node(bytes, *number)?;
        require(
            leaf.previous
                == index
                    .checked_sub(1)
                    .map(|index| leaf_numbers[index])
                    .unwrap_or(0)
                && leaf.next == leaf_numbers.get(index + 1).copied().unwrap_or(0),
            "hfs: broken leaf links",
        )?;
    }
    for pair in records.windows(2) {
        require(
            compare(pair[0].0, pair[1].0, kind)? == Ordering::Less,
            "hfs: unordered leaf chain",
        )?;
    }
    Ok(records)
}
