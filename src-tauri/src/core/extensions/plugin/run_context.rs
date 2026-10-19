//! What the host keeps for one run while the plugin works: the run's view,
//! the read-only file handles it issued, temporary copies, find cursors,
//! the record writer and the read faults met on the way.
//!
//! Every id the context issues starts with the run id (`<run>.f3`,
//! `<run>.view`), so a plugin → host call reaches its run by id alone, and
//! a handle of one run is never valid in another. Reads are wrapped in the
//! evidence's read-fault collection: bytes the backend had to synthesize
//! (bad sectors) are reported with the run instead of passing as evidence.

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{BufWriter, Write as _},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

use lovelycarve_plugin::protocol::{
    self, EmitAck, EmitBatch, FileInfo, FileRead, FileReadv, FindPage, Lease, NodeInfo, NodePage,
    RunProgress, RunWarn, ViewFind, ViewList, ViewPath,
};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use super::{
    view::{FindScan, HostView},
    PluginError, PluginPermission, Result,
};
use crate::core::{
    analysis::results::record_store::{RecordWriteError, RecordWriter},
    evidence::image::{BadRegion, ReadFaultReport},
    filesystem::{
        evidence_session::EvidenceSession,
        tree::{
            fs_tree::{self, FsNode},
            fs_tree_export::is_windows_device_name,
        },
    },
};

/// Largest file one lease copies.
const MAX_MATERIALIZED_FILE: u64 = 4 * 1024 * 1024 * 1024;
/// Bytes and leases one run may hold at once.
const MAX_RUN_MATERIALIZED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_RUN_LEASES: usize = 16;
/// Handles one run may open.
const MAX_FILES: usize = 1_000_000;
/// Find cursors one run may keep.
const MAX_SCANS: usize = 8;

/// Reports `(current, total)` progress of a run.
pub type ProgressSink = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

#[derive(Clone)]
struct RegisteredFile {
    info: FileInfo,
    node_id: String,
}

struct LeaseEntry {
    directory: PathBuf,
    size: u64,
}

/// Host state of one run (see the module docs).
pub struct RunContext {
    run_id: String,
    image: Arc<EvidenceSession>,
    view: Option<Arc<HostView>>,
    permissions: HashSet<PluginPermission>,
    files: Mutex<HashMap<String, RegisteredFile>>,
    next_id: AtomicU64,
    leases: Mutex<HashMap<String, LeaseEntry>>,
    materialized_bytes: AtomicU64,
    materialize_root: PathBuf,
    scans: Mutex<HashMap<String, FindScan>>,
    writer: Mutex<RecordWriter>,
    progress: ProgressSink,
    faults: Mutex<Vec<BadRegion>>,
    cancellation: CancellationToken,
}

impl RunContext {
    pub fn new(
        run_id: String,
        image: Arc<EvidenceSession>,
        view: Option<Arc<HostView>>,
        permissions: impl IntoIterator<Item = PluginPermission>,
        writer: RecordWriter,
        progress: ProgressSink,
    ) -> Self {
        let materialize_root = std::env::temp_dir().join(format!(
            "lovelycarve-plugin-{}-{run_id}",
            std::process::id()
        ));
        Self {
            run_id,
            image,
            view,
            permissions: permissions.into_iter().collect(),
            files: Mutex::default(),
            next_id: AtomicU64::new(1),
            leases: Mutex::default(),
            materialized_bytes: AtomicU64::new(0),
            materialize_root,
            scans: Mutex::default(),
            writer: Mutex::new(writer),
            progress,
            faults: Mutex::default(),
            cancellation: CancellationToken::new(),
        }
    }

    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Ends every host call of the run in flight (`CANCELLED`).
    pub fn cancel(&self) {
        self.cancellation.cancel();
    }

    /// Records and warnings stored so far.
    pub fn counts(&self) -> (u64, u64, u64) {
        let writer = self.writer.lock();
        (writer.records(), writer.warnings(), writer.time_points())
    }

    /// The faults the run's reads met.
    pub fn read_faults(&self) -> ReadFaultReport {
        ReadFaultReport::from_regions(self.faults.lock().clone())
    }

    /// Warns from the host side (for example an input that vanished).
    pub fn host_warning(&self, code: &str, message: &str, path: Option<&str>) {
        if let Err(error) = self.writer.lock().warn(code, message, path, None) {
            tracing::warn!(run_id = %self.run_id, "run warning not stored: {error:#}");
        }
    }

    fn issue(&self, kind: char) -> String {
        format!(
            "{}.{kind}{}",
            self.run_id,
            self.next_id.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// Registers a file-tree node as one of the run's input handles.
    pub fn register_node(
        &self,
        node: &FsNode,
        view_path: Option<String>,
        root: Option<String>,
        root_path: Option<String>,
    ) -> Result<FileInfo> {
        self.register_file(
            &node.id,
            &node.name,
            view_path,
            root,
            root_path,
            node.deleted,
            super::view::timestamps(&node.timestamps),
        )
    }

    /// Registers a file node as one of the run's handles.
    #[allow(clippy::too_many_arguments)]
    fn register_file(
        &self,
        node_id: &str,
        name: &str,
        view_path: Option<String>,
        root: Option<String>,
        root_path: Option<String>,
        deleted: bool,
        timestamps: protocol::Timestamps,
    ) -> Result<FileInfo> {
        let stat = fs_tree::file_stat(&self.image, node_id).map_err(|error| {
            tracing::debug!(%node_id, "plugin file stat failed: {error:#}");
            PluginError::Rpc {
                code: "FILE_UNAVAILABLE".to_owned(),
                message: format!("{name} is unavailable"),
            }
        })?;
        let mut files = self.files.lock();
        if files.len() >= MAX_FILES {
            return Err(PluginError::ResourceLimit(format!(
                "a run opens at most {MAX_FILES} files"
            )));
        }
        let info = FileInfo {
            file_id: self.issue('f'),
            name: name.to_owned(),
            size: stat.size,
            mime: Some(stat.mime),
            view_path,
            root,
            root_path,
            node_id: Some(node_id.to_owned()),
            deleted,
            timestamps,
        };
        files.insert(
            info.file_id.clone(),
            RegisteredFile {
                info: info.clone(),
                node_id: node_id.to_owned(),
            },
        );
        Ok(info)
    }

    fn file(&self, file_id: &str) -> Result<RegisteredFile> {
        self.files
            .lock()
            .get(file_id)
            .cloned()
            .ok_or_else(|| PluginError::Rpc {
                code: "INVALID_FILE_HANDLE".to_owned(),
                message: "the file handle does not belong to this run".to_owned(),
            })
    }

    fn view(&self, view_id: &str) -> Result<&Arc<HostView>> {
        self.view
            .as_ref()
            .filter(|view| view.id() == view_id)
            .ok_or_else(|| PluginError::Rpc {
                code: "INVALID_VIEW".to_owned(),
                message: "the run has no such view".to_owned(),
            })
    }

    fn check_run(&self, run_id: &str) -> Result<()> {
        if run_id == self.run_id {
            Ok(())
        } else {
            Err(PluginError::Rpc {
                code: "INVALID_RUN".to_owned(),
                message: "the run id does not match".to_owned(),
            })
        }
    }

    fn check_cancelled(&self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            Err(PluginError::Rpc {
                code: "CANCELLED".to_owned(),
                message: "the run was cancelled".to_owned(),
            })
        } else {
            Ok(())
        }
    }

    /// Reads with the run's fault collection active.
    fn collect<T>(&self, read: impl FnOnce() -> T) -> T {
        let (output, report) = self.image.collect_read_faults(read);
        if report.read_faults_detected {
            self.faults.lock().extend(report.bad_regions);
        }
        output
    }

    // ── run.* ──────────────────────────────────────────────────────────────

    pub fn emit(&self, batch: EmitBatch) -> Result<EmitAck> {
        self.check_run(&batch.run_id)?;
        self.check_cancelled()?;
        if batch.records.len() > protocol::MAX_BATCH_RECORDS {
            return Err(PluginError::ResourceLimit(format!(
                "a batch holds at most {} records",
                protocol::MAX_BATCH_RECORDS
            )));
        }
        let accepted =
            self.writer
                .lock()
                .insert_batch(&batch.records)
                .map_err(|error| match error {
                    RecordWriteError::Rejected { code, message } => PluginError::Rpc {
                        code: code.to_owned(),
                        message,
                    },
                    RecordWriteError::Store(error) => PluginError::Rpc {
                        code: "HOST_STORE_FAILED".to_owned(),
                        message: format!("records could not be stored: {error:#}"),
                    },
                })?;
        Ok(EmitAck { accepted })
    }

    pub fn warn(&self, warning: RunWarn) -> Result<()> {
        self.check_run(&warning.run_id)?;
        let file = warning
            .file_id
            .as_deref()
            .and_then(|file_id| self.file(file_id).ok());
        let path = file.as_ref().and_then(|file| {
            file.info
                .view_path
                .clone()
                .or_else(|| file.info.root_path.clone())
                .or_else(|| Some(file.info.name.clone()))
        });
        self.writer
            .lock()
            .warn(
                &warning.code,
                &warning.message,
                path.as_deref(),
                file.as_ref().map(|file| file.node_id.as_str()),
            )
            .map_err(|error| PluginError::Rpc {
                code: "HOST_STORE_FAILED".to_owned(),
                message: format!("the warning could not be stored: {error:#}"),
            })
    }

    pub fn progress(&self, progress: RunProgress) -> Result<()> {
        self.check_run(&progress.run_id)?;
        (self.progress)(progress.current, progress.total);
        Ok(())
    }

    // ── file.* ─────────────────────────────────────────────────────────────

    pub fn read(&self, read: &FileRead) -> Result<Vec<u8>> {
        self.check_cancelled()?;
        if read.length > protocol::MAX_READ {
            return Err(PluginError::ResourceLimit(format!(
                "file.read length {} exceeds {}",
                read.length,
                protocol::MAX_READ
            )));
        }
        let file = self.file(&read.file_id)?;
        self.read_range(&file, read.offset, read.length)
    }

    fn read_range(&self, file: &RegisteredFile, offset: u64, length: u64) -> Result<Vec<u8>> {
        if offset >= file.info.size || length == 0 {
            return Ok(Vec::new());
        }
        let length = usize::try_from(length.min(file.info.size - offset)).unwrap_or(usize::MAX);
        let mut bytes = self
            .collect(|| fs_tree::read_file_range(&self.image, &file.node_id, offset, length))
            .map_err(|error| {
                tracing::debug!(file_id = %file.info.file_id, "plugin file read failed: {error:#}");
                PluginError::Rpc {
                    code: "FILE_READ_FAILED".to_owned(),
                    message: format!("{} could not be read", file.info.name),
                }
            })?;
        bytes.truncate(length);
        Ok(bytes)
    }

    pub fn readv(&self, read: &FileReadv) -> Result<Vec<Vec<u8>>> {
        self.check_cancelled()?;
        if read.ranges.len() > protocol::MAX_READV_RANGES {
            return Err(PluginError::ResourceLimit(format!(
                "file.readv takes at most {} ranges",
                protocol::MAX_READV_RANGES
            )));
        }
        let total = read
            .ranges
            .iter()
            .try_fold(0_u64, |total, (_, length)| total.checked_add(*length))
            .ok_or_else(|| PluginError::ResourceLimit("file.readv length overflows".to_owned()))?;
        if total > protocol::MAX_READV_BYTES {
            return Err(PluginError::ResourceLimit(format!(
                "file.readv reads at most {} bytes",
                protocol::MAX_READV_BYTES
            )));
        }
        let file = self.file(&read.file_id)?;
        read.ranges
            .iter()
            .map(|(offset, length)| self.read_range(&file, *offset, *length))
            .collect()
    }

    pub fn materialize(&self, file_id: &str) -> Result<Lease> {
        self.check_cancelled()?;
        if !self
            .permissions
            .contains(&PluginPermission::FileMaterialize)
        {
            return Err(PluginError::Rpc {
                code: "PERMISSION_DENIED".to_owned(),
                message: "the package does not declare the file_materialize permission".to_owned(),
            });
        }
        let file = self.file(file_id)?;
        if file.info.size > MAX_MATERIALIZED_FILE {
            return Err(PluginError::ResourceLimit(format!(
                "files larger than {MAX_MATERIALIZED_FILE} bytes are not copied"
            )));
        }
        {
            let leases = self.leases.lock();
            let used = self.materialized_bytes.load(Ordering::Acquire);
            if leases.len() >= MAX_RUN_LEASES
                || used.saturating_add(file.info.size) > MAX_RUN_MATERIALIZED_BYTES
            {
                return Err(PluginError::ResourceLimit(
                    "the run holds too many temporary copies; release some first".to_owned(),
                ));
            }
            self.materialized_bytes
                .fetch_add(file.info.size, Ordering::AcqRel);
        }
        let lease_id = self.issue('l');
        let directory = self.materialize_root.join(lease_id.replace('.', "_"));
        let copied = self.copy_out(&file, &directory);
        let (path, size) = match copied {
            Ok(copied) => copied,
            Err(error) => {
                self.materialized_bytes
                    .fetch_sub(file.info.size, Ordering::AcqRel);
                remove_directory(&directory);
                return Err(error);
            }
        };
        self.leases.lock().insert(
            lease_id.clone(),
            LeaseEntry {
                directory,
                size: file.info.size,
            },
        );
        Ok(Lease {
            lease_id,
            path: path.to_string_lossy().into_owned(),
            size,
        })
    }

    fn copy_out(
        &self,
        file: &RegisteredFile,
        directory: &std::path::Path,
    ) -> Result<(PathBuf, u64)> {
        fs::create_dir_all(directory)?;
        let path = directory.join(safe_file_name(&file.info.name));
        let output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut writer = BufWriter::new(output);
        let mut written = 0_u64;
        let streamed = self.collect(|| {
            fs_tree::stream_file_chunks(&self.image, &file.node_id, 1024 * 1024, |chunk| {
                if self.cancellation.is_cancelled() {
                    anyhow::bail!("the run was cancelled");
                }
                written = written
                    .checked_add(chunk.len() as u64)
                    .filter(|written| *written <= file.info.size)
                    .ok_or_else(|| anyhow::anyhow!("the file grew beyond its size"))?;
                writer.write_all(chunk)?;
                Ok(())
            })
        });
        let finished = streamed.and_then(|_| writer.flush().map_err(anyhow::Error::from));
        if let Err(error) = finished {
            tracing::debug!(file_id = %file.info.file_id, "plugin materialization failed: {error:#}");
            return Err(PluginError::Rpc {
                code: "FILE_MATERIALIZE_FAILED".to_owned(),
                message: format!("{} could not be copied", file.info.name),
            });
        }
        if written != file.info.size {
            return Err(PluginError::Rpc {
                code: "FILE_MATERIALIZE_FAILED".to_owned(),
                message: format!(
                    "{} ended after {written} of {} bytes",
                    file.info.name, file.info.size
                ),
            });
        }
        Ok((path, written))
    }

    pub fn release(&self, lease_id: &str) -> Result<()> {
        let lease = self
            .leases
            .lock()
            .remove(lease_id)
            .ok_or_else(|| PluginError::Rpc {
                code: "INVALID_LEASE".to_owned(),
                message: "the lease does not belong to this run".to_owned(),
            })?;
        self.materialized_bytes
            .fetch_sub(lease.size, Ordering::AcqRel);
        remove_directory(&lease.directory);
        Ok(())
    }

    // ── view.* ─────────────────────────────────────────────────────────────

    pub fn view_stat(&self, request: &ViewPath) -> Result<Option<NodeInfo>> {
        self.check_cancelled()?;
        self.collect(|| self.view(&request.view_id)?.stat(&request.path))
    }

    pub fn view_list(&self, request: &ViewList) -> Result<NodePage> {
        self.check_cancelled()?;
        if request.limit == 0 || request.limit > protocol::MAX_LIST_PAGE {
            return Err(PluginError::Rpc {
                code: "INVALID_PARAMS".to_owned(),
                message: format!("view.list limit must be 1..={}", protocol::MAX_LIST_PAGE),
            });
        }
        self.collect(|| {
            self.view(&request.view_id)?
                .list(&request.path, request.skip, request.limit)
        })
    }

    pub fn view_open(&self, request: &ViewPath) -> Result<Option<FileInfo>> {
        self.check_cancelled()?;
        let view = self.view(&request.view_id)?;
        let Some(file) = self.collect(|| view.locate_file(&request.path))? else {
            return Ok(None);
        };
        self.register_node(
            &file.node,
            Some(file.view_path),
            Some(file.root),
            Some(file.root_path),
        )
        .map(Some)
    }

    pub fn view_find(&self, request: &ViewFind) -> Result<FindPage> {
        self.check_cancelled()?;
        let view = self.view(&request.view_id)?;
        let (cursor, mut scan) = match &request.cursor {
            Some(cursor) => {
                let scan = self
                    .scans
                    .lock()
                    .remove(cursor)
                    .ok_or_else(|| PluginError::Rpc {
                        code: "INVALID_CURSOR".to_owned(),
                        message: "the find cursor is unknown or already used".to_owned(),
                    })?;
                (cursor.clone(), scan)
            }
            None => {
                if self.scans.lock().len() >= MAX_SCANS {
                    return Err(PluginError::ResourceLimit(format!(
                        "a run keeps at most {MAX_SCANS} unfinished finds"
                    )));
                }
                (self.issue('c'), FindScan::new(&request.patterns)?)
            }
        };
        let paths = self.collect(|| view.find_page(&mut scan))?;
        let visited = scan.visited;
        let next = if scan.is_complete() {
            None
        } else {
            self.scans.lock().insert(cursor.clone(), scan);
            Some(cursor)
        };
        Ok(FindPage {
            paths,
            visited,
            cursor: next,
        })
    }
}

impl Drop for RunContext {
    fn drop(&mut self) {
        remove_directory(&self.materialize_root);
    }
}

fn remove_directory(directory: &std::path::Path) {
    if let Err(error) = fs::remove_dir_all(directory) {
        if error.kind() != std::io::ErrorKind::NotFound {
            tracing::debug!(path = %directory.display(), "plugin temporary copy cleanup failed: {error}");
        }
    }
}

/// A host file name for an evidence file name (temporary copies keep the
/// name so tools recognise the type).
fn safe_file_name(name: &str) -> String {
    let cleaned = name
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
        .take(120)
        .collect::<String>();
    let cleaned = cleaned.trim_matches([' ', '.']);
    if cleaned.is_empty() {
        return "evidence.bin".to_owned();
    }
    let stem = cleaned.split('.').next().unwrap_or(cleaned);
    if is_windows_device_name(stem) {
        format!("_{cleaned}")
    } else {
        cleaned.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::safe_file_name;

    #[test]
    fn temporary_copies_get_host_safe_names() {
        assert_eq!(safe_file_name("a?.bin"), "a.bin");
        assert_eq!(safe_file_name("con.txt"), "_con.txt");
        assert_eq!(safe_file_name(" .. "), "evidence.bin");
    }
}
