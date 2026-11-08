//! Read-only DiskGenius PMFX sparse/compressed image support.
//!
//! PMFX stores a logical disk as fixed-size blocks addressed by an L1/L2 block
//! map. Compressed entries point at zlib payloads; absent entries read as zero.
//! The shape is close to the EWF chunk reader, but the format is simple
//! enough to keep native and dependency-light.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use anyhow::{anyhow, Context};
use binrw::{BinRead, BinReaderExt};
use flate2::read::ZlibDecoder;

pub const PMFX_SIGNATURE: [u8; 4] = *b"PMFX";

const BASE_HEADER_SIZE: usize = 0x40;
const IMAGE_NAME_OFFSET: usize = 0x58;
const COMPRESSION_ZLIB: u16 = 9;
const CHAINED_BLOCK_FLAG: u32 = 0x8000_0000;
const ALLOCATION_SIZE_MASK: u32 = 0x7fff_ffff;
const TABLE_ALIGNMENT: u64 = 0x40;
const L2_CACHE_LIMIT: usize = 8;
const BLOCK_CACHE_LIMIT: usize = 8;
const MAX_HEADER_SIZE: usize = 1024 * 1024;
const MAX_BLOCK_SIZE: u64 = 128 * 1024 * 1024;
const MAX_L2_TABLE_SIZE: u64 = 256 * 1024 * 1024;
const MAX_PAYLOAD_SLOP: u64 = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct PmfxHeader {
    pub header_size: usize,
    pub compression_type: u16,
    pub flags: u32,
    pub sector_size: usize,
    pub total_sectors: u64,
    pub block_sectors: u32,
    pub entries_per_l2: u32,
    pub l1_count: u32,
    pub l1_offset: u64,
    pub image_name: String,
    pub logical_size: u64,
    pub block_size: u64,
    pub block_count: u64,
}

#[derive(Debug, BinRead)]
#[br(little, magic = b"PMFX")]
struct RawPmfxHeader {
    #[br(seek_before = SeekFrom::Start(0x08))]
    header_size: u16,
    compression_type: u16,
    flags: u32,
    sector_size: u32,
    total_sectors: u64,
    block_sectors: u32,
    entries_per_l2: u32,
    l1_count: u32,
    l1_offset: u32,
}

impl PmfxHeader {
    fn compressed(&self) -> bool {
        self.compression_type != 0
    }
}

#[derive(Debug)]
pub struct PmfxImage {
    file: File,
    file_len: u64,
    header: PmfxHeader,
    l1: Vec<u64>,
    l2_cache: HashMap<usize, Vec<u8>>,
    block_cache: HashMap<u64, Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
struct BlockEntry {
    offset: u64,
    allocation_size: u32,
    stored_size: u32,
    chained: bool,
}

#[derive(Debug, BinRead)]
#[br(little)]
struct CompressedBlockEntry {
    offset: u64,
    allocation_with_flag: u32,
    stored_size: u32,
}

#[derive(Debug, BinRead)]
#[br(little)]
struct RawBlockEntry {
    offset: u64,
}

#[derive(Debug, BinRead)]
#[br(little)]
struct ChainedFragmentEntry {
    offset: u64,
    size: u32,
}

impl BlockEntry {
    fn sparse() -> Self {
        Self {
            offset: 0,
            allocation_size: 0,
            stored_size: 0,
            chained: false,
        }
    }

    fn present(self) -> bool {
        self.offset != 0
    }
}

impl PmfxImage {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let mut file = File::open(path)
            .with_context(|| format!("failed to open PMFX image: {}", path.display()))?;
        let file_len = file
            .metadata()
            .with_context(|| format!("failed to stat PMFX image: {}", path.display()))?
            .len();
        let header = read_header(&mut file)?;
        if header.compressed() && header.compression_type != COMPRESSION_ZLIB {
            anyhow::bail!(
                "PMFX compression type {} is not implemented",
                header.compression_type
            );
        }
        let l1 = read_l1(&mut file, &header)?;

        Ok(Self {
            file,
            file_len,
            header,
            l1,
            l2_cache: HashMap::new(),
            block_cache: HashMap::new(),
        })
    }

    pub fn media_size(&self) -> u64 {
        self.header.logical_size
    }

    pub fn sector_size(&self) -> usize {
        self.header.sector_size
    }

    /// Reads at most one logical PMFX block at `offset` into `buf`.
    ///
    /// Returning at block boundaries lets the caller release its outer mutex
    /// between blocks during large exports, matching the fairness pattern used
    /// for EWF/VMDK offset reads.
    pub fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> anyhow::Result<usize> {
        if buf.is_empty() || offset >= self.header.logical_size {
            return Ok(0);
        }

        let available = self
            .header
            .logical_size
            .saturating_sub(offset)
            .min(buf.len() as u64) as usize;
        let block_index = offset / self.header.block_size;
        let within_block = usize::try_from(offset % self.header.block_size)
            .context("PMFX block offset does not fit in memory address space")?;
        let block = self.decoded_block(block_index)?;
        if within_block >= block.len() {
            return Ok(0);
        }

        let take = available.min(block.len() - within_block);
        buf[..take].copy_from_slice(&block[within_block..within_block + take]);
        Ok(take)
    }

    fn decoded_block(&mut self, block_index: u64) -> anyhow::Result<&[u8]> {
        if !self.block_cache.contains_key(&block_index) {
            let block = self.decode_block(block_index)?;
            if self.block_cache.len() >= BLOCK_CACHE_LIMIT {
                self.block_cache.clear();
            }
            self.block_cache.insert(block_index, block);
        }
        self.block_cache
            .get(&block_index)
            .map(Vec::as_slice)
            .ok_or_else(|| anyhow!("PMFX decoded block cache miss for block {block_index}"))
    }

    fn decode_block(&mut self, block_index: u64) -> anyhow::Result<Vec<u8>> {
        let final_size = self.block_final_size(block_index)?;
        if final_size == 0 {
            return Ok(Vec::new());
        }

        let entry = self.block_entry(block_index)?;
        if !entry.present() {
            return Ok(vec![0; final_size]);
        }

        let payload = self.read_physical_payload(entry)?;
        if !self.header.compressed() || u64::from(entry.stored_size) == self.header.block_size {
            if payload.len() < final_size {
                anyhow::bail!("short PMFX raw block {block_index}");
            }
            return Ok(payload[..final_size].to_vec());
        }

        let mut decoder = ZlibDecoder::new(payload.as_slice());
        let block_size = block_size_usize(self.header.block_size)?;
        let mut decoded = Vec::with_capacity(block_size);
        decoder
            .read_to_end(&mut decoded)
            .with_context(|| format!("zlib decompress PMFX block {block_index}"))?;
        if decoded.len() != block_size {
            anyhow::bail!(
                "bad PMFX zlib output for block {block_index}: {} != {}",
                decoded.len(),
                block_size
            );
        }
        decoded.truncate(final_size);
        Ok(decoded)
    }

    fn block_final_size(&self, block_index: u64) -> anyhow::Result<usize> {
        if block_index >= self.header.block_count {
            return Ok(0);
        }
        let block_offset = block_index
            .checked_mul(self.header.block_size)
            .ok_or_else(|| anyhow!("PMFX block offset overflow"))?;
        let remaining = self.header.logical_size.saturating_sub(block_offset);
        usize::try_from(remaining.min(self.header.block_size))
            .context("PMFX block size does not fit in memory address space")
    }

    fn block_entry(&mut self, block_index: u64) -> anyhow::Result<BlockEntry> {
        let entries_per_l2 = u64::from(self.header.entries_per_l2);
        let l1_index = usize::try_from(block_index / entries_per_l2)
            .context("PMFX L1 index does not fit in memory address space")?;
        let l2_index = usize::try_from(block_index % entries_per_l2)
            .context("PMFX L2 index does not fit in memory address space")?;

        if l1_index >= self.l1.len() || self.l1[l1_index] == 0 {
            return Ok(BlockEntry::sparse());
        }

        let compressed = self.header.compressed();
        let l2 = self.load_l2(l1_index)?;
        if compressed {
            let base = l2_index
                .checked_mul(16)
                .ok_or_else(|| anyhow!("PMFX L2 compressed entry offset overflow"))?;
            let raw = read_compressed_block_entry(l2, base)?;
            Ok(BlockEntry {
                offset: raw.offset,
                allocation_size: raw.allocation_with_flag & ALLOCATION_SIZE_MASK,
                stored_size: raw.stored_size,
                chained: (raw.allocation_with_flag & CHAINED_BLOCK_FLAG) != 0,
            })
        } else {
            let base = l2_index
                .checked_mul(8)
                .ok_or_else(|| anyhow!("PMFX L2 raw entry offset overflow"))?;
            let raw = read_raw_block_entry(l2, base)?;
            Ok(BlockEntry {
                offset: raw.offset,
                allocation_size: block_size_u32(self.header.block_size)?,
                stored_size: block_size_u32(self.header.block_size)?,
                chained: false,
            })
        }
    }

    fn load_l2(&mut self, l1_index: usize) -> anyhow::Result<&[u8]> {
        if !self.l2_cache.contains_key(&l1_index) {
            let entry_size = if self.header.compressed() {
                16_u64
            } else {
                8_u64
            };
            let table_size_u64 = u64::from(self.header.entries_per_l2)
                .checked_mul(entry_size)
                .ok_or_else(|| anyhow!("PMFX L2 table size overflow"))?;
            if table_size_u64 > MAX_L2_TABLE_SIZE {
                anyhow::bail!("PMFX L2 table is too large: {table_size_u64} bytes");
            }
            let table_size = usize::try_from(table_size_u64)
                .context("PMFX L2 table size does not fit in memory address space")?;
            let offset = self.l1[l1_index];
            let raw = self
                .read_exact_vec_at(offset, table_size)
                .with_context(|| format!("read PMFX L2 table {l1_index} at 0x{offset:x}"))?;
            if self.l2_cache.len() >= L2_CACHE_LIMIT {
                self.l2_cache.clear();
            }
            self.l2_cache.insert(l1_index, raw);
        }
        self.l2_cache
            .get(&l1_index)
            .map(Vec::as_slice)
            .ok_or_else(|| anyhow!("PMFX L2 cache miss for table {l1_index}"))
    }

    fn read_physical_payload(&mut self, entry: BlockEntry) -> anyhow::Result<Vec<u8>> {
        if !entry.present() || entry.stored_size == 0 {
            return Ok(Vec::new());
        }
        let stored_size = payload_len_usize(self.header.block_size, entry.stored_size)?;
        if !entry.chained {
            return self.read_exact_vec_at(entry.offset, stored_size);
        }

        let allocation_size = usize::try_from(entry.allocation_size)
            .context("PMFX chained allocation size does not fit in memory address space")?;
        if allocation_size < 4 {
            anyhow::bail!("invalid PMFX chained payload allocation");
        }
        let head = self.read_exact_vec_at(entry.offset, allocation_size)?;
        let fragment_count = BinReaderExt::read_le::<u32>(&mut Cursor::new(head.as_slice()))
            .context("read PMFX chained fragment count")? as usize;
        let table_len = 4_usize
            .checked_add(
                fragment_count
                    .checked_mul(12)
                    .ok_or_else(|| anyhow!("PMFX chained fragment table overflow"))?,
            )
            .ok_or_else(|| anyhow!("PMFX chained fragment table overflow"))?;
        if table_len > head.len() {
            anyhow::bail!("invalid PMFX chained fragment table");
        }

        let mut fragments = Vec::with_capacity(fragment_count);
        for index in 0..fragment_count {
            let base = 4 + index * 12;
            let fragment = read_chained_fragment_entry(&head, base)?;
            fragments.push((fragment.offset, fragment.size));
        }

        let mut out = Vec::with_capacity(stored_size);
        let inline = &head[table_len..];
        let inline_take = inline.len().min(stored_size);
        out.extend_from_slice(&inline[..inline_take]);

        for (fragment_offset, fragment_size) in fragments {
            if out.len() >= stored_size {
                break;
            }
            let want = usize::try_from(fragment_size)
                .context("PMFX chained fragment size does not fit in memory address space")?
                .min(stored_size - out.len());
            let chunk = self.read_exact_vec_at(fragment_offset, want)?;
            out.extend_from_slice(&chunk);
        }

        if out.len() != stored_size {
            anyhow::bail!("PMFX chained payload ended early");
        }
        Ok(out)
    }

    fn read_exact_vec_at(&mut self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let end = offset
            .checked_add(len as u64)
            .ok_or_else(|| anyhow!("PMFX physical read offset overflow"))?;
        if end > self.file_len {
            anyhow::bail!(
                "PMFX physical read 0x{offset:x}..0x{end:x} exceeds file size 0x{:x}",
                self.file_len
            );
        }

        let mut buf = vec![0u8; len];
        read_exact_at(&mut self.file, offset, &mut buf)?;
        Ok(buf)
    }
}

pub fn has_pmfx_signature(path: &Path) -> anyhow::Result<bool> {
    let mut file =
        File::open(path).with_context(|| format!("failed to open image: {}", path.display()))?;
    let mut sig = [0u8; 4];
    match file.read_exact(&mut sig) {
        Ok(()) => Ok(sig == PMFX_SIGNATURE),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(err) => Err(err).with_context(|| format!("read PMFX signature: {}", path.display())),
    }
}

fn read_header(file: &mut File) -> anyhow::Result<PmfxHeader> {
    let mut base = [0u8; BASE_HEADER_SIZE];
    read_exact_at(file, 0, &mut base)?;
    let fixed = RawPmfxHeader::read(&mut Cursor::new(&base)).context("read PMFX fixed header")?;
    let header_size = fixed.header_size as usize;
    if !(BASE_HEADER_SIZE..=MAX_HEADER_SIZE).contains(&header_size) {
        anyhow::bail!("invalid PMFX header size: 0x{header_size:x}");
    }

    let inspect_size = header_size;
    let mut raw = vec![0u8; inspect_size];
    read_exact_at(file, 0, &mut raw)?;

    if fixed.sector_size == 0 || fixed.block_sectors == 0 || fixed.entries_per_l2 == 0 {
        anyhow::bail!("invalid PMFX geometry");
    }

    let sector_size = usize::try_from(fixed.sector_size)
        .context("PMFX sector size does not fit in memory address space")?;
    let block_size = u64::from(fixed.block_sectors)
        .checked_mul(sector_size as u64)
        .ok_or_else(|| anyhow!("PMFX block size overflow"))?;
    if block_size == 0 || block_size > MAX_BLOCK_SIZE {
        anyhow::bail!("invalid PMFX block size: {block_size}");
    }
    let logical_size = fixed
        .total_sectors
        .checked_mul(sector_size as u64)
        .ok_or_else(|| anyhow!("PMFX logical size overflow"))?;
    let block_count = if fixed.total_sectors == 0 {
        0
    } else {
        fixed.total_sectors.div_ceil(u64::from(fixed.block_sectors))
    };
    let image_name = read_utf16le_name(&raw);

    Ok(PmfxHeader {
        header_size,
        compression_type: fixed.compression_type,
        flags: fixed.flags,
        sector_size,
        total_sectors: fixed.total_sectors,
        block_sectors: fixed.block_sectors,
        entries_per_l2: fixed.entries_per_l2,
        l1_count: fixed.l1_count,
        l1_offset: u64::from(fixed.l1_offset),
        image_name,
        logical_size,
        block_size,
        block_count,
    })
}

fn read_l1(file: &mut File, header: &PmfxHeader) -> anyhow::Result<Vec<u64>> {
    if header.l1_count == 0 {
        return Ok(Vec::new());
    }
    let bytes = u64::from(header.l1_count)
        .checked_mul(8)
        .ok_or_else(|| anyhow!("PMFX L1 table size overflow"))?;
    let rounded = align_up(bytes, TABLE_ALIGNMENT)?;
    let table_len = usize::try_from(rounded)
        .context("PMFX L1 table size does not fit in memory address space")?;
    let mut raw = vec![0u8; table_len];
    read_exact_at(file, header.l1_offset, &mut raw)
        .with_context(|| format!("read PMFX L1 table at 0x{:x}", header.l1_offset))?;

    let mut l1 = Vec::with_capacity(header.l1_count as usize);
    let mut cursor = Cursor::new(raw.as_slice());
    for _ in 0..header.l1_count as usize {
        l1.push(BinReaderExt::read_le::<u64>(&mut cursor).context("read PMFX L1 entry")?);
    }
    Ok(l1)
}

fn read_exact_at(file: &mut File, offset: u64, buf: &mut [u8]) -> anyhow::Result<()> {
    file.seek(SeekFrom::Start(offset))
        .with_context(|| format!("seek PMFX image to 0x{offset:x}"))?;
    file.read_exact(buf)
        .with_context(|| format!("read PMFX image at 0x{offset:x}"))
}

fn read_utf16le_name(raw: &[u8]) -> String {
    let Some(name_raw) = raw.get(IMAGE_NAME_OFFSET..) else {
        return String::new();
    };
    let (pairs, _odd_tail) = name_raw.as_chunks::<2>();
    let units: Vec<u16> = pairs
        .iter()
        .map(|chunk| u16::from_le_bytes(*chunk))
        .take_while(|unit| *unit != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

fn align_up(value: u64, alignment: u64) -> anyhow::Result<u64> {
    if alignment == 0 {
        anyhow::bail!("invalid PMFX table alignment");
    }
    let mask = alignment - 1;
    value
        .checked_add(mask)
        .map(|rounded| rounded & !mask)
        .ok_or_else(|| anyhow!("PMFX table alignment overflow"))
}

fn block_size_usize(block_size: u64) -> anyhow::Result<usize> {
    usize::try_from(block_size).context("PMFX block size does not fit in memory address space")
}

fn block_size_u32(block_size: u64) -> anyhow::Result<u32> {
    u32::try_from(block_size).context("PMFX block size does not fit in PMFX raw block entry")
}

fn payload_len_usize(block_size: u64, stored_size: u32) -> anyhow::Result<usize> {
    let stored_size = u64::from(stored_size);
    if stored_size > block_size.saturating_add(MAX_PAYLOAD_SLOP) {
        anyhow::bail!("PMFX payload is too large: {stored_size} bytes");
    }
    usize::try_from(stored_size).context("PMFX payload size does not fit in memory address space")
}

fn read_compressed_block_entry(
    bytes: &[u8],
    offset: usize,
) -> anyhow::Result<CompressedBlockEntry> {
    let slice = bytes
        .get(offset..)
        .ok_or_else(|| anyhow!("PMFX compressed block entry at 0x{offset:x} is out of bounds"))?;
    CompressedBlockEntry::read(&mut Cursor::new(slice))
        .with_context(|| format!("read PMFX compressed block entry at 0x{offset:x}"))
}

fn read_raw_block_entry(bytes: &[u8], offset: usize) -> anyhow::Result<RawBlockEntry> {
    let slice = bytes
        .get(offset..)
        .ok_or_else(|| anyhow!("PMFX raw block entry at 0x{offset:x} is out of bounds"))?;
    RawBlockEntry::read(&mut Cursor::new(slice))
        .with_context(|| format!("read PMFX raw block entry at 0x{offset:x}"))
}

fn read_chained_fragment_entry(
    bytes: &[u8],
    offset: usize,
) -> anyhow::Result<ChainedFragmentEntry> {
    let slice = bytes
        .get(offset..)
        .ok_or_else(|| anyhow!("PMFX chained fragment at 0x{offset:x} is out of bounds"))?;
    ChainedFragmentEntry::read(&mut Cursor::new(slice))
        .with_context(|| format!("read PMFX chained fragment at 0x{offset:x}"))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::write::ZlibEncoder;
    use flate2::Compression;
    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn reads_zlib_and_raw_blocks() {
        let block_size = 1024usize;
        let mut block0 = vec![0u8; block_size];
        block0[510..512].copy_from_slice(&[0x55, 0xaa]);
        let block1 = vec![0x5au8; block_size];
        let compressed0 = zlib(&block0);

        let header_size = 0x100usize;
        let l1_offset = 0x100usize;
        let l2_offset = 0x200usize;
        let payload0_offset = 0x300usize;
        let payload1_offset = payload0_offset + compressed0.len();
        let total_len = payload1_offset + block1.len();
        let mut bytes = vec![0u8; total_len];

        bytes[0..4].copy_from_slice(b"PMFX");
        bytes[0x08..0x0a].copy_from_slice(&(header_size as u16).to_le_bytes());
        bytes[0x0a..0x0c].copy_from_slice(&COMPRESSION_ZLIB.to_le_bytes());
        bytes[0x10..0x14].copy_from_slice(&512u32.to_le_bytes());
        bytes[0x14..0x1c].copy_from_slice(&4u64.to_le_bytes());
        bytes[0x1c..0x20].copy_from_slice(&2u32.to_le_bytes());
        bytes[0x20..0x24].copy_from_slice(&4u32.to_le_bytes());
        bytes[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
        bytes[0x28..0x2c].copy_from_slice(&(l1_offset as u32).to_le_bytes());
        write_utf16le(&mut bytes[0x58..0x80], "PmfxImage");
        bytes[l1_offset..l1_offset + 8].copy_from_slice(&(l2_offset as u64).to_le_bytes());

        let entry0 = l2_offset;
        bytes[entry0..entry0 + 8].copy_from_slice(&(payload0_offset as u64).to_le_bytes());
        bytes[entry0 + 8..entry0 + 12].copy_from_slice(&0x1000u32.to_le_bytes());
        bytes[entry0 + 12..entry0 + 16].copy_from_slice(&(compressed0.len() as u32).to_le_bytes());

        let entry1 = l2_offset + 16;
        bytes[entry1..entry1 + 8].copy_from_slice(&(payload1_offset as u64).to_le_bytes());
        bytes[entry1 + 8..entry1 + 12].copy_from_slice(&(block_size as u32).to_le_bytes());
        bytes[entry1 + 12..entry1 + 16].copy_from_slice(&(block_size as u32).to_le_bytes());

        bytes[payload0_offset..payload0_offset + compressed0.len()].copy_from_slice(&compressed0);
        bytes[payload1_offset..payload1_offset + block1.len()].copy_from_slice(&block1);

        let mut file = NamedTempFile::new().expect("temp file");
        file.write_all(&bytes).expect("write synthetic PMFX");

        let mut image = PmfxImage::open(file.path()).expect("open synthetic PMFX");
        assert_eq!(image.media_size(), 2048);
        assert_eq!(image.sector_size(), 512);

        let mut buf = vec![0u8; 2048];
        let first = image.read_at(0, &mut buf).expect("read first block");
        let second = image
            .read_at(first as u64, &mut buf[first..])
            .expect("read second block");
        assert_eq!(first, 1024);
        assert_eq!(second, 1024);
        assert_eq!(&buf[..1024], &block0);
        assert_eq!(&buf[1024..], &block1);
    }

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).expect("compress data");
        encoder.finish().expect("finish zlib stream")
    }

    fn write_utf16le(dst: &mut [u8], value: &str) {
        for (index, unit) in value.encode_utf16().enumerate() {
            let start = index * 2;
            if start + 2 > dst.len() {
                break;
            }
            dst[start..start + 2].copy_from_slice(&unit.to_le_bytes());
        }
    }
}
