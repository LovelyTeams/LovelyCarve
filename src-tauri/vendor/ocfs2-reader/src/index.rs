//! Validate the secondary directory index against authoritative dirents.
//! Native char signedness has no recorded version bit; both TEA byte variants
//! are checked consistently for a whole directory, without changing names.
use crate::{
    allocation::Allocation,
    directory::Directory,
    extents::{Root, Tree},
    header::{Header, Record},
    inode::Inode,
    le16, le32, le64, require,
};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::HashMap;

#[derive(Clone, Copy)]
enum Characters {
    Signed,
    Unsigned,
}
fn hash(header: &Header, name: &str, characters: Characters) -> (u32, u32) {
    if matches!(name, "." | "..") {
        return (0, 0);
    }
    let mut left = header.directory_seed[0];
    let mut right = header.directory_seed[1];
    let raw = name.as_bytes();
    for start in (0..raw.len()).step_by(16) {
        let remaining = raw.len() - start;
        let padding = (remaining as u32).wrapping_mul(0x01010101);
        let mut words = [padding; 4];
        for (index, byte) in raw[start..raw.len().min(start + 16)].iter().enumerate() {
            let value = match characters {
                Characters::Signed => *byte as i8 as i32 as u32,
                Characters::Unsigned => *byte as u32,
            };
            words[index / 4] = (words[index / 4] << 8).wrapping_add(value);
        }
        let mut sum = 0u32;
        let mut first = left;
        let mut second = right;
        for _ in 0..16 {
            sum = sum.wrapping_add(0x9e3779b9);
            first = first.wrapping_add(
                (second << 4).wrapping_add(words[0])
                    ^ second.wrapping_add(sum)
                    ^ (second >> 5).wrapping_add(words[1]),
            );
            second = second.wrapping_add(
                (first << 4).wrapping_add(words[2])
                    ^ first.wrapping_add(sum)
                    ^ (first >> 5).wrapping_add(words[3]),
            );
        }
        left = left.wrapping_add(first);
        right = right.wrapping_add(second);
    }
    (left, right)
}
type Counts = HashMap<(u32, u32, u64), usize>;
fn entries(
    raw: &[u8],
    offset: usize,
    output: &mut Counts,
    directory: &Directory,
    range: Option<(u64, u64, u32, u32)>,
) -> Result<usize> {
    let capacity = le16(raw, offset + 4)? as usize;
    let used = le16(raw, offset + 6)? as usize;
    require(
        used <= capacity && offset + 8 + capacity * 16 <= raw.len(),
        "ocfs2: directory index entry capacity",
    )?;
    for index in 0..used {
        let record = &raw[offset + 8 + index * 16..offset + 24 + index * 16];
        let major = le32(record, 0)?;
        let minor = le32(record, 4)?;
        let block = le64(record, 8)?;
        require(
            directory.blocks.contains(&block),
            "ocfs2: directory index points outside directory data",
        )?;
        if let Some((lower, upper, ordinal, mask)) = range {
            require(
                lower <= major as u64 && (major as u64) < upper && minor & mask == ordinal,
                "ocfs2: directory hash outside index bucket",
            )?;
        }
        *output.entry((major, minor, block)).or_default() += 1;
        require(
            output.len() <= 1_000_000,
            "ocfs2: directory index work limit",
        )?;
    }
    Ok(used)
}
pub(crate) fn validate(
    source: &mut impl BlockSource,
    header: &Header,
    inode: &Inode,
    directory: &Directory,
    allocation: Option<&Allocation>,
) -> Result<()> {
    if inode.dynamic & 8 == 0 {
        return Ok(());
    }
    let raw = header.record(source, inode.directory_index, Record::IndexRoot)?;
    require(
        le64(&raw, 48)? == inode.address && raw[44] & !1 == 0,
        "ocfs2: directory index owner/flags",
    )?;
    if let Some(allocation) = allocation {
        allocation.check_raw(header, inode.directory_index, Record::IndexRoot, &raw)?;
    }
    let expected_count = le32(&raw, 56)? as usize;
    require(
        expected_count == directory.entries.len(),
        "ocfs2: directory index/name count",
    )?;
    let mut actual = Counts::new();
    let mut count = 0;
    if raw[44] & 1 != 0 {
        require(
            le32(&raw, 40)? == 0 && le64(&raw, 32)? == 0,
            "ocfs2: inline directory index allocation",
        )?;
        count += entries(&raw, 192, &mut actual, directory, None)?;
    } else {
        let tree = Tree::parse(
            source,
            header,
            Root {
                raw: &raw,
                offset: 192,
                end: raw.len(),
                last_leaf: le64(&raw, 32)?,
            },
        )?;
        require(
            tree.clusters == le32(&raw, 40)? as u64,
            "ocfs2: directory index cluster count",
        )?;
        if let Some(allocation) = allocation {
            for block in &tree.metadata {
                allocation.check_record(source, header, *block, Record::Extent)?;
            }
        }
        let per_cluster = header.cluster_size / header.block_size;
        require(
            tree.clusters * per_cluster as u64 <= 131072,
            "ocfs2: directory index leaf work limit",
        )?;
        for (index, extent) in tree.extents.iter().enumerate() {
            require(
                extent.flags == 0 && extent.length == header.cluster_size as u64,
                "ocfs2: directory index cluster layout",
            )?;
            if let Some(allocation) = allocation {
                allocation.check_data(header, extent.physical, extent.length)?;
            }
            let lower = extent.logical / header.cluster_size as u64;
            let upper = tree
                .extents
                .get(index + 1)
                .map_or(1u64 << 32, |next| next.logical / header.cluster_size as u64);
            for ordinal in 0..per_cluster {
                let block = extent.physical / header.block_size as u64 + ordinal as u64;
                let raw = header.record(source, block, Record::IndexLeaf)?;
                count += entries(
                    &raw,
                    40,
                    &mut actual,
                    directory,
                    Some((lower, upper, ordinal, per_cluster - 1)),
                )?;
                require(count <= 1_000_000, "ocfs2: directory index entry limit")?;
            }
        }
    }
    require(
        count == expected_count,
        "ocfs2: directory index entry total",
    )?;
    for characters in [Characters::Signed, Characters::Unsigned] {
        let mut expected = Counts::new();
        for entry in &directory.entries {
            let (major, minor) = hash(header, &entry.name, characters);
            *expected.entry((major, minor, entry.block)).or_default() += 1;
        }
        if expected == actual {
            return Ok(());
        }
    }
    Err(FoldError::Corrupt(
        "ocfs2: directory index hashes do not match names",
    ))
}
