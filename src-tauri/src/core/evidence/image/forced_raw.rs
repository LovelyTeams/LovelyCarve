//! Explicit RAW compatibility mode, isolated from strict evidence opening.
use super::*;

impl DiskImage {
    pub fn raw_read_mode(&self) -> RawReadMode {
        if self.source.forced_raw_reader().is_some() {
            RawReadMode::Force
        } else {
            RawReadMode::Strict
        }
    }

    /// Open the selected file and its consecutive successors as RAW, even when
    /// the prefix is incomplete. This policy belongs to this session only.
    pub fn open_forced_raw(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::open_forced_raw_cancellable(
            uuid::Uuid::new_v4().to_string(),
            path.as_ref(),
            &noop_observer,
            &tokio_util::sync::CancellationToken::new(),
        )
    }

    pub(crate) fn open_forced_raw_cancellable(
        id: String,
        path: &Path,
        progress: OpenObserver<'_>,
        cancel: &tokio_util::sync::CancellationToken,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(!cancel.is_cancelled(), "image load cancelled");
        progress(OpenProgress::Opening);
        let reader = smraw::RawImage::open_forced(path)
            .with_context(|| format!("failed to force open RAW image: {}", path.display()))?;
        if reader.media_size() % 512 != 0 {
            tracing::warn!(
                "RAW image size {} is not a multiple of 512; sector navigation ignores the incomplete final sector",
                reader.media_size()
            );
        }
        tracing::warn!(
            "forced RAW mode: {} consecutive segment(s), {} available bytes from {}; completeness is not established",
            reader.segments().len(), reader.media_size(), path.display()
        );
        Self::from_source_cancellable(
            id,
            path.to_path_buf(),
            ImageSource::SplitRaw { reader },
            progress,
            cancel,
        )
    }
}

impl ImageSource {
    pub(super) fn forced_raw_reader(&self) -> Option<&smraw::RawImage> {
        match self {
            Self::SplitRaw { reader } if reader.is_forced() => Some(reader),
            _ => None,
        }
    }

    /// Normal reads clip at EOF. Forced RAW synthesizes only a request that
    /// starts wholly outside capacity; crossing reads still return a prefix.
    pub(super) fn read_length(&self, offset: u64, requested: usize) -> usize {
        if let Some(reader) = self.forced_raw_reader() {
            if reader.take_capacity_warning(offset, requested) {
                tracing::warn!("RAW read exceeds accounted image capacity: offset={offset}, length={requested}, capacity={}", self.len());
            }
            if offset >= self.len() {
                return requested;
            }
        }
        self.len().saturating_sub(offset).min(requested as u64) as usize
    }

    pub(super) fn read_forced_raw(
        &self,
        reader: &smraw::RawImage,
        offset: u64,
        destination: &mut [u8],
    ) -> anyhow::Result<(usize, Vec<BadRegion>)> {
        let outcome = reader.read_at_forced(offset, destination)?;
        if outcome.capacity_warning {
            tracing::warn!("RAW read exceeds accounted image capacity: offset={offset}, length={}, capacity={}", destination.len(), self.len());
        }
        if let Some(error) = outcome.short_read {
            tracing::warn!(
                "partial RAW read at {offset}: {}/{} bytes: {error}",
                outcome.bytes_read,
                destination.len()
            );
        }
        let faults = if outcome.synthetic && outcome.bytes_read > 0 {
            vec![BadRegion {
                offset,
                len: outcome.bytes_read as u64,
            }]
        } else {
            Vec::new()
        };
        Ok((outcome.bytes_read, faults))
    }
}
