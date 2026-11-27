use crate::{le16, le32, le64, require};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::VecDeque;

pub const MASTER_OFFSET: u64 = 65_536;
pub const BLOCK_SIZE: usize = 4096;
const MASTER_BLOCK: u64 = MASTER_OFFSET / BLOCK_SIZE as u64;
const BITS_PER_BITMAP: u64 = (BLOCK_SIZE as u64 - 4) * 8;

#[derive(Debug, Clone)]
pub struct Header {
    pub blocks: u64,
    pub uuid: [u8; 16],
    pub label: String,
    pub release: u32,
    pub protected_nodes: bool,
    pub large_keys: bool,
    pub(crate) root: u64,
    pub(crate) height: u8,
    pub(crate) mkfs_id: u32,
    pub(crate) flushes: u64,
    needs_backup: bool,
}

impl Header {
    pub fn inspect(master: &[u8], format: &[u8], source_size: u64) -> Result<Self> {
        require(master.starts_with(b"ReIsEr4\0"), "reiser4: master magic")?;
        require(
            format.get(52..68) == Some(b"ReIsEr40FoRmAt\0\0"),
            "reiser4: format magic",
        )?;
        if le16(master, 16)? != 0 || usize::from(le16(master, 18)?) != BLOCK_SIZE {
            return Err(FoldError::Unsupported("reiser4: disk format or block size"));
        }
        let blocks = le64(format, 0)?;
        let root = le64(format, 16)?;
        let height = le16(format, 68)?;
        let flags = le64(format, 72)?;
        let version = le32(format, 80)?;
        let node_format = le32(format, 84)?;
        let release = version & 0x7fff_ffff;
        if release > 2 || flags & !1 != 0 || node_format > 1 {
            return Err(FoldError::Unsupported(
                "reiser4: format release, flags or node plugin",
            ));
        }
        require(
            blocks >= MASTER_BLOCK + 9
                && blocks <= source_size / BLOCK_SIZE as u64
                && le64(format, 8)? <= blocks
                && root > MASTER_BLOCK + 6
                && root < blocks
                && (1..=16).contains(&height),
            "reiser4: volume geometry",
        )?;
        let uuid = crate::field(master, 20)?;
        let raw_label: [u8; 16] = crate::field(master, 36)?;
        let label = raw_label.split(|byte| *byte == 0).next().unwrap_or(&[]);
        Ok(Self {
            blocks,
            uuid,
            label: String::from_utf8_lossy(label).into_owned(),
            release,
            protected_nodes: node_format == 1,
            large_keys: flags & 1 != 0,
            root,
            height: height as u8,
            mkfs_id: le32(format, 48)?,
            flushes: le64(format, 40)?,
            needs_backup: version >> 31 != 0,
        })
    }

    pub fn read(source: &mut impl BlockSource) -> Result<Self> {
        let mut master = [0; 52];
        let mut format = [0; 88];
        source.read_at(MASTER_OFFSET, &mut master)?;
        source.read_at(MASTER_OFFSET + BLOCK_SIZE as u64, &mut format)?;
        Self::inspect(&master, &format, source.len())
    }

    pub(crate) fn check_clean(&self, source: &mut impl BlockSource) -> Result<()> {
        let mut header = [0; 8];
        let mut footer = [0; 8];
        let mut status = [0; 32];
        source.read_at((MASTER_BLOCK + 3) * BLOCK_SIZE as u64, &mut header)?;
        source.read_at((MASTER_BLOCK + 4) * BLOCK_SIZE as u64, &mut footer)?;
        source.read_at((MASTER_BLOCK + 5) * BLOCK_SIZE as u64, &mut status)?;
        require(
            status.starts_with(b"ReiSeR4StATusBl\0"),
            "reiser4: status magic",
        )?;
        require(
            u64::from_le_bytes(header) < self.blocks && u64::from_le_bytes(footer) < self.blocks,
            "reiser4: journal pointer outside volume",
        )?;
        if self.needs_backup
            || header != footer
            || le64(&status, 16)? != 0
            || le64(&status, 24)? != 0
        {
            return Err(FoldError::Unsupported(
                "reiser4: volume requires journal replay or repair",
            ));
        }
        Ok(())
    }
}

#[derive(Default)]
pub(crate) struct Allocation {
    maps: VecDeque<(u64, Box<[u8; BLOCK_SIZE]>)>,
}

impl Allocation {
    pub(crate) fn check(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        start: u64,
        count: u64,
    ) -> Result<()> {
        let stop = start
            .checked_add(count)
            .filter(|stop| *stop <= header.blocks)
            .ok_or(FoldError::Corrupt("reiser4: extent outside volume"))?;
        require(
            start > MASTER_BLOCK + 6,
            "reiser4: reserved block reference",
        )?;
        let mut pos = start;
        while pos < stop {
            let group = pos / BITS_PER_BITMAP;
            let base = group * BITS_PER_BITMAP;
            let map_block = if group == 0 { MASTER_BLOCK + 2 } else { base };
            require(pos != map_block, "reiser4: bitmap block reference")?;
            let end = stop.min(base + BITS_PER_BITMAP);
            let bitmap = self.map(source, map_block)?;
            for bit in (pos - base) as usize..(end - base) as usize {
                require(
                    bitmap[4 + bit / 8] & (1 << (bit % 8)) != 0,
                    "reiser4: reference to unallocated block",
                )?;
            }
            pos = end;
        }
        Ok(())
    }

    fn map(&mut self, source: &mut impl BlockSource, block: u64) -> Result<&[u8; BLOCK_SIZE]> {
        if let Some(index) = self.maps.iter().position(|(id, _)| *id == block) {
            if let Some(entry) = self.maps.remove(index) {
                self.maps.push_front(entry);
            }
        } else {
            let mut data = Box::new([0; BLOCK_SIZE]);
            source.read_at(block * BLOCK_SIZE as u64, data.as_mut_slice())?;
            let (mut low, mut high) = (1u32, 0u32);
            for &byte in &data[4..] {
                low += u32::from(byte);
                high += low;
            }
            require(
                (high % 65_521) << 16 | (low % 65_521) == le32(&data[..], 0)?,
                "reiser4: allocation bitmap checksum",
            )?;
            if self.maps.len() == 16 {
                self.maps.pop_back();
            }
            self.maps.push_front((block, data));
        }
        Ok(&self.maps[0].1)
    }
}
