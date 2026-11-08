//! Binary format definitions for DMG files
//!
//! DMG files have the following structure:
//! 1. Data blocks (compressed partition data)
//! 2. XML plist containing block maps (blkx)
//! 3. Koly trailer (512 bytes at end of file)

use byteorder::{BigEndian, ReadBytesExt};
use std::io::{Read, Seek, SeekFrom};

use crate::error::{DppError, Result};

/// Koly magic bytes "koly" (0x6B6F6C79)
pub const KOLY_MAGIC: &[u8; 4] = b"koly";

/// Mish magic bytes "mish" (0x6D697368)
pub const MISH_MAGIC: &[u8; 4] = b"mish";

/// Koly header size in bytes
pub const KOLY_SIZE: usize = 512;

/// Block chunk types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum BlockType {
    /// Zero-filled block (no data stored)
    ZeroFill = 0x00000000,
    /// Raw/uncompressed data
    Raw = 0x00000001,
    /// Ignore/skip block
    Ignore = 0x00000002,
    /// ADC compressed (legacy)
    Adc = 0x80000004,
    /// Zlib compressed
    Zlib = 0x80000005,
    /// Bzip2 compressed
    Bzip2 = 0x80000006,
    /// LZFSE compressed
    Lzfse = 0x80000007,
    /// XZ (LZMA) compressed
    Xz = 0x80000008,
    /// Comment block (no data)
    Comment = 0x7FFFFFFE,
    /// End of partition marker
    End = 0xFFFFFFFF,
}

impl TryFrom<u32> for BlockType {
    type Error = DppError;

    fn try_from(value: u32) -> Result<Self> {
        match value {
            0x00000000 => Ok(BlockType::ZeroFill),
            0x00000001 => Ok(BlockType::Raw),
            0x00000002 => Ok(BlockType::Ignore),
            0x80000004 => Ok(BlockType::Adc),
            0x80000005 => Ok(BlockType::Zlib),
            0x80000006 => Ok(BlockType::Bzip2),
            0x80000007 => Ok(BlockType::Lzfse),
            0x80000008 => Ok(BlockType::Xz),
            0x7FFFFFFE => Ok(BlockType::Comment),
            0xFFFFFFFF => Ok(BlockType::End),
            _ => Err(DppError::UnsupportedCompression(value)),
        }
    }
}

/// Koly trailer structure (512 bytes at end of DMG)
#[derive(Debug, Clone)]
pub struct KolyHeader {
    /// Magic bytes "koly"
    pub magic: [u8; 4],
    /// Version (usually 4)
    pub version: u32,
    /// Header size (512)
    pub header_size: u32,
    /// Flags
    pub flags: u32,
    /// Running data fork offset
    pub running_data_fork_offset: u64,
    /// Data fork offset
    pub data_fork_offset: u64,
    /// Data fork length
    pub data_fork_length: u64,
    /// Resource fork offset
    pub rsrc_fork_offset: u64,
    /// Resource fork length
    pub rsrc_fork_length: u64,
    /// Segment number
    pub segment_number: u32,
    /// Segment count
    pub segment_count: u32,
    /// Segment ID (UUID)
    pub segment_id: [u8; 16],
    /// Data checksum type (2 = CRC32)
    pub data_checksum_type: u32,
    /// Data checksum size
    pub data_checksum_size: u32,
    /// Data checksum (up to 128 bytes, typically 32)
    pub data_checksum: [u8; 128],
    /// XML plist offset
    pub plist_offset: u64,
    /// XML plist length
    pub plist_length: u64,
    /// Reserved (64 bytes)
    pub reserved: [u8; 64],
    /// Master checksum type
    pub master_checksum_type: u32,
    /// Master checksum size
    pub master_checksum_size: u32,
    /// Master checksum (128 bytes)
    pub master_checksum: [u8; 128],
    /// Image variant
    pub image_variant: u32,
    /// Sector count
    pub sector_count: u64,
}

impl KolyHeader {
    /// Read koly header from the end of a file
    pub fn read<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        // Seek to 512 bytes before end
        reader.seek(SeekFrom::End(-(KOLY_SIZE as i64)))?;

        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if &magic != KOLY_MAGIC {
            return Err(DppError::InvalidMagic);
        }

        let version = reader.read_u32::<BigEndian>()?;
        let header_size = reader.read_u32::<BigEndian>()?;
        let flags = reader.read_u32::<BigEndian>()?;
        let running_data_fork_offset = reader.read_u64::<BigEndian>()?;
        let data_fork_offset = reader.read_u64::<BigEndian>()?;
        let data_fork_length = reader.read_u64::<BigEndian>()?;
        let rsrc_fork_offset = reader.read_u64::<BigEndian>()?;
        let rsrc_fork_length = reader.read_u64::<BigEndian>()?;
        let segment_number = reader.read_u32::<BigEndian>()?;
        let segment_count = reader.read_u32::<BigEndian>()?;

        let mut segment_id = [0u8; 16];
        reader.read_exact(&mut segment_id)?;

        let data_checksum_type = reader.read_u32::<BigEndian>()?;
        let data_checksum_size = reader.read_u32::<BigEndian>()?;

        let mut data_checksum = [0u8; 128];
        reader.read_exact(&mut data_checksum)?;

        let plist_offset = reader.read_u64::<BigEndian>()?;
        let plist_length = reader.read_u64::<BigEndian>()?;

        let mut reserved = [0u8; 64];
        reader.read_exact(&mut reserved)?;

        let master_checksum_type = reader.read_u32::<BigEndian>()?;
        let master_checksum_size = reader.read_u32::<BigEndian>()?;

        let mut master_checksum = [0u8; 128];
        reader.read_exact(&mut master_checksum)?;

        let image_variant = reader.read_u32::<BigEndian>()?;
        let sector_count = reader.read_u64::<BigEndian>()?;

        // Skip final reserved bytes (12 bytes to reach 512 total)

        Ok(KolyHeader {
            magic,
            version,
            header_size,
            flags,
            running_data_fork_offset,
            data_fork_offset,
            data_fork_length,
            rsrc_fork_offset,
            rsrc_fork_length,
            segment_number,
            segment_count,
            segment_id,
            data_checksum_type,
            data_checksum_size,
            data_checksum,
            plist_offset,
            plist_length,
            reserved,
            master_checksum_type,
            master_checksum_size,
            master_checksum,
            image_variant,
            sector_count,
        })
    }

    /// Write koly header to a writer
    pub fn write<W: std::io::Write>(&self, writer: &mut W) -> Result<()> {
        use byteorder::WriteBytesExt;

        writer.write_all(&self.magic)?;
        writer.write_u32::<BigEndian>(self.version)?;
        writer.write_u32::<BigEndian>(self.header_size)?;
        writer.write_u32::<BigEndian>(self.flags)?;
        writer.write_u64::<BigEndian>(self.running_data_fork_offset)?;
        writer.write_u64::<BigEndian>(self.data_fork_offset)?;
        writer.write_u64::<BigEndian>(self.data_fork_length)?;
        writer.write_u64::<BigEndian>(self.rsrc_fork_offset)?;
        writer.write_u64::<BigEndian>(self.rsrc_fork_length)?;
        writer.write_u32::<BigEndian>(self.segment_number)?;
        writer.write_u32::<BigEndian>(self.segment_count)?;
        writer.write_all(&self.segment_id)?;
        writer.write_u32::<BigEndian>(self.data_checksum_type)?;
        writer.write_u32::<BigEndian>(self.data_checksum_size)?;
        writer.write_all(&self.data_checksum)?;
        writer.write_u64::<BigEndian>(self.plist_offset)?;
        writer.write_u64::<BigEndian>(self.plist_length)?;
        writer.write_all(&self.reserved)?;
        writer.write_u32::<BigEndian>(self.master_checksum_type)?;
        writer.write_u32::<BigEndian>(self.master_checksum_size)?;
        writer.write_all(&self.master_checksum)?;
        writer.write_u32::<BigEndian>(self.image_variant)?;
        writer.write_u64::<BigEndian>(self.sector_count)?;
        // Write final padding to reach 512 bytes total
        // Header so far: 4+4+4+4+8+8+8+8+8+4+4+16+4+4+128+8+8+64+4+4+128+4+8 = 444 bytes
        // Need 512 - 444 = 68 bytes of padding
        writer.write_all(&[0u8; 68])?;

        Ok(())
    }
}

/// Block run descriptor in a mish block map
#[derive(Debug, Clone)]
pub struct BlockRun {
    /// Block type (compression/encoding)
    pub block_type: BlockType,
    /// Comment (usually 0)
    pub comment: u32,
    /// Sector number (512-byte sectors)
    pub sector_number: u64,
    /// Sector count
    pub sector_count: u64,
    /// Compressed offset in data fork
    pub compressed_offset: u64,
    /// Compressed length
    pub compressed_length: u64,
}

impl BlockRun {
    /// Read a block run from raw bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        if data.len() < 40 {
            return Err(DppError::InvalidBlockMap("block run too short".into()));
        }

        let mut cursor = std::io::Cursor::new(data);
        let block_type_raw = cursor.read_u32::<BigEndian>()?;
        let block_type = BlockType::try_from(block_type_raw)?;
        let comment = cursor.read_u32::<BigEndian>()?;
        let sector_number = cursor.read_u64::<BigEndian>()?;
        let sector_count = cursor.read_u64::<BigEndian>()?;
        let compressed_offset = cursor.read_u64::<BigEndian>()?;
        let compressed_length = cursor.read_u64::<BigEndian>()?;

        Ok(BlockRun {
            block_type,
            comment,
            sector_number,
            sector_count,
            compressed_offset,
            compressed_length,
        })
    }

    /// Convert to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        use byteorder::WriteBytesExt;

        let mut buf = Vec::with_capacity(40);
        buf.write_u32::<BigEndian>(self.block_type as u32).unwrap();
        buf.write_u32::<BigEndian>(self.comment).unwrap();
        buf.write_u64::<BigEndian>(self.sector_number).unwrap();
        buf.write_u64::<BigEndian>(self.sector_count).unwrap();
        buf.write_u64::<BigEndian>(self.compressed_offset).unwrap();
        buf.write_u64::<BigEndian>(self.compressed_length).unwrap();
        buf
    }
}

/// Mish (block map) header structure
///
/// The mish header is 204 bytes total:
/// - 4 bytes: magic "mish"
/// - 4 bytes: version
/// - 8 bytes: first_sector
/// - 8 bytes: sector_count
/// - 8 bytes: data_offset
/// - 4 bytes: buffers_needed
/// - 4 bytes: block_descriptor_count
/// - 24 bytes: reserved1
/// - 4 bytes: checksum_type
/// - 4 bytes: checksum_size
/// - 128 bytes: checksum
/// - 4 bytes: reserved2 (total block count sometimes)
#[derive(Debug, Clone)]
pub struct MishHeader {
    /// Magic bytes "mish"
    pub magic: [u8; 4],
    /// Version
    pub version: u32,
    /// First sector
    pub first_sector: u64,
    /// Sector count
    pub sector_count: u64,
    /// Data offset
    pub data_offset: u64,
    /// Buffers needed
    pub buffers_needed: u32,
    /// Block descriptor count
    pub block_descriptor_count: u32,
    /// Reserved
    pub reserved: [u8; 24],
    /// Checksum type
    pub checksum_type: u32,
    /// Checksum size
    pub checksum_size: u32,
    /// Checksum (128 bytes)
    pub checksum: [u8; 128],
    /// Actual block count (at offset 200 in mish header)
    pub actual_block_count: u32,
    /// Block runs
    pub block_runs: Vec<BlockRun>,
}

impl MishHeader {
    /// Parse mish header from base64-decoded data
    pub fn from_bytes(data: &[u8]) -> Result<Self> {
        // Header is 204 bytes + block runs (40 bytes each)
        if data.len() < 204 {
            return Err(DppError::InvalidBlockMap("mish data too short".into()));
        }

        let mut cursor = std::io::Cursor::new(data);

        let mut magic = [0u8; 4];
        cursor.read_exact(&mut magic)?;
        if &magic != MISH_MAGIC {
            return Err(DppError::InvalidBlockMap(format!(
                "invalid mish magic: {:?}",
                magic
            )));
        }

        let version = cursor.read_u32::<BigEndian>()?;
        let first_sector = cursor.read_u64::<BigEndian>()?;
        let sector_count = cursor.read_u64::<BigEndian>()?;
        let data_offset = cursor.read_u64::<BigEndian>()?;
        let buffers_needed = cursor.read_u32::<BigEndian>()?;
        let block_descriptor_count = cursor.read_u32::<BigEndian>()?;

        let mut reserved = [0u8; 24];
        cursor.read_exact(&mut reserved)?;

        let checksum_type = cursor.read_u32::<BigEndian>()?;
        let checksum_size = cursor.read_u32::<BigEndian>()?;

        let mut checksum = [0u8; 128];
        cursor.read_exact(&mut checksum)?;

        // Read the actual block count from reserved2 field (at offset 200)
        // The field at offset 36 (block_descriptor_count) often contains the partition index
        let actual_block_count = cursor.read_u32::<BigEndian>()?;

        // Parse block runs (40 bytes each)
        let mut block_runs = Vec::with_capacity(actual_block_count as usize);
        for _ in 0..actual_block_count {
            let mut run_data = [0u8; 40];
            cursor.read_exact(&mut run_data)?;
            block_runs.push(BlockRun::from_bytes(&run_data)?);
        }

        Ok(MishHeader {
            magic,
            version,
            first_sector,
            sector_count,
            data_offset,
            buffers_needed,
            block_descriptor_count,
            reserved,
            checksum_type,
            checksum_size,
            checksum,
            actual_block_count,
            block_runs,
        })
    }

    /// Calculate total uncompressed size in bytes
    pub fn uncompressed_size(&self) -> u64 {
        self.sector_count * 512
    }

    /// Calculate total compressed size in bytes
    pub fn compressed_size(&self) -> u64 {
        self.block_runs.iter().map(|r| r.compressed_length).sum()
    }
}

/// Partition entry from the DMG plist
#[derive(Debug, Clone)]
pub struct PartitionEntry {
    /// Partition name
    pub name: String,
    /// Partition ID
    pub id: i32,
    /// Attributes
    pub attributes: u32,
    /// Block map (mish data)
    pub block_map: MishHeader,
}

/// Check if data has the koly magic at the end (512 bytes from end)
pub fn is_dmg<R: Read + Seek>(reader: &mut R) -> bool {
    let pos = reader.stream_position().ok();
    let result = (|| {
        reader.seek(SeekFrom::End(-(KOLY_SIZE as i64)))?;
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        Ok::<_, std::io::Error>(magic == *KOLY_MAGIC)
    })();

    // Restore position
    if let Some(p) = pos {
        let _ = reader.seek(SeekFrom::Start(p));
    }

    result.unwrap_or(false)
}
