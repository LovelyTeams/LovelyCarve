use std::ops::Range;

use lamfold::{BlockSource, FoldError, Result};

use crate::{be16, be32, require};

const DC42_BYTES: usize = 84;
const DC42_TAG_BYTES: u64 = 12;
const MDB_OFFSET: u64 = 1024;
const MDB_BYTES: u64 = 64;
pub(crate) const SECTOR: u64 = 512;
pub const PROBE_BYTES: usize = DC42_BYTES + (MDB_OFFSET + MDB_BYTES) as usize;
const MAX_BLOCKS: usize = 0xffd;

#[derive(Clone, Copy, Debug)]
struct Container {
    base: u64,
    size: u64,
    checksums: Option<(u32, u32, u64)>,
}

impl Container {
    fn inspect(prefix: &[u8], total: u64) -> Result<Self> {
        let raw = Self {
            base: 0,
            size: total,
            checksums: None,
        };
        if prefix.len() < DC42_BYTES || be16(prefix, 82) != 0x0100 || prefix[0] > 63 {
            return Ok(raw);
        }
        let size = u64::from(be32(prefix, 64));
        let tags = u64::from(be32(prefix, 68));
        let geometry = match prefix[80] {
            0 => 409_600,
            1 => 819_200,
            2 => 737_280,
            3 => 1_474_560,
            _ => 0,
        };
        let embedded_magic = prefix.get(DC42_BYTES + 1024..DC42_BYTES + 1026) == Some(b"\xd2\xd7");
        let valid = geometry != 0
            && size == geometry
            && (tags == 0 || tags == size / SECTOR * DC42_TAG_BYTES)
            && total == DC42_BYTES as u64 + size + tags;
        if !valid {
            require(
                !embedded_magic,
                "invalid MFS Disk Copy 4.2 container geometry",
            )?;
            return Ok(raw);
        }
        Ok(Self {
            base: DC42_BYTES as u64,
            size,
            checksums: Some((be32(prefix, 72), be32(prefix, 76), tags)),
        })
    }

    fn verify(self, source: &mut impl BlockSource) -> Result<()> {
        let Some((data, tag, tags)) = self.checksums else {
            return Ok(());
        };
        require(
            checksum(source, self.base, self.size)? == data,
            "MFS Disk Copy 4.2 data checksum mismatch",
        )?;
        let first = tags.min(DC42_TAG_BYTES);
        require(
            checksum(source, self.base + self.size + first, tags - first)? == tag,
            "MFS Disk Copy 4.2 tag checksum mismatch",
        )
    }
}

fn checksum(source: &mut impl BlockSource, mut offset: u64, mut length: u64) -> Result<u32> {
    let mut sum = 0u32;
    let mut bytes = [0; 8192];
    while length != 0 {
        let count = length.min(bytes.len() as u64) as usize;
        source.read_at(offset, &mut bytes[..count])?;
        for word in bytes[..count].as_chunks::<2>().0 {
            sum = sum
                .wrapping_add(u32::from(u16::from_be_bytes(*word)))
                .rotate_right(1);
        }
        offset += count as u64;
        length -= count as u64;
    }
    Ok(sum)
}

#[derive(Clone, Debug)]
pub struct Header {
    pub name: String,
    pub file_count: u16,
    pub block_size: u64,
    pub directory: Range<u64>,
    pub(crate) allocation: Range<u64>,
    pub(crate) count: usize,
    container: Container,
    mdb_offset: u64,
}

impl Header {
    pub fn disk_copy(&self) -> bool {
        self.container.checksums.is_some()
    }

    pub fn mdb_offset(&self) -> u64 {
        self.mdb_offset
    }

    pub fn backup(&self) -> bool {
        self.mdb_offset != self.container.base + MDB_OFFSET
    }

    pub fn locate(
        prefix: &[u8],
        size: u64,
        read: impl FnOnce(u64, &mut [u8]) -> Result<()>,
    ) -> Result<Option<Self>> {
        let container = Container::inspect(prefix, size)?;
        let primary = container.base + MDB_OFFSET;
        let bytes = prefix.get(primary as usize..).unwrap_or_default();
        let primary = Self::parse(bytes, container, primary);
        if matches!(primary, Ok(Some(_))) || !matches!(container.size, 409_600 | 819_200) {
            return primary;
        }
        let backup = container.base + container.size - 2 * SECTOR;
        let mut bytes = [0; MDB_BYTES as usize];
        read(backup, &mut bytes)?;
        if let Ok(Some(header)) = Self::parse(&bytes, container, backup) {
            return Ok(Some(header));
        }
        primary
    }

    fn parse(bytes: &[u8], container: Container, mdb_offset: u64) -> Result<Option<Self>> {
        if bytes.get(..2) != Some(b"\xd2\xd7") {
            return Ok(None);
        }
        require(
            bytes.len() >= MDB_BYTES as usize,
            "truncated MFS master directory block",
        )?;
        let dir_start = u64::from(be16(bytes, 14)) * SECTOR;
        let dir_end = dir_start + u64::from(be16(bytes, 16)) * SECTOR;
        let count = usize::from(be16(bytes, 18));
        let block_size = u64::from(be32(bytes, 20));
        let data_start = u64::from(be16(bytes, 28)) * SECTOR;
        let data_end = data_start + count as u64 * block_size;
        let map_end = MDB_OFFSET + MDB_BYTES + (count as u64 * 3).div_ceil(2);
        let backup = mdb_offset != container.base + MDB_OFFSET;
        let data_limit = if backup {
            container.size - 2 * SECTOR
        } else {
            container.size
        };
        require(
            block_size != 0
                && block_size.is_multiple_of(SECTOR)
                && (1..=MAX_BLOCKS).contains(&count)
                && dir_start >= map_end
                && dir_end > dir_start
                && dir_end <= data_limit
                && data_start >= map_end
                && data_end <= data_limit
                && (!backup || MDB_BYTES + (count as u64 * 3).div_ceil(2) <= 2 * SECTOR),
            "invalid MFS directory, block map or allocation geometry",
        )?;
        let name_length = usize::from(bytes[36]);
        require(name_length <= 27, "invalid MFS volume name length")?;
        Ok(Some(Self {
            name: classic_hfs::decode_mac_roman(&bytes[37..37 + name_length]),
            file_count: be16(bytes, 12),
            block_size,
            directory: (container.base + dir_start)..(container.base + dir_end),
            allocation: (container.base + data_start)..(container.base + data_end),
            count,
            container,
            mdb_offset,
        }))
    }

    pub(crate) fn load_map(&self, source: &mut impl BlockSource) -> Result<Vec<u16>> {
        self.container.verify(source)?;
        let mut packed = vec![0; (self.count * 3).div_ceil(2)];
        source.read_at(self.mdb_offset + MDB_BYTES, &mut packed)?;
        let map: Vec<_> = (0..self.count)
            .map(|index| {
                let byte = index + index / 2;
                let word = u16::from_be_bytes([packed[byte], packed[byte + 1]]);
                if index % 2 == 0 {
                    word >> 4
                } else {
                    word & 0xfff
                }
            })
            .collect();

        let begin = self.directory.start.max(self.allocation.start);
        let end = self.directory.end.min(self.allocation.end);
        if begin < end {
            let first = ((begin - self.allocation.start) / self.block_size) as usize;
            let last = (end - self.allocation.start).div_ceil(self.block_size) as usize;
            if !map[first..last].iter().all(|&block| block == 0xfff) {
                return Err(FoldError::Corrupt(
                    "MFS directory overlaps unreserved allocation blocks",
                ));
            }
        }
        Ok(map)
    }
}
