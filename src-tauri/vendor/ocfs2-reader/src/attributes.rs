//! Inode-local, flat-block and indexed-bucket xattrs, plus external value trees.
//! Names retain native namespaces and values remain binary.
use crate::{
    allocation::Allocation,
    extents::{Root, Stream, Tree},
    header::{Header, Record},
    inode::Inode,
    le16, le32, le64, require,
};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct Attribute {
    pub name: String,
    pub bytes: Vec<u8>,
    pub resident: bool,
}
struct Context<'a, S: BlockSource> {
    source: &'a mut S,
    header: &'a Header,
    allocation: &'a Allocation,
    metadata: HashSet<u64>,
    data: Vec<(u64, u64, u8)>,
    output: Vec<Attribute>,
    names: HashSet<String>,
    bytes: usize,
    references: Option<crate::refcounts::Refcounts>,
}
pub(crate) fn read(
    source: &mut impl BlockSource,
    header: &Header,
    inode: &Inode,
    allocation: &Allocation,
    file: &Tree,
) -> Result<Vec<Attribute>> {
    require(
        inode.dynamic & 4 == 0 || inode.dynamic & 2 != 0,
        "ocfs2: inline attributes without xattr flag",
    )?;
    require(
        inode.attribute_block == 0 || inode.dynamic & 2 != 0,
        "ocfs2: external attributes without xattr flag",
    )?;
    let references = crate::refcounts::Refcounts::load(source, header, inode, Some(allocation))?;
    let mut context = Context {
        source,
        header,
        allocation,
        metadata: file.metadata.clone(),
        data: file
            .extents
            .iter()
            .map(|extent| {
                (
                    extent.physical,
                    extent.physical + extent.length,
                    extent.flags,
                )
            })
            .collect(),
        output: Vec::new(),
        names: HashSet::new(),
        bytes: 0,
        references,
    };
    if let Some(references) = &context.references {
        context.metadata.extend(references.metadata.iter().copied());
    }
    context.metadata.insert(inode.address);
    if inode.inline_attributes != 0 {
        context.region(&inode.raw[inode.data_end()..], Region::Flat)?;
    }
    if inode.attribute_block != 0 {
        let raw = header.record(context.source, inode.attribute_block, Record::Xattr)?;
        context.metadata_block(inode.attribute_block, Record::Xattr, &raw)?;
        let flags = le16(&raw, 32)?;
        if flags & !1 != 0 {
            return Err(FoldError::Unsupported("ocfs2: xattr block flags"));
        }
        if flags & 1 == 0 {
            context.region(&raw[48..], Region::Flat)?;
        } else {
            let tree = Tree::parse(
                context.source,
                header,
                Root {
                    raw: &raw,
                    offset: 64,
                    end: raw.len(),
                    last_leaf: le64(&raw, 56)?,
                },
            )?;
            require(
                tree.clusters == le32(&raw, 48)? as u64,
                "ocfs2: xattr bucket tree cluster count",
            )?;
            context.tree(&tree)?;
            let mut total_buckets = 0;
            for (index, extent) in tree.extents.iter().enumerate() {
                require(
                    extent.flags == 0 && extent.length.is_multiple_of(4096),
                    "ocfs2: xattr bucket extent layout",
                )?;
                let mut raw = vec![0; 4096];
                crate::exact(context.source, extent.physical, &mut raw)?;
                let buckets = le16(&raw, 6)? as u64;
                require(
                    buckets != 0 && buckets <= extent.length / 4096 && buckets <= 65536,
                    "ocfs2: xattr bucket count",
                )?;
                total_buckets += buckets;
                require(
                    total_buckets <= 131072,
                    "ocfs2: total xattr bucket work limit",
                )?;
                let lower = extent.logical / header.cluster_size as u64;
                let upper = tree
                    .extents
                    .get(index + 1)
                    .map_or(1u64 << 32, |next| next.logical / header.cluster_size as u64);
                for bucket in 0..buckets {
                    if bucket != 0 {
                        crate::exact(context.source, extent.physical + bucket * 4096, &mut raw)?;
                    }
                    header.verify(&raw, 8)?;
                    context.region(&raw, Region::Bucket { lower, upper })?;
                }
            }
        }
    }
    context.data.sort_unstable_by_key(|range| range.0);
    let mut maximum = 0;
    let mut nonref = 0;
    for range in &mut context.data {
        require(
            range.0 >= if range.2 & 2 != 0 { nonref } else { maximum },
            "ocfs2: attribute/file physical overlap",
        )?;
        maximum = maximum.max(range.1);
        if range.2 & 2 == 0 {
            nonref = nonref.max(range.1);
        }
        range.1 = maximum;
    }
    for block in context.metadata {
        let at = block * header.block_size as u64;
        let index = context.data.partition_point(|range| range.0 <= at);
        require(
            index
                .checked_sub(1)
                .is_none_or(|index| context.data[index].1 <= at),
            "ocfs2: attribute metadata/data overlap",
        )?;
    }
    Ok(context.output)
}
#[derive(Clone, Copy)]
enum Region {
    Flat,
    Bucket { lower: u64, upper: u64 },
}
impl<S: BlockSource> Context<'_, S> {
    fn metadata_block(&mut self, block: u64, kind: Record, raw: &[u8]) -> Result<()> {
        require(
            self.metadata.len() < 131072 && self.metadata.insert(block),
            "ocfs2: attribute metadata cycle/alias/work limit",
        )?;
        self.allocation.check_raw(self.header, block, kind, raw)
    }
    fn tree(&mut self, tree: &Tree) -> Result<()> {
        crate::refcounts::Refcounts::check(self.references.as_ref(), self.header, tree)?;
        for block in &tree.metadata {
            let raw = self.header.record(self.source, *block, Record::Extent)?;
            self.metadata_block(*block, Record::Extent, &raw)?;
        }
        require(
            self.data.len() + tree.extents.len() <= 1_000_000,
            "ocfs2: total attribute extent work limit",
        )?;
        for extent in &tree.extents {
            self.allocation
                .check_data(self.header, extent.physical, extent.length)?;
            self.data.push((
                extent.physical,
                extent.physical + extent.length,
                extent.flags,
            ));
        }
        Ok(())
    }
    fn region(&mut self, raw: &[u8], region: Region) -> Result<()> {
        let count = le16(raw, 0)? as usize;
        let entry_end = 16 + count * 16;
        require(
            entry_end
                <= match region {
                    Region::Flat => raw.len(),
                    Region::Bucket { .. } => self.header.block_size as usize,
                },
            "ocfs2: attribute entry-array bounds",
        )?;
        let mut occupied = Vec::new();
        let mut previous_hash = 0;
        for index in 0..count {
            let record = &raw[16 + index * 16..32 + index * 16];
            let recorded_hash = le32(record, 0)?;
            let offset = le16(record, 4)? as usize;
            let name_length = record[6] as usize;
            let namespace = record[7] & 0x7f;
            let local = record[7] & 0x80 != 0;
            let size = le64(record, 8)?;
            require(
                size <= 65536 && offset >= entry_end && offset.is_multiple_of(4),
                "ocfs2: attribute name/value bounds",
            )?;
            let value_start = offset + name_length.next_multiple_of(4);
            let stored = if local { size as usize } else { 48 };
            let end = value_start
                .checked_add(stored)
                .ok_or(FoldError::Corrupt("ocfs2: attribute length overflow"))?;
            let limit = match region {
                Region::Flat => raw.len(),
                Region::Bucket { .. } => {
                    ((offset / self.header.block_size as usize) + 1)
                        * self.header.block_size as usize
                }
            };
            require(
                end <= raw.len() && end <= limit,
                "ocfs2: attribute name/value crosses storage boundary",
            )?;
            occupied.push((offset, end));
            let suffix_bytes = raw
                .get(offset..offset + name_length)
                .ok_or(FoldError::Corrupt("ocfs2: attribute name outside region"))?;
            require(!suffix_bytes.contains(&0), "ocfs2: NUL in attribute name")?;
            let mut unsigned = self.header.uuid_hash;
            let mut signed = unsigned;
            for byte in suffix_bytes {
                unsigned = unsigned.rotate_left(5) ^ *byte as u32;
                signed = signed.rotate_left(5) ^ (*byte as i8 as i32 as u32);
            }
            require(
                recorded_hash == unsigned || recorded_hash == signed,
                "ocfs2: attribute name hash",
            )?;
            if let Region::Bucket { lower, upper } = region {
                require(
                    (lower..upper).contains(&(recorded_hash as u64))
                        && (index == 0 || previous_hash <= recorded_hash),
                    "ocfs2: attribute bucket hash order/range",
                )?;
            }
            previous_hash = recorded_hash;
            let suffix = String::from_utf8(suffix_bytes.to_vec())
                .map_err(|_| FoldError::Unsupported("ocfs2: non-UTF-8 attribute name"))?;
            let name = match namespace {
                1 | 4 | 5 => {
                    require(!suffix.is_empty(), "ocfs2: empty attribute suffix")?;
                    let prefix = match namespace {
                        1 => "user.",
                        4 => "trusted.",
                        5 => "security.",
                        _ => return Err(FoldError::Corrupt("ocfs2: attribute namespace")),
                    };
                    format!("{prefix}{suffix}")
                }
                2 => {
                    require(suffix.is_empty(), "ocfs2: ACL attribute suffix")?;
                    "system.posix_acl_access".into()
                }
                3 => {
                    require(suffix.is_empty(), "ocfs2: ACL attribute suffix")?;
                    "system.posix_acl_default".into()
                }
                _ => return Err(FoldError::Unsupported("ocfs2: unknown attribute namespace")),
            };
            require(
                self.names.insert(name.clone()) && self.output.len() < 65536,
                "ocfs2: duplicate/excessive attributes",
            )?;
            self.bytes += size as usize;
            require(
                self.bytes <= 64 * 1024 * 1024,
                "ocfs2: attribute value memory limit",
            )?;
            let bytes = if local {
                raw[value_start..end].to_vec()
            } else {
                let value = &raw[value_start..end];
                let tree = Tree::parse(
                    self.source,
                    self.header,
                    Root {
                        raw: value,
                        offset: 16,
                        end: value.len(),
                        last_leaf: le64(value, 8)?,
                    },
                )?;
                require(
                    tree.clusters == le32(value, 0)? as u64,
                    "ocfs2: attribute value cluster count",
                )?;
                self.tree(&tree)?;
                let stream = Stream::external(tree, size)?;
                let mut bytes = vec![0; size as usize];
                require(
                    stream.read(self.source, 0, &mut bytes)? == bytes.len(),
                    "ocfs2: incomplete attribute value",
                )?;
                bytes
            };
            self.output.push(Attribute {
                name,
                bytes,
                resident: local,
            });
        }
        occupied.sort_unstable();
        for pair in occupied.windows(2) {
            require(
                pair[0].1 <= pair[1].0,
                "ocfs2: overlapping attribute storage",
            )?;
        }
        Ok(())
    }
}
