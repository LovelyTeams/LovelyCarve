//! Read-only Parallels v2 descriptors, plain/sparse extents and parent chains.
//! No C ABI, global state, native libraries or implicit descriptor recovery.
#![forbid(unsafe_code)]

mod descriptor;
mod storage;

pub use descriptor::{Descriptor, ExtentDescriptor, Guid, ImageDescriptor, ImageType, Snapshot};
pub use storage::{SourceFile, SparseHeader};

use std::collections::{HashSet, VecDeque};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Resource limits and explicit extent-directory/snapshot overrides.
#[derive(Clone, Debug)]
pub struct OpenOptions {
    pub extent_directory: Option<PathBuf>,
    pub snapshot: Option<Guid>,
    pub maximum_descriptor_bytes: u64,
    pub maximum_bat_entries: usize,
    pub maximum_open_handles: usize,
    pub maximum_chain_depth: usize,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            extent_directory: None,
            snapshot: None,
            maximum_descriptor_bytes: 8 * 1024 * 1024,
            maximum_bat_entries: 16 * 1024 * 1024,
            maximum_open_handles: 16,
            maximum_chain_depth: 128,
        }
    }
}

#[derive(Debug)]
struct Extent {
    offset: u64,
    size: u64,
    layers: Vec<storage::Layer>,
}

/// A logical guest disk. `read_at` is thread-safe and does not move the cursor.
#[derive(Debug)]
pub struct PhdiImage {
    descriptor: Descriptor,
    extents: Vec<Extent>,
    sources: Vec<SourceFile>,
    handles: Mutex<VecDeque<(usize, File)>>,
    maximum_open_handles: usize,
    position: u64,
}

impl PhdiImage {
    /// Opens DiskDescriptor.xml, a .hdd bundle, or a standalone v2 sparse HDS.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::open_with_options(path, &OpenOptions::default(), &|| false)
    }

    /// Cancellation is polled directly; callers can pass a token-backed closure.
    pub fn open_with_options(
        path: impl AsRef<Path>,
        options: &OpenOptions,
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<Self> {
        check_cancelled(cancelled)?;
        if options.maximum_open_handles == 0 || options.maximum_chain_depth == 0 {
            return Err(invalid("PHDI resource limits must be nonzero"));
        }
        let path = path.as_ref();
        let path = if path.is_dir() {
            path.join("DiskDescriptor.xml")
        } else {
            path.to_owned()
        };
        let mut file = File::open(&path)?;
        let mut signature = [0; 16];
        let count = file.read(&mut signature)?;
        file.rewind()?;
        let standalone = count == 16 && is_sparse_signature(&signature);
        let mut sources = Vec::new();
        let descriptor = if standalone {
            if options.snapshot.is_some() {
                return Err(invalid("standalone HDS has no snapshot graph"));
            }
            let header = SparseHeader::read(&mut file)?;
            Descriptor::standalone(header.media_size, header.block_size, &path)
        } else {
            let source = SourceFile::capture(&path)?;
            let mut bytes = Vec::new();
            file.take(options.maximum_descriptor_bytes.saturating_add(1))
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > options.maximum_descriptor_bytes {
                return Err(invalid("PHDI descriptor exceeds configured limit"));
            }
            source.verify()?;
            sources.push(source);
            Descriptor::parse(&bytes)?
        };
        let chain = descriptor.chain(options.snapshot, options.maximum_chain_depth)?;
        let directory = options
            .extent_directory
            .as_deref()
            .unwrap_or_else(|| path.parent().unwrap_or(Path::new(".")));
        let mut extents = Vec::new();
        for extent in &descriptor.extents {
            check_cancelled(cancelled)?;
            let mut layers = Vec::new();
            for (chain_index, identifier) in chain.iter().enumerate() {
                let images: Vec<_> = extent
                    .images
                    .iter()
                    .filter(|image| image.identifier == *identifier)
                    .collect();
                // An unchanged split extent may omit its child image entirely.
                if images.is_empty() && chain_index + 1 < chain.len() {
                    continue;
                }
                if images.len() != 1 {
                    return Err(invalid(
                        "PHDI extent is missing a unique image for a parent-chain member",
                    ));
                }
                let image = images[0];
                let filename = if standalone {
                    path.clone()
                } else {
                    resolve_extent(directory, &image.filename)?
                };
                let source = SourceFile::capture(&filename)?;
                let source_index = sources.len();
                let layer = storage::Layer::open(
                    &source,
                    source_index,
                    image.kind,
                    extent.size,
                    extent.block_size,
                    options.maximum_bat_entries,
                    cancelled,
                )?;
                source.verify()?;
                sources.push(source);
                layers.push(layer);
            }
            extents.push(Extent {
                offset: extent.offset,
                size: extent.size,
                layers,
            });
        }
        Ok(Self {
            descriptor,
            extents,
            sources,
            handles: Mutex::new(VecDeque::new()),
            maximum_open_handles: options.maximum_open_handles,
            position: 0,
        })
    }

    pub fn media_size(&self) -> u64 {
        self.descriptor.media_size
    }
    pub fn sector_size(&self) -> u32 {
        self.descriptor.logical_sector_size
    }
    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }
    /// Includes the descriptor and every opened ancestor extent for fingerprinting.
    pub fn source_files(&self) -> &[SourceFile] {
        &self.sources
    }
    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn read_at(&self, offset: u64, output: &mut [u8]) -> io::Result<usize> {
        self.read_at_with_cancel(offset, output, &|| false)
    }

    pub fn read_at_with_cancel(
        &self,
        offset: u64,
        output: &mut [u8],
        cancelled: &dyn Fn() -> bool,
    ) -> io::Result<usize> {
        check_cancelled(cancelled)?;
        // Metadata changes must fail even when the requested bytes are sparse.
        for source in &self.sources {
            source.verify()?;
        }
        let length = (self.media_size().saturating_sub(offset)).min(output.len() as u64) as usize;
        let mut filled = 0;
        while filled < length {
            check_cancelled(cancelled)?;
            let logical = offset + filled as u64;
            let extent = self
                .extents
                .iter()
                .find(|extent| logical >= extent.offset && logical - extent.offset < extent.size)
                .ok_or_else(|| invalid("PHDI extent coverage gap"))?;
            let local = logical - extent.offset;
            let mut amount = (extent.size - local).min((length - filled) as u64);
            let mut allocation = None;
            for layer in &extent.layers {
                let (physical, available) = layer.map(local)?;
                amount = amount.min(available);
                if let Some(physical) = physical {
                    allocation = Some((layer.source_index, physical));
                    break;
                }
            }
            let amount =
                usize::try_from(amount).map_err(|_| invalid("PHDI read length overflow"))?;
            if amount == 0 {
                return Err(invalid("PHDI mapping made no progress"));
            }
            let target = &mut output[filled..filled + amount];
            if let Some((index, physical)) = allocation {
                self.read_source(index, physical, target)?;
            } else {
                target.fill(0);
            }
            filled += amount;
        }
        Ok(filled)
    }

    fn read_source(&self, index: usize, offset: u64, output: &mut [u8]) -> io::Result<()> {
        let source = &self.sources[index];
        let mut handles = self
            .handles
            .lock()
            .map_err(|_| invalid("PHDI handle lock poisoned"))?;
        let mut file = if let Some(position) = handles.iter().position(|(entry, _)| *entry == index)
        {
            handles
                .remove(position)
                .ok_or_else(|| invalid("PHDI handle disappeared"))?
                .1
        } else {
            if handles.len() >= self.maximum_open_handles {
                handles.pop_front();
            }
            File::open(&source.path)?
        };
        source.verify_file(&file)?;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(output).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!(
                    "PHDI allocated read at {offset} in {}: {error}",
                    source.path.display()
                ),
            )
        })?;
        source.verify_file(&file)?;
        source.verify()?;
        handles.push_back((index, file));
        Ok(())
    }
}

impl Read for PhdiImage {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        let count = self.read_at(self.position, output)?;
        self.position += count as u64;
        Ok(count)
    }
}
impl Seek for PhdiImage {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let next = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(offset) => i128::from(self.position) + i128::from(offset),
            SeekFrom::End(offset) => i128::from(self.media_size()) + i128::from(offset),
        };
        self.position = u64::try_from(next).map_err(|_| invalid("PHDI seek outside u64 range"))?;
        Ok(self.position)
    }
}

fn check_cancelled(cancelled: &dyn Fn() -> bool) -> io::Result<()> {
    if cancelled() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "PHDI operation cancelled",
        ))
    } else {
        Ok(())
    }
}

/// Recognizes supported and known unsupported sparse signatures.
pub fn is_sparse_signature(signature: &[u8]) -> bool {
    signature == b"WithoutFreeSpace" || signature == b"WithouFreSpacExt"
}

fn resolve_extent(directory: &Path, filename: &str) -> io::Result<PathBuf> {
    let filename = filename.replace('\\', "/");
    let relative = Path::new(&filename);
    if filename.contains(':')
        || relative.components().any(|component| {
            !matches!(
                component,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
    {
        return Err(invalid(
            "PHDI extent filename must stay inside the extent directory",
        ));
    }
    let root = directory.canonicalize()?;
    let path = root.join(relative).canonicalize()?;
    if !path.starts_with(&root) {
        return Err(invalid("PHDI extent symlink escapes the extent directory"));
    }
    Ok(path)
}

pub(crate) fn unique_guids(values: impl Iterator<Item = Guid>) -> io::Result<()> {
    let mut seen = HashSet::new();
    for value in values {
        if !seen.insert(value) {
            return Err(invalid("duplicate PHDI GUID"));
        }
    }
    Ok(())
}
