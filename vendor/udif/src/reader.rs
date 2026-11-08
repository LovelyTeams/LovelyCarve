//! DMG reader implementation
//!
//! Provides streaming and full decompression of DMG disk images.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

#[cfg(feature = "parallel")]
use rayon::prelude::*;

use crate::checksum::{has_checksum, verify_crc32};
use crate::error::{DppError, Result};
use crate::format::{BlockType, KolyHeader, MishHeader, PartitionEntry};

/// Sector size in bytes
const SECTOR_SIZE: u64 = 512;

/// Read from a decoder until the buffer is full or EOF.
/// Unlike `read()`, this loops to handle decoders that return partial data.
///
/// PROVISIONAL(anomaly-channel): an over-long decode is truncated silently. The
/// truncation is necessary; the silence is not.
pub(crate) fn read_full<R: Read>(reader: &mut R, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut total = 0;
    while total < buf.len() {
        match reader.read(&mut buf[total..])? {
            0 => break, // EOF
            n => total += n,
        }
    }
    Ok(total)
}

/// Fill `buf` from a decoder, failing if it yields fewer bytes than the block
/// map declared. Zeroing the rest would pass off padding as recovered data.
///
/// PROVISIONAL(anomaly-channel): should become a reported partial recovery.
pub(crate) fn decode_exact<R: Read>(reader: &mut R, buf: &mut [u8], format: &str) -> Result<()> {
    let decoded = read_full(reader, buf)?;
    if decoded != buf.len() {
        return Err(DppError::Decompression(format!(
            "{format} decoded {decoded} bytes, expected {}",
            buf.len()
        )));
    }
    Ok(())
}

/// Decode an LZFSE block, failing if its length disagrees with the block map.
///
/// PROVISIONAL(anomaly-channel): should become a reported partial recovery.
fn decode_lzfse_exact(compressed: &[u8], buf: &mut [u8]) -> Result<()> {
    let mut decoded = Vec::with_capacity(buf.len());
    let n = lzfse_rust::decode_bytes(compressed, &mut decoded)
        .map_err(|e| DppError::Decompression(format!("LZFSE: {e:?}")))?;
    if n as usize != buf.len() {
        return Err(DppError::Decompression(format!(
            "lzfse decoded {n} bytes, expected {}",
            buf.len()
        )));
    }
    buf.copy_from_slice(&decoded);
    Ok(())
}

/// Options for DMG reader
#[derive(Debug, Clone)]
pub struct DmgReaderOptions {
    /// Whether to verify checksums when opening the DMG
    pub verify_checksums: bool,
}

impl Default for DmgReaderOptions {
    fn default() -> Self {
        Self {
            verify_checksums: true,
        }
    }
}

/// DMG reader for parsing and extracting disk images
pub struct DmgReader<R> {
    reader: R,
    koly: KolyHeader,
    partitions: Vec<PartitionEntry>,
}

impl<R: Read + Seek> DmgReader<R> {
    /// Create a new DMG reader with default options (checksum verification enabled)
    pub fn new(reader: R) -> Result<Self> {
        Self::with_options(reader, DmgReaderOptions::default())
    }

    /// Create a new DMG reader with custom options
    pub fn with_options(mut reader: R, options: DmgReaderOptions) -> Result<Self> {
        // Read koly header
        let koly = KolyHeader::read(&mut reader)?;

        // Verify data fork checksum if enabled
        if options.verify_checksums {
            Self::verify_data_fork_checksum(&mut reader, &koly)?;
        }

        // Read and parse plist
        reader.seek(SeekFrom::Start(koly.plist_offset))?;
        let mut plist_data = vec![0u8; koly.plist_length as usize];
        reader.read_exact(&mut plist_data)?;

        let partitions = parse_plist(&plist_data)?;

        // Verify master checksum (CRC32 of all mish checksums)
        if options.verify_checksums {
            Self::verify_master_checksum(&koly, &partitions)?;
        }

        Ok(DmgReader {
            reader,
            koly,
            partitions,
        })
    }

    /// Verify the data fork checksum
    fn verify_data_fork_checksum(reader: &mut R, koly: &KolyHeader) -> Result<()> {
        // Skip if no checksum is set
        if !has_checksum(koly.data_checksum_type, &koly.data_checksum) {
            return Ok(());
        }

        // Read the data fork
        reader.seek(SeekFrom::Start(koly.data_fork_offset))?;
        let mut data_fork = vec![0u8; koly.data_fork_length as usize];
        reader.read_exact(&mut data_fork)?;

        // Verify checksum
        verify_crc32(koly.data_checksum_type, &koly.data_checksum, &data_fork)
            .map_err(|(expected, actual)| DppError::ChecksumMismatch { expected, actual })
    }

    /// Verify the master checksum (CRC32 of all mish checksums concatenated)
    fn verify_master_checksum(koly: &KolyHeader, partitions: &[PartitionEntry]) -> Result<()> {
        // Skip if no checksum is set
        if !has_checksum(koly.master_checksum_type, &koly.master_checksum) {
            return Ok(());
        }

        // Concatenate all mish checksums
        let mut all_checksums = Vec::new();
        for partition in partitions {
            // Each mish checksum is the first 4 bytes of the 128-byte array
            all_checksums.extend_from_slice(&partition.block_map.checksum[..4]);
        }

        // Verify master checksum
        verify_crc32(
            koly.master_checksum_type,
            &koly.master_checksum,
            &all_checksums,
        )
        .map_err(|(expected, actual)| DppError::ChecksumMismatch { expected, actual })
    }

    /// Get the koly header
    pub fn koly(&self) -> &KolyHeader {
        &self.koly
    }

    /// Get all partitions
    pub fn partitions(&self) -> &[PartitionEntry] {
        &self.partitions
    }

    /// Get partition by name
    pub fn partition(&self, name: &str) -> Option<&PartitionEntry> {
        self.partitions.iter().find(|p| p.name == name)
    }

    /// List all partition names
    pub fn list_partitions(&self) -> Vec<&str> {
        self.partitions.iter().map(|p| p.name.as_str()).collect()
    }

    /// Get DMG statistics
    pub fn stats(&self) -> DmgStats {
        let total_sectors: u64 = self
            .partitions
            .iter()
            .map(|p| p.block_map.sector_count)
            .sum();
        let total_compressed: u64 = self
            .partitions
            .iter()
            .map(|p| p.block_map.compressed_size())
            .sum();

        DmgStats {
            version: self.koly.version,
            sector_count: self.koly.sector_count,
            partition_count: self.partitions.len(),
            total_uncompressed: total_sectors * SECTOR_SIZE,
            total_compressed,
            data_fork_length: self.koly.data_fork_length,
        }
    }

    /// Decompress a specific partition to raw disk data
    pub fn decompress_partition(&mut self, partition_id: i32) -> Result<Vec<u8>> {
        let partition = self
            .partitions
            .iter()
            .find(|p| p.id == partition_id)
            .ok_or_else(|| DppError::FileNotFound(format!("partition {}", partition_id)))?
            .clone();

        let total_size = partition.block_map.sector_count * SECTOR_SIZE;
        let mut output = vec![0u8; total_size as usize];

        for block_run in &partition.block_map.block_runs {
            let out_offset = block_run.sector_number * SECTOR_SIZE;
            let out_size = block_run.sector_count * SECTOR_SIZE;

            match block_run.block_type {
                BlockType::ZeroFill => {
                    // Already zero-filled
                }
                BlockType::Raw | BlockType::Ignore => {
                    if block_run.compressed_length > 0 {
                        self.reader.seek(SeekFrom::Start(
                            self.koly.data_fork_offset + block_run.compressed_offset,
                        ))?;
                        // Read only compressed_length bytes (actual stored size)
                        // remaining bytes in the sector stay zero-filled
                        let end = out_offset as usize + block_run.compressed_length as usize;
                        self.reader
                            .read_exact(&mut output[out_offset as usize..end])?;
                    }
                }
                BlockType::Zlib => {
                    self.reader.seek(SeekFrom::Start(
                        self.koly.data_fork_offset + block_run.compressed_offset,
                    ))?;
                    let mut compressed = vec![0u8; block_run.compressed_length as usize];
                    self.reader.read_exact(&mut compressed)?;

                    let mut decoder = flate2::read::ZlibDecoder::new(&compressed[..]);
                    let slice = &mut output[out_offset as usize..(out_offset + out_size) as usize];
                    decode_exact(&mut decoder, slice, "zlib")?;
                }
                BlockType::Bzip2 => {
                    self.reader.seek(SeekFrom::Start(
                        self.koly.data_fork_offset + block_run.compressed_offset,
                    ))?;
                    let mut compressed = vec![0u8; block_run.compressed_length as usize];
                    self.reader.read_exact(&mut compressed)?;

                    let mut decoder = bzip2::read::BzDecoder::new(&compressed[..]);
                    let slice = &mut output[out_offset as usize..(out_offset + out_size) as usize];
                    decode_exact(&mut decoder, slice, "bzip2")?;
                }
                BlockType::Lzfse => {
                    self.reader.seek(SeekFrom::Start(
                        self.koly.data_fork_offset + block_run.compressed_offset,
                    ))?;
                    let mut compressed = vec![0u8; block_run.compressed_length as usize];
                    self.reader.read_exact(&mut compressed)?;

                    let end = out_offset as usize + out_size as usize;
                    decode_lzfse_exact(&compressed, &mut output[out_offset as usize..end])?;
                }
                BlockType::Xz => {
                    self.reader.seek(SeekFrom::Start(
                        self.koly.data_fork_offset + block_run.compressed_offset,
                    ))?;
                    let mut compressed = vec![0u8; block_run.compressed_length as usize];
                    self.reader.read_exact(&mut compressed)?;

                    let mut decoder = lzma_rust2::XzReader::new(&compressed[..], false);
                    let slice = &mut output[out_offset as usize..(out_offset + out_size) as usize];
                    decode_exact(&mut decoder, slice, "xz")?;
                }
                BlockType::Adc => {
                    return Err(DppError::Unsupported("ADC compression".into()));
                }
                BlockType::Comment | BlockType::End => {
                    // No data
                }
            }
        }

        Ok(output)
    }

    /// Decompress a partition and stream to a writer block-by-block.
    /// Only uses ~block_size memory per block instead of buffering the full partition.
    /// Integrity is ensured by koly checksums verified on open.
    /// Returns the total number of bytes written.
    pub fn decompress_partition_to<W: Write>(
        &mut self,
        partition_id: i32,
        writer: &mut W,
    ) -> Result<u64> {
        let partition = self
            .partitions
            .iter()
            .find(|p| p.id == partition_id)
            .ok_or_else(|| DppError::FileNotFound(format!("partition {}", partition_id)))?
            .clone();

        let block_size = partition.block_map.sector_count * SECTOR_SIZE;
        let mut bytes_written = 0u64;

        for block_run in &partition.block_map.block_runs {
            let out_offset = block_run.sector_number * SECTOR_SIZE;
            let out_size = block_run.sector_count * SECTOR_SIZE;

            // Emit zero padding if there's a gap between the current position and this block
            if out_offset > bytes_written {
                let gap = (out_offset - bytes_written) as usize;
                let zeros = vec![0u8; gap];
                writer.write_all(&zeros)?;
                bytes_written += gap as u64;
            }

            match block_run.block_type {
                BlockType::ZeroFill => {
                    let zeros = vec![0u8; out_size as usize];
                    writer.write_all(&zeros)?;
                    bytes_written += out_size;
                }
                BlockType::Raw | BlockType::Ignore => {
                    if block_run.compressed_length > 0 {
                        self.reader.seek(SeekFrom::Start(
                            self.koly.data_fork_offset + block_run.compressed_offset,
                        ))?;
                        let mut buf = vec![0u8; block_run.compressed_length as usize];
                        self.reader.read_exact(&mut buf)?;
                        writer.write_all(&buf)?;
                        bytes_written += block_run.compressed_length;
                        // PROVISIONAL(anomaly-channel): the zeroed remainder
                        // below is indistinguishable from recovered data.
                        let remaining = out_size
                            .checked_sub(block_run.compressed_length)
                            .ok_or_else(|| {
                                DppError::InvalidBlockMap(format!(
                                    "block run stores {} bytes but declares only {}",
                                    block_run.compressed_length, out_size
                                ))
                            })?;
                        if remaining > 0 {
                            let zeros = vec![0u8; remaining as usize];
                            writer.write_all(&zeros)?;
                            bytes_written += remaining;
                        }
                    } else {
                        let zeros = vec![0u8; out_size as usize];
                        writer.write_all(&zeros)?;
                        bytes_written += out_size;
                    }
                }
                BlockType::Zlib => {
                    self.reader.seek(SeekFrom::Start(
                        self.koly.data_fork_offset + block_run.compressed_offset,
                    ))?;
                    let mut compressed = vec![0u8; block_run.compressed_length as usize];
                    self.reader.read_exact(&mut compressed)?;

                    let mut decoder = flate2::read::ZlibDecoder::new(&compressed[..]);
                    let mut decompressed = vec![0u8; out_size as usize];
                    decode_exact(&mut decoder, &mut decompressed, "zlib")?;
                    writer.write_all(&decompressed)?;
                    bytes_written += out_size;
                }
                BlockType::Bzip2 => {
                    self.reader.seek(SeekFrom::Start(
                        self.koly.data_fork_offset + block_run.compressed_offset,
                    ))?;
                    let mut compressed = vec![0u8; block_run.compressed_length as usize];
                    self.reader.read_exact(&mut compressed)?;

                    let mut decoder = bzip2::read::BzDecoder::new(&compressed[..]);
                    let mut decompressed = vec![0u8; out_size as usize];
                    decode_exact(&mut decoder, &mut decompressed, "bzip2")?;
                    writer.write_all(&decompressed)?;
                    bytes_written += out_size;
                }
                BlockType::Lzfse => {
                    self.reader.seek(SeekFrom::Start(
                        self.koly.data_fork_offset + block_run.compressed_offset,
                    ))?;
                    let mut compressed = vec![0u8; block_run.compressed_length as usize];
                    self.reader.read_exact(&mut compressed)?;

                    let mut block = vec![0u8; out_size as usize];
                    decode_lzfse_exact(&compressed, &mut block)?;
                    writer.write_all(&block)?;
                    bytes_written += out_size;
                }
                BlockType::Xz => {
                    self.reader.seek(SeekFrom::Start(
                        self.koly.data_fork_offset + block_run.compressed_offset,
                    ))?;
                    let mut compressed = vec![0u8; block_run.compressed_length as usize];
                    self.reader.read_exact(&mut compressed)?;

                    let mut decoder = lzma_rust2::XzReader::new(&compressed[..], false);
                    let mut decompressed = vec![0u8; out_size as usize];
                    decode_exact(&mut decoder, &mut decompressed, "xz")?;
                    writer.write_all(&decompressed)?;
                    bytes_written += out_size;
                }
                BlockType::Adc => {
                    return Err(DppError::Unsupported("ADC compression".into()));
                }
                BlockType::Comment | BlockType::End => {
                    // No data
                }
            }
        }

        // Pad to full partition size if needed
        if bytes_written < block_size {
            let remaining = (block_size - bytes_written) as usize;
            let zeros = vec![0u8; remaining];
            writer.write_all(&zeros)?;
            bytes_written += remaining as u64;
        }

        Ok(bytes_written)
    }

    /// Decompress the main HFS+ partition (largest one)
    pub fn decompress_main_partition(&mut self) -> Result<Vec<u8>> {
        let id = self.main_partition_id()?;
        self.decompress_partition(id)
    }

    /// Stream the main HFS+/APFS partition to a writer.
    pub fn decompress_main_partition_to<W: Write>(&mut self, writer: &mut W) -> Result<u64> {
        let id = self.main_partition_id()?;
        self.decompress_partition_to(id, writer)
    }

    /// Find the partition ID of the main HFS+/APFS partition.
    pub fn main_partition_id(&self) -> Result<i32> {
        let partition = self
            .partitions
            .iter()
            .filter(|p| {
                p.name.contains("Apple_HFS")
                    || p.name.contains("Apple_HFSX")
                    || p.name.contains("Apple_APFS")
            })
            .max_by_key(|p| p.block_map.sector_count)
            .or_else(|| {
                self.partitions
                    .iter()
                    .max_by_key(|p| p.block_map.sector_count)
            })
            .ok_or_else(|| DppError::FileNotFound("no partitions found".into()))?;
        Ok(partition.id)
    }

    /// Find the partition ID of the main HFS+/HFSX partition (excludes APFS).
    /// Returns `Err(FileNotFound)` if no HFS-compatible partition exists.
    pub fn hfs_partition_id(&self) -> Result<i32> {
        let partition = self
            .partitions
            .iter()
            .filter(|p| p.name.contains("Apple_HFS") || p.name.contains("Apple_HFSX"))
            .max_by_key(|p| p.block_map.sector_count)
            .ok_or_else(|| DppError::FileNotFound("no HFS+/HFSX partition found".into()))?;
        Ok(partition.id)
    }

    /// Decompress all partitions into a single raw disk image
    pub fn decompress_all(&mut self) -> Result<Vec<u8>> {
        let total_sectors = self.koly.sector_count;
        let total_size = total_sectors * SECTOR_SIZE;
        let mut output = vec![0u8; total_size as usize];

        for partition in self.partitions.clone() {
            for block_run in &partition.block_map.block_runs {
                let out_offset =
                    (partition.block_map.first_sector + block_run.sector_number) * SECTOR_SIZE;
                let out_size = block_run.sector_count * SECTOR_SIZE;

                if out_offset + out_size > total_size {
                    continue; // Skip out-of-bounds blocks
                }

                match block_run.block_type {
                    BlockType::ZeroFill => {}
                    BlockType::Raw | BlockType::Ignore => {
                        if block_run.compressed_length > 0 {
                            self.reader.seek(SeekFrom::Start(
                                self.koly.data_fork_offset + block_run.compressed_offset,
                            ))?;
                            // Read only compressed_length bytes (actual stored size)
                            // remaining bytes in the sector stay zero-filled
                            let end = out_offset as usize + block_run.compressed_length as usize;
                            self.reader
                                .read_exact(&mut output[out_offset as usize..end])?;
                        }
                    }
                    BlockType::Zlib => {
                        self.reader.seek(SeekFrom::Start(
                            self.koly.data_fork_offset + block_run.compressed_offset,
                        ))?;
                        let mut compressed = vec![0u8; block_run.compressed_length as usize];
                        self.reader.read_exact(&mut compressed)?;

                        let mut decoder = flate2::read::ZlibDecoder::new(&compressed[..]);
                        let end = (out_offset + out_size) as usize;
                        decode_exact(&mut decoder, &mut output[out_offset as usize..end], "zlib")?;
                    }
                    BlockType::Bzip2 => {
                        self.reader.seek(SeekFrom::Start(
                            self.koly.data_fork_offset + block_run.compressed_offset,
                        ))?;
                        let mut compressed = vec![0u8; block_run.compressed_length as usize];
                        self.reader.read_exact(&mut compressed)?;

                        let mut decoder = bzip2::read::BzDecoder::new(&compressed[..]);
                        let end = (out_offset + out_size) as usize;
                        decode_exact(&mut decoder, &mut output[out_offset as usize..end], "bzip2")?;
                    }
                    BlockType::Lzfse => {
                        self.reader.seek(SeekFrom::Start(
                            self.koly.data_fork_offset + block_run.compressed_offset,
                        ))?;
                        let mut compressed = vec![0u8; block_run.compressed_length as usize];
                        self.reader.read_exact(&mut compressed)?;

                        let end = out_offset as usize + out_size as usize;
                        decode_lzfse_exact(&compressed, &mut output[out_offset as usize..end])?;
                    }
                    BlockType::Xz => {
                        self.reader.seek(SeekFrom::Start(
                            self.koly.data_fork_offset + block_run.compressed_offset,
                        ))?;
                        let mut compressed = vec![0u8; block_run.compressed_length as usize];
                        self.reader.read_exact(&mut compressed)?;

                        let mut decoder = lzma_rust2::XzReader::new(&compressed[..], false);
                        let slice =
                            &mut output[out_offset as usize..(out_offset + out_size) as usize];
                        decode_exact(&mut decoder, slice, "xz")?;
                    }
                    BlockType::Adc => {
                        return Err(DppError::Unsupported("ADC compression".into()));
                    }
                    BlockType::Comment | BlockType::End => {}
                }
            }
        }

        Ok(output)
    }

    /// Decompress a specific partition to raw disk data.
    ///
    /// Automatically uses parallel decompression when the `parallel` feature
    /// is enabled.
    pub fn decompress_partition_auto(&mut self, partition_id: i32) -> Result<Vec<u8>> {
        #[cfg(feature = "parallel")]
        {
            self.decompress_partition_parallel(partition_id)
        }
        #[cfg(not(feature = "parallel"))]
        {
            self.decompress_partition(partition_id)
        }
    }

    /// Decompress the main HFS+ partition using auto-selected strategy.
    pub fn decompress_main_partition_auto(&mut self) -> Result<Vec<u8>> {
        let id = self.main_partition_id()?;
        self.decompress_partition_auto(id)
    }

    /// Stream the main HFS+/APFS partition to a writer, using auto-selected strategy.
    pub fn decompress_main_partition_to_auto<W: Write>(&mut self, writer: &mut W) -> Result<u64> {
        #[cfg(feature = "parallel")]
        {
            let id = self.main_partition_id()?;
            self.decompress_partition_to_parallel(id, writer)
        }
        #[cfg(not(feature = "parallel"))]
        {
            self.decompress_main_partition_to(writer)
        }
    }

    /// Get info about block compression types used
    pub fn compression_info(&self) -> CompressionInfo {
        let mut info = CompressionInfo::default();

        for partition in &self.partitions {
            for block_run in &partition.block_map.block_runs {
                match block_run.block_type {
                    BlockType::ZeroFill => info.zero_fill_blocks += 1,
                    BlockType::Raw => info.raw_blocks += 1,
                    BlockType::Zlib => info.zlib_blocks += 1,
                    BlockType::Bzip2 => info.bzip2_blocks += 1,
                    BlockType::Lzfse => info.lzfse_blocks += 1,
                    BlockType::Xz => info.xz_blocks += 1,
                    BlockType::Adc => info.adc_blocks += 1,
                    _ => {}
                }
            }
        }

        info
    }
}

impl DmgReader<BufReader<File>> {
    /// Open a DMG file from a path with default options (checksum verification enabled)
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        Self::open_with_options(path, DmgReaderOptions::default())
    }

    /// Open a DMG file from a path with custom options
    pub fn open_with_options<P: AsRef<Path>>(path: P, options: DmgReaderOptions) -> Result<Self> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        Self::with_options(reader, options)
    }
}

/// A block read from disk, ready for parallel decompression.
#[cfg(feature = "parallel")]
struct ReadBlock {
    /// Block compression type
    block_type: BlockType,
    /// Compressed data read from disk
    data: Vec<u8>,
    /// Output offset in the decompressed buffer
    out_offset: usize,
    /// Expected decompressed size
    out_size: usize,
}

/// Decompress a single block into the provided output slice.
#[cfg(feature = "parallel")]
fn decompress_block(block: &ReadBlock, output: &mut [u8]) -> Result<()> {
    match block.block_type {
        BlockType::Raw | BlockType::Ignore => {
            let copy_len = block.data.len().min(output.len());
            output[..copy_len].copy_from_slice(&block.data[..copy_len]);
        }
        BlockType::Zlib => {
            let mut decoder = flate2::read::ZlibDecoder::new(&block.data[..]);
            decode_exact(&mut decoder, output, "zlib")?;
        }
        BlockType::Bzip2 => {
            let mut decoder = bzip2::read::BzDecoder::new(&block.data[..]);
            decode_exact(&mut decoder, output, "bzip2")?;
        }
        BlockType::Lzfse => {
            decode_lzfse_exact(&block.data, output)?;
        }
        BlockType::Xz => {
            let mut decoder = lzma_rust2::XzReader::new(&block.data[..], false);
            decode_exact(&mut decoder, output, "xz")?;
        }
        _ => {
            // ZeroFill, Comment, End — should not appear in ReadBlock list
        }
    }
    Ok(())
}

#[cfg(feature = "parallel")]
impl<R: Read + Seek> DmgReader<R> {
    /// Decompress a specific partition using parallel block decompression.
    ///
    /// Phase 1: Sequentially reads all compressed blocks from disk.
    /// Phase 2: Decompresses blocks in parallel using rayon, writing directly
    /// into non-overlapping slices of the output buffer.
    pub fn decompress_partition_parallel(&mut self, partition_id: i32) -> Result<Vec<u8>> {
        let partition = self
            .partitions
            .iter()
            .find(|p| p.id == partition_id)
            .ok_or_else(|| DppError::FileNotFound(format!("partition {}", partition_id)))?
            .clone();

        let total_size = (partition.block_map.sector_count * SECTOR_SIZE) as usize;

        // Phase 1: Sequential I/O — read all compressed blocks
        let mut blocks = Vec::new();
        for block_run in &partition.block_map.block_runs {
            let out_offset = (block_run.sector_number * SECTOR_SIZE) as usize;
            let out_size = (block_run.sector_count * SECTOR_SIZE) as usize;

            match block_run.block_type {
                BlockType::ZeroFill | BlockType::Comment | BlockType::End => {
                    // No data to read; output buffer is pre-zeroed
                }
                BlockType::Adc => {
                    return Err(DppError::Unsupported("ADC compression".into()));
                }
                BlockType::Raw | BlockType::Ignore => {
                    if block_run.compressed_length > 0 {
                        self.reader.seek(SeekFrom::Start(
                            self.koly.data_fork_offset + block_run.compressed_offset,
                        ))?;
                        let mut data = vec![0u8; block_run.compressed_length as usize];
                        self.reader.read_exact(&mut data)?;
                        blocks.push(ReadBlock {
                            block_type: block_run.block_type,
                            data,
                            out_offset,
                            out_size,
                        });
                    }
                }
                _ => {
                    // Compressed block (Zlib, Bzip2, Lzfse, Xz)
                    self.reader.seek(SeekFrom::Start(
                        self.koly.data_fork_offset + block_run.compressed_offset,
                    ))?;
                    let mut data = vec![0u8; block_run.compressed_length as usize];
                    self.reader.read_exact(&mut data)?;
                    blocks.push(ReadBlock {
                        block_type: block_run.block_type,
                        data,
                        out_offset,
                        out_size,
                    });
                }
            }
        }

        // Phase 2: Parallel decompression with direct zero-copy writes
        // Pre-allocate the output buffer (zeroed for ZeroFill blocks)
        let mut output = vec![0u8; total_size];

        // Sort blocks by out_offset to carve non-overlapping slices
        blocks.sort_by_key(|b| b.out_offset);

        // Build (block, &mut slice) pairs using split_at_mut
        let mut slices: Vec<(&ReadBlock, &mut [u8])> = Vec::with_capacity(blocks.len());
        let mut remaining = output.as_mut_slice();
        let mut current_pos = 0usize;

        for block in &blocks {
            // Skip past any gap before this block
            if block.out_offset > current_pos {
                let gap = block.out_offset - current_pos;
                remaining = &mut remaining[gap..];
                current_pos += gap;
            }

            // Carve out this block's slice
            let (block_slice, rest) = remaining.split_at_mut(block.out_size);
            slices.push((block, block_slice));
            remaining = rest;
            current_pos += block.out_size;
        }

        // Decompress all blocks in parallel
        let results: Vec<Result<()>> = slices
            .into_par_iter()
            .map(|(block, slice)| decompress_block(block, slice))
            .collect();

        // Propagate first error
        for result in results {
            result?;
        }

        Ok(output)
    }

    /// Decompress a partition using parallel decompression and stream to a writer.
    ///
    /// Decompresses all blocks in parallel, then writes the complete result.
    /// Returns the total number of bytes written.
    pub fn decompress_partition_to_parallel<W: Write>(
        &mut self,
        partition_id: i32,
        writer: &mut W,
    ) -> Result<u64> {
        let output = self.decompress_partition_parallel(partition_id)?;
        let len = output.len() as u64;
        writer.write_all(&output)?;
        Ok(len)
    }
}

/// Statistics about a DMG file
#[derive(Debug, Clone)]
pub struct DmgStats {
    /// DMG version
    pub version: u32,
    /// Total sector count
    pub sector_count: u64,
    /// Number of partitions
    pub partition_count: usize,
    /// Total uncompressed size in bytes
    pub total_uncompressed: u64,
    /// Total compressed size in bytes
    pub total_compressed: u64,
    /// Data fork length
    pub data_fork_length: u64,
}

impl DmgStats {
    /// Calculate compression ratio
    pub fn compression_ratio(&self) -> f64 {
        if self.total_uncompressed == 0 {
            return 1.0;
        }
        self.total_compressed as f64 / self.total_uncompressed as f64
    }

    /// Calculate space savings percentage
    pub fn space_savings(&self) -> f64 {
        (1.0 - self.compression_ratio()) * 100.0
    }
}

/// Information about compression methods used
#[derive(Debug, Clone, Default)]
pub struct CompressionInfo {
    pub zero_fill_blocks: u32,
    pub raw_blocks: u32,
    pub zlib_blocks: u32,
    pub bzip2_blocks: u32,
    pub lzfse_blocks: u32,
    pub xz_blocks: u32,
    pub adc_blocks: u32,
}

/// Parse the DMG plist to extract partition info
fn parse_plist(plist_data: &[u8]) -> Result<Vec<PartitionEntry>> {
    // Parse using plist crate
    let plist: plist::Value = plist::from_bytes(plist_data)
        .map_err(|e| DppError::InvalidPlist(format!("plist parse error: {}", e)))?;

    let dict = plist
        .as_dictionary()
        .ok_or_else(|| DppError::InvalidPlist("expected dictionary".into()))?;

    let resource_fork = dict
        .get("resource-fork")
        .and_then(|v| v.as_dictionary())
        .ok_or_else(|| DppError::InvalidPlist("missing resource-fork".into()))?;

    let blkx = resource_fork
        .get("blkx")
        .and_then(|v| v.as_array())
        .ok_or_else(|| DppError::InvalidPlist("missing blkx array".into()))?;

    let mut partitions = Vec::with_capacity(blkx.len());

    for entry in blkx {
        let entry_dict = entry
            .as_dictionary()
            .ok_or_else(|| DppError::InvalidPlist("blkx entry not a dictionary".into()))?;

        let name = entry_dict
            .get("Name")
            .and_then(|v| v.as_string())
            .unwrap_or("")
            .to_string();

        let id = entry_dict
            .get("ID")
            .and_then(|v| v.as_string())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        let attributes = entry_dict
            .get("Attributes")
            .and_then(|v| v.as_string())
            .and_then(|s| {
                if let Some(hex) = s.strip_prefix("0x") {
                    u32::from_str_radix(hex, 16).ok()
                } else {
                    s.parse().ok()
                }
            })
            .unwrap_or(0);

        let data = entry_dict
            .get("Data")
            .and_then(|v| v.as_data())
            .ok_or_else(|| DppError::InvalidPlist("missing Data in blkx entry".into()))?;

        let block_map = MishHeader::from_bytes(data)?;

        partitions.push(PartitionEntry {
            name,
            id,
            attributes,
            block_map,
        });
    }

    Ok(partitions)
}

/// Convenience function to open a DMG file
pub fn open<P: AsRef<Path>>(path: P) -> Result<DmgReader<BufReader<File>>> {
    DmgReader::open(path)
}

/// Check if a file is a valid DMG
pub fn is_dmg<P: AsRef<Path>>(path: P) -> bool {
    File::open(path)
        .ok()
        .map(BufReader::new)
        .map(|mut r| crate::format::is_dmg(&mut r))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn zlib_compress(data: &[u8]) -> Vec<u8> {
        use flate2::{Compression, write::ZlibEncoder};
        use std::io::Write;
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn lzfse_compress(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        lzfse_rust::encode_bytes(data, &mut out).unwrap();
        out
    }

    #[test]
    fn test_decode_lzfse_exact_accepts_a_full_block() {
        let compressed = lzfse_compress(&[0xCD; 512]);
        let mut output = vec![0u8; 512];

        decode_lzfse_exact(&compressed, &mut output).unwrap();

        assert_eq!(output, vec![0xCD; 512]);
    }

    #[test]
    fn test_decode_lzfse_exact_rejects_a_short_block() {
        // Decodes to 256 bytes while the block map claims 512. The tail used to
        // be left zeroed and returned as if it were recovered data.
        let compressed = lzfse_compress(&[0xCD; 256]);
        let mut output = vec![0u8; 512];

        let err = decode_lzfse_exact(&compressed, &mut output).unwrap_err();

        assert!(matches!(err, DppError::Decompression(_)), "got {err:?}");
    }

    #[test]
    fn test_decode_exact_accepts_a_full_block() {
        let compressed = zlib_compress(&[0xAB; 64]);
        let mut decoder = flate2::read::ZlibDecoder::new(Cursor::new(compressed));
        let mut output = [0u8; 64];

        decode_exact(&mut decoder, &mut output, "zlib").unwrap();

        assert_eq!(output, [0xAB; 64]);
    }

    #[test]
    fn test_decode_exact_rejects_a_short_block() {
        // Decodes to 32 bytes while the block map claims 64. The trailing 32
        // bytes used to stay zeroed and were handed back as recovered data.
        let compressed = zlib_compress(&[0xAB; 32]);
        let mut decoder = flate2::read::ZlibDecoder::new(Cursor::new(compressed));
        let mut output = [0u8; 64];

        let err = decode_exact(&mut decoder, &mut output, "zlib").unwrap_err();

        assert!(matches!(err, DppError::Decompression(_)), "got {err:?}");
    }
}
