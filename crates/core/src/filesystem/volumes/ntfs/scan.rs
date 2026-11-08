//! Whole-MFT tree edges and alternate stream discovery.
use super::*;

impl<'a> NtfsVolume<'a> {
    /// Sequentially scans every MFT slot once and returns one [`NtfsTreeEdge`]
    /// per `$FILE_NAME` of every readable record — in-use and deleted alike —
    /// plus every surfaced named ADS ([`NtfsAdsStream`]) of every in-use file.
    /// This is the bulk primitive behind a fast whole-volume snapshot: the
    /// caller rebuilds the directory tree in memory from the parent references
    /// (see `core::catalog::snapshot::volume_snapshot`) instead of re-opening the volume and
    /// walking a B-tree directory index per directory, which on an E01 image
    /// means re-parsing the boot sector and re-reading the 128 KiB `$UpCase`
    /// table on every single directory.
    ///
    /// Scanning stops once `max_edges` edges are collected; the returned flag
    /// is `true` when that cap was hit so the caller can mark the snapshot
    /// truncated. `on_progress` receives the running record count every
    /// [`SCAN_CHECK_EVERY`] records; cancellation is polled at the same cadence
    /// and surfaces as `Err("snapshot build cancelled")`.
    pub fn scan_tree_edges(
        &self,
        max_edges: usize,
        cancel: &CancellationToken,
        on_progress: &mut dyn FnMut(u64),
    ) -> Result<(Vec<NtfsTreeEdge>, Vec<NtfsAdsStream>, bool)> {
        let mut fs = self.reader();
        let mft = self
            .ntfs
            .file(&mut fs, 0)
            .map_err(|err| anyhow!("failed to read $MFT record 0: {err}"))?;
        let mft_data_item = mft
            .data(&mut fs, "")
            .ok_or_else(|| anyhow!("$MFT has no $DATA attribute"))?
            .map_err(|err| anyhow!("$MFT $DATA attribute: {err}"))?;
        let mft_data_attr = mft_data_item
            .to_attribute()
            .map_err(|err| anyhow!("$MFT $DATA to_attribute: {err}"))?;
        let total_bytes = mft_data_attr.value_length();
        let record_size = self.ntfs.file_record_size().max(1) as u64;
        let total_records = total_bytes / record_size;

        let mut edges = Vec::new();
        let mut ads = Vec::new();
        let mut truncated = false;
        for record_number in 0..total_records {
            if record_number % SCAN_CHECK_EVERY == 0 {
                if cancel.is_cancelled() {
                    anyhow::bail!("snapshot build cancelled");
                }
                on_progress(record_number);
            }
            if edges.len() >= max_edges {
                truncated = true;
                break;
            }
            self.collect_record_edges(&mut fs, record_number, &mut edges, &mut ads);
        }
        on_progress(edges.len() as u64);
        Ok((edges, ads, truncated))
    }

    /// Appends one [`NtfsTreeEdge`] per distinct parent/name edge that names
    /// `record_number` via its `$FILE_NAME` attributes, and one
    /// [`NtfsAdsStream`] per surfaced named ADS of an in-use file. Unreadable
    /// records and the `.` self-entry are skipped silently so one bad slot
    /// never aborts a whole-volume scan.
    pub(super) fn collect_record_edges(
        &self,
        fs: &mut PartitionReader<'_>,
        record_number: u64,
        out: &mut Vec<NtfsTreeEdge>,
        ads_out: &mut Vec<NtfsAdsStream>,
    ) {
        let file = match self.ntfs.file(fs, record_number) {
            Ok(file) => file,
            Err(err) => {
                tracing::trace!("NTFS MFT record {record_number} unreadable: {err}");
                return;
            }
        };
        let deleted = !file.flags().contains(NtfsFileFlags::IN_USE);
        let is_directory = file.is_directory();
        let link_target = match super::reparse::read_link(&file, fs) {
            Ok(target) => target,
            Err(err) => {
                tracing::warn!("NTFS reparse record {record_number}: {err}");
                None
            }
        };
        // Resolve the canonical `$DATA` length once; per-edge we fall back to
        // the size cached in that edge's own `$FILE_NAME` if `$DATA` is absent.
        let data_size = if is_directory {
            None
        } else {
            data_value_length(&file, fs)
        };
        for (parent_record, name) in self.best_filenames_by_parent(&file, fs) {
            let display = name.name().to_string_lossy();
            if display.is_empty() || display == "." {
                continue;
            }
            let (kind, size) = if let Some(target) = &link_target {
                (NodeKind::Symlink, target.len() as u64)
            } else if is_directory {
                (NodeKind::Dir, 0)
            } else {
                (
                    NodeKind::File,
                    data_size.unwrap_or_else(|| name.data_size()),
                )
            };
            out.push(NtfsTreeEdge {
                record_number,
                parent_record_number: parent_record,
                node: FsNode {
                    id: self.node_id(record_number),
                    name: display,
                    parent_id: Some(self.parent_id(parent_record)),
                    kind,
                    size,
                    deleted,
                    inode: Some(record_number),
                    timestamps: file_name_timestamps(&name),
                    link_target: link_target.clone(),
                    has_snapshot: false,
                },
            });
        }

        // Named ADS surface as independent rows for live files AND live
        // directories: the volume root and ordinary folders are a classic
        // hiding spot (`\:secret.txt`) and must be surfaced. A deleted record's streams are recovery
        // noise and stay out. Each surfaced stream (see `is_surfaced_ads_name`)
        // becomes one `NtfsAdsStream` keyed back to this record; the same name
        // is kept once even when a non-resident attribute is split across pieces
        // (matching `list_data_streams`' de-dup). The root directory (which has
        // no `$FILE_NAME` edge of its own) is reattached by the snapshot builder.
        if !deleted {
            let mut seen = HashSet::<String>::new();
            let mut iter = file.attributes();
            while let Some(item) = iter.next(fs) {
                let Ok(item) = item else { continue };
                let Ok(attribute) = item.to_attribute() else {
                    continue;
                };
                if !matches!(attribute.ty(), Ok(NtfsAttributeType::Data)) {
                    continue;
                }
                let Ok(name) = attribute.name() else { continue };
                let name = name.to_string_lossy();
                if !is_surfaced_ads_name(&name) {
                    continue;
                }
                if !seen.insert(name.clone()) {
                    continue;
                }
                ads_out.push(NtfsAdsStream {
                    record_number,
                    name,
                    size: attribute.value_length(),
                });
            }
        }
    }
}
