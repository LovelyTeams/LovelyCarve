//! Native EA blocks, optional indirect lists and external ED value blocks.
//! Values stay raw; namespace prefixes are carried explicitly in each name.
use crate::{allocation::Allocation, header::Header, inode::Inode, meta, require, u32be, u64be};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct Attribute {
    pub name: String,
    pub bytes: Vec<u8>,
    pub resident: bool,
}
pub(crate) fn read(
    source: &mut impl BlockSource,
    header: &Header,
    inode: &Inode,
    allocation: &mut Allocation,
    mut used: HashSet<u64>,
) -> Result<Vec<Attribute>> {
    if inode.eattr == 0 {
        require(inode.flags & 8 == 0, "gfs2: indirect EA flag without block")?;
        return Ok(Vec::new());
    }
    let mut blocks = Vec::new();
    if inode.flags & 8 != 0 {
        require(used.insert(inode.eattr), "gfs2: EA indirect overlaps file")?;
        allocation.check(source, header, inode.eattr, false)?;
        let raw = header.read_block(source, inode.eattr)?;
        meta(&raw, 5, 500)?;
        let mut ended = false;
        for pointer in raw[24..].as_chunks::<8>().0 {
            let block = u64be(pointer, 0)?;
            if block == 0 {
                ended = true;
            } else {
                require(!ended, "gfs2: EA indirect pointer gap")?;
                blocks.push(block);
            }
        }
        require(!blocks.is_empty(), "gfs2: empty EA indirect")?;
    } else {
        blocks.push(inode.eattr);
    }
    let mut output = Vec::new();
    let mut names = HashSet::new();
    let mut total_bytes = 0usize;
    for block in blocks {
        require(used.insert(block), "gfs2: EA block alias")?;
        allocation.check(source, header, block, false)?;
        let raw = header.read_block(source, block)?;
        meta(&raw, 10, 1600)?;
        let mut offset = 24;
        let mut last = false;
        while !last {
            let record = raw
                .get(offset..)
                .ok_or(FoldError::Corrupt("gfs2: EA offset"))?;
            require(record.len() >= 16, "gfs2: short EA header")?;
            let length = u32be(record, 0)? as usize;
            let size = u32be(record, 4)? as usize;
            let name_length = record[8] as usize;
            let namespace = record[9];
            let flags = record[10];
            let pointers = record[11] as usize;
            require(
                length >= 16
                    && length.is_multiple_of(8)
                    && length <= record.len()
                    && flags & !1 == 0,
                "gfs2: EA record length/flags",
            )?;
            last = flags & 1 != 0;
            if namespace != 0 {
                require(
                    name_length != 0 && 16 + name_length <= length && size <= 65536,
                    "gfs2: EA name/value length",
                )?;
                let prefix = match namespace {
                    1 => "user.",
                    2 => "system.",
                    3 => "security.",
                    4 if header.format == 1802 => "trusted.",
                    _ => return Err(FoldError::Unsupported("gfs2: EA namespace/version")),
                };
                let name = String::from_utf8(record[16..16 + name_length].to_vec())
                    .map_err(|_| FoldError::Unsupported("gfs2: non-UTF-8 attribute name"))?;
                require(!name.contains('\0'), "gfs2: NUL in attribute name")?;
                let name = format!("{prefix}{name}");
                require(
                    names.insert(name.clone()) && output.len() < 65536,
                    "gfs2: duplicate/excessive EA names",
                )?;
                total_bytes = total_bytes
                    .checked_add(size)
                    .ok_or(FoldError::Corrupt("gfs2: EA bytes overflow"))?;
                require(
                    total_bytes <= 64 * 1024 * 1024,
                    "gfs2: total attribute work limit",
                )?;
                let mut bytes = Vec::with_capacity(size);
                if pointers == 0 {
                    require(
                        16 + name_length + size <= length,
                        "gfs2: inline EA value bounds",
                    )?;
                    bytes.extend_from_slice(&record[16 + name_length..16 + name_length + size]);
                } else {
                    let at = (16 + name_length).next_multiple_of(8);
                    require(
                        at + pointers * 8 <= length
                            && pointers == size.div_ceil(header.block_size as usize - 24),
                        "gfs2: EA pointer coverage",
                    )?;
                    for pointer in record[at..at + pointers * 8].as_chunks::<8>().0 {
                        let address = u64be(pointer, 0)?;
                        require(used.insert(address), "gfs2: EA value block alias")?;
                        allocation.check(source, header, address, false)?;
                        let value = header.read_block(source, address)?;
                        meta(&value, 11, 1700)?;
                        let take = (size - bytes.len()).min(value.len() - 24);
                        bytes.extend_from_slice(&value[24..24 + take]);
                    }
                }
                require(bytes.len() == size, "gfs2: incomplete EA value")?;
                output.push(Attribute {
                    name,
                    bytes,
                    resident: pointers == 0,
                });
            }
            offset += length;
            require(
                !last || offset == raw.len(),
                "gfs2: last EA does not cover block",
            )?;
        }
    }
    Ok(output)
}
