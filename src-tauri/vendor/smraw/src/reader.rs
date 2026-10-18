use crate::{discover, invalid, AbortHandle, MediaMetadata, MAX_SEGMENTS};
use std::collections::{HashSet, VecDeque};
use std::fs::{self, File, Metadata};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Source identity and logical geometry captured at open time.
#[derive(Clone, Debug)]
pub struct Segment {
    pub path: PathBuf,
    pub offset: u64,
    pub len: u64,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
}

impl Segment {
    fn validate(&self, metadata: &Metadata) -> io::Result<()> {
        if !metadata.is_file()
            || metadata.len() != self.len
            || metadata.modified().ok() != self.modified
            || metadata.created().ok() != self.created
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("RAW source changed since open: {}", self.path.display()),
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
struct HandleCache {
    limit: usize,
    handles: VecDeque<(usize, Arc<File>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OpenMode {
    Strict,
    Force,
}

/// Read-only logical concatenation with positioned, concurrent I/O.
/// The cache bounds idle handles; outstanding reads may temporarily retain
/// evicted handles. I/O runs outside the cache lock.
#[derive(Debug)]
pub struct RawImage {
    pub(crate) segments: Vec<Segment>,
    media_size: u64,
    handles: Mutex<HandleCache>,
    pub(crate) abort: AbortHandle,
    mode: OpenMode,
    pub(crate) capacity_warned: std::sync::atomic::AtomicBool,
    /// Optional caller declarations; never populated from RAW source bytes.
    pub metadata: MediaMetadata,
}

impl RawImage {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        Self::open_segments(discover(path)?.paths)
    }

    /// Open in caller-supplied order. Unequal segment lengths are supported;
    /// empty members are rejected except for a single empty RAW file.
    pub fn open_segments(paths: impl IntoIterator<Item = PathBuf>) -> io::Result<Self> {
        Self::from_paths(paths, OpenMode::Strict)
    }

    pub(crate) fn from_paths(
        paths: impl IntoIterator<Item = PathBuf>,
        mode: OpenMode,
    ) -> io::Result<Self> {
        let mut segments = Vec::new();
        let mut identities = HashSet::new();
        let mut media_size = 0u64;
        for path in paths {
            if segments.len() >= MAX_SEGMENTS {
                return Err(invalid("RAW segment count limit exceeded"));
            }
            // Resolve relative paths now, so changing cwd cannot change a reopen.
            let path = fs::canonicalize(path)?;
            if !identities.insert(path.clone()) {
                return Err(invalid(format!(
                    "duplicate RAW segment: {}",
                    path.display()
                )));
            }
            let metadata = if mode == OpenMode::Force {
                fs::metadata(&path)?
            } else {
                File::open(&path)?.metadata()?
            };
            if !metadata.is_file() {
                return Err(invalid(format!(
                    "RAW segment is not a regular file: {}",
                    path.display()
                )));
            }
            segments.push(Segment {
                path,
                offset: media_size,
                len: metadata.len(),
                modified: metadata.modified().ok(),
                created: metadata.created().ok(),
            });
            media_size = media_size
                .checked_add(metadata.len())
                .ok_or_else(|| invalid("RAW media size overflow"))?;
        }
        if segments.is_empty() {
            return Err(invalid("RAW segment list is empty"));
        }
        if mode == OpenMode::Strict
            && segments.len() > 1
            && segments.iter().any(|segment| segment.len == 0)
        {
            return Err(invalid("empty member in split RAW image"));
        }
        Ok(Self {
            segments,
            media_size,
            handles: Mutex::new(HandleCache {
                limit: 64,
                handles: VecDeque::new(),
            }),
            abort: AbortHandle::default(),
            mode,
            capacity_warned: std::sync::atomic::AtomicBool::new(false),
            metadata: MediaMetadata::default(),
        })
    }

    pub fn media_size(&self) -> u64 {
        self.media_size
    }

    pub fn is_forced(&self) -> bool {
        self.mode == OpenMode::Force
    }
    pub fn is_empty(&self) -> bool {
        self.media_size == 0
    }
    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }
    /// Segment containing a logical byte; EOF has no containing segment.
    pub fn segment_at(&self, offset: u64) -> Option<&Segment> {
        if offset >= self.media_size {
            return None;
        }
        let index = self
            .segments
            .partition_point(|segment| segment.offset <= offset)
            .checked_sub(1)?;
        self.segments.get(index)
    }
    pub fn maximum_segment_size(&self) -> u64 {
        self.segments
            .iter()
            .map(|segment| segment.len)
            .max()
            .unwrap_or(0)
    }
    pub fn abort_handle(&self) -> AbortHandle {
        self.abort.clone()
    }

    /// RAW has no declared length. This verifies a caller's independent length.
    pub fn require_media_size(&self, expected: u64) -> io::Result<()> {
        if self.media_size != expected {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "RAW media size {} differs from expected {expected}",
                    self.media_size
                ),
            ));
        }
        Ok(())
    }

    pub fn set_maximum_open_handles(&self, limit: usize) -> io::Result<()> {
        if limit == 0 || limit > MAX_SEGMENTS {
            return Err(invalid("RAW open handle limit must be 1..=65536"));
        }
        let mut cache = self
            .handles
            .lock()
            .map_err(|_| io::Error::other("RAW cache lock poisoned"))?;
        cache.limit = limit;
        cache.handles.truncate(limit);
        Ok(())
    }

    fn handle(&self, index: usize) -> io::Result<Arc<File>> {
        self.open_handle(index, OpenMode::Strict)
    }

    pub(crate) fn open_handle(&self, index: usize, mode: OpenMode) -> io::Result<Arc<File>> {
        let mut cache = self
            .handles
            .lock()
            .map_err(|_| io::Error::other("RAW cache lock poisoned"))?;
        if let Some(position) = cache
            .handles
            .iter()
            .position(|(cached, _)| *cached == index)
        {
            let (_, file) = cache
                .handles
                .remove(position)
                .ok_or_else(|| io::Error::other("RAW cache entry missing"))?;
            cache.handles.push_front((index, Arc::clone(&file)));
            return Ok(file);
        }
        let segment = self
            .segments
            .get(index)
            .ok_or_else(|| invalid("RAW segment index out of bounds"))?;
        let file = Arc::new(File::open(&segment.path)?);
        if mode == OpenMode::Strict {
            segment.validate(&file.metadata()?)?;
        }
        cache.handles.push_front((index, Arc::clone(&file)));
        let limit = cache.limit;
        cache.handles.truncate(limit);
        Ok(file)
    }

    pub(crate) fn reset_handles(&self) -> io::Result<()> {
        self.handles
            .lock()
            .map_err(|_| io::Error::other("RAW cache lock poisoned"))?
            .handles
            .clear();
        Ok(())
    }

    pub fn validate_sources(&self) -> io::Result<()> {
        for segment in &self.segments {
            self.abort.check()?;
            segment.validate(&fs::metadata(&segment.path)?)?;
        }
        Ok(())
    }

    /// Fill through segment boundaries; EOF clips only at the logical end.
    /// A changed or truncated member fails, without synthetic zeroes.
    pub fn read_at(&self, offset: u64, destination: &mut [u8]) -> io::Result<usize> {
        self.abort.check()?;
        let available = self
            .media_size
            .saturating_sub(offset)
            .min(destination.len() as u64) as usize;
        if available == 0 {
            return Ok(0);
        }
        let mut filled = 0usize;
        let mut index = self
            .segments
            .partition_point(|segment| segment.offset <= offset)
            .saturating_sub(1);
        while filled < available {
            self.abort.check()?;
            let segment = self
                .segments
                .get(index)
                .ok_or_else(|| io::Error::other("RAW segment mapping incomplete"))?;
            let local = offset + filled as u64 - segment.offset;
            let length = (segment.len - local).min((available - filled) as u64) as usize;
            let file = self.handle(index)?;
            segment.validate(&file.metadata()?)?;
            let mut copied = 0usize;
            while copied < length {
                self.abort.check()?;
                let end = (copied + 1024 * 1024).min(length);
                let destination = &mut destination[filled + copied..filled + end];
                #[cfg(windows)]
                let result = std::os::windows::fs::FileExt::seek_read(
                    &*file,
                    destination,
                    local + copied as u64,
                );
                #[cfg(unix)]
                let result =
                    std::os::unix::fs::FileExt::read_at(&*file, destination, local + copied as u64);
                let count = match result {
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    result => result?,
                };
                if count == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        format!(
                            "short RAW read in {} at {}",
                            segment.path.display(),
                            local + copied as u64
                        ),
                    ));
                }
                copied += count;
            }
            segment.validate(&file.metadata()?)?;
            filled += length;
            index += 1;
        }
        Ok(filled)
    }

    pub fn cursor(&self) -> RawCursor<'_> {
        RawCursor {
            image: self,
            position: 0,
        }
    }
}

/// Independent stream position; multiple cursors share immutable source bytes.
pub struct RawCursor<'a> {
    image: &'a RawImage,
    position: u64,
}

impl RawCursor<'_> {
    pub fn position(&self) -> u64 {
        self.position
    }
    pub fn segment(&self) -> Option<&Segment> {
        self.image.segment_at(self.position)
    }
}

impl Read for RawCursor<'_> {
    fn read(&mut self, destination: &mut [u8]) -> io::Result<usize> {
        let count = self.image.read_at(self.position, destination)?;
        self.position += count as u64;
        Ok(count)
    }
}

impl Seek for RawCursor<'_> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.image.abort.check()?;
        let position = match from {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.image.media_size) + i128::from(delta),
        };
        self.position = u64::try_from(position).map_err(|_| invalid("RAW seek out of bounds"))?;
        Ok(self.position)
    }
}
