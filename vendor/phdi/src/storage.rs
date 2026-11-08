//! Sparse header/BAT checks and read-only source stability.
use crate::{check_cancelled, invalid, ImageType};
use std::fs::{File, Metadata};
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct SourceFile {
    pub path: PathBuf,
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
}
impl SourceFile {
    pub(crate) fn capture(path: &Path) -> io::Result<Self> {
        let path = path.canonicalize()?;
        let metadata = path.metadata()?;
        if !metadata.is_file() {
            return Err(invalid("PHDI extent is not a regular file"));
        }
        Ok(Self {
            path,
            len: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
        })
    }
    fn verify_metadata(&self, metadata: Metadata) -> io::Result<()> {
        if metadata.len() != self.len
            || metadata.modified().ok() != self.modified
            || metadata.created().ok() != self.created
        {
            return Err(invalid(format!(
                "PHDI source changed after open: {}",
                self.path.display()
            )));
        }
        Ok(())
    }
    pub(crate) fn verify(&self) -> io::Result<()> {
        self.verify_metadata(self.path.metadata()?)
    }
    pub(crate) fn verify_file(&self, file: &File) -> io::Result<()> {
        self.verify_metadata(file.metadata()?)
    }
}

#[derive(Clone, Debug)]
pub struct SparseHeader {
    pub version: u32,
    pub heads: u32,
    pub cylinders: u32,
    pub block_size: u64,
    pub bat_entries: u32,
    pub media_size: u64,
    pub creator: [u8; 4],
    pub data_start: u64,
}
impl SparseHeader {
    pub fn read(reader: &mut (impl Read + Seek)) -> io::Result<Self> {
        reader.seek(SeekFrom::Start(0))?;
        let mut bytes = [0; 64];
        reader.read_exact(&mut bytes)?;
        if &bytes[..16] == b"WithouFreSpacExt" {
            return Err(invalid("extended Parallels HDS is unsupported"));
        }
        if &bytes[..16] != b"WithoutFreeSpace" {
            return Err(invalid("invalid Parallels sparse signature"));
        }
        let field = |offset| {
            u32::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
            ])
        };
        let version = field(16);
        if version != 2 {
            return Err(invalid(format!(
                "unsupported Parallels HDS version {version}"
            )));
        }
        let block_size = u64::from(field(28)) * 512;
        let sectors = u64::from_le_bytes(
            bytes[36..44]
                .try_into()
                .map_err(|_| invalid("invalid sector field"))?,
        );
        let media_size = sectors
            .checked_mul(512)
            .ok_or_else(|| invalid("PHDI media size overflow"))?;
        let bat_entries = field(32);
        if block_size == 0
            || media_size == 0
            || u64::from(bat_entries) != media_size.div_ceil(block_size)
        {
            return Err(invalid("PHDI block count/size does not cover disk size"));
        }
        Ok(Self {
            version,
            heads: field(20),
            cylinders: field(24),
            block_size,
            bat_entries,
            media_size,
            creator: bytes[44..48]
                .try_into()
                .map_err(|_| invalid("invalid creator field"))?,
            data_start: u64::from(field(48)) * 512,
        })
    }
}

#[derive(Debug)]
pub(crate) struct Layer {
    pub source_index: usize,
    size: u64,
    sparse: Option<(SparseHeader, Vec<u32>)>,
}
impl Layer {
    pub(crate) fn open(
        source: &SourceFile,
        source_index: usize,
        kind: ImageType,
        size: u64,
        block_size: u64,
        maximum_entries: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Self> {
        let file = File::open(&source.path)?;
        source.verify_file(&file)?;
        let mut file = BufReader::with_capacity(64 * 1024, file);
        let sparse = match kind {
            ImageType::Plain => {
                if source.len < size {
                    return Err(invalid("truncated PHDI plain extent"));
                }
                None
            }
            ImageType::Sparse => {
                let header = SparseHeader::read(&mut file)?;
                if header.media_size != size || header.block_size != block_size {
                    return Err(invalid("PHDI sparse header disagrees with extent geometry"));
                }
                let count = usize::try_from(header.bat_entries)
                    .map_err(|_| invalid("BAT count overflow"))?;
                if count > maximum_entries {
                    return Err(invalid("PHDI BAT exceeds configured entry limit"));
                }
                let table_end = 64 + u64::from(header.bat_entries) * 4;
                let minimum_data = table_end.div_ceil(512) * 512;
                let data_start = if header.data_start == 0 {
                    minimum_data
                } else {
                    header.data_start
                };
                if data_start < minimum_data || data_start > source.len {
                    return Err(invalid("PHDI data start overlaps BAT or exceeds source"));
                }
                let mut entries = Vec::new();
                entries
                    .try_reserve_exact(count)
                    .map_err(|_| invalid("cannot allocate PHDI BAT"))?;
                let mut allocated = Vec::new();
                allocated
                    .try_reserve_exact(count)
                    .map_err(|_| invalid("cannot allocate PHDI allocation ranges"))?;
                for index in 0..count {
                    if index % 4096 == 0 {
                        check_cancelled(cancelled)?;
                    }
                    let mut bytes = [0; 4];
                    file.read_exact(&mut bytes)?;
                    let entry = u32::from_le_bytes(bytes);
                    if entry != 0 {
                        let physical = u64::from(entry) * 512;
                        let logical = index as u64 * header.block_size;
                        let required = (size - logical).min(header.block_size);
                        let end = physical
                            .checked_add(required)
                            .ok_or_else(|| invalid("PHDI allocated range overflow"))?;
                        if physical < data_start || end > source.len {
                            return Err(invalid(
                                "PHDI allocated block overlaps metadata or is truncated",
                            ));
                        }
                        allocated.push((physical, end));
                    }
                    entries.push(entry);
                }
                allocated.sort_unstable();
                if allocated.windows(2).any(|pair| pair[0].1 > pair[1].0) {
                    return Err(invalid("PHDI allocated blocks overlap"));
                }
                Some((header, entries))
            }
        };
        Ok(Self {
            source_index,
            size,
            sparse,
        })
    }

    pub(crate) fn map(&self, offset: u64) -> io::Result<(Option<u64>, u64)> {
        if offset >= self.size {
            return Err(invalid("PHDI layer read outside extent"));
        }
        if let Some((header, entries)) = &self.sparse {
            let index = usize::try_from(offset / header.block_size)
                .map_err(|_| invalid("PHDI BAT index overflow"))?;
            let entry = *entries
                .get(index)
                .ok_or_else(|| invalid("missing PHDI BAT entry"))?;
            let within = offset % header.block_size;
            let available = (header.block_size - within).min(self.size - offset);
            Ok((
                (entry != 0).then_some(u64::from(entry) * 512 + within),
                available,
            ))
        } else {
            Ok((Some(offset), self.size - offset))
        }
    }
}
