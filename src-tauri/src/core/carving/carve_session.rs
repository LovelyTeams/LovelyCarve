//! In-memory state for background recovery sessions: an optional filesystem
//! metadata phase followed by an optional raw-signature carving phase.

use std::{sync::Arc, time::Duration};

use parking_lot::RwLock;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::core::carving::carver::{CarveArtifact, CarveScanProgress};
use crate::core::filesystem::recovery::{RecoveryOptions, RecoveryScan};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CarveSessionStatus {
    Queued,
    Running,
    Paused,
    Completed,
    Cancelled,
    Failed,
}

impl CarveSessionStatus {
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
}

/// The phase a session is running, or ran last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CarveScanPhase {
    /// Directory entries, MFT records and lost FAT/NTFS/exFAT volumes.
    Filesystem,
    /// Raw file signatures.
    Signature,
}

/// Filesystem recovery state; present only when the session enables a filesystem family.
#[derive(Debug)]
pub struct FilesystemPhase {
    pub options: RecoveryOptions,
    pub done: u64,
    pub total: Option<u64>,
    pub result: Option<Arc<RecoveryScan>>,
    /// A failed phase keeps the session running when signature carving follows.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FilesystemScanSummary {
    pub options: RecoveryOptions,
    pub done: u64,
    pub total: Option<u64>,
    pub finished: bool,
    pub volumes: usize,
    pub directories: usize,
    pub files: usize,
    pub truncated: bool,
    pub read_faults_detected: bool,
    pub error: Option<String>,
}

#[derive(Debug)]
pub struct CarveScanSession {
    pub scan_id: String,
    pub task_id: String,
    pub image_id: String,
    pub created_unix_ms: i64,
    pub status: CarveSessionStatus,
    pub phase: CarveScanPhase,
    /// Whether the raw-signature phase runs after the optional filesystem phase.
    pub signatures: bool,
    pub filesystem: Option<FilesystemPhase>,
    pub scope_label: String,
    pub bytes_scanned: u64,
    pub bytes_total: u64,
    pub signatures_found: u64,
    pub artifacts: Vec<CarveArtifact>,
    pub current_offset: u64,
    pub truncated: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CarveScanSnapshot {
    pub scan_id: String,
    pub task_id: String,
    pub image_id: String,
    pub created_unix_ms: i64,
    pub status: CarveSessionStatus,
    pub phase: CarveScanPhase,
    pub signatures: bool,
    pub filesystem: Option<FilesystemScanSummary>,
    pub scope_label: String,
    pub bytes_scanned: u64,
    pub bytes_total: u64,
    pub signatures_found: u64,
    pub artifacts_found: u64,
    pub current_offset: u64,
    pub truncated: bool,
    pub error: Option<String>,
}

impl CarveScanSession {
    /// A signature-only session; see [`Self::with_filesystem_recovery`].
    pub fn new(
        scan_id: String,
        task_id: String,
        image_id: String,
        scope_label: String,
        bytes_total: u64,
    ) -> Self {
        Self {
            scan_id,
            task_id,
            image_id,
            created_unix_ms: chrono::Utc::now().timestamp_millis(),
            status: CarveSessionStatus::Queued,
            phase: CarveScanPhase::Signature,
            signatures: true,
            filesystem: None,
            scope_label,
            bytes_scanned: 0,
            bytes_total,
            signatures_found: 0,
            artifacts: Vec::new(),
            current_offset: 0,
            truncated: false,
            error: None,
        }
    }

    /// Runs filesystem recovery first; `signatures` keeps or drops the carving phase.
    pub fn with_filesystem_recovery(mut self, options: RecoveryOptions, signatures: bool) -> Self {
        self.phase = CarveScanPhase::Filesystem;
        self.signatures = signatures;
        self.filesystem = Some(FilesystemPhase {
            options,
            done: 0,
            total: None,
            result: None,
            error: None,
        });
        self
    }

    pub fn snapshot(&self) -> CarveScanSnapshot {
        CarveScanSnapshot {
            scan_id: self.scan_id.clone(),
            task_id: self.task_id.clone(),
            image_id: self.image_id.clone(),
            created_unix_ms: self.created_unix_ms,
            status: self.status,
            phase: self.phase,
            signatures: self.signatures,
            filesystem: self.filesystem.as_ref().map(FilesystemPhase::summary),
            scope_label: self.scope_label.clone(),
            bytes_scanned: self.bytes_scanned,
            bytes_total: self.bytes_total,
            signatures_found: self.signatures_found,
            artifacts_found: self.artifacts.len() as u64,
            current_offset: self.current_offset,
            truncated: self.truncated,
            error: self.error.clone(),
        }
    }

    pub fn apply_progress(&mut self, progress: &CarveScanProgress) {
        self.bytes_scanned = progress.bytes_scanned;
        self.bytes_total = progress.bytes_total;
        self.signatures_found = progress.signatures_found;
        self.current_offset = progress.current_offset;
    }
}

impl FilesystemPhase {
    fn summary(&self) -> FilesystemScanSummary {
        let result = self.result.as_deref();
        FilesystemScanSummary {
            options: self.options,
            done: self.done,
            total: self.total,
            finished: result.is_some() || self.error.is_some(),
            volumes: result.map_or(0, |scan| scan.volumes.len()),
            directories: result.map_or(0, |scan| scan.directories.len()),
            files: result.map_or(0, |scan| scan.candidates.len()),
            truncated: result.is_some_and(|scan| scan.truncated),
            read_faults_detected: result.is_some_and(|scan| scan.read_faults.read_faults_detected),
            error: self.error.clone(),
        }
    }
}

/// Blocks a worker between reads while its session is paused.
pub fn wait_while_paused(session: &RwLock<CarveScanSession>, cancellation: &CancellationToken) {
    while session.read().status == CarveSessionStatus::Paused && !cancellation.is_cancelled() {
        std::thread::sleep(Duration::from_millis(40));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::image::ReadFaultReport;

    #[test]
    fn filesystem_summary_reports_the_phase_and_its_result() {
        let options = RecoveryOptions {
            fat: true,
            ntfs: false,
            exfat: false,
            include_existing: false,
        };
        let mut session = CarveScanSession::new(
            "scan".into(),
            "task".into(),
            "image".into(),
            "整盘字节流".into(),
            1024,
        )
        .with_filesystem_recovery(options, false);
        let pending = session.snapshot();
        assert_eq!(pending.phase, CarveScanPhase::Filesystem);
        assert!(!pending.signatures);
        assert_eq!(
            pending.filesystem,
            Some(FilesystemScanSummary {
                options,
                done: 0,
                total: None,
                finished: false,
                volumes: 0,
                directories: 0,
                files: 0,
                truncated: false,
                read_faults_detected: false,
                error: None,
            })
        );
        let phase = session.filesystem.as_mut().unwrap();
        phase.done = 512;
        phase.total = Some(1024);
        phase.result = Some(Arc::new(RecoveryScan {
            scan_id: "scan".into(),
            image_id: "image".into(),
            options,
            ranges: Vec::new(),
            volumes: Vec::new(),
            candidates: Vec::new(),
            directories: Vec::new(),
            truncated: true,
            warnings: Vec::new(),
            read_faults: ReadFaultReport::default(),
        }));
        let summary = session.snapshot().filesystem.unwrap();
        assert!(summary.finished);
        assert!(summary.truncated);
        assert_eq!((summary.done, summary.total), (512, Some(1024)));
        assert_eq!(
            CarveScanSession::new("a".into(), "b".into(), "c".into(), "d".into(), 1)
                .snapshot()
                .filesystem,
            None
        );
    }
}
