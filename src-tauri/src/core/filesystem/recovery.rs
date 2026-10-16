//! Read-only discovery of lost FAT/NTFS/exFAT volumes and recovery candidates.
//! Candidate identity includes the scan and volume, independent of source partition IDs.
//! Signature carving is a separate phase of the same scan session.

use super::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{FsNode, NodeKind},
};
use crate::core::evidence::{
    image::{DiskImage, ReadFaultReport},
    partitions::{Partition, PartitionKind},
};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, sync::Arc};
use tokio_util::sync::CancellationToken;

pub mod discovery;
pub mod export;
mod layout;
mod recycle;
mod volume;

pub const MAX_CANDIDATES: usize = 20_000;
pub const MAX_VOLUMES: usize = 128;

/// Filesystem families to recover; every family off disables filesystem recovery.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct RecoveryOptions {
    #[serde(default)]
    pub fat: bool,
    #[serde(default)]
    pub ntfs: bool,
    #[serde(default)]
    pub exfat: bool,
    /// Also list live files of known volumes; lost volumes always list every file.
    #[serde(default)]
    pub include_existing: bool,
}

impl RecoveryOptions {
    pub fn enabled(&self) -> bool {
        self.fat || self.ntfs || self.exfat
    }

    fn accepts(&self, kind: PartitionKind) -> bool {
        match kind {
            PartitionKind::Fat12 | PartitionKind::Fat16 | PartitionKind::Fat32 => self.fat,
            PartitionKind::Ntfs => self.ntfs,
            PartitionKind::ExFat => self.exfat,
            _ => false,
        }
    }
}

/// A scanned sector range. Discovery also reports known volumes starting inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SectorRange {
    pub start_sector: u64,
    pub sector_count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryVolume {
    pub partition: Partition,
    pub lost: bool,
    pub reconstruction: Option<super::volumes::ntfs::RecoveredNtfsGeometry>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryCandidate {
    pub id: String,
    pub volume_index: usize,
    pub path: String,
    pub node: FsNode,
    /// A readable mapping is a candidate, never a guarantee of original contents.
    pub readable: bool,
    /// exFAT initialized prefix; the remaining logical tail must be zero.
    pub initialized_size: Option<u64>,
    pub source_path: Option<String>,
    pub source_node: Option<FsNode>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryDirectory {
    pub source_path: Option<String>,
    pub source_node: Option<FsNode>,
    pub volume_index: usize,
    pub path: String,
    pub node: FsNode,
}

#[derive(Debug, Clone, Serialize)]
pub struct RecoveryScan {
    pub scan_id: String,
    pub image_id: String,
    pub options: RecoveryOptions,
    pub ranges: Vec<SectorRange>,
    pub volumes: Vec<RecoveryVolume>,
    pub candidates: Vec<RecoveryCandidate>,
    pub directories: Vec<RecoveryDirectory>,
    pub truncated: bool,
    pub warnings: Vec<String>,
    pub read_faults: ReadFaultReport,
}

pub fn scan(
    image: &Arc<EvidenceSession>,
    scan_id: String,
    options: RecoveryOptions,
    ranges: Vec<SectorRange>,
    cancel: &CancellationToken,
    progress: &mut dyn FnMut(u64, Option<u64>, &str),
) -> Result<RecoveryScan> {
    image.require_source()?;
    image.validate_source_file_stable()?;
    if !options.enabled() {
        bail!("Select at least one filesystem family");
    }
    let (volumes, truncated) = discovery::discover(image, &options, &ranges, cancel, progress)?;
    let mut result = RecoveryScan {
        scan_id,
        image_id: image.id.clone(),
        options,
        ranges,
        volumes,
        candidates: Vec::new(),
        directories: Vec::new(),
        truncated,
        warnings: Vec::new(),
        read_faults: ReadFaultReport::default(),
    };
    for index in 0..result.volumes.len() {
        check_cancel(cancel)?;
        let recovered_volume = result.volumes[index].clone();
        let partition = recovered_volume.partition.clone();
        let mut volume_options = result.options;
        volume_options.include_existing |= recovered_volume.lost;
        progress(
            0,
            None,
            &format!("{} @ LBA {}", family(partition.kind), partition.start_lba),
        );
        let scanned = volume::scan_volume(
            image.disk_image(),
            &recovered_volume,
            &volume_options,
            index,
            &mut result,
            cancel,
            progress,
        );
        match scanned {
            Ok(truncated) => result.truncated |= truncated,
            Err(error) if cancel.is_cancelled() => return Err(error),
            Err(error) => result.volumes[index].error = Some(format!("{error:#}")),
        }
        if recovered_volume.reconstruction.is_some() {
            recycle::restore_recycle_names(
                image.disk_image(),
                &recovered_volume,
                index,
                &mut result.candidates,
                &mut result.directories,
                cancel,
            )?;
        }
        if result.candidates.len() >= MAX_CANDIDATES {
            result.truncated = true;
            break;
        }
    }
    check_cancel(cancel)?;
    image.validate_source_file_stable()?;
    Ok(result)
}

fn check_cancel(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        bail!("File recovery cancelled");
    }
    Ok(())
}

pub fn family(kind: PartitionKind) -> &'static str {
    match kind {
        PartitionKind::Fat12 => "FAT12",
        PartitionKind::Fat16 => "FAT16",
        PartitionKind::Fat32 => "FAT32",
        PartitionKind::Ntfs => "NTFS",
        PartitionKind::ExFat => "exFAT",
        _ => "Unsupported",
    }
}

#[cfg(test)]
pub(crate) mod tests;
