//! Observations of actual detector work, independent of any embedding host.
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DetectStage {
    LvmMetadata,
    MdMetadata,
    MdArrays,
    Firmware,
    LvmVolumes,
    Heuristic,
    Registering,
    Complete,
}

/// Counts belong to this stage only. `None` means its work is not yet bounded;
/// neither these counts nor stage order represent a percentage of the scan.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectProgress {
    pub stage: DetectStage,
    pub current: usize,
    pub total: Option<usize>,
}

impl DetectProgress {
    pub fn new(stage: DetectStage, current: usize, total: Option<usize>) -> Self {
        Self {
            stage,
            current,
            total,
        }
    }
}
