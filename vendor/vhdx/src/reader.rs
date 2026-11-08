use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use crate::backing::{Backing, ReadSeekSend};
use crate::bat::{Bat, ReadTarget};
use crate::error::{Result, VhdxError};
use crate::header::{parse_active_header, REGION_TABLE1_OFFSET, REGION_TABLE2_OFFSET};
use crate::log::LogOverlay;
use crate::metadata::{parse_metadata, VhdxMetadata};
use crate::region::parse_region_table;
use crate::FILE_MAGIC;

/// Read-only VHDX container reader.
///
/// Implements `Read + Seek` over the virtual sector stream.
///
/// # Bounded memory
/// The reader holds the BAT (a small `Vec<u64>`), the parsed metadata, and a
/// log overlay (empty for a clean image, ~log-sized for a dirty one) — never the
/// whole container. Block bytes are fetched on demand from a positioned
/// [`Backing`], so peak RSS does not scale with image size. [`VhdxReader::open`]
/// takes the bounded [`Backing::File`] path; [`VhdxReader::from_bytes`] keeps the
/// legacy in-RAM path (backed by [`Backing::Mem`]).
#[derive(Debug)]
pub struct VhdxReader {
    backing: Backing,
    overlay: LogOverlay,
    bat: Bat,
    meta: VhdxMetadata,
    pos: u64,
    parent: Option<Box<VhdxReader>>,
}

/// Minimum container size: covers magic, both headers, and both region tables.
const MIN_CONTAINER_SIZE: u64 = 0x0025_0000;

impl VhdxReader {
    /// Open a VHDX container from a path with **bounded** memory.
    ///
    /// Reads only the small fixed structures (headers, region table, metadata,
    /// BAT, and — for a dirty image — the log region) at construction; the
    /// payload blocks are read on demand. A 2 TB image no longer means a 2 TB
    /// heap.
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        Self::from_backing(Backing::File(file), None)
    }

    /// Construct a reader over an arbitrary positioned [`Backing`].
    ///
    /// The escape hatch behind [`open`](Self::open) (and the issen zip bridge):
    /// a [`Backing::Sub`] reads a STORED zip entry in place, a [`Backing::Mem`]
    /// reads inflated/owned bytes. Parsing and reads are identical across all
    /// three backings.
    pub fn from_backing(backing: Backing, parent: Option<Box<VhdxReader>>) -> Result<Self> {
        Self::parse_backing(backing, parent)
    }

    /// Open a VHDX container from owned bytes (legacy in-RAM path).
    ///
    /// The whole buffer is held in RAM (via [`Backing::Mem`]) and the dirty log,
    /// if any, is replayed in place before parsing — byte-identical to the
    /// historical behaviour. Prefer [`open`](Self::open) for files on disk.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self> {
        Self::parse_bytes(data, None)
    }

    /// Open a differencing (child) disk with its parent chain (in-RAM path).
    ///
    /// Reads absent blocks from `parent` instead of returning zeros.
    pub fn from_bytes_with_parent(data: Vec<u8>, parent: VhdxReader) -> Result<Self> {
        Self::parse_bytes(data, Some(Box::new(parent)))
    }

    /// Open a differencing (child) disk over an arbitrary backing with its
    /// parent chain.
    pub fn from_backing_with_parent(backing: Backing, parent: VhdxReader) -> Result<Self> {
        Self::parse_backing(backing, Some(Box::new(parent)))
    }

    /// Open a VHDX container from any boxed `Read + Seek + Send` reader.
    ///
    /// This is the entry point used by the forensic-vfs engine, which hands a
    /// `SourceCursor` (a `Read + Seek + Send`) to the reader. The `len` is
    /// determined once at construction by seeking to the end of the reader;
    /// subsequent `read_at` calls lock a mutex, seek, and read — no mmap,
    /// no unsafe, fully `forbid(unsafe)` compliant.
    ///
    /// # Errors
    /// Returns the same errors as [`open`](Self::open): [`VhdxError::BadMagic`]
    /// if the stream does not start with the VHDX file magic, and
    /// [`VhdxError::ContainerTooSmall`] if the stream is shorter than the
    /// minimum VHDX container size.
    pub fn open_reader(mut reader: Box<dyn ReadSeekSend>) -> Result<Self> {
        // Measure the length once up front by seeking to the end. A seek error
        // here is surfaced as an I/O error wrapped in VhdxError.
        let len = reader
            .seek(std::io::SeekFrom::End(0))
            .map_err(VhdxError::Io)?;
        reader
            .seek(std::io::SeekFrom::Start(0))
            .map_err(VhdxError::Io)?;
        let inner = std::sync::Mutex::new(reader);
        Self::from_backing(Backing::Reader { inner, len }, None)
    }

    /// In-RAM parse: replay the log in place, then parse from the buffer and
    /// keep it as the [`Backing::Mem`].
    fn parse_bytes(mut data: Vec<u8>, parent: Option<Box<VhdxReader>>) -> Result<Self> {
        if data.len() < 8 || &data[0..8] != FILE_MAGIC {
            return Err(VhdxError::BadMagic);
        }
        if (data.len() as u64) < MIN_CONTAINER_SIZE {
            return Err(VhdxError::ContainerTooSmall(MIN_CONTAINER_SIZE));
        }
        crate::log::apply(&mut data)?;
        // The log is already folded into `data`, so the overlay is empty and
        // the backing serves the committed bytes directly.
        let backing = Backing::from_bytes(data);
        Self::assemble(backing, LogOverlay::default(), parent)
    }

    /// Bounded parse: read the small fixed structures via positioned reads, then
    /// build the log overlay (empty for a clean image).
    fn parse_backing(backing: Backing, parent: Option<Box<VhdxReader>>) -> Result<Self> {
        let mut magic = [0u8; 8];
        let n = backing.read_at(&mut magic, 0)?;
        if n < 8 || &magic != FILE_MAGIC {
            return Err(VhdxError::BadMagic);
        }
        if backing.len() < MIN_CONTAINER_SIZE {
            return Err(VhdxError::ContainerTooSmall(MIN_CONTAINER_SIZE));
        }
        let overlay = {
            // The header lives in the first 1 MB; read that slice to select the
            // active header and (if dirty) drive the log overlay.
            let head = backing.read_exact_at(0, MIN_CONTAINER_SIZE as usize)?;
            let header = parse_active_header(&head)?;
            crate::log::build_overlay(&backing, &header)?
        };
        Self::assemble(backing, overlay, parent)
    }

    /// Shared tail: parse region table → metadata → BAT from small positioned
    /// reads (the overlay patches any log-dirtied structure bytes), validate,
    /// and assemble the reader.
    fn assemble(
        backing: Backing,
        overlay: LogOverlay,
        parent: Option<Box<VhdxReader>>,
    ) -> Result<Self> {
        // Region tables sit at fixed offsets within the first 1 MB; read that
        // prefix once (small, bounded) and parse the region table from it,
        // applying the log overlay so a log-patched region table is honoured.
        let mut head = backing.read_exact_at(0, MIN_CONTAINER_SIZE as usize)?;
        overlay.patch(&mut head, 0);

        let container_len = backing.len();
        let regions = parse_region_table(&head, REGION_TABLE1_OFFSET as usize, container_len)
            .or_else(|_| parse_region_table(&head, REGION_TABLE2_OFFSET as usize, container_len))?;

        // Metadata + BAT can live past the first 1 MB; read each region by its
        // own offset/length and patch with the overlay.
        let meta_region = read_region(
            &backing,
            &overlay,
            regions.metadata.file_offset,
            regions.metadata.length,
        )?;
        let meta = parse_metadata(&meta_region, 0, regions.metadata.length)?;
        meta.validate()?;
        if meta.has_parent && parent.is_none() {
            return Err(VhdxError::DifferencingNotSupported);
        }

        let bat_region = read_region(
            &backing,
            &overlay,
            regions.bat.file_offset,
            regions.bat.length,
        )?;
        let bat = Bat::parse(&bat_region, 0, regions.bat.length, meta.clone())?;

        Ok(Self {
            backing,
            overlay,
            bat,
            meta,
            pos: 0,
            parent,
        })
    }

    pub fn virtual_disk_size(&self) -> u64 {
        self.meta.virtual_disk_size
    }

    pub fn logical_sector_size(&self) -> u32 {
        self.meta.logical_sector_size
    }
}

/// Read a region `[file_offset, file_offset+length)` from the backing into a
/// fresh buffer and apply the log overlay, so region/metadata/BAT parsing sees
/// the committed state — exactly as the in-place `apply` path would.
///
/// The returned buffer is offset-zero based, so callers parse with offset `0`.
fn read_region(
    backing: &Backing,
    overlay: &LogOverlay,
    file_offset: u64,
    length: u32,
) -> Result<Vec<u8>> {
    let end = file_offset.saturating_add(u64::from(length));
    if backing.len() < end {
        return Err(VhdxError::OffsetOutOfBounds);
    }
    let mut buf = backing.read_exact_at(file_offset, length as usize)?;
    if buf.len() < length as usize {
        return Err(VhdxError::OffsetOutOfBounds);
    }
    overlay.patch(&mut buf, file_offset);
    Ok(buf)
}

impl Read for VhdxReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.meta.virtual_disk_size {
            return Ok(0);
        }
        let remaining = self.meta.virtual_disk_size - self.pos;
        let to_read = buf.len().min(remaining as usize);
        let block_size = u64::from(self.meta.block_size);
        let mut written = 0;

        while written < to_read {
            let virtual_byte = self.pos + written as u64;
            let block_end = ((virtual_byte / block_size) + 1) * block_size;
            let mut this_chunk = (to_read - written).min((block_end - virtual_byte) as usize);

            match self
                .bat
                .read_target_for_byte(virtual_byte)
                .map_err(|e| io::Error::other(e.to_string()))?
            {
                ReadTarget::File(file_offset) => {
                    self.read_file_chunk(&mut buf[written..written + this_chunk], file_offset)?;
                }
                ReadTarget::Parent => {
                    self.read_parent_chunk(&mut buf[written..written + this_chunk], virtual_byte)?;
                }
                ReadTarget::Zero => {
                    buf[written..written + this_chunk].fill(0);
                }
                ReadTarget::Partial {
                    file_offset,
                    bitmap_byte_file_offset,
                    bitmap_bit,
                } => {
                    let mut bitmap = [0u8; 1];
                    let n = self.backing.read_at(&mut bitmap, bitmap_byte_file_offset)?;
                    if n < bitmap.len() {
                        return Err(io::Error::new(
                            io::ErrorKind::UnexpectedEof,
                            "VHDX sector bitmap truncated",
                        ));
                    }
                    self.overlay.patch(&mut bitmap, bitmap_byte_file_offset);

                    // One bitmap byte describes eight consecutive logical
                    // sectors, and byte boundaries never straddle a block
                    // because sectors-per-block is always a multiple of eight.
                    // Serve the whole run of following sectors that share this
                    // sector's owner, so a partial block costs one bitmap read
                    // per run rather than one per sector.
                    let owned_by_child = bitmap[0] & (1 << bitmap_bit) != 0;
                    let mut run_sectors = 1u64;
                    for bit in (bitmap_bit + 1)..8 {
                        if (bitmap[0] & (1 << bit) != 0) != owned_by_child {
                            break;
                        }
                        run_sectors += 1;
                    }

                    let logical_sector_size = u64::from(self.meta.logical_sector_size);
                    let run_end =
                        ((virtual_byte / logical_sector_size) + run_sectors) * logical_sector_size;
                    this_chunk = this_chunk.min((run_end - virtual_byte) as usize);

                    if owned_by_child {
                        self.read_file_chunk(&mut buf[written..written + this_chunk], file_offset)?;
                    } else {
                        self.read_parent_chunk(
                            &mut buf[written..written + this_chunk],
                            virtual_byte,
                        )?;
                    }
                }
            }
            written += this_chunk;
        }

        self.pos += written as u64;
        Ok(written)
    }
}

impl VhdxReader {
    fn read_file_chunk(&self, dst: &mut [u8], file_offset: u64) -> io::Result<()> {
        let n = self.backing.read_at(dst, file_offset)?;
        if n < dst.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "VHDX data truncated",
            ));
        }
        self.overlay.patch(dst, file_offset);
        Ok(())
    }

    /// Serve bytes this image does not describe: from the parent when there is
    /// one, otherwise zeros.
    fn read_parent_chunk(&mut self, dst: &mut [u8], virtual_byte: u64) -> io::Result<()> {
        if let Some(ref mut parent) = self.parent {
            parent
                .seek(SeekFrom::Start(virtual_byte))
                .map_err(io::Error::other)?;
            parent.read_exact(dst)
        } else {
            dst.fill(0);
            Ok(())
        }
    }
}

impl Seek for VhdxReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let new_pos = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::Current(n) => self.pos as i64 + n,
            SeekFrom::End(n) => self.meta.virtual_disk_size as i64 + n,
        };
        if new_pos < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start",
            ));
        }
        self.pos = new_pos as u64;
        Ok(self.pos)
    }
}
