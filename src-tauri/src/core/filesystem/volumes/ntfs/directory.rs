//! Directory traversal and filesystem node projection.
use super::*;

impl<'a> NtfsVolume<'a> {
    /// Resolve exact evidence names, preserving case-distinct directories.
    /// Reparse targets are returned as records and are never traversed implicitly.
    pub fn find_record_by_path(&self, path: &str) -> Result<u64> {
        let mut current = ROOT_RECORD;
        let components: Vec<_> = path
            .split(['/', '\\'])
            .filter(|component| !component.is_empty() && *component != ".")
            .collect();
        for (index, component) in components.iter().copied().enumerate() {
            anyhow::ensure!(
                component != "..",
                "NTFS path must not escape through parent components"
            );
            let children = self.children(current)?;
            let node = children
                .iter()
                .find(|node| node.name == component)
                .ok_or_else(|| anyhow!("NTFS path component not found: {component}"))?;
            current = node
                .inode
                .ok_or_else(|| anyhow!("NTFS path component has no record"))?;
            if index + 1 < components.len() {
                anyhow::ensure!(
                    node.kind == NodeKind::Dir,
                    "NTFS path traverses a non-directory or reparse point: {component}"
                );
            }
        }
        Ok(current)
    }
    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children_with_deleted(ROOT_RECORD, None)
    }

    /// Active directory listing only — never merges deleted MFT entries.
    /// Used internally by [`Self::children_with_deleted`] and by tests.
    pub fn children(&self, record_number: u64) -> Result<Vec<FsNode>> {
        let mut fs = self.reader();
        let file = self
            .ntfs
            .file(&mut fs, record_number)
            .map_err(|err| anyhow!("NTFS file record {record_number}: {err}"))?;
        // Deleted directories can retain the directory flag after $INDEX_ROOT
        // is removed. Their remaining index blocks may already be reused, so
        // only the deleted-record snapshot supplies children for these records.
        if !file.is_directory() || !file.flags().contains(NtfsFileFlags::IN_USE) {
            return Ok(Vec::new());
        }
        let index = file
            .directory_index(&mut fs)
            .map_err(|err| anyhow!("NTFS directory index: {err}"))?;
        let mut iter = index.entries();
        let mut out = Vec::new();
        let mut seen = HashSet::new();

        while let Some(entry) = iter.next(&mut fs) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(err) => {
                    tracing::debug!("NTFS index entry skipped: {err}");
                    continue;
                }
            };
            let key = match entry.key() {
                Some(Ok(key)) => key,
                Some(Err(err)) => {
                    tracing::debug!("NTFS index entry key error: {err}");
                    continue;
                }
                None => continue,
            };
            // Skip pure DOS 8.3 aliases — they duplicate the canonical Win32 entry.
            if matches!(key.namespace(), NtfsFileNamespace::Dos) {
                continue;
            }
            let child_file = match entry.to_file(&self.ntfs, &mut fs) {
                Ok(child) => child,
                Err(err) => {
                    tracing::debug!("NTFS to_file failed: {err}");
                    continue;
                }
            };
            let child_record = child_file.file_record_number();
            let name = key.name().to_string_lossy();
            if !seen.insert((child_record, key.name().0.to_vec())) {
                continue;
            }
            if name == "." {
                continue;
            }
            let link_target = match super::reparse::read_link(&child_file, &mut fs) {
                Ok(target) => target,
                Err(err) => {
                    tracing::warn!("NTFS reparse record {child_record}: {err}");
                    None
                }
            };
            let kind = if link_target.is_some() {
                NodeKind::Symlink
            } else if key.is_directory() {
                NodeKind::Dir
            } else {
                NodeKind::File
            };
            let size = if matches!(kind, NodeKind::File) {
                let mut size = key.data_size();
                if let Some(Ok(item)) = child_file.data(&mut fs, "") {
                    if let Ok(attribute) = item.to_attribute() {
                        size = attribute.value_length();
                    }
                }
                size
            } else {
                0
            };
            out.push(FsNode {
                id: self.node_id(child_record),
                name,
                parent_id: Some(self.parent_id(record_number)),
                kind,
                size,
                deleted: false,
                inode: Some(child_record),
                timestamps: file_name_timestamps(&key),
                link_target,
                has_snapshot: false,
            });
        }

        out.sort_by(|a, b| {
            let ak = u8::from(a.kind != NodeKind::Dir);
            let bk = u8::from(b.kind != NodeKind::Dir);
            ak.cmp(&bk)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(out)
    }

    /// Directory listing that also attaches every cached deleted MFT record
    /// whose `$FILE_NAME` parent reference matches `record_number`. The
    /// active listing skips indexes belonging to deleted directories. A
    /// snapshot's child references do not prove that the parent is deleted;
    /// unreadable records and damaged live directory indexes remain errors.
    pub fn children_with_deleted(
        &self,
        record_number: u64,
        snapshot: Option<&NtfsDeletedSnapshot>,
    ) -> Result<Vec<FsNode>> {
        let mut out = self.children(record_number)?;

        if let Some(snap) = snapshot {
            let already: HashSet<u64> = out.iter().filter_map(|node| node.inode).collect();
            for raw in snap.for_parent(record_number) {
                if already.contains(&raw.record_number) {
                    continue;
                }
                out.push(self.fs_node_from_deleted(raw, record_number));
            }
            out.sort_by(|a, b| {
                let ak = u8::from(a.kind != NodeKind::Dir);
                let bk = u8::from(b.kind != NodeKind::Dir);
                ak.cmp(&bk)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
        }

        Ok(out)
    }
}
