//! Deleted MFT record discovery and captured data runs.
use super::*;

impl<'a> NtfsVolume<'a> {
    /// Walks every MFT slot once and returns a snapshot of every record
    /// whose `IN_USE` flag is cleared. Each entry carries its parent record
    /// reference (from `$FILE_NAME`) so callers can attach deleted children
    /// directly under their original parent directory in subsequent
    /// directory listings. System-reserved records (0–15) are skipped.
    /// Returns at most [`DELETED_SCAN_MAX_RESULTS`] entries to bound memory.
    pub fn build_deleted_snapshot(&self) -> Result<NtfsDeletedSnapshot> {
        let mut fs = self.reader();
        // Record count from $MFT $DATA, falling back to $MFTMirr (P1-3).
        let total_records = self.mft_total_records()?;

        let mut entries = Vec::new();
        for record_number in SYSTEM_RESERVED_RECORDS..total_records {
            if entries.len() >= DELETED_SCAN_MAX_RESULTS {
                break;
            }
            entries.extend(
                self.read_deleted_entries(&mut fs, record_number)
                    .into_iter()
                    .take(DELETED_SCAN_MAX_RESULTS - entries.len()),
            );
        }

        Ok(NtfsDeletedSnapshot::new(entries))
    }

    /// The best `$FILE_NAME` of `file` per parent directory: prefers the
    /// canonical Win32 / combined namespace, falls back to Posix, and drops
    /// pure-DOS 8.3 aliases (they duplicate the Win32 entry — same rule the
    /// per-directory listing applies at [`Self::children`]). One entry per
    /// distinct (parent, raw name), including multiple names in one directory.
    pub(super) fn best_filenames_by_parent<T: Read + Seek>(
        &self,
        file: &ntfs::NtfsFile,
        fs: &mut T,
    ) -> Vec<(u64, NtfsFileName)> {
        let mut best: HashMap<(u64, Vec<u8>), (u8, NtfsFileName)> = HashMap::new();
        let mut iter = file.attributes();
        while let Some(item) = iter.next(fs) {
            let item = match item {
                Ok(item) => item,
                Err(err) => {
                    tracing::trace!("NTFS attribute walk skipped: {err}");
                    continue;
                }
            };
            let attribute = match item.to_attribute() {
                Ok(attribute) => attribute,
                Err(_) => continue,
            };
            if !matches!(attribute.ty(), Ok(NtfsAttributeType::FileName)) {
                continue;
            }
            let name: NtfsFileName = match attribute.structured_value(fs) {
                Ok(name) => name,
                Err(err) => {
                    tracing::trace!("NTFS $FILE_NAME parse skipped: {err}");
                    continue;
                }
            };
            // Lower rank wins; pure DOS aliases are dropped entirely.
            let rank: u8 = match name.namespace() {
                NtfsFileNamespace::Win32AndDos => 0,
                NtfsFileNamespace::Win32 => 1,
                NtfsFileNamespace::Posix => 2,
                NtfsFileNamespace::Dos => continue,
            };
            let parent = name.parent_directory_reference().file_record_number();
            let identity = (parent, name.name().0.to_vec());
            match best.get(&identity) {
                Some((existing_rank, _)) if *existing_rank <= rank => {}
                _ => {
                    best.insert(identity, (rank, name));
                }
            }
        }
        best.into_iter()
            .map(|((parent, _), (_, name))| (parent, name))
            .collect()
    }

    pub(super) fn read_deleted_entries(
        &self,
        fs: &mut PartitionReader<'_>,
        record_number: u64,
    ) -> Vec<NtfsDeletedRaw> {
        let file = match self.ntfs.file(fs, record_number) {
            Ok(file) => file,
            Err(err) => {
                tracing::trace!("NTFS MFT record {record_number} unreadable: {err}");
                return Vec::new();
            }
        };
        if file.flags().contains(NtfsFileFlags::IN_USE) {
            return Vec::new();
        }

        let names = self.best_filenames_by_parent(&file, fs);

        let is_directory = file.is_directory();
        let data_size = data_value_length(&file, fs);
        let size = if is_directory {
            0
        } else {
            data_size.unwrap_or(0)
        };

        let data_runs = if is_directory {
            Vec::new()
        } else {
            self.capture_data_runs(&file, fs, size)
        };

        names
            .into_iter()
            .filter_map(|(parent_record_number, name)| {
                let display = name.name().to_string_lossy();
                if display.is_empty() {
                    return None;
                }
                Some(NtfsDeletedRaw {
                    record_number,
                    parent_record_number,
                    name: display,
                    is_directory,
                    size: if is_directory {
                        0
                    } else {
                        data_size.unwrap_or_else(|| name.data_size())
                    },
                    modified: ntfs_time_to_rfc3339(name.modification_time()),
                    accessed: ntfs_time_to_rfc3339(name.access_time()),
                    created: ntfs_time_to_rfc3339(name.creation_time()),
                    mft_changed: ntfs_time_to_rfc3339(name.mft_record_modification_time()),
                    data_runs: data_runs.clone(),
                })
            })
            .collect()
    }

    /// Captures the deleted record's unnamed `$DATA` non-resident runlist as
    /// image-absolute byte runs `(offset, len)`, clamped to the volume and to
    /// `logical_size` (the `$DATA` valid length). Returns empty for resident
    /// data, streams with sparse holes/uninitialized tails, compressed/encrypted streams, or BitLocker
    /// backings (no stable image offset). The runs let a deleted file be read
    /// even after its MFT record is reused (P1-5). Clamping to `logical_size`
    /// keeps the recovered content from including the final cluster's slack
    /// tail — runs are otherwise cluster-granular (allocated size), which would
    /// emit up to `cluster_size - 1` trailing bytes past the reported size.
    pub(super) fn capture_data_runs(
        &self,
        file: &ntfs::NtfsFile,
        fs: &mut PartitionReader<'_>,
        logical_size: u64,
    ) -> Vec<(u64, u64)> {
        let Some(_) = self.backing.base_offset() else {
            return Vec::new();
        };
        let length = self.backing.length();
        let Some(Ok(item)) = file.data(fs, "") else {
            return Vec::new();
        };
        let Ok(attribute) = item.to_attribute() else {
            return Vec::new();
        };
        if !matches!(attribute.ty(), Ok(NtfsAttributeType::Data)) {
            return Vec::new();
        }
        let flags = attribute.flags();
        if flags.contains(NtfsAttributeFlags::COMPRESSED)
            || flags.contains(NtfsAttributeFlags::ENCRYPTED)
        {
            return Vec::new();
        }
        let storage = match attribute.storage_info() {
            Ok(Some(storage)) if storage.initialized_size >= logical_size => storage,
            Ok(_) => return Vec::new(),
            Err(err) => {
                tracing::debug!("NTFS deleted storage geometry: {err}");
                return Vec::new();
            }
        };
        let extents = match attribute.data_extents(fs) {
            Ok(extents) => extents,
            Err(err) => {
                tracing::debug!("NTFS deleted storage mapping: {err}");
                return Vec::new();
            }
        };
        let mut runs = Vec::new();
        let mut emitted = 0u64;
        for extent in extents {
            if extent.logical_offset >= logical_size {
                break;
            }
            // The legacy captured-run model cannot represent holes or an
            // uninitialized tail. Keep the logical record reader in that case.
            let Some(position) = extent.data_position.value() else {
                return Vec::new();
            };
            let count = extent
                .allocated_size
                .min(logical_size - extent.logical_offset);
            if extent.logical_offset != emitted
                || position
                    .get()
                    .checked_add(count)
                    .is_none_or(|end| end > length)
            {
                return Vec::new();
            }
            let ranges = match self.backing.physical_ranges(position.get(), count) {
                Ok(ranges) => ranges,
                Err(error) => {
                    tracing::warn!("NTFS deleted source mapping: {error:#}");
                    return Vec::new();
                }
            };
            runs.extend(ranges);
            emitted += count;
        }
        if emitted != logical_size || storage.allocated_size < logical_size {
            return Vec::new();
        }
        runs
    }

    /// Builds an [`FsNode`] for a single cached deleted-record entry,
    /// re-anchored under `parent_record` so the FsTree view shows it inline
    /// with its surviving siblings (or under root when its parent is unavailable).
    pub fn fs_node_from_deleted(&self, raw: &NtfsDeletedRaw, parent_record: u64) -> FsNode {
        FsNode {
            id: self.node_id(raw.record_number),
            name: raw.name.clone(),
            parent_id: Some(self.parent_id(parent_record)),
            kind: if raw.is_directory {
                NodeKind::Dir
            } else {
                NodeKind::File
            },
            size: raw.size,
            deleted: true,
            inode: Some(raw.record_number),
            timestamps: Mactb {
                modified: raw.modified.clone(),
                accessed: raw.accessed.clone(),
                created: raw.created.clone(),
                mft_changed: raw.mft_changed.clone(),
                birth: raw.created.clone(),
            },
            link_target: None,
            has_snapshot: false,
        }
    }
}
