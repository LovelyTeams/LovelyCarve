use crate::{field, le16, le32, le64, require};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::VecDeque;

pub const SECTOR_BYTES: usize = 512;
const SECTOR: u64 = SECTOR_BYTES as u64;
pub(crate) const DIRECTORY_BYTES: u64 = 128 * 1024;

#[derive(Clone, Debug)]
pub struct Header {
    pub volume_bytes: u64,
    pub cluster_bytes: u64,
    pub clusters: u64,
    pub(crate) root: u64,
    pub(crate) root_mode: u32,
    pub(crate) data_start: u64,
    bitmap: u64,
    publish: u64,
    nodes: u64,
    exclusive_mount: u32,
}

impl Header {
    pub fn inspect(bytes: &[u8], source_size: u64) -> Result<Self> {
        require(
            bytes.get(8..18) == Some(b"OracleCFS\0"),
            "ocfs1: volume signature",
        )?;
        if le32(bytes, 4)? != 1 || le32(bytes, 0)? != 2 {
            return Err(FoldError::Unsupported("ocfs1: volume revision"));
        }
        let volume_bytes = le64(bytes, 272)?;
        let bitmap = le64(bytes, 288)?;
        let publish = le64(bytes, 296)?;
        let vote = le64(bytes, 304)?;
        let data_start = le64(bytes, 320)?;
        let root = le64(bytes, 336)?;
        let cluster_bytes = le64(bytes, 352)?;
        let nodes = le64(bytes, 360)?;
        let clusters = le64(bytes, 368)?;
        let dir_bytes = le64(bytes, 376)?;
        let file_bytes = le64(bytes, 384)?;
        require(
            le64(bytes, 280)? == 0
                && volume_bytes <= source_size
                && volume_bytes.is_multiple_of(SECTOR)
                && [bitmap, publish, vote, data_start, root]
                    .iter()
                    .all(|offset| offset.is_multiple_of(SECTOR))
                && matches!(dir_bytes, 0 | DIRECTORY_BYTES)
                && matches!(file_bytes, 0 | SECTOR)
                && (1..=32).contains(&nodes)
                && cluster_bytes.is_power_of_two()
                && cluster_bytes >= SECTOR
                && clusters > 0,
            "ocfs1: unsupported volume geometry",
        )?;
        let bitmap_bytes = clusters.div_ceil(8);
        let data_end = cluster_bytes
            .checked_mul(clusters)
            .and_then(|length| data_start.checked_add(length))
            .ok_or(FoldError::Corrupt("ocfs1: cluster range overflow"))?;
        require(
            publish >= 2 * SECTOR
                && publish
                    .checked_add(nodes * SECTOR)
                    .is_some_and(|end| end <= vote)
                && vote
                    .checked_add(nodes * SECTOR)
                    .is_some_and(|end| end <= bitmap)
                && bitmap
                    .checked_add(bitmap_bytes)
                    .is_some_and(|end| end <= data_start)
                && data_end <= volume_bytes
                && root >= data_start
                && root
                    .checked_add(DIRECTORY_BYTES)
                    .is_some_and(|end| end <= data_end),
            "ocfs1: out-of-bounds volume structures",
        )?;
        Ok(Self {
            volume_bytes,
            cluster_bytes,
            clusters,
            root,
            root_mode: 0o040000 | (le32(bytes, 424)? & 0o7777),
            data_start,
            bitmap,
            publish,
            nodes,
            exclusive_mount: le32(bytes, 436)?,
        })
    }

    pub fn read(source: &mut impl BlockSource) -> Result<Self> {
        let mut bytes = [0; SECTOR_BYTES];
        source.read_at(0, &mut bytes)?;
        Self::inspect(&bytes, source.len())
    }

    pub(crate) fn check_clean(&self, source: &mut impl BlockSource) -> Result<()> {
        if self.exclusive_mount != u32::MAX {
            return Err(FoldError::Unsupported("ocfs1: exclusive mount recorded"));
        }
        let mut bytes = [0; SECTOR_BYTES];
        for node in 0..self.nodes {
            source.read_at(self.publish + node * SECTOR, &mut bytes)?;
            if le32(&bytes, 12)? != 0 {
                return Err(FoldError::Unsupported("ocfs1: node requires recovery"));
            }
        }
        Ok(())
    }

    pub(crate) fn label(&self, source: &mut impl BlockSource) -> Result<(String, [u8; 16])> {
        let mut bytes = [0; SECTOR_BYTES];
        source.read_at(SECTOR, &mut bytes)?;
        let label_len = usize::from(le16(&bytes, 112)?);
        let id_len = le16(&bytes, 130)?;
        require(
            label_len <= 64 && id_len <= 16 && le16(&bytes, 196)? <= 64,
            "ocfs1: volume label geometry",
        )?;
        let label = String::from_utf8_lossy(&bytes[48..48 + label_len])
            .trim_end_matches('\0')
            .to_owned();
        Ok((label, field(&bytes, 114)?))
    }
}

#[derive(Default)]
pub(crate) struct Allocation {
    pages: VecDeque<(u64, [u8; SECTOR_BYTES])>,
}

impl Allocation {
    pub(crate) fn check(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        offset: u64,
        length: u64,
    ) -> Result<()> {
        require(
            length > 0 && offset >= header.data_start,
            "ocfs1: data outside allocation area",
        )?;
        let end = offset
            .checked_add(length)
            .ok_or(FoldError::Corrupt("ocfs1: data range overflow"))?;
        let first = (offset - header.data_start) / header.cluster_bytes;
        let stop = (end - header.data_start).div_ceil(header.cluster_bytes);
        require(stop <= header.clusters, "ocfs1: data beyond last cluster")?;
        let bits_per_sector = SECTOR * 8;
        let mut cluster = first;
        while cluster < stop {
            let page = cluster / bits_per_sector;
            let page_start = page * bits_per_sector;
            let page_end = stop.min(page_start + bits_per_sector);
            let data = self.page(source, header, page)?;
            for bit in (cluster - page_start) as usize..(page_end - page_start) as usize {
                require(
                    data[bit / 8] & (1 << (bit % 8)) != 0,
                    "ocfs1: reference to a free cluster",
                )?;
            }
            cluster = page_end;
        }
        Ok(())
    }

    fn page(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        index: u64,
    ) -> Result<&[u8; SECTOR_BYTES]> {
        if let Some(position) = self.pages.iter().position(|(saved, _)| *saved == index) {
            let entry = self
                .pages
                .remove(position)
                .expect("position comes from the same cache");
            self.pages.push_front(entry);
        } else {
            let page_offset = index * SECTOR;
            let len = (header.clusters.div_ceil(8) - page_offset).min(SECTOR) as usize;
            let mut bytes = [0; SECTOR_BYTES];
            source.read_at(header.bitmap + page_offset, &mut bytes[..len])?;
            if self.pages.len() == 64 {
                self.pages.pop_back();
            }
            self.pages.push_front((index, bytes));
        }
        Ok(&self.pages[0].1)
    }
}
