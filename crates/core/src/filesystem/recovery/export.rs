//! Exclusive outputs, bounded logical streams, hashes and a recovery manifest.
//! Files keep their recovered folders under one folder per volume (see `layout`).
use super::*;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{BufWriter, ErrorKind, Write},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryExport {
    pub directory: String,
    pub recovered: usize,
    pub failed: usize,
    pub bytes_written: u64,
    pub errors: Vec<String>,
    pub report_path: String,
    pub read_faults: ReadFaultReport,
}

pub fn recover(
    image: &Arc<EvidenceSession>,
    scan: &RecoveryScan,
    ids: &[String],
    destination: &Path,
    cancel: &CancellationToken,
    progress: &mut dyn FnMut(u64, Option<u64>, &str),
) -> Result<RecoveryExport> {
    image.require_source()?;
    image.validate_source_file_stable()?;
    if ids.is_empty() {
        bail!("No recovery candidates selected");
    }
    let ids = ids.iter().collect::<HashSet<_>>();
    let selected = scan
        .candidates
        .iter()
        .filter(|candidate| ids.contains(&candidate.id))
        .collect::<Vec<_>>();
    if selected.len() != ids.len() {
        bail!("Recovery results are stale; scan again");
    }
    if selected.iter().any(|candidate| !candidate.readable) {
        bail!("Selected entries have no readable data mapping");
    }
    check_cancel(cancel)?;
    // Every invocation gets its own directory. Existing files cannot be truncated.
    fs::create_dir_all(destination)?;
    let directory = destination.join(format!("recovery-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&directory)?;
    let report_path = directory.join("recovery-report.jsonl");
    let mut report = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&report_path)?,
    );
    writeln!(
        report,
        "{}",
        serde_json::json!({"source": image.path, "scan_id": scan.scan_id, "options": scan.options, "volumes": scan.volumes, "scan_read_faults": scan.read_faults, "warning": "Recovered mappings may contain overwritten data; successful export does not prove original contents."})
    )?;
    let mut result = RecoveryExport {
        directory: directory.display().to_string(),
        recovered: 0,
        failed: 0,
        bytes_written: 0,
        errors: Vec::new(),
        report_path: report_path.display().to_string(),
        read_faults: ReadFaultReport::default(),
    };
    let mut layout = layout::ExportLayout::new(scan);
    for (index, candidate) in selected.iter().enumerate() {
        check_cancel(cancel)?;
        let components = layout.file_path(candidate);
        let relative = components.join("/");
        let output = components
            .iter()
            .fold(directory.clone(), |path, component| path.join(component));
        let (recovered, faults) = image.collect_read_faults(|| {
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)?;
            }
            recover_one(image, scan, candidate, &output, cancel)
        });
        match recovered {
            Ok(hash) => {
                result.recovered += 1;
                result.bytes_written = result
                    .bytes_written
                    .checked_add(candidate.node.size)
                    .context("Recovered size overflow")?;
                writeln!(
                    report,
                    "{}",
                    serde_json::json!({"candidate": candidate, "output": output, "relative_output": relative, "sha256": hash, "read_faults": faults})
                )?;
            }
            Err(error) => {
                remove_empty_folders(&directory, &output);
                if cancel.is_cancelled() {
                    return Err(error);
                }
                result.failed += 1;
                let message = format!("{}: {error:#}", candidate.path);
                writeln!(
                    report,
                    "{}",
                    serde_json::json!({"candidate": candidate, "error": message, "read_faults": faults})
                )?;
                if result.errors.len() < 100 {
                    result.errors.push(message);
                }
            }
        }
        // The report records provenance for successful and failed reads.
        progress(
            index as u64 + 1,
            Some(selected.len() as u64),
            &candidate.path,
        );
    }
    image.validate_source_file_stable()?;
    report.flush()?;
    Ok(result)
}

fn recover_one(
    image: &Arc<EvidenceSession>,
    scan: &RecoveryScan,
    candidate: &RecoveryCandidate,
    path: &Path,
    cancel: &CancellationToken,
) -> Result<String> {
    let mut output = PartialFile::new(path.to_path_buf())?;
    let mut written = 0u64;
    let mut hash = Sha256::new();
    let mut visitor = |bytes: &[u8]| -> Result<()> {
        check_cancel(cancel)?;
        written = written
            .checked_add(bytes.len() as u64)
            .context("Recovery size overflow")?;
        if written > candidate.node.size {
            bail!("File stream exceeds logical size");
        }
        output
            .writer
            .as_mut()
            .context("Recovery output closed")?
            .write_all(bytes)?;
        hash.update(bytes);
        Ok(())
    };
    if candidate.node.size > 0 {
        let recovered_volume = scan
            .volumes
            .get(candidate.volume_index)
            .context("Invalid recovery volume")?;
        let partition = &recovered_volume.partition;
        if candidate.node.id.starts_with("deleted:run:") {
            let mut fields = candidate.node.id.rsplit(':');
            let length = parse_hex(fields.next())?;
            let offset = parse_hex(fields.next())?;
            let base = partition
                .start_lba
                .checked_mul(image.sector_size as u64)
                .context("Volume offset overflow")?;
            let end = partition
                .length_lba
                .checked_mul(image.sector_size as u64)
                .and_then(|length| base.checked_add(length))
                .context("Volume bound overflow")?;
            if length != candidate.node.size
                || offset < base
                || offset
                    .checked_add(length)
                    .is_none_or(|last| last > end || last > image.len())
            {
                bail!("Deleted data mapping is outside its volume");
            }
            let initialized = candidate.initialized_size.unwrap_or(length);
            if initialized > length {
                bail!("Initialized data exceeds logical file size");
            }
            let mut done = 0;
            while done < initialized {
                check_cancel(cancel)?;
                let count = (initialized - done).min(1024 * 1024) as usize;
                let bytes = image.read_range(offset + done, count)?;
                if bytes.len() != count {
                    bail!("Short read in deleted file");
                }
                visitor(&bytes)?;
                done += count as u64;
            }
            if initialized < length {
                let zeros = vec![0; 1024 * 1024];
                while done < length {
                    let count = (length - done).min(zeros.len() as u64) as usize;
                    visitor(&zeros[..count])?;
                    done += count as u64;
                }
            }
        } else {
            volume::Reader::open(image.disk_image(), recovered_volume)?
                .stream(&candidate.node, &mut visitor)?;
        }
    }
    if written != candidate.node.size {
        bail!(
            "Short recovery: expected {}, read {written}",
            candidate.node.size
        );
    }
    output
        .writer
        .as_mut()
        .context("Recovery output closed")?
        .flush()?;
    output.keep = true;
    Ok(hex::encode(hash.finalize()))
}

/// Removes folders a failed output leaves empty, up to but excluding the export root.
fn remove_empty_folders(root: &Path, file: &Path) {
    let mut current = file.parent();
    while let Some(folder) = current {
        if folder == root || !folder.starts_with(root) {
            break;
        }
        match fs::remove_dir(folder) {
            Ok(()) => current = folder.parent(),
            // Never created: its ancestors may still be empty.
            Err(error) if error.kind() == ErrorKind::NotFound => current = folder.parent(),
            Err(error) if error.kind() == ErrorKind::DirectoryNotEmpty => break,
            Err(error) => {
                tracing::warn!(
                    "Cannot remove empty recovery folder {}: {error}",
                    folder.display()
                );
                break;
            }
        }
    }
}

fn parse_hex(value: Option<&str>) -> Result<u64> {
    Ok(u64::from_str_radix(
        value
            .context("Missing data mapping")?
            .trim_start_matches("0x"),
        16,
    )?)
}

struct PartialFile {
    path: PathBuf,
    writer: Option<BufWriter<fs::File>>,
    keep: bool,
}
impl PartialFile {
    fn new(path: PathBuf) -> Result<Self> {
        let writer = BufWriter::new(
            OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?,
        );
        Ok(Self {
            path,
            writer: Some(writer),
            keep: false,
        })
    }
}
impl Drop for PartialFile {
    fn drop(&mut self) {
        drop(self.writer.take());
        if !self.keep {
            // Close the Windows file handle before removing a failed output.
            if let Err(error) = fs::remove_file(&self.path) {
                tracing::warn!(
                    "Cannot remove partial recovery {}: {error}",
                    self.path.display()
                );
            }
        }
    }
}
