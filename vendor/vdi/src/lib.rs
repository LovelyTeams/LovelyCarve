//! Read-only parser for VirtualBox VDI disk images.
//!
//! This vendored fork accepts dynamic and fixed VDI 1.1 images. Differencing
//! images deliberately remain unsupported because resolving their parent chain
//! requires external image discovery and UUID validation.

use std::io::{self, Read, Seek, SeekFrom, Write};

use anyhow::{Context, ensure};
use positioned_io2::ReadAt;

use crate::header::VdiHeader;

pub mod header;
pub mod slice;

const IMAGE_TYPE_DYNAMIC: u32 = 1;
const IMAGE_TYPE_FIXED: u32 = 2;
const BLOCK_FREE: u32 = u32::MAX;
const BLOCK_ZERO: u32 = u32::MAX - 1;
const MAX_BLOCK_MAP_ENTRIES: usize = 4 * 1024 * 1024;

pub struct VdiDisk {
    pub header: VdiHeader,
    pub block_size: usize,
    /// Absolute file offsets of allocated block payloads. Sparse and explicit
    /// zero blocks are represented by `None`.
    pub block_offsets: Vec<Option<u64>>,
    reader: Box<dyn ReadAt + Send + Sync>,
    position: u64,
}

impl VdiDisk {
    pub fn open<R>(reader: R) -> anyhow::Result<Self>
    where
        R: ReadAt + Send + Sync + 'static,
    {
        let mut header_bytes = [0_u8; VdiHeader::ENCODED_SIZE];
        reader
            .read_exact_at(0, &mut header_bytes)
            .context("failed to read VDI header")?;
        let header = VdiHeader::decode(&header_bytes)?;

        ensure!(
            header.signature == VdiHeader::SIGNATURE,
            "invalid VDI signature: 0x{:08x}",
            header.signature
        );
        ensure!(
            header.version == VdiHeader::VERSION,
            "unsupported VDI version 0x{:08x}; expected 1.1",
            header.version
        );
        ensure!(
            header.header_size >= VdiHeader::MINIMUM_BODY_SIZE,
            "VDI header body is too small: {} bytes",
            header.header_size
        );
        ensure!(
            matches!(header.image_type, IMAGE_TYPE_DYNAMIC | IMAGE_TYPE_FIXED),
            "unsupported VDI image type {}; only dynamic and fixed images are supported",
            header.image_type
        );
        ensure!(header.disk_size > 0, "VDI virtual disk size is zero");
        ensure!(header.block_size > 0, "VDI block size is zero");
        ensure!(header.sector_size > 0, "VDI sector size is zero");
        ensure!(
            header.blocks_allocated <= header.blocks_in_image,
            "VDI allocated block count exceeds its block map"
        );

        let block_size_u64 = u64::from(header.block_size);
        let required_blocks = header.disk_size.div_ceil(block_size_u64);
        ensure!(
            u64::from(header.blocks_in_image) >= required_blocks,
            "VDI block map does not cover the virtual disk"
        );
        ensure!(
            u64::from(header.block_offsets_offset) >= VdiHeader::ENCODED_SIZE as u64,
            "VDI block map overlaps the parsed header"
        );

        let block_count = usize::try_from(header.blocks_in_image)
            .context("VDI block count does not fit in memory")?;
        ensure!(
            block_count <= MAX_BLOCK_MAP_ENTRIES,
            "VDI block map exceeds the supported 4,194,304 entries"
        );
        let map_size = block_count
            .checked_mul(4)
            .context("VDI block map size overflow")?;
        let map_end = u64::from(header.block_offsets_offset)
            .checked_add(u64::try_from(map_size).context("VDI block map is too large")?)
            .context("VDI block map offset overflow")?;
        ensure!(
            u64::from(header.data_offset) >= map_end,
            "VDI data area overlaps its block map"
        );

        let mut block_offsets_raw = vec![0_u8; map_size];
        reader
            .read_exact_at(
                u64::from(header.block_offsets_offset),
                &mut block_offsets_raw,
            )
            .context("failed to read VDI block map")?;

        let physical_block_size = block_size_u64
            .checked_add(u64::from(header.block_extra))
            .context("VDI physical block size overflow")?;
        let block_offsets = block_offsets_raw
            .chunks_exact(4)
            .map(|chunk| {
                let location = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if matches!(location, BLOCK_FREE | BLOCK_ZERO) {
                    return Ok(None);
                }
                let allocation_offset = u64::from(location)
                    .checked_mul(physical_block_size)
                    .context("VDI block file offset overflow")?;
                let payload_offset = u64::from(header.data_offset)
                    .checked_add(allocation_offset)
                    .and_then(|offset| offset.checked_add(u64::from(header.block_extra)))
                    .context("VDI block payload offset overflow")?;
                Ok(Some(payload_offset))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Self {
            header,
            block_size: usize::try_from(header.block_size)
                .context("VDI block size does not fit this platform")?,
            block_offsets,
            reader: Box::new(reader),
            position: 0,
        })
    }

    pub fn disk_size(&self) -> u64 {
        self.header.disk_size
    }

    pub fn sector_size(&self) -> u32 {
        self.header.sector_size
    }

    pub fn read_at(&self, position: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.read_virtual_at(position, buf)
    }

    pub fn slice(&self, range: std::ops::Range<u64>) -> io::Result<slice::Slice<'_>> {
        slice::Slice::new(self, range)
    }

    pub fn slice_owned(self, range: std::ops::Range<u64>) -> io::Result<slice::OwnedSlice> {
        slice::OwnedSlice::new(self, range)
    }

    fn read_virtual_at(&self, mut position: u64, buf: &mut [u8]) -> io::Result<usize> {
        if position >= self.header.disk_size || buf.is_empty() {
            return Ok(0);
        }

        let remaining_disk =
            usize::try_from(self.header.disk_size - position).unwrap_or(usize::MAX);
        let target_len = buf.len().min(remaining_disk);
        let mut total_read = 0;

        while total_read < target_len {
            let block_index_u64 = position / u64::from(self.header.block_size);
            let block_index = usize::try_from(block_index_u64).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "VDI block index is too large")
            })?;
            let block_offset = usize::try_from(position % u64::from(self.header.block_size))
                .map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "VDI block offset is too large")
                })?;
            let block_file_offset = self.block_offsets.get(block_index).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "VDI block map ended before the virtual disk",
                )
            })?;
            let read_len = (target_len - total_read).min(self.block_size - block_offset);
            let destination = &mut buf[total_read..total_read + read_len];

            if let Some(file_offset) = block_file_offset {
                let physical_offset =
                    file_offset
                        .checked_add(block_offset as u64)
                        .ok_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidData,
                                "VDI block read offset overflow",
                            )
                        })?;
                self.reader
                    .read_exact_at(physical_offset, destination)
                    .map_err(|error| {
                        io::Error::new(
                            error.kind(),
                            format!("failed to read VDI block {block_index}: {error}"),
                        )
                    })?;
            } else {
                destination.fill(0);
            }

            total_read += read_len;
            position += read_len as u64;
        }

        Ok(total_read)
    }
}

impl ReadAt for VdiDisk {
    fn read_at(&self, position: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.read_virtual_at(position, buf)
    }
}

impl Read for VdiDisk {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let read = self.read_virtual_at(self.position, buf)?;
        self.position += read as u64;
        Ok(read)
    }
}

impl Write for VdiDisk {
    fn write(&mut self, _buf: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "VdiDisk does not support write operations",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Seek for VdiDisk {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.position = checked_seek_position(self.position, self.header.disk_size, position)?;
        Ok(self.position)
    }
}

pub(crate) fn checked_seek_position(current: u64, end: u64, position: SeekFrom) -> io::Result<u64> {
    let (base, offset) = match position {
        SeekFrom::Start(offset) => return validate_seek(offset, end),
        SeekFrom::End(offset) => (end, offset),
        SeekFrom::Current(offset) => (current, offset),
    };
    let new_position = if offset >= 0 {
        base.checked_add(offset as u64)
    } else {
        base.checked_sub(offset.unsigned_abs())
    }
    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "seek out of bounds"))?;
    validate_seek(new_position, end)
}

fn validate_seek(position: u64, end: u64) -> io::Result<u64> {
    if position > end {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "seek out of bounds",
        ));
    }
    Ok(position)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct MemoryReader(Vec<u8>);

    impl ReadAt for MemoryReader {
        fn read_at(&self, position: u64, buf: &mut [u8]) -> io::Result<usize> {
            let start = usize::try_from(position)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "offset too large"))?;
            let Some(source) = self.0.get(start..) else {
                return Ok(0);
            };
            let read_len = source.len().min(buf.len());
            buf[..read_len].copy_from_slice(&source[..read_len]);
            Ok(read_len)
        }
    }

    fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }

    fn write_u64(bytes: &mut [u8], offset: usize, value: u64) {
        bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
    }

    fn image(image_type: u32, second_map_entry: u32, block_extra: u32) -> Vec<u8> {
        const MAP_OFFSET: usize = 512;
        const DATA_OFFSET: usize = 1024;
        const BLOCK_SIZE: usize = 512;

        let mut bytes = vec![0_u8; DATA_OFFSET + block_extra as usize + BLOCK_SIZE];
        bytes[..0x40].copy_from_slice(&[b' '; 0x40]);
        write_u32(&mut bytes, 64, VdiHeader::SIGNATURE);
        write_u32(&mut bytes, 68, VdiHeader::VERSION);
        write_u32(&mut bytes, 72, VdiHeader::MINIMUM_BODY_SIZE);
        write_u32(&mut bytes, 76, image_type);
        write_u32(&mut bytes, 340, MAP_OFFSET as u32);
        write_u32(&mut bytes, 344, DATA_OFFSET as u32);
        write_u32(&mut bytes, 360, 512);
        write_u64(&mut bytes, 368, (BLOCK_SIZE * 2) as u64);
        write_u32(&mut bytes, 376, BLOCK_SIZE as u32);
        write_u32(&mut bytes, 380, block_extra);
        write_u32(&mut bytes, 384, 2);
        write_u32(&mut bytes, 388, 1);
        write_u32(&mut bytes, MAP_OFFSET, 0);
        write_u32(&mut bytes, MAP_OFFSET + 4, second_map_entry);

        let payload_start = DATA_OFFSET + block_extra as usize;
        for (index, byte) in bytes[payload_start..payload_start + BLOCK_SIZE]
            .iter_mut()
            .enumerate()
        {
            *byte = (index % 251) as u8;
        }
        bytes
    }

    #[test]
    fn reads_allocated_and_sparse_blocks_and_clamps_at_eof() -> anyhow::Result<()> {
        let disk = VdiDisk::open(MemoryReader(image(IMAGE_TYPE_DYNAMIC, BLOCK_FREE, 0)))?;
        let mut bytes = vec![0xaa; 520];
        let read = disk.read_at(508, &mut bytes)?;

        assert_eq!(read, 516);
        assert_eq!(&bytes[..4], &[6, 7, 8, 9]);
        assert!(bytes[4..516].iter().all(|byte| *byte == 0));
        assert_eq!(&bytes[516..], &[0xaa; 4]);
        Ok(())
    }

    #[test]
    fn honors_block_extra_and_explicit_zero_blocks() -> anyhow::Result<()> {
        let disk = VdiDisk::open(MemoryReader(image(IMAGE_TYPE_FIXED, BLOCK_ZERO, 16)))?;
        let mut first = [0_u8; 8];
        disk.read_at(0, &mut first)?;
        let mut second = [1_u8; 8];
        disk.read_at(512, &mut second)?;

        assert_eq!(first, [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(second, [0; 8]);
        assert_eq!(disk.disk_size(), 1024);
        assert_eq!(disk.sector_size(), 512);
        Ok(())
    }

    #[test]
    fn rejects_differencing_images() {
        let error = VdiDisk::open(MemoryReader(image(4, BLOCK_FREE, 0)))
            .err()
            .expect("differencing VDI should be rejected");
        assert!(error.to_string().contains("unsupported VDI image type 4"));
    }

    #[test]
    fn slices_enforce_guest_bounds() -> anyhow::Result<()> {
        let disk = VdiDisk::open(MemoryReader(image(IMAGE_TYPE_DYNAMIC, BLOCK_FREE, 0)))?;
        assert!(disk.slice(1000..1025).is_err());

        let mut slice = disk.slice(508..516)?;
        let mut bytes = [0xaa; 12];
        assert_eq!(slice.read(&mut bytes)?, 8);
        assert_eq!(&bytes[..8], &[6, 7, 8, 9, 0, 0, 0, 0]);
        assert_eq!(&bytes[8..], &[0xaa; 4]);
        assert_eq!(slice.seek(SeekFrom::End(-2))?, 6);
        Ok(())
    }
}
