use crate::{invalid, AbortHandle, MediaMetadata};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Writer for new output files only. Existing evidence is never writable.
/// Single output is `<basename>.raw`; split output is `.raw.001` onwards,
/// following libsmraw's output names. At most 999 output segments are allowed.
/// Unfinished outputs are removed on drop; `finish` checks complete length and
/// syncs every segment before retaining them.
#[derive(Debug)]
pub struct RawWriter {
    basename: PathBuf,
    media_size: u64,
    segment_size: u64,
    segment_count: u64,
    files: Vec<File>,
    paths: Vec<PathBuf>,
    position: u64,
    written: u64,
    failed: bool,
    finished: bool,
    abort: AbortHandle,
    /// Caller metadata; persistence is the caller's responsibility.
    pub metadata: MediaMetadata,
}

impl RawWriter {
    pub fn create(
        basename: impl AsRef<Path>,
        media_size: u64,
        maximum_segment_size: u64,
    ) -> io::Result<Self> {
        if maximum_segment_size == 0 {
            return Err(invalid("RAW maximum segment size must be nonzero"));
        }
        let segment_count = media_size.div_ceil(maximum_segment_size).max(1);
        if segment_count > 999 {
            return Err(invalid("RAW writer supports at most 999 segments"));
        }
        let basename = basename.as_ref();
        if basename.file_name().is_none() {
            return Err(invalid("RAW basename must name an output file"));
        }
        let mut writer = Self {
            basename: basename.to_path_buf(),
            media_size,
            segment_size: maximum_segment_size,
            segment_count,
            files: Vec::new(),
            paths: Vec::new(),
            position: 0,
            written: 0,
            failed: false,
            finished: false,
            abort: AbortHandle::default(),
            metadata: MediaMetadata::default(),
        };
        // Create the first file now to report collisions/permissions immediately.
        writer.ensure_file(0)?;
        Ok(writer)
    }

    pub fn abort_handle(&self) -> AbortHandle {
        self.abort.clone()
    }
    pub fn media_size(&self) -> u64 {
        self.media_size
    }
    pub fn maximum_segment_size(&self) -> u64 {
        self.segment_size
    }
    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }
    pub fn position(&self) -> u64 {
        self.position
    }

    fn ensure_file(&mut self, index: usize) -> io::Result<()> {
        while self.files.len() <= index {
            let identifier = self.files.len() + 1;
            let mut name = self.basename.as_os_str().to_os_string();
            name.push(if self.segment_count == 1 {
                ".raw".to_owned()
            } else {
                format!(".raw.{identifier:03}")
            });
            let path = PathBuf::from(name);
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            self.paths.push(path);
            self.files.push(file);
        }
        Ok(())
    }

    fn write_at_inner(&mut self, offset: u64, source: &[u8]) -> io::Result<usize> {
        self.abort.check()?;
        if self.failed {
            return Err(io::Error::other(
                "RAW writer failed; outputs will be discarded",
            ));
        }
        let end = offset
            .checked_add(source.len() as u64)
            .ok_or_else(|| invalid("RAW write offset overflow"))?;
        if offset > self.written || end > self.media_size {
            return Err(invalid(
                "RAW write would leave a hole or exceed declared media size",
            ));
        }
        let mut written = 0usize;
        while written < source.len() {
            self.abort.check()?;
            let absolute = offset + written as u64;
            let index = (absolute / self.segment_size) as usize;
            let local = absolute % self.segment_size;
            let count = (self.segment_size - local)
                .min((source.len() - written) as u64)
                .min(1024 * 1024) as usize;
            self.ensure_file(index)?;
            let file = self
                .files
                .get_mut(index)
                .ok_or_else(|| io::Error::other("RAW output segment missing"))?;
            file.seek(SeekFrom::Start(local))?;
            file.write_all(&source[written..written + count])?;
            written += count;
        }
        self.written = self.written.max(end);
        Ok(written)
    }

    /// Modify only bytes in outputs owned by this writer. No sparse gaps.
    /// The cursor is independent of this positioned operation.
    pub fn write_at(&mut self, offset: u64, source: &[u8]) -> io::Result<usize> {
        let result = self.write_at_inner(offset, source);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    pub fn finish(mut self) -> io::Result<Vec<PathBuf>> {
        self.abort.check()?;
        if self.failed || self.written != self.media_size {
            return Err(io::Error::other(format!(
                "incomplete RAW output: {} of {} bytes",
                self.written, self.media_size
            )));
        }
        for (index, file) in self.files.iter().enumerate() {
            let expected = self
                .segment_size
                .min(self.media_size - index as u64 * self.segment_size);
            if file.metadata()?.len() != expected {
                return Err(io::Error::other("RAW output length changed before finish"));
            }
            file.sync_all()?;
        }
        self.finished = true;
        Ok(self.paths.clone())
    }
}

impl Write for RawWriter {
    fn write(&mut self, source: &[u8]) -> io::Result<usize> {
        let count = self.write_at(self.position, source)?;
        self.position += count as u64;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.abort.check()?;
        for file in &mut self.files {
            file.flush()?;
        }
        Ok(())
    }
}

impl Seek for RawWriter {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.abort.check()?;
        let position = match from {
            SeekFrom::Start(position) => i128::from(position),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => i128::from(self.media_size) + i128::from(delta),
        };
        let position =
            u64::try_from(position).map_err(|_| invalid("RAW output seek out of bounds"))?;
        if position > self.written {
            return Err(invalid("RAW output seek would leave a hole"));
        }
        self.position = position;
        Ok(position)
    }
}

impl Drop for RawWriter {
    fn drop(&mut self) {
        self.files.clear(); // Release Windows handles before deleting outputs.
        if !self.finished {
            for path in &self.paths {
                if let Err(error) = fs::remove_file(path) {
                    eprintln!(
                        "failed to remove partial RAW output {}: {error}",
                        path.display()
                    );
                }
            }
        }
    }
}
