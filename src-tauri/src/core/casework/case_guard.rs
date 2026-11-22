//! The logical case lock: the one list of edits a locked case refuses.
//!
//! Locking freezes what the case is: its name, the evidence that belongs to
//! it, the examiner's platform choices and the published volume snapshots.
//! Browsing, analysis and automatic upkeep keep running: detection results
//! are still written back, missing volumes still get their first snapshot and
//! a built snapshot still gets its index. Every refusal goes through
//! [`super::CaseState::ensure_unlocked`], so the desktop app and the CLI
//! refuse exactly the same edits.

/// An edit that a locked case refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockedEdit {
    Rename,
    AddEvidence,
    RemoveEvidence,
    SetPlatform,
    BuildSnapshot,
    DropSnapshot,
}

impl LockedEdit {
    fn activity(self) -> &'static str {
        match self {
            Self::Rename => "renaming the case",
            Self::AddEvidence => "adding evidence",
            Self::RemoveEvidence => "removing evidence",
            Self::SetPlatform => "changing the evidence platform",
            Self::BuildSnapshot => "building snapshots",
            Self::DropSnapshot => "deleting snapshots",
        }
    }
}

/// A locked case refused `edit`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("case is locked; unlock it before {}", edit.activity())]
pub struct CaseLocked {
    pub edit: LockedEdit,
}
