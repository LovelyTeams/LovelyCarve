//! DPP - DMG + PKG + PBZX parser
//!
//! A cross-platform library for working with Apple disk images (DMG files).
//!
//! # Features
//!
//! - **List** partitions in DMG files
//! - **Extract** raw partition data
//! - **Create** DMG files with various compression methods
//! - **Cross-platform** - works on Windows, Linux, and macOS
//!
//! # Supported Compression
//!
//! - Raw (uncompressed)
//! - Zlib
//! - Bzip2
//! - LZFSE (Apple's native compression)
//! - XZ (LZMA)
//!
//! # Example
//!
//! ```no_run
//! use udif::{DmgArchive, Result};
//!
//! fn main() -> Result<()> {
//!     // Open a DMG file
//!     let mut archive = DmgArchive::open("image.dmg")?;
//!
//!     // List partitions
//!     for partition in archive.partitions() {
//!         println!("{}: {} sectors", partition.name, partition.sectors);
//!     }
//!
//!     // Extract main partition
//!     let data = archive.extract_main_partition()?;
//!     std::fs::write("partition.raw", &data)?;
//!
//!     Ok(())
//! }
//! ```

pub mod checksum;
pub mod error;
pub mod format;
pub mod reader;
pub mod writer;

pub use checksum::{CHECKSUM_TYPE_CRC32, CHECKSUM_TYPE_NONE, crc32};
pub use error::{DppError, Result};
pub use format::{BlockType, KolyHeader, MishHeader, PartitionEntry};
pub use reader::{CompressionInfo, DmgReader, DmgReaderOptions, DmgStats, is_dmg, open};
pub use writer::{CompressionMethod, DmgWriter, create, create_from_data, create_from_file};

/// Partition filesystem type detected from the partition name
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionType {
    /// HFS+ (case-insensitive)
    Hfs,
    /// HFSX (case-sensitive HFS+)
    Hfsx,
    /// Apple APFS
    Apfs,
    /// Other or unknown partition type
    Other,
}

impl PartitionType {
    /// Classify a partition from its DMG partition name (e.g. "Apple_HFSX")
    pub fn from_partition_name(name: &str) -> Self {
        if name.contains("Apple_HFSX") {
            PartitionType::Hfsx
        } else if name.contains("Apple_HFS") {
            PartitionType::Hfs
        } else if name.contains("Apple_APFS") {
            PartitionType::Apfs
        } else {
            PartitionType::Other
        }
    }

    /// Returns `true` if this partition can be parsed as HFS+
    pub fn is_hfs_compatible(&self) -> bool {
        matches!(self, PartitionType::Hfs | PartitionType::Hfsx)
    }
}

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

/// High-level DMG archive interface
pub struct DmgArchive {
    reader: DmgReader<BufReader<File>>,
}

/// Partition information
#[derive(Debug, Clone)]
pub struct PartitionInfo {
    /// Partition name
    pub name: String,
    /// Partition ID
    pub id: i32,
    /// Number of sectors (512 bytes each)
    pub sectors: u64,
    /// Uncompressed size in bytes
    pub size: u64,
    /// Compressed size in bytes
    pub compressed_size: u64,
    /// Filesystem type detected from partition name
    pub partition_type: PartitionType,
}

impl DmgArchive {
    /// Open a DMG file with default options (checksum verification enabled)
    pub fn open<P: AsRef<Path>>(path: P) -> Result<Self> {
        let reader = DmgReader::open(path)?;
        Ok(DmgArchive { reader })
    }

    /// Open a DMG file with custom options
    pub fn open_with_options<P: AsRef<Path>>(
        path: P,
        options: reader::DmgReaderOptions,
    ) -> Result<Self> {
        let reader = DmgReader::open_with_options(path, options)?;
        Ok(DmgArchive { reader })
    }

    /// Get archive statistics
    pub fn stats(&self) -> DmgStats {
        self.reader.stats()
    }

    /// Get compression info
    pub fn compression_info(&self) -> CompressionInfo {
        self.reader.compression_info()
    }

    /// List all partitions
    pub fn partitions(&self) -> Vec<PartitionInfo> {
        self.reader
            .partitions()
            .iter()
            .map(|p| PartitionInfo {
                name: p.name.clone(),
                id: p.id,
                sectors: p.block_map.sector_count,
                size: p.block_map.uncompressed_size(),
                compressed_size: p.block_map.compressed_size(),
                partition_type: PartitionType::from_partition_name(&p.name),
            })
            .collect()
    }

    /// Get partition by name
    pub fn partition(&self, name: &str) -> Option<PartitionInfo> {
        self.reader.partition(name).map(|p| PartitionInfo {
            name: p.name.clone(),
            id: p.id,
            sectors: p.block_map.sector_count,
            size: p.block_map.uncompressed_size(),
            compressed_size: p.block_map.compressed_size(),
            partition_type: PartitionType::from_partition_name(&p.name),
        })
    }

    /// Extract a partition by ID.
    ///
    /// When the `parallel` feature is enabled, blocks are decompressed in
    /// parallel using rayon for significantly faster extraction.
    pub fn extract_partition(&mut self, id: i32) -> Result<Vec<u8>> {
        self.reader.decompress_partition_auto(id)
    }

    /// Extract a partition by name.
    ///
    /// When the `parallel` feature is enabled, blocks are decompressed in
    /// parallel using rayon for significantly faster extraction.
    pub fn extract_partition_by_name(&mut self, name: &str) -> Result<Vec<u8>> {
        let partition = self
            .reader
            .partition(name)
            .ok_or_else(|| DppError::FileNotFound(name.to_string()))?;
        self.reader.decompress_partition_auto(partition.id)
    }

    /// Extract the main HFS+/APFS partition.
    ///
    /// When the `parallel` feature is enabled, blocks are decompressed in
    /// parallel using rayon for significantly faster extraction.
    pub fn extract_main_partition(&mut self) -> Result<Vec<u8>> {
        self.reader.decompress_main_partition_auto()
    }

    /// Extract all partitions as a raw disk image
    pub fn extract_all(&mut self) -> Result<Vec<u8>> {
        self.reader.decompress_all()
    }

    /// Stream a partition to a writer block-by-block (low memory usage)
    pub fn extract_partition_to<W: std::io::Write>(
        &mut self,
        id: i32,
        writer: &mut W,
    ) -> Result<u64> {
        self.reader.decompress_partition_to(id, writer)
    }

    /// Stream the main HFS+/APFS partition to a writer (low memory usage)
    pub fn extract_main_partition_to<W: std::io::Write>(&mut self, writer: &mut W) -> Result<u64> {
        self.reader.decompress_main_partition_to_auto(writer)
    }

    /// Get the ID of the main HFS+/APFS partition
    pub fn main_partition_id(&self) -> Result<i32> {
        self.reader.main_partition_id()
    }

    /// Get the ID of the main HFS+/HFSX partition (excludes APFS).
    /// Returns `Err(FileNotFound)` if no HFS-compatible partition exists.
    pub fn hfs_partition_id(&self) -> Result<i32> {
        self.reader.hfs_partition_id()
    }

    /// Extract a partition to a file
    pub fn extract_partition_to_file<P: AsRef<Path>>(&mut self, id: i32, path: P) -> Result<()> {
        let data = self.reader.decompress_partition_auto(id)?;
        std::fs::write(path, &data)?;
        Ok(())
    }

    /// Extract main partition to a file
    pub fn extract_main_partition_to_file<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        let data = self.reader.decompress_main_partition_auto()?;
        std::fs::write(path, &data)?;
        Ok(())
    }

    /// Get the raw koly header
    pub fn koly(&self) -> &KolyHeader {
        self.reader.koly()
    }
}

/// Builder for creating DMG files
pub struct DmgBuilder {
    compression: CompressionMethod,
    compression_level: u32,
    chunk_size: usize,
    partitions: Vec<(String, Vec<u8>)>,
    skip_checksums: bool,
}

impl Default for DmgBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl DmgBuilder {
    /// Create a new DMG builder
    pub fn new() -> Self {
        DmgBuilder {
            compression: CompressionMethod::Zlib,
            compression_level: 6,
            chunk_size: 1024 * 1024,
            partitions: Vec::new(),
            skip_checksums: false,
        }
    }

    /// Set compression method
    pub fn compression(mut self, method: CompressionMethod) -> Self {
        self.compression = method;
        self
    }

    /// Set compression level (0-9)
    pub fn compression_level(mut self, level: u32) -> Self {
        self.compression_level = level;
        self
    }

    /// Set chunk size
    pub fn chunk_size(mut self, size: usize) -> Self {
        self.chunk_size = size;
        self
    }

    /// Skip checksum generation for faster DMG creation
    pub fn skip_checksums(mut self, skip: bool) -> Self {
        self.skip_checksums = skip;
        self
    }

    /// Add a partition
    pub fn add_partition(mut self, name: &str, data: Vec<u8>) -> Self {
        self.partitions.push((name.to_string(), data));
        self
    }

    /// Build and write the DMG to a file
    pub fn build<P: AsRef<Path>>(self, path: P) -> Result<()> {
        let mut writer = DmgWriter::create(path)?
            .compression(self.compression)
            .compression_level(self.compression_level)
            .chunk_size(self.chunk_size)
            .skip_checksums(self.skip_checksums);

        for (name, data) in self.partitions {
            writer.add_partition(&name, &data)?;
        }

        writer.finish()
    }
}

/// Quick check if a file is a valid DMG
pub fn check_dmg<P: AsRef<Path>>(path: P) -> bool {
    is_dmg(path)
}

/// Get statistics about a DMG file
pub fn stats<P: AsRef<Path>>(path: P) -> Result<DmgStats> {
    let reader = DmgReader::open(path)?;
    Ok(reader.stats())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[cfg(feature = "parallel")]
mod parallel_tests;
