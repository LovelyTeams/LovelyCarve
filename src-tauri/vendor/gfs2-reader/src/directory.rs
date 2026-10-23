//! Stuffed linear directories and exhash tables. Referenced leaves, collision
//! chains, directory counts, hashes and repeated bucket spans are validated.
use crate::{
    disk_hash, header::Header, inode::Inode, meta, require, stream::Stream, u16be, u32be, u64be,
    Inum,
};
use lamfold::{BlockSource, FileKind, FoldError, Result};
use std::collections::{HashMap, HashSet};

pub(crate) struct Entry {
    pub name: String,
    pub number: Inum,
    pub kind: FileKind,
}
pub(crate) struct Directory {
    pub entries: Vec<Entry>,
    pub leaves: HashSet<u64>,
}
impl Directory {
    pub fn read(
        source: &mut impl BlockSource,
        header: &Header,
        inode: &Inode,
        stream: &Stream,
    ) -> Result<Self> {
        require(
            inode.kind() == FileKind::Directory && inode.entries <= 1_000_000,
            "gfs2: directory kind/count",
        )?;
        let mut output = Self {
            entries: Vec::new(),
            leaves: HashSet::new(),
        };
        if inode.flags & 2 == 0 {
            require(
                inode.height == 0 && inode.depth == 0,
                "gfs2: linear directory must be stuffed",
            )?;
            output.records(&inode.raw[232..], None)?;
        } else {
            let count = 1usize << inode.depth;
            require(
                inode.size == count as u64 * 8,
                "gfs2: exhash table size/depth",
            )?;
            let mut table = vec![0; count * 8];
            require(
                stream.read(source, header, 0, &mut table)? == table.len(),
                "gfs2: short exhash table",
            )?;
            let mut buckets = HashMap::<u64, (usize, usize)>::new();
            for (index, raw) in table.as_chunks::<8>().0.iter().enumerate() {
                let block = u64be(raw, 0)?;
                require(block != 0, "gfs2: missing exhash leaf")?;
                let bucket = buckets.entry(block).or_insert((index, 0));
                require(
                    index == bucket.0 + bucket.1,
                    "gfs2: disjoint repeated leaf buckets",
                )?;
                bucket.1 += 1;
            }
            for (block, (first, repeats)) in buckets {
                let mut next = block;
                let mut chain = 0;
                while next != 0 {
                    require(
                        output.leaves.len() < 65536 && output.leaves.insert(next),
                        "gfs2: leaf cycle/alias/work limit",
                    )?;
                    let raw = header.read_block(source, next)?;
                    meta(&raw, 6, 600)?;
                    let depth = u16be(&raw, 24)?;
                    require(
                        depth <= inode.depth
                            && repeats == 1usize << (inode.depth - depth)
                            && first % repeats == 0,
                        "gfs2: leaf depth/bucket span",
                    )?;
                    require(
                        u32be(&raw, 28)? == 1200,
                        "gfs2: leaf directory-entry format",
                    )?;
                    let owner = u64be(&raw, 40)?;
                    require(
                        owner == 0 || owner == inode.number.address,
                        "gfs2: leaf owner identity",
                    )?;
                    let start = output.entries.len();
                    output.records(&raw[104..], Some((inode.depth, first, repeats)))?;
                    require(
                        output.entries.len() - start == u16be(&raw, 26)? as usize,
                        "gfs2: leaf entry count",
                    )?;
                    next = u64be(&raw, 32)?;
                    chain += 1;
                    require(chain <= 65536, "gfs2: leaf collision-chain limit")?;
                }
            }
        }
        require(
            output.entries.len() == inode.entries as usize,
            "gfs2: directory entry total",
        )?;
        let mut names = HashSet::new();
        for entry in &output.entries {
            require(
                names.insert(entry.name.clone()),
                "gfs2: duplicate directory name",
            )?;
        }
        let own = output
            .entries
            .iter()
            .find(|entry| entry.name == ".")
            .ok_or(FoldError::Corrupt("gfs2: missing dot entry"))?;
        require(
            own.number == inode.number && own.kind == FileKind::Directory,
            "gfs2: dot identity",
        )?;
        let parent = output
            .entries
            .iter()
            .find(|entry| entry.name == "..")
            .ok_or(FoldError::Corrupt("gfs2: missing parent entry"))?;
        require(parent.kind == FileKind::Directory, "gfs2: parent kind")?;
        Ok(output)
    }
    fn records(&mut self, raw: &[u8], bucket: Option<(u16, usize, usize)>) -> Result<()> {
        let mut position = 0;
        while position < raw.len() {
            let record = raw
                .get(position..)
                .ok_or(FoldError::Corrupt("gfs2: record offset"))?;
            let length = u16be(record, 20)? as usize;
            let name_length = u16be(record, 22)? as usize;
            require(
                length >= 40 && length.is_multiple_of(8) && length <= record.len(),
                "gfs2: directory record length",
            )?;
            let number = Inum::parse(record, 0)?;
            if number.address == 0 || number.formal == 0 {
                require(
                    position == 0 && number.address == 0 && number.formal == 0,
                    "gfs2: invalid directory sentinel",
                )?;
            } else {
                require(
                    name_length != 0 && name_length <= 255 && 40 + name_length <= length,
                    "gfs2: directory name length",
                )?;
                let name_bytes = &record[40..40 + name_length];
                require(
                    !name_bytes.iter().any(|byte| matches!(byte, 0 | b'/')),
                    "gfs2: invalid name component",
                )?;
                let hash = disk_hash(name_bytes);
                require(hash == u32be(record, 16)?, "gfs2: directory name hash")?;
                if let Some((depth, first, repeats)) = bucket {
                    let index = if depth == 0 {
                        0
                    } else {
                        hash as usize >> (32 - depth)
                    };
                    require(
                        (first..first + repeats).contains(&index),
                        "gfs2: name outside exhash bucket",
                    )?;
                }
                let name = String::from_utf8(name_bytes.to_vec())
                    .map_err(|_| FoldError::Unsupported("gfs2: non-UTF-8 directory name"))?;
                let kind = match u16be(record, 24)? {
                    4 => FileKind::Directory,
                    8 => FileKind::Regular,
                    10 => FileKind::Symlink,
                    1 | 2 | 6 | 12 => FileKind::Other,
                    _ => return Err(FoldError::Corrupt("gfs2: directory entry type")),
                };
                require(
                    self.entries.len() < 1_000_000,
                    "gfs2: directory entry work limit",
                )?;
                self.entries.push(Entry { name, number, kind });
            }
            position += length;
        }
        Ok(())
    }
}
