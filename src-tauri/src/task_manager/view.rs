//! What the task center shows of a native task: what it works on, how far
//! it got, what it found and how it ended. Commands fill the view through
//! their registration; the registry keeps it with the task and, for kinds
//! listed for the session, after the task ends.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::{TaskKind, TaskProgressUnit};

/// Whether and how the task center lists a kind of task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskListing {
    /// Short work behind one page (previews, probes, scans the page shows
    /// itself): not listed.
    Hidden,
    /// Listed while it runs and, for the rest of the session, after it ends.
    Session,
    /// Listed while it runs; afterwards its domain store lists it (search
    /// history, timeline status, carving sessions).
    Domain,
}

impl TaskKind {
    pub fn listing(self) -> TaskListing {
        match self {
            Self::AutoDetect
            | Self::BitlockerExport
            | Self::CarveRecovery
            | Self::DirectoryExport
            | Self::DockerPrepare
            | Self::EncryptedVolumeUnlock
            | Self::ImageExport
            | Self::ImageHash
            | Self::ImageLoad
            | Self::SessionSnapshot
            | Self::VolumeSnapshot => TaskListing::Session,
            Self::CarveScan | Self::GlobalSearch | Self::TimelineBuild => TaskListing::Domain,
            Self::BitlockerScan
            | Self::BitlockerUnlock
            | Self::FilevaultInspect
            | Self::FlatList
            | Self::IosBackupAudit
            | Self::IosBackupUnlock
            | Self::MediaMaterialization
            | Self::OcrScan
            | Self::QrScan
            | Self::ShadowCopyOpen => TaskListing::Hidden,
        }
    }
}

/// A count a task reports, by meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskCount {
    /// Keyword and pattern hits.
    Hits,
    /// Files with at least one hit.
    MatchedFiles,
    /// Timeline rows written so far.
    Rows,
    /// Timeline rows from file times.
    FileRows,
    /// Timeline rows from analysis results.
    EventRows,
    /// Directory entries read or indexed.
    Entries,
    /// Files found by a recovery scan.
    Found,
    /// Files written to the destination.
    Files,
    /// Source bytes that could not be read and were filled with zeroes.
    SyntheticBytes,
    /// Unreadable source regions met on the way.
    BadRegions,
}

/// A value from a task's work shown as it is (identifiers the interface
/// may translate, or names from the evidence).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskLabel {
    /// Detected platform id (`android`, `windows` …).
    Platform,
    /// The platform's vendor display name.
    PlatformName,
    /// Detected evidence type id.
    EvidenceType,
    /// Encryption scheme of a volume being unlocked (`luks` …).
    Scheme,
    /// Cipher of an unlocked volume.
    Cipher,
    /// The archive member being decompressed.
    Item,
}

/// A yes-or-no fact about a task's result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskFlag {
    /// The work stopped at its entry or result limit.
    Truncated,
    /// The result is saved with the case and reused next time.
    Persisted,
    /// An earlier result was reused instead of doing the work again.
    Reused,
    /// The evidence was identified as a backup.
    Backup,
}

/// The part of a task's work running now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TaskStage {
    /// Opening an evidence file.
    Opening,
    /// Decompressing an archive member.
    Decompressing,
    DetectingPartitions,
    DetectingFilesystems,
    /// Reading a volume's directories.
    Filesystem,
    /// Indexing what was read.
    Index,
    /// Carving by file signatures.
    Signature,
}

/// Work done so far; `total` is unknown until the task can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskAmount {
    pub unit: TaskProgressUnit,
    pub done: u64,
    pub total: Option<u64>,
}

/// The task center's view of one native task.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    /// What the task works on, as parts the interface formats: paths,
    /// keywords, volume keys, container names.
    pub subject: Vec<String>,
    pub stage: Option<TaskStage>,
    pub progress: Option<TaskAmount>,
    pub counts: BTreeMap<TaskCount, u64>,
    pub labels: BTreeMap<TaskLabel, String>,
    pub flags: BTreeSet<TaskFlag>,
}

/// How a task ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum TaskOutcome {
    Succeeded,
    Failed {
        error: String,
    },
    Cancelled,
    /// The task went away without reporting how it ended.
    Interrupted,
}

/// An error that ends a task. Implementers tell a cancellation apart from a
/// failure, and describe a failure the way the task center shows it.
pub trait TaskFailure {
    /// The task stopped because it was cancelled.
    fn is_cancellation(&self) -> bool;
    /// What went wrong.
    fn describe(&self) -> String;
}

impl TaskOutcome {
    /// The outcome of a task that returned `result`.
    pub fn of<T, E: TaskFailure>(result: &Result<T, E>) -> Self {
        match result {
            Ok(_) => Self::Succeeded,
            Err(error) if error.is_cancellation() => Self::Cancelled,
            Err(error) => Self::Failed {
                error: error.describe(),
            },
        }
    }
}
