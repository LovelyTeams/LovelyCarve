//! Filesystem file reads, ranges, streaming and metadata.
use super::*;

impl ZfsVolume {
    pub fn read_file(
        &self,
        dataset_path: &str,
        file_path: &str,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        if file_path.is_empty() {
            bail!("cannot read ZFS filesystem root as a file");
        }
        let drive = self.open_drive()?;
        let mos = drive
            .get_most_recent_mos()
            .map_err(|err| anyhow!("ZFS get_most_recent_mos: {err}"))?;
        let dsl_dir = walk_dsl_dir(&mos, dataset_path)?;
        let dataset = dsl_dir.get_head_dataset()?;
        let os = dataset.get_object_set()?;
        let zpl = os
            .as_zpl()
            .ok_or_else(|| anyhow!("dataset {dataset_path} is not a ZPL filesystem"))?;

        let (parent_path, leaf) = split_path(file_path);
        let dir = walk_zpl_dir(&zpl, parent_path)?;
        let key = ZapString::from_byte_slice(leaf.as_bytes())
            .ok_or_else(|| anyhow!("filename {leaf} contains a NUL byte"))?;
        let entry = dir
            .get_child(&key)
            .map_err(|err| anyhow!("ZFS get_child {leaf}: {err}"))?
            .ok_or_else(|| anyhow!("ZFS file not found: {file_path}"))?;
        let file = match entry {
            ZfsDirEntry::File(f) => f,
            ZfsDirEntry::Dir(_) => bail!("{file_path} is a directory, not a file"),
        };

        use std::io::Read;
        let total = zpl_file_size(&file).unwrap_or(u64::MAX);
        let cap = (max_bytes as u64).min(total) as usize;
        let mut reader = file
            .reader()
            .map_err(|err| anyhow!("ZFS reader {file_path}: {err}"))?;
        let mut out = vec![0u8; cap];
        let mut filled = 0usize;
        while filled < out.len() {
            let n = reader
                .read(&mut out[filled..])
                .map_err(|err| anyhow!("ZFS read {file_path}: {err}"))?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        out.truncate(filled);
        Ok(out)
    }

    pub fn stream_file<F>(
        &self,
        dataset_path: &str,
        file_path: &str,
        chunk_size: usize,
        mut visitor: F,
    ) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        if file_path.is_empty() {
            bail!("cannot stream ZFS filesystem root as a file");
        }
        let drive = self.open_drive()?;
        let mos = drive
            .get_most_recent_mos()
            .map_err(|err| anyhow!("ZFS get_most_recent_mos: {err}"))?;
        let dsl_dir = walk_dsl_dir(&mos, dataset_path)?;
        let dataset = dsl_dir.get_head_dataset()?;
        let os = dataset.get_object_set()?;
        let zpl = os
            .as_zpl()
            .ok_or_else(|| anyhow!("dataset {dataset_path} is not a ZPL filesystem"))?;
        let (parent_path, leaf) = split_path(file_path);
        let dir = walk_zpl_dir(&zpl, parent_path)?;
        let key = ZapString::from_byte_slice(leaf.as_bytes())
            .ok_or_else(|| anyhow!("filename {leaf} contains a NUL byte"))?;
        let entry = dir
            .get_child(&key)
            .map_err(|err| anyhow!("ZFS get_child {leaf}: {err}"))?
            .ok_or_else(|| anyhow!("ZFS file not found: {file_path}"))?;
        let file = match entry {
            ZfsDirEntry::File(f) => f,
            ZfsDirEntry::Dir(_) => bail!("{file_path} is a directory, not a file"),
        };

        use std::io::Read;
        let mut reader = file
            .reader()
            .map_err(|err| anyhow!("ZFS reader {file_path}: {err}"))?;
        let mut buf = vec![0u8; chunk_size.max(STREAM_FLOOR)];
        let mut total = 0u64;
        loop {
            let n = reader
                .read(&mut buf)
                .map_err(|err| anyhow!("ZFS stream {file_path}: {err}"))?;
            if n == 0 {
                break;
            }
            visitor(&buf[..n])?;
            total += n as u64;
        }
        Ok(total)
    }

    pub fn file_stat(&self, dataset_path: &str, file_path: &str, node_id: &str) -> Result<FsStat> {
        if file_path.is_empty() {
            bail!("cannot stat ZFS filesystem root as a file");
        }
        let drive = self.open_drive()?;
        let mos = drive
            .get_most_recent_mos()
            .map_err(|err| anyhow!("ZFS get_most_recent_mos: {err}"))?;
        let dsl_dir = walk_dsl_dir(&mos, dataset_path)?;
        let dataset = dsl_dir.get_head_dataset()?;
        let os = dataset.get_object_set()?;
        let zpl = os
            .as_zpl()
            .ok_or_else(|| anyhow!("dataset {dataset_path} is not a ZPL filesystem"))?;
        let (parent_path, leaf) = split_path(file_path);
        let dir = walk_zpl_dir(&zpl, parent_path)?;
        let key = ZapString::from_byte_slice(leaf.as_bytes())
            .ok_or_else(|| anyhow!("filename {leaf} contains a NUL byte"))?;
        let entry = dir
            .get_child(&key)
            .map_err(|err| anyhow!("ZFS get_child {leaf}: {err}"))?
            .ok_or_else(|| anyhow!("ZFS file not found: {file_path}"))?;
        let file = match entry {
            ZfsDirEntry::File(f) => f,
            ZfsDirEntry::Dir(_) => bail!("{file_path} is a directory, not a file"),
        };

        use std::io::Read;
        let size = zpl_file_size(&file).unwrap_or(0);
        let mode = zpl_file_mode(&file);
        let want = SAMPLE_BYTES.min(size as usize);
        let mut sample = vec![0u8; want];
        let mut reader = file
            .reader()
            .map_err(|err| anyhow!("ZFS reader {file_path}: {err}"))?;
        let mut filled = 0usize;
        while filled < sample.len() {
            let n = reader
                .read(&mut sample[filled..])
                .map_err(|err| anyhow!("ZFS sample {file_path}: {err}"))?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        sample.truncate(filled);

        let magic_hex = if sample.is_empty() {
            None
        } else {
            Some(
                sample
                    .iter()
                    .take(16)
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<Vec<_>>()
                    .join(" "),
            )
        };
        let entropy = if sample.is_empty() {
            None
        } else {
            Some(shannon_entropy(&sample))
        };

        Ok(FsStat {
            node_id: node_id.to_string(),
            size,
            inode: None,
            mode,
            mime: detect_mime_bytes(&sample).to_string(),
            magic_hex,
            entropy,
            // ZFS stores file content as a tree of `BlockPtr`s with up to
            // six indirection levels — surfacing `FsDataRun`s would require
            // walking every indirect block per file, which `zfs-rs` does not
            // expose. Leave empty for now.
            data_runs: Vec::<FsDataRun>::new(),
        })
    }
}

pub(super) fn zpl_file_size<Z>(file: &ZfsZPLFile<'_, '_, Z>) -> Option<u64>
where
    Z: zfs_rs::ZAP,
{
    let attrs = file.list_attributes().ok()?;
    for (attr, value) in &attrs {
        if attr.name.as_byte_slice() == b"ZPL_SIZE" {
            if let SAValue::U64(values) = value {
                if values.len() == 1 {
                    return Some(values[0]);
                }
            }
        }
    }
    None
}

pub(super) fn zpl_file_mode<Z>(file: &ZfsZPLFile<'_, '_, Z>) -> Option<u16>
where
    Z: zfs_rs::ZAP,
{
    let attrs = file.list_attributes().ok()?;
    for (attr, value) in &attrs {
        if attr.name.as_byte_slice() == b"ZPL_MODE" {
            if let SAValue::U64(values) = value {
                if values.len() == 1 {
                    return Some(values[0] as u16);
                }
            }
        }
    }
    None
}

pub(super) fn zpl_file_timestamps<Z>(file: &ZfsZPLFile<'_, '_, Z>) -> Mactb
where
    Z: zfs_rs::ZAP,
{
    let mut ts = Mactb::default();
    let attrs = match file.list_attributes() {
        Ok(v) => v,
        Err(_) => return ts,
    };
    for (attr, value) in &attrs {
        let secs = match value {
            // ZPL timestamps are stored as U64Array of length 2: [seconds, nanos].
            SAValue::U64(values) if !values.is_empty() => values[0],
            _ => continue,
        };
        let bytes = attr.name.as_byte_slice();
        let formatted = format_zfs_time(secs);
        match bytes {
            b"ZPL_ATIME" => ts.accessed = Some(formatted),
            b"ZPL_MTIME" => ts.modified = Some(formatted),
            b"ZPL_CTIME" => ts.mft_changed = Some(formatted),
            b"ZPL_CRTIME" => {
                ts.created = Some(formatted.clone());
                ts.birth = Some(formatted);
            }
            _ => {}
        }
    }
    ts
}

pub(super) fn format_zfs_time(seconds: u64) -> String {
    DateTime::<Utc>::from_timestamp(seconds as i64, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_default()
}
