//! The fileset inode map is an ordinary xtree-backed file. IAGs contain 128
//! bitmap words, each describing the 32 inodes of the corresponding extent.
use crate::{
    allocation::Allocation,
    exact, le32, le64,
    metadata::{Header, Physical},
    require,
    xtree::Map,
};
use lamfold::{BlockSource, FileKind, FileTimes, FoldError, Result};

#[derive(Clone)]
pub(crate) struct Inode {
    pub number: u32,
    pub raw: [u8; 512],
    pub length: u64,
    pub blocks: u64,
    pub mode: u32,
    pub attributes: Vec<Physical>,
}
impl Inode {
    pub fn decode(
        raw: [u8; 512],
        number: u32,
        fileset: u32,
        stamp: u32,
        header: &Header,
    ) -> Result<Self> {
        require(
            le32(&raw, 0)? == stamp
                && le32(&raw, 4)? == fileset
                && le32(&raw, 8)? == number
                && le32(&raw, 40)? > 0,
            "jfs: inode stamp/fileset/number/links",
        )?;
        let length = le64(&raw, 24)?;
        require(
            length <= (1u64 << 40) * u64::from(header.block_size),
            "jfs: file length exceeds 40-bit logical block address space",
        )?;
        let mode = le32(&raw, 52)?;
        let blocks = le64(&raw, 32)?;
        require(blocks <= header.blocks, "jfs: inode allocated block count")?;
        let mut attributes = Vec::new();
        for offset in [88, 104] {
            match raw[offset] {
                0 => {}
                0x20 => {
                    let extent = Physical::parse(&raw, offset + 8, header)?;
                    let size = u64::from(le32(&raw, offset + 4)?);
                    require(
                        size > 0 && size <= extent.length(header),
                        "jfs: external EA/ACL length",
                    )?;
                    attributes.push(extent);
                }
                0x40 => require(le32(&raw, offset + 4)? <= 128, "jfs: inline EA/ACL length")?,
                _ => {
                    return Err(FoldError::Unsupported(
                        "jfs: indexed/file/corrupt EA/ACL descriptor",
                    ))
                }
            }
        }
        let inode = Self {
            number,
            raw,
            length,
            blocks,
            mode,
            attributes,
        };
        inode.times()?;
        Ok(inode)
    }
    pub fn kind(&self) -> FileKind {
        match self.mode & 0xf000 {
            0x4000 => FileKind::Directory,
            0x8000 => FileKind::Regular,
            0xa000 => FileKind::Symlink,
            _ => FileKind::Other,
        }
    }
    pub fn times(&self) -> Result<FileTimes> {
        let time = |offset| -> Result<Option<(i64, u32)>> {
            let nanos = le32(&self.raw, offset + 4)?;
            require(nanos < 1_000_000_000, "jfs: timestamp nanoseconds")?;
            // Linux expands the on-disk unsigned seconds; casting to i32
            // would incorrectly wrap dates after 2038 back into the past.
            Ok(Some((i64::from(le32(&self.raw, offset)?), nanos)))
        };
        Ok(FileTimes {
            accessed: time(56)?,
            changed: time(64)?,
            modified: time(72)?,
            created: time(80)?,
        })
    }
    pub fn inline_link(&self) -> Result<Option<&[u8]>> {
        if self.kind() != FileKind::Symlink || self.length >= 256 {
            return Ok(None);
        }
        let length = self.length as usize;
        require(
            self.raw[256 + length] == 0 && !self.raw[256..256 + length].contains(&0),
            "jfs: inline symlink termination",
        )?;
        require(
            length < 128 || self.mode & 0x40000 == 0 && self.raw[104] & 0x40 == 0,
            "jfs: inline symlink/EA overlap",
        )?;
        Ok(Some(&self.raw[256..256 + length]))
    }
    pub fn data_map(&self, source: &mut impl BlockSource, header: &Header) -> Result<Map> {
        require(
            matches!(self.kind(), FileKind::Regular | FileKind::Symlink),
            "jfs: inode has no regular content",
        )?;
        if self.inline_link()?.is_some() {
            return Ok(Map::default());
        }
        if self.mode & 0x40000 != 0 {
            require(
                crate::le16(&self.raw, 244)? <= 10,
                "jfs: extent root overlaps inline EA area",
            )?;
        }
        Map::parse(source, header, &self.raw[224..512])
    }
    pub fn verify_allocation(
        &self,
        source: &mut impl BlockSource,
        header: &Header,
        allocation: &mut Allocation,
        extents: impl Iterator<Item = Physical>,
    ) -> Result<()> {
        let mut extents: Vec<_> = extents.chain(self.attributes.iter().copied()).collect();
        extents.sort_by_key(|extent| extent.start);
        let count: u64 = extents.iter().map(|extent| u64::from(extent.count)).sum();
        require(
            count == self.blocks,
            "jfs: inode allocation/extent count mismatch",
        )?;
        for pair in extents.windows(2) {
            require(
                pair[0].start + u64::from(pair[0].count) <= pair[1].start,
                "jfs: overlapping inode allocations",
            )?;
        }
        for extent in extents {
            allocation.check(source, header, extent)?;
        }
        Ok(())
    }
}

pub(crate) struct Inodes {
    map: Map,
    length: u64,
    stamp: u32,
    groups: u32,
    cache: Vec<(u32, Box<[u8; 4096]>)>,
}
impl Inodes {
    pub fn open(
        source: &mut impl BlockSource,
        header: &Header,
        inode: Inode,
        map: Map,
        allocation: &mut Allocation,
    ) -> Result<Self> {
        require(
            inode.length >= 8192 && inode.length.is_multiple_of(4096),
            "jfs: fileset inode-map length",
        )?;
        inode.verify_allocation(
            source,
            header,
            allocation,
            map.pages
                .iter()
                .copied()
                .chain(map.runs.iter().map(|run| run.physical)),
        )?;
        let mut control = [0; 4096];
        map.exact(source, header, inode.length, 0, &mut control)?;
        let groups = le32(&control, 4)?;
        require(
            groups > 0
                && groups <= 1 << 20
                && (u64::from(groups) + 1) * 4096 <= inode.length
                && le32(&control, 16)? == 16384 / header.block_size
                && le32(&control, 20)? == (16384 / header.block_size).ilog2(),
            "jfs: inode-map control geometry",
        )?;
        Ok(Self {
            map,
            length: inode.length,
            stamp: le32(&inode.raw, 0)?,
            groups,
            cache: Vec::new(),
        })
    }
    pub fn read(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        allocation: &mut Allocation,
        number: u32,
    ) -> Result<Inode> {
        let group = number / 4096;
        require(group < self.groups, "jfs: inode beyond fileset IAG count")?;
        let cached = if let Some(index) = self.cache.iter().position(|(cached, _)| *cached == group)
        {
            index
        } else {
            let mut page = Box::new([0; 4096]);
            self.map.exact(
                source,
                header,
                self.length,
                (u64::from(group) + 1) * 4096,
                page.as_mut(),
            )?;
            require(
                le32(page.as_ref(), 8)? == group && page[2048..2560] == page[2560..3072],
                "jfs: IAG number/working-persistent inode bitmap mismatch",
            )?;
            if self.cache.len() == 64 {
                self.cache.clear();
            }
            self.cache.push((group, page));
            self.cache.len() - 1
        };
        let page = self.cache[cached].1.as_ref();
        let slot = ((number % 4096) / 32) as usize;
        require(
            le32(page, 2560 + slot * 4)? & (0x80000000 >> (number % 32)) != 0,
            "jfs: unallocated fileset inode",
        )?;
        let extent = Physical::parse(page, 3072 + slot * 8, header)?;
        require(extent.length(header) == 16384, "jfs: inode extent length")?;
        allocation.check(source, header, extent)?;
        let mut raw = [0; 512];
        exact(
            source,
            extent.byte(header) + u64::from(number % 32) * 512,
            &mut raw,
        )?;
        require(
            Physical::parse(&raw, 16, header)? == extent,
            "jfs: inode extent identity",
        )?;
        Inode::decode(raw, number, 16, self.stamp, header)
    }
}
