//! source implementation.
use super::*;

pub(super) enum ImageSource {
    /// Saved evidence identity without a byte source. Reads always fail.
    Indexed {
        format: ImageFormat,
        size_bytes: u64,
    },
    Raw {
        mmap: Mmap,
        size_bytes: u64,
        format: ImageFormat,
    },
    /// Read-only split RAW; positioned I/O spans all source members.
    SplitRaw {
        reader: smraw::RawImage,
    },
    Ewf {
        // parking_lot's eventual-fairness lock: under sustained contention
        // (e.g. a long fs_digest hashing loop racing fs_children), it
        // periodically hands the lock to a waiting thread instead of
        // letting the just-released thread re-acquire immediately. std's
        // Windows SRWLock has no such fairness so navigation IPCs would
        // starve for hundreds of ms at a time.
        reader: Box<FairMutex<EwfHandle>>,
        size_bytes: u64,
        sector_size: usize,
    },
    Vmdk {
        reader: FairMutex<VmdkHandle>,
        size_bytes: u64,
    },
    Qcow2 {
        reader: qcow2::Qcow2<File>,
        size_bytes: u64,
    },
    Phdi {
        reader: phdi::PhdiImage,
    },
    Vdi {
        reader: vdi::VdiDisk,
        size_bytes: u64,
        sector_size: usize,
    },
    VhdFamily {
        reader: FairMutex<VhdHandle>,
        size_bytes: u64,
        sector_size: usize,
        format: ImageFormat,
    },
    /// Apple disk image (UDIF / DMG). The container's main HFS+ / HFSX /
    /// APFS partition is decompressed into a tempfile at open time and
    /// mmap'd here — every downstream layer (partition detection,
    /// volume adapters, hex view) sees the extracted bytes as if they were
    /// a plain RAW image. `_tempfile` keeps the file alive on disk for as
    /// long as `mmap` is borrowing it; `Mmap`'s docs require the backing
    /// file to outlive the mapping, otherwise the reader dereferences a
    /// pointer to a deleted handle. It is an anonymous delete-on-close file,
    /// so the OS removes it even if the process exits without dropping it.
    Dmg {
        mmap: Mmap,
        size_bytes: u64,
        _tempfile: File,
    },
    Pmfx {
        reader: FairMutex<PmfxImage>,
        size_bytes: u64,
        sector_size: usize,
    },
    /// Block device assembled at runtime (e.g. mdadm RAID0 over LVM LVs
    /// over multiple physical images). Reads are forwarded to the inner
    /// `SectorSource` — typically a `Raid0Source` chain that resolves down
    /// to the original `DiskImage` mmaps. The `label` is a human-readable
    /// hint surfaced to the frontend (`mac:0`, etc.); the synthetic path
    /// `assembled://<label>` keeps `ImageInfo::path` populated.
    Assembled {
        source: Arc<dyn SectorSource + Send + Sync>,
        size_bytes: u64,
        label: String,
    },
    /// Docker / OCI image archive. Reads are served from the outer tar
    /// mmap held inside the `DockerImage`, so the hex view still sees the
    /// raw on-disk bytes; navigation and file reads go through the merged
    /// container filesystem in `core::filesystem::tree::fs_tree`'s docker dispatch branch.
    Docker {
        image: Arc<DockerImage>,
    },
    /// Plain `.tar` archive. Reads are served from the outer-tar mmap held
    /// inside the `TarImage` (so hex sees the raw on-disk bytes); navigation
    /// and file reads go through the merged file tree in `core::filesystem::tree::fs_tree`'s tar
    /// dispatch branch.
    Tar {
        image: Arc<TarImage>,
    },
    /// `.zip` archive. Reads (`read_range`) are served from the outer-zip mmap
    /// held inside the `ZipImage` (so hex sees the raw on-disk bytes);
    /// navigation and file reads go through the merged file tree in
    /// `core::filesystem::tree::fs_tree`'s zip dispatch branch, which decompresses members.
    Zip {
        image: Arc<ZipImage>,
    },
    /// `.7z` archive. Reads (`read_range`) are served from the outer 7z mmap
    /// for hex / raw export; navigation and file reads go through the logical
    /// archive tree and decompress members on demand.
    Sevenz {
        image: Arc<SevenzImage>,
    },
    /// AD1 logical image. Reads (`read_range`) are served from the segment
    /// mmaps held inside the `Ad1Image`, back to back; navigation and file
    /// reads go through the item tree and inflate data chunks on demand.
    Ad1 {
        image: Arc<Ad1Image>,
    },
    /// Host-attached physical disk opened via positioned reads (no mmap —
    /// Windows physical-drive handles can't be mmap'd, and block devices
    /// on Linux/macOS don't have a stable in-memory representation). The
    /// `FairMutex<File>` serialises seek+read calls against the same OS
    /// handle while keeping the per-call lock window short so concurrent
    /// fs_children / fs_stat IPCs interleave fairly.
    Block {
        reader: FairMutex<File>,
        size_bytes: u64,
        sector_size: u32,
        path: PathBuf,
    },
    /// Folder picked by the user as a logical-evidence source. Reads go
    /// through `core::filesystem::tree::fs_tree`'s folder branch rather than this enum, but
    /// the variant still tracks total-size for `ImageInfo::size_bytes` so
    /// the workspace shows something coherent next to other images.
    Folder {
        image: Arc<FolderImage>,
        size_bytes: u64,
    },
    /// LDPlayer `.ldbk` backup. `source` is the concatenated, native VMDK-backed
    /// stream over the extracted `*.vmdk` disks (it also owns the extracted
    /// tempfiles, so the bytes stay alive as long as this variant does); reads
    /// forward straight to it. See `core::evidence::readers::ldplayer_image`.
    Ldplayer {
        source: Arc<dyn SectorSource + Send + Sync>,
        size_bytes: u64,
    },
}

impl std::fmt::Debug for ImageSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Indexed { format, size_bytes } => f
                .debug_struct("Indexed")
                .field("format", format)
                .field("size_bytes", size_bytes)
                .finish(),
            Self::Raw {
                size_bytes, format, ..
            } => f
                .debug_struct("Raw")
                .field("size_bytes", size_bytes)
                .field("format", format)
                .finish(),
            Self::SplitRaw { reader } => f
                .debug_struct("SplitRaw")
                .field("size_bytes", &reader.media_size())
                .field("segments", &reader.segments().len())
                .finish(),
            Self::Ewf {
                size_bytes,
                sector_size,
                ..
            } => f
                .debug_struct("Ewf")
                .field("size_bytes", size_bytes)
                .field("sector_size", sector_size)
                .finish(),
            Self::Vmdk { size_bytes, .. } => f
                .debug_struct("Vmdk")
                .field("size_bytes", size_bytes)
                .finish(),
            Self::Qcow2 { size_bytes, .. } => f
                .debug_struct("Qcow2")
                .field("size_bytes", size_bytes)
                .finish(),
            Self::Phdi { reader } => f
                .debug_struct("Phdi")
                .field("size_bytes", &reader.media_size())
                .finish(),
            Self::Vdi {
                size_bytes,
                sector_size,
                ..
            } => f
                .debug_struct("Vdi")
                .field("size_bytes", size_bytes)
                .field("sector_size", sector_size)
                .finish(),
            Self::VhdFamily {
                size_bytes,
                sector_size,
                format,
                ..
            } => f
                .debug_struct("VhdFamily")
                .field("size_bytes", size_bytes)
                .field("sector_size", sector_size)
                .field("format", format)
                .finish(),
            Self::Dmg { size_bytes, .. } => f
                .debug_struct("Dmg")
                .field("size_bytes", size_bytes)
                .finish(),
            Self::Pmfx {
                size_bytes,
                sector_size,
                ..
            } => f
                .debug_struct("Pmfx")
                .field("size_bytes", size_bytes)
                .field("sector_size", sector_size)
                .finish(),
            Self::Assembled {
                size_bytes, label, ..
            } => f
                .debug_struct("Assembled")
                .field("size_bytes", size_bytes)
                .field("label", label)
                .finish(),
            Self::Docker { image } => f
                .debug_struct("Docker")
                .field("path", &image.path())
                .field("size_bytes", &image.outer_len())
                .finish(),
            Self::Tar { image } => f
                .debug_struct("Tar")
                .field("path", &image.path())
                .field("size_bytes", &image.outer_len())
                .finish(),
            Self::Zip { image } => f
                .debug_struct("Zip")
                .field("path", &image.path())
                .field("size_bytes", &image.outer_len())
                .finish(),
            Self::Sevenz { image } => f
                .debug_struct("Sevenz")
                .field("path", &image.path())
                .field("size_bytes", &image.outer_len())
                .finish(),
            Self::Ad1 { image } => f
                .debug_struct("Ad1")
                .field("path", &image.path())
                .field("size_bytes", &image.outer_len())
                .finish(),
            Self::Block {
                size_bytes,
                sector_size,
                path,
                ..
            } => f
                .debug_struct("Block")
                .field("path", path)
                .field("size_bytes", size_bytes)
                .field("sector_size", sector_size)
                .finish(),
            Self::Folder {
                image, size_bytes, ..
            } => f
                .debug_struct("Folder")
                .field("root", &image.root_path())
                .field("size_bytes", size_bytes)
                .finish(),
            Self::Ldplayer { size_bytes, .. } => f
                .debug_struct("Ldplayer")
                .field("size_bytes", size_bytes)
                .finish(),
        }
    }
}

impl ImageSource {
    pub(super) fn format(&self) -> ImageFormat {
        match self {
            Self::Indexed { format, .. } => *format,
            Self::Raw { format, .. } => *format,
            Self::SplitRaw { .. } => ImageFormat::Raw,
            Self::Ewf { .. } => ImageFormat::Ewf,
            Self::Vmdk { .. } => ImageFormat::Vmdk,
            Self::Qcow2 { .. } => ImageFormat::Qcow2,
            Self::Phdi { .. } => ImageFormat::Phdi,
            Self::Vdi { .. } => ImageFormat::Vdi,
            Self::VhdFamily { format, .. } => *format,
            Self::Dmg { .. } => ImageFormat::Dmg,
            Self::Pmfx { .. } => ImageFormat::Pmfx,
            Self::Assembled { .. } => ImageFormat::Assembled,
            Self::Docker { .. } => ImageFormat::Docker,
            Self::Tar { .. } => ImageFormat::Tar,
            Self::Zip { .. } => ImageFormat::Zip,
            Self::Sevenz { .. } => ImageFormat::Sevenz,
            Self::Ad1 { .. } => ImageFormat::Ad1,
            Self::Block { .. } => ImageFormat::PhysicalDisk,
            Self::Folder { .. } => ImageFormat::Folder,
            Self::Ldplayer { .. } => ImageFormat::Ldplayer,
        }
    }

    pub(super) fn len(&self) -> u64 {
        match self {
            Self::Indexed { size_bytes, .. } => *size_bytes,
            Self::Raw { size_bytes, .. } => *size_bytes,
            Self::SplitRaw { reader } => reader.media_size(),
            Self::Ewf { size_bytes, .. } => *size_bytes,
            Self::Vmdk { size_bytes, .. } => *size_bytes,
            Self::Qcow2 { size_bytes, .. } => *size_bytes,
            Self::Phdi { reader } => reader.media_size(),
            Self::Vdi { size_bytes, .. } => *size_bytes,
            Self::VhdFamily { size_bytes, .. } => *size_bytes,
            Self::Dmg { size_bytes, .. } => *size_bytes,
            Self::Pmfx { size_bytes, .. } => *size_bytes,
            Self::Assembled { size_bytes, .. } => *size_bytes,
            Self::Docker { image } => image.outer_len(),
            Self::Tar { image } => image.outer_len(),
            Self::Zip { image } => image.outer_len(),
            Self::Sevenz { image } => image.outer_len(),
            Self::Ad1 { image } => image.outer_len(),
            Self::Block { size_bytes, .. } => *size_bytes,
            Self::Folder { size_bytes, .. } => *size_bytes,
            Self::Ldplayer { size_bytes, .. } => *size_bytes,
        }
    }

    /// Whether this source benefits from the located block cache — true for
    /// FFI/native readers whose every miss costs a decompress or sparse-map
    /// lookup. Mmap-backed and trivial sources read straight through.
    pub(super) fn is_ffi_backed(&self) -> bool {
        match self {
            Self::Ewf { .. }
            | Self::Vmdk { .. }
            | Self::Qcow2 { .. }
            | Self::Phdi { .. }
            | Self::Vdi { .. }
            | Self::VhdFamily { .. }
            | Self::Pmfx { .. } => true,
            Self::Indexed { .. }
            | Self::Raw { .. }
            | Self::SplitRaw { .. }

            | Self::Dmg { .. }
            | Self::Assembled { .. }
            | Self::Docker { .. }
            | Self::Tar { .. }
            // Zip's `read_range` serves raw outer-archive bytes (hex view) from
            // the mmap; file-content reads go through the adapter's own
            // decompression path, not `read_range`. Either way no per-read
            // decompress happens here, so the located block cache buys nothing.
            | Self::Zip { .. }
            | Self::Sevenz { .. }
            | Self::Ad1 { .. }
            | Self::Block { .. }
            | Self::Folder { .. }
            // mmap- (stored) or RAM- (compressed) backed via the sparse VMDK
            // reader — no per-read decompress, so the located block cache buys
            // nothing here.
            | Self::Ldplayer { .. } => false,
        }
    }

    pub(super) fn sector_size(&self) -> usize {
        match self {
            Self::Phdi { reader } => reader.sector_size() as usize,
            Self::VhdFamily { sector_size, .. }
            | Self::Pmfx { sector_size, .. }
            | Self::Ewf { sector_size, .. }
            | Self::Vdi { sector_size, .. } => *sector_size,
            Self::Block { sector_size, .. } => *sector_size as usize,
            Self::Raw {
                format: ImageFormat::XiaomiFirmware | ImageFormat::AndroidBackup,
                ..
            } => 1,
            _ => 512,
        }
    }

    pub(super) fn read_range(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_range_traced(offset, len).map(|(buf, _)| buf)
    }

    /// Like [`Self::read_range_traced`], but fills a caller-owned buffer and
    /// returns the number of bytes written. This avoids one temporary `Vec` on
    /// mmap-backed sources and lets FFI readers write straight into the
    /// sequential I/O buffer used by hash/export loops.
    pub(super) fn read_into_range_traced(
        &self,
        offset: u64,
        dst: &mut [u8],
    ) -> anyhow::Result<(usize, Vec<BadRegion>)> {
        if matches!(self, Self::Indexed { .. }) {
            anyhow::bail!(INDEX_ONLY_READ_ERROR);
        }
        if let Some(reader) = self.forced_raw_reader() {
            return self.read_forced_raw(reader, offset, dst);
        }
        let image_len = self.len();
        let start = offset.min(image_len);
        let available = image_len.saturating_sub(start).min(dst.len() as u64) as usize;
        if available == 0 {
            return Ok((0, Vec::new()));
        }
        let dst = &mut dst[..available];

        match self {
            Self::Indexed { .. } => anyhow::bail!(INDEX_ONLY_READ_ERROR),
            Self::Raw { mmap, .. } => {
                let start = usize::try_from(start)
                    .context("image offset does not fit in memory address space")?;
                dst.copy_from_slice(&mmap[start..start + available]);
                Ok((available, Vec::new()))
            }
            Self::SplitRaw { reader } => {
                let filled = reader.read_at(start, dst)?;
                Ok((filled, Vec::new()))
            }
            Self::Phdi { reader } => {
                let filled = reader
                    .read_at(start, dst)
                    .with_context(|| format!("failed to read PHDI at offset {start}"))?;
                Ok((filled, Vec::new()))
            }
            Self::Dmg { mmap, .. } => {
                let start = usize::try_from(start)
                    .context("image offset does not fit in memory address space")?;
                dst.copy_from_slice(&mmap[start..start + available]);
                Ok((available, Vec::new()))
            }
            Self::Ewf { reader, .. } => {
                let (filled, faults) = tolerant_fill(start, dst, |off, dst| {
                    let reader = reader.lock();
                    reader.read_at(off, dst)
                });
                Ok((filled, faults))
            }
            Self::Vmdk { reader, .. } => {
                let (filled, faults) = tolerant_fill(start, dst, |off, dst| {
                    let mut reader = reader.lock();
                    reader.read_at(off, dst)
                });
                Ok((filled, faults))
            }
            Self::Qcow2 { reader, .. } => {
                let filled = reader
                    .read_at(start, dst)
                    .with_context(|| format!("failed to read QCOW2 at offset {start}"))?;
                Ok((filled, Vec::new()))
            }
            Self::Vdi { reader, .. } => {
                let filled = reader
                    .read_at(start, dst)
                    .with_context(|| format!("failed to read VDI at offset {start}"))?;
                Ok((filled, Vec::new()))
            }
            Self::VhdFamily { reader, .. } => {
                let (filled, faults) = tolerant_fill(start, dst, |off, dst| {
                    let mut reader = reader.lock();
                    reader.read_at(off, dst)
                });
                Ok((filled, faults))
            }
            Self::Pmfx { reader, .. } => {
                let (filled, faults) = tolerant_fill(start, dst, |off, dst| {
                    let mut reader = reader.lock();
                    reader.read_at(off, dst)
                });
                Ok((filled, faults))
            }
            Self::Docker { image } => {
                let start = usize::try_from(start)
                    .context("image offset does not fit in memory address space")?;
                dst.copy_from_slice(&image.outer_bytes()[start..start + available]);
                Ok((available, Vec::new()))
            }
            Self::Tar { image } => {
                let start = usize::try_from(start)
                    .context("image offset does not fit in memory address space")?;
                dst.copy_from_slice(&image.outer_bytes()[start..start + available]);
                Ok((available, Vec::new()))
            }
            Self::Zip { image } => {
                let start = usize::try_from(start)
                    .context("image offset does not fit in memory address space")?;
                dst.copy_from_slice(&image.outer_bytes()[start..start + available]);
                Ok((available, Vec::new()))
            }
            Self::Sevenz { image } => {
                let start = usize::try_from(start)
                    .context("image offset does not fit in memory address space")?;
                dst.copy_from_slice(&image.outer_bytes()[start..start + available]);
                Ok((available, Vec::new()))
            }
            Self::Ad1 { image } => Ok((image.read_outer(start, dst), Vec::new())),
            Self::Assembled { .. }
            | Self::Ldplayer { .. }
            | Self::Block { .. }
            | Self::Folder { .. } => {
                let (buf, faults) = self.read_range_traced(start, available)?;
                let filled = buf.len().min(dst.len());
                dst[..filled].copy_from_slice(&buf[..filled]);
                Ok((filled, faults))
            }
        }
    }

    /// Like [`Self::read_range`], but also returns the absolute-offset spans
    /// that had to be fabricated as zeros because the backend could not read
    /// them. Bad-sector-tolerant readers and explicitly forced RAW produce
    /// faults; assembled devices pass on the faults of their members.
    pub(super) fn read_range_traced(
        &self,
        offset: u64,
        len: usize,
    ) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        if matches!(self, Self::Indexed { .. }) {
            anyhow::bail!(INDEX_ONLY_READ_ERROR);
        }
        if let Some(reader) = self.forced_raw_reader() {
            let mut output = vec![0; self.read_length(offset, len)];
            let (filled, faults) = self.read_forced_raw(reader, offset, &mut output)?;
            output.truncate(filled);
            return Ok((output, faults));
        }
        let image_len = self.len();
        let start = offset.min(image_len);
        let available = image_len.saturating_sub(start).min(len as u64) as usize;
        if available == 0 {
            return Ok((Vec::new(), Vec::new()));
        }

        match self {
            Self::Indexed { .. } => anyhow::bail!(INDEX_ONLY_READ_ERROR),
            Self::Raw { mmap, .. } => {
                let start = usize::try_from(start)
                    .context("image offset does not fit in memory address space")?;
                Ok((mmap[start..start + available].to_vec(), Vec::new()))
            }
            Self::SplitRaw { reader } => {
                let mut buffer = vec![0; available];
                let filled = reader.read_at(start, &mut buffer)?;
                buffer.truncate(filled);
                Ok((buffer, Vec::new()))
            }
            Self::Dmg { mmap, .. } => {
                let start = usize::try_from(start)
                    .context("image offset does not fit in memory address space")?;
                Ok((mmap[start..start + available].to_vec(), Vec::new()))
            }
            Self::Ewf { reader, .. } => {
                // Keep the existing fair read scheduling around the pure Rust decoder.
                let mut buf = vec![0u8; available];
                let (filled, faults) = tolerant_fill(start, &mut buf, |off, dst| {
                    let reader = reader.lock();
                    reader.read_at(off, dst)
                });
                buf.truncate(filled);
                Ok((buf, faults))
            }
            Self::Vmdk { reader, .. } => {
                // Keep each decoder lock scoped to one offset-addressed read.
                let mut buf = vec![0u8; available];
                let (filled, faults) = tolerant_fill(start, &mut buf, |off, dst| {
                    let mut reader = reader.lock();
                    reader.read_at(off, dst)
                });
                buf.truncate(filled);
                Ok((buf, faults))
            }
            Self::Qcow2 { reader, .. } => {
                let mut buf = vec![0u8; available];
                let filled = reader
                    .read_at(start, &mut buf)
                    .with_context(|| format!("failed to read QCOW2 at offset {start}"))?;
                buf.truncate(filled);
                Ok((buf, Vec::new()))
            }
            Self::Phdi { reader } => {
                let mut buf = vec![0u8; available];
                let filled = reader
                    .read_at(start, &mut buf)
                    .with_context(|| format!("failed to read PHDI at offset {start}"))?;
                buf.truncate(filled);
                Ok((buf, Vec::new()))
            }
            Self::Vdi { reader, .. } => {
                let mut buf = vec![0u8; available];
                let filled = reader
                    .read_at(start, &mut buf)
                    .with_context(|| format!("failed to read VDI at offset {start}"))?;
                buf.truncate(filled);
                Ok((buf, Vec::new()))
            }
            Self::VhdFamily { reader, .. } => {
                // Serialize the seek/read pair over the Rust virtual-disk reader.
                let mut buf = vec![0u8; available];
                let (filled, faults) = tolerant_fill(start, &mut buf, |off, dst| {
                    let mut reader = reader.lock();
                    reader.read_at(off, dst)
                });
                buf.truncate(filled);
                Ok((buf, faults))
            }
            Self::Pmfx { reader, .. } => {
                let mut buf = vec![0u8; available];
                let (filled, faults) = tolerant_fill(start, &mut buf, |off, dst| {
                    let mut reader = reader.lock();
                    reader.read_at(off, dst)
                });
                buf.truncate(filled);
                Ok((buf, faults))
            }
            Self::Assembled { source, .. } => {
                // Composite devices (RAID, LVM) report member faults at their
                // own offsets; keep the spans inside the bytes returned.
                let (bytes, faults) = source.read_traced(start, available)?;
                let faults = collect_read_fault_intersections(&faults, start, bytes.len() as u64);
                Ok((bytes, faults))
            }
            Self::Ldplayer { source, .. } => Ok((source.read(start, available)?, Vec::new())),
            Self::Docker { image } => {
                let start = usize::try_from(start)
                    .context("image offset does not fit in memory address space")?;
                Ok((
                    image.outer_bytes()[start..start + available].to_vec(),
                    Vec::new(),
                ))
            }
            Self::Tar { image } => {
                let start = usize::try_from(start)
                    .context("image offset does not fit in memory address space")?;
                Ok((
                    image.outer_bytes()[start..start + available].to_vec(),
                    Vec::new(),
                ))
            }
            Self::Zip { image } => {
                let start = usize::try_from(start)
                    .context("image offset does not fit in memory address space")?;
                Ok((
                    image.outer_bytes()[start..start + available].to_vec(),
                    Vec::new(),
                ))
            }
            Self::Sevenz { image } => {
                let start = usize::try_from(start)
                    .context("image offset does not fit in memory address space")?;
                Ok((
                    image.outer_bytes()[start..start + available].to_vec(),
                    Vec::new(),
                ))
            }
            Self::Ad1 { image } => {
                let mut buf = vec![0u8; available];
                let copied = image.read_outer(start, &mut buf);
                buf.truncate(copied);
                Ok((buf, Vec::new()))
            }
            Self::Block {
                reader,
                size_bytes,
                sector_size,
                ..
            } => {
                let file = reader.lock();
                let buf = physical_disk::read_aligned(
                    &file,
                    *sector_size,
                    *size_bytes,
                    start,
                    available,
                )?;
                Ok((buf, Vec::new()))
            }
            Self::Folder { .. } => {
                // Folder evidence has no byte-stream backing — every
                // command that hits `read_range` against a folder is a bug
                // in the caller (hex is explicitly
                // gated upstream). Return an empty vec so the command at
                // least fails loudly with an obvious zero-length result
                // rather than panicking.
                anyhow::bail!("folder evidence does not support byte-level reads")
            }
        }
    }
}

impl SectorSource for ImageSource {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_range(offset, len)
    }
    fn size(&self) -> u64 {
        self.len()
    }
    fn read_traced(&self, offset: u64, len: usize) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        self.read_range_traced(offset, len)
    }
}

/// Stat the source file behind an image: `(on_disk_bytes, modified_ms,
/// created_ms)`. Physical drives and synthetic assembled sources have no
/// stat-able backing file; folder sources stat for timestamps but a
/// directory's own length is meaningless so the footprint stays `None`. For
/// EWF the footprint is the summed segment-file size captured at open.
pub(super) fn source_file_meta(
    path: &Path,
    source: &ImageSource,
) -> (Option<u64>, Option<i64>, Option<i64>) {
    match source {
        ImageSource::Indexed { .. } | ImageSource::Block { .. } | ImageSource::Assembled { .. } => {
            (None, None, None)
        }
        _ => {
            let Ok(meta) = std::fs::metadata(path) else {
                return (None, None, None);
            };
            let modified = meta.modified().ok().and_then(unix_ms);
            let created = meta.created().ok().and_then(unix_ms);
            let on_disk = match source {
                ImageSource::Ewf { reader, .. } => reader.lock().segments_on_disk_bytes(),
                ImageSource::Vmdk { reader, .. } => reader.lock().files_on_disk_bytes(),
                ImageSource::Ad1 { image } => Some(image.outer_len()),
                ImageSource::SplitRaw { reader } => Some(reader.media_size()),
                ImageSource::Folder { .. } => None,
                _ => Some(meta.len()),
            };
            (on_disk, modified, created)
        }
    }
}

pub(super) fn open_raw_mmap(path: &Path, format: ImageFormat) -> anyhow::Result<ImageSource> {
    let file =
        File::open(path).with_context(|| format!("failed to open image: {}", path.display()))?;
    let mmap = unsafe { Mmap::map(&file)? };
    Ok(ImageSource::Raw {
        size_bytes: mmap.len() as u64,
        mmap,
        format,
    })
}

pub(super) fn source_file_snapshot(path: &Path) -> anyhow::Result<SourceFileSnapshot> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("failed to stat source image file: {}", path.display()))?;
    Ok(SourceFileSnapshot {
        path: path.to_path_buf(),
        len: meta.len(),
        modified_unix_ms: meta.modified().ok().and_then(unix_ms),
        created_unix_ms: meta.created().ok().and_then(unix_ms),
    })
}

pub(super) fn validate_source_file_snapshots(
    expected: &SourceFileSnapshot,
    current: &SourceFileSnapshot,
) -> anyhow::Result<()> {
    if current.len != expected.len {
        anyhow::bail!(
            "源镜像文件已变化：{} 打开时大小为 {} 字节，当前为 {} 字节。请重新加载镜像后再出具哈希或导出结论。",
            expected.path.display(),
            expected.len,
            current.len
        );
    }
    if let (Some(expected_modified), Some(current_modified)) =
        (expected.modified_unix_ms, current.modified_unix_ms)
    {
        if current_modified != expected_modified {
            anyhow::bail!(
                "源镜像文件已变化：{} 打开时修改时间为 {}，当前为 {}。请重新加载镜像后再出具哈希或导出结论。",
                expected.path.display(),
                expected_modified,
                current_modified
            );
        }
    }
    if let (Some(expected_created), Some(current_created)) =
        (expected.created_unix_ms, current.created_unix_ms)
    {
        if current_created != expected_created {
            anyhow::bail!(
                "源镜像文件已变化：{} 打开时创建时间为 {}，当前为 {}。请重新加载镜像后再出具哈希或导出结论。",
                expected.path.display(),
                expected_created,
                current_created
            );
        }
    }
    Ok(())
}

pub(super) fn unix_ms(time: std::time::SystemTime) -> Option<i64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
}

/// Sector-source view used only while constructing a `DiskImage`. FFI and
/// sparse container sources need the same 64 KiB located cache during
/// partition/filesystem probing that normal reads use after construction;
/// otherwise every small signature read reaches the decompressor directly.
/// The cache and any bad-region observations are moved into the finished
/// `DiskImage`, so the warm probe blocks remain useful to the first UI reads.
pub(super) struct OpenSectorSource<'a> {
    pub(super) source: &'a ImageSource,
    pub(super) cache: Option<&'a SharedBlockCache>,
    pub(super) bad_regions: &'a Mutex<Vec<BadRegion>>,
}

impl SectorSource for OpenSectorSource<'_> {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        let Some(cache) = self.cache else {
            let (data, faults) = self.source.read_range_traced(offset, len)?;
            if !faults.is_empty() {
                let mut regions = self
                    .bad_regions
                    .lock()
                    .map_err(|_| anyhow!("image read-fault lock poisoned"))?;
                regions.extend(faults);
                *regions = normalize_bad_regions(std::mem::take(&mut *regions));
            }
            return Ok(data);
        };

        let image_len = self.source.len();
        let start = offset.min(image_len);
        let available = image_len.saturating_sub(start).min(len as u64) as usize;
        if available == 0 {
            return Ok(Vec::new());
        }
        let end = start + available as u64;
        let mut output = vec![0u8; available];
        let mut filled = 0usize;
        let mut block = start - (start % CACHE_BLOCK);
        while block < end {
            let block_len = CACHE_BLOCK.min(image_len - block) as usize;
            let (data, faults) = cache.get_or_insert_with(block, || {
                let (data, faults) = self.source.read_range_traced(block, block_len)?;
                Ok((Arc::<[u8]>::from(data), faults))
            })?;
            if !faults.is_empty() {
                if let Ok(mut regions) = self.bad_regions.lock() {
                    regions.extend(faults);
                    *regions = normalize_bad_regions(std::mem::take(&mut *regions));
                }
            }

            let block_end = block + data.len() as u64;
            let copy_from = start.max(block);
            let copy_to = end.min(block_end);
            if copy_to > copy_from {
                let source_start = (copy_from - block) as usize;
                let source_end = (copy_to - block) as usize;
                let target_start = (copy_from - start) as usize;
                let target_end = target_start + (source_end - source_start);
                output[target_start..target_end].copy_from_slice(&data[source_start..source_end]);
                filled = filled.max(target_end);
            }
            if data.len() as u64 != CACHE_BLOCK {
                break;
            }
            block += CACHE_BLOCK;
        }
        output.truncate(filled);
        Ok(output)
    }

    fn size(&self) -> u64 {
        self.source.len()
    }
}

impl SectorSource for DiskImage {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_range(offset, len)
    }

    fn size(&self) -> u64 {
        self.len()
    }

    fn read_traced(&self, offset: u64, len: usize) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        self.read_range_traced(offset, len)
    }
}

mod open;
