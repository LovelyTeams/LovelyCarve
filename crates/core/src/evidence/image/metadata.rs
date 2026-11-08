//! metadata implementation.
use super::*;

impl DiskImage {
    pub(crate) fn optical_track_sector_size(&self, partition_id: u8) -> Option<usize> {
        match &self.source {
            ImageSource::Cue { image } => image
                .info
                .tracks
                .get(usize::from(partition_id))
                .map(|track| track.sector_bytes),
            _ => None,
        }
    }

    pub fn info(&self) -> ImageInfo {
        ImageInfo {
            raw_read_mode: self.raw_read_mode(),
            unlocked_partitions: Vec::new(),
            index_only: self.is_index_only(),
            id: self.id.clone(),
            path: self.path.display().to_string(),
            source_format: match &self.derived {
                Some(DerivedImage::Decrypted { .. }) if self.folder.is_some() => {
                    ImageFormat::Folder
                }
                Some(DerivedImage::Decrypted { .. }) => ImageFormat::Decrypted,
                Some(DerivedImage::ShadowCopy { .. }) => ImageFormat::ShadowCopy,
                None => self.source.format(),
            },
            size_bytes: self.len(),
            sector_size: if self.is_index_only() {
                0
            } else {
                self.sector_size
            },
            partitions: self.partitions.clone(),
            table_scheme: self.table_scheme,
            disk_id: self.disk_id.clone(),
            on_disk_bytes: self.on_disk_bytes,
            modified_unix_ms: self.modified_unix_ms,
            created_unix_ms: self.created_unix_ms,
            ewf: self.ewf_acquiry.clone(),
            optical_disc: match &self.source {
                ImageSource::Cue { image } => Some(image.info.clone()),
                _ => None,
            },
            ios_backup: self
                .folder
                .as_ref()
                .and_then(|folder| folder.ios_info())
                .cloned(),
            shadow_copy: match &self.derived {
                Some(DerivedImage::ShadowCopy { origin, .. }) => Some(origin.clone()),
                Some(DerivedImage::Decrypted { .. }) | None => None,
            },
        }
    }

    pub fn validate_source_file_stable(&self) -> anyhow::Result<()> {
        self.require_source()?;
        for expected in self.source_file_stability_snapshots_at_open() {
            let current = source_file_snapshot(&expected.path)?;
            validate_source_file_snapshots(&expected, &current)?;
        }
        Ok(())
    }

    /// True when source files have been projected into Android backup folders.
    pub fn has_expanded_backups(&self) -> bool {
        self.zip
            .as_ref()
            .is_some_and(|zip| !zip.backup_archives().is_empty())
            || self
                .folder
                .as_ref()
                .is_some_and(|folder| !folder.backup_archives().is_empty())
    }

    /// Stable identity for the exact evidence source observed when this image
    /// was opened. The fingerprint intentionally hashes metadata rather than
    /// the full media: it is cheap enough to attach to every index run while
    /// still changing when a segment path, size, or source timestamp changes.
    pub fn evidence_fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"lovelycarve-evidence-fingerprint-v1\0");
        hasher.update(format!("{:?}", self.source.format()).as_bytes());
        hasher.update([0]);
        hasher.update(self.path.to_string_lossy().as_bytes());
        hasher.update([0]);
        hasher.update(self.len().to_le_bytes());
        if self.raw_read_mode() == RawReadMode::Force {
            hasher.update(b"forced-raw-v1\0");
        }
        hasher.update((self.sector_size as u64).to_le_bytes());
        hasher.update(self.on_disk_bytes.unwrap_or(u64::MAX).to_le_bytes());
        hasher.update(self.modified_unix_ms.unwrap_or(i64::MIN).to_le_bytes());
        hasher.update(self.created_unix_ms.unwrap_or(i64::MIN).to_le_bytes());

        // The source bytes may be identical across reader upgrades, but an old
        // file-only index cannot describe the newly expanded directory view.
        // Include which backups actually decoded, so partial/failed decoding
        // also cannot reuse a snapshot of a different projection. A ZIP backup
        // restored from its saved listing hashes the same values as one
        // decoded at open.
        let backups: Vec<(&str, usize, u64)> = match (&self.zip, &self.folder) {
            (Some(zip), _) => zip
                .backup_archives()
                .iter()
                .map(|backup| (backup.label(), backup.entry_count(), backup.total_size()))
                .collect(),
            (None, Some(folder)) => folder
                .backup_archives()
                .iter()
                .map(|archive| (archive.label(), archive.entry_count(), archive.total_size()))
                .collect(),
            (None, None) => Vec::new(),
        };
        if !backups.is_empty() {
            hasher.update(b"android-backup-directory-projection-v2\0");
            for (label, entry_count, total_size) in backups {
                hasher.update((label.len() as u64).to_le_bytes());
                hasher.update(label.as_bytes());
                hasher.update((entry_count as u64).to_le_bytes());
                hasher.update(total_size.to_le_bytes());
            }
        }

        let mut snapshots = self.source_file_stability_snapshots_at_open();
        snapshots.sort_by(|left, right| left.path.cmp(&right.path));
        for snapshot in snapshots {
            hasher.update(snapshot.path.to_string_lossy().as_bytes());
            hasher.update([0]);
            hasher.update(snapshot.len.to_le_bytes());
            hasher.update(snapshot.modified_unix_ms.unwrap_or(i64::MIN).to_le_bytes());
            hasher.update(snapshot.created_unix_ms.unwrap_or(i64::MIN).to_le_bytes());
        }
        format!("sha256:{}", hex::encode(hasher.finalize()))
    }

    pub(super) fn source_file_stability_snapshots_at_open(&self) -> Vec<SourceFileSnapshot> {
        match &self.source {
            ImageSource::Raw { .. }
            | ImageSource::Qcow2 { .. }
            | ImageSource::Vdi { .. }
            | ImageSource::VhdFamily { .. }
            | ImageSource::Dmg { .. }
            | ImageSource::Pmfx { .. }
            | ImageSource::Docker { .. }
            | ImageSource::Tar { .. }
            | ImageSource::Zip { .. }
            | ImageSource::Sevenz { .. }
            | ImageSource::Ldplayer { .. } => self
                .on_disk_bytes
                .map(|len| SourceFileSnapshot {
                    path: self.path.clone(),
                    len,
                    modified_unix_ms: self.modified_unix_ms,
                    created_unix_ms: self.created_unix_ms,
                })
                .into_iter()
                .collect(),
            ImageSource::SplitRaw { reader } => reader
                .segments()
                .iter()
                .map(|segment| SourceFileSnapshot {
                    path: segment.path.clone(),
                    len: segment.len,
                    modified_unix_ms: segment.modified.and_then(unix_ms),
                    created_unix_ms: segment.created.and_then(unix_ms),
                })
                .collect(),
            ImageSource::Cue { image } => std::iter::once(&image.descriptor)
                .chain(image.reader.segments())
                .map(|segment| SourceFileSnapshot {
                    path: segment.path.clone(),
                    len: segment.len,
                    modified_unix_ms: segment.modified.and_then(unix_ms),
                    created_unix_ms: segment.created.and_then(unix_ms),
                })
                .collect(),
            ImageSource::Phdi { reader } => reader
                .source_files()
                .iter()
                .map(|source| SourceFileSnapshot {
                    path: source.path.clone(),
                    len: source.len,
                    modified_unix_ms: source.modified.and_then(unix_ms),
                    created_unix_ms: source.created.and_then(unix_ms),
                })
                .collect(),
            ImageSource::Ewf { reader, .. } => reader
                .lock()
                .segment_file_snapshots()
                .iter()
                .map(|snapshot| SourceFileSnapshot {
                    path: snapshot.path.clone(),
                    len: snapshot.len,
                    modified_unix_ms: snapshot.modified_unix_ms,
                    created_unix_ms: snapshot.created_unix_ms,
                })
                .collect(),
            ImageSource::Vmdk { reader, .. } => reader
                .lock()
                .file_snapshots()
                .iter()
                .map(|snapshot| SourceFileSnapshot {
                    path: snapshot.path.clone(),
                    len: snapshot.len,
                    modified_unix_ms: snapshot.modified_unix_ms,
                    created_unix_ms: snapshot.created_unix_ms,
                })
                .collect(),
            ImageSource::Ad1 { image } => image
                .segment_files()
                .iter()
                .map(|segment| SourceFileSnapshot {
                    path: segment.path.clone(),
                    len: segment.len,
                    modified_unix_ms: segment.modified_unix_ms,
                    created_unix_ms: segment.created_unix_ms,
                })
                .collect(),
            ImageSource::Indexed { .. }
            | ImageSource::Assembled { .. }
            | ImageSource::Block { .. }
            | ImageSource::Folder { .. } => Vec::new(),
        }
    }
}

pub(crate) fn find_partition<'a>(
    image: &'a DiskImage,
    part_id: &str,
) -> anyhow::Result<&'a Partition> {
    image.require_source()?;
    let part_id = part_id.parse::<u8>()?;
    image
        .partitions
        .iter()
        .find(|partition| partition.id == part_id)
        .ok_or_else(|| anyhow::anyhow!("partition not found: {part_id}"))
}
