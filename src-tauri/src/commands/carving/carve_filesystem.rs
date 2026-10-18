//! Filesystem-recovery phase of a scan session, its results and recovered-file export.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use crate::host::State;
use parking_lot::RwLock;
use tokio_util::sync::CancellationToken;

use crate::app::error::AppError;
use crate::commands::carving::carve::scan_session;
use crate::core::carving::carve_session::{wait_while_paused, CarveScanSession};
use crate::core::carving::carver::{CarveRegion, CarveScanError};
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::recovery::{self, export::RecoveryExport, RecoveryScan, SectorRange};
use crate::state::AppState;
use crate::task_manager::{TaskManager, TaskProgressUnit};

/// Whole sectors inside the scan regions; a partial edge sector cannot hold a boot record.
pub(crate) fn sector_ranges(regions: &[CarveRegion], sector_size: u64) -> Vec<SectorRange> {
    regions
        .iter()
        .filter_map(|region| {
            let start_sector = region.offset.div_ceil(sector_size);
            let end_sector = region.offset.saturating_add(region.length) / sector_size;
            let sector_count = end_sector.checked_sub(start_sector)?;
            (sector_count > 0).then_some(SectorRange {
                start_sector,
                sector_count,
            })
        })
        .collect()
}

/// Runs filesystem recovery when the session enables it. A failure is kept on
/// the phase and ends the session only when no carving phase follows.
pub(crate) fn run_filesystem_phase(
    image: &Arc<EvidenceSession>,
    session: &RwLock<CarveScanSession>,
    ranges: Vec<SectorRange>,
    cancellation: &CancellationToken,
    progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<(), CarveScanError> {
    let (scan_id, options) = {
        let scan = session.read();
        match &scan.filesystem {
            Some(phase) => (scan.scan_id.clone(), phase.options),
            None => return Ok(()),
        }
    };
    let (result, faults) = image.collect_read_faults(|| {
        recovery::scan(
            image,
            scan_id,
            options,
            ranges,
            cancellation,
            &mut |done, total, _| {
                wait_while_paused(session, cancellation);
                if let Some(phase) = session.write().filesystem.as_mut() {
                    phase.done = done;
                    phase.total = total;
                }
                progress(done, total);
            },
        )
    });
    let mut scan = session.write();
    let signatures = scan.signatures;
    let Some(phase) = scan.filesystem.as_mut() else {
        return Ok(());
    };
    match result {
        Ok(mut recovered) => {
            recovered.read_faults = faults;
            phase.result = Some(Arc::new(recovered));
            Ok(())
        }
        Err(_) if cancellation.is_cancelled() => Err(CarveScanError::Cancelled),
        Err(error) => {
            phase.error = Some(format!("{error:#}"));
            if signatures {
                Ok(())
            } else {
                Err(CarveScanError::Failed(error))
            }
        }
    }
}

/// Recovered volumes, directories and files of a session; `None` until that phase finishes.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn carve_filesystem_results(
    state: State<'_, AppState>,
    scan_id: String,
) -> Result<Option<RecoveryScan>, AppError> {
    let session = scan_session(&state, &scan_id).await?;
    let result = session
        .read()
        .filesystem
        .as_ref()
        .and_then(|phase| phase.result.clone());
    Ok(result.map(|scan| (*scan).clone()))
}

/// The finished filesystem result of a session, after checking that every
/// selected candidate still exists and has a readable mapping.
pub(crate) fn filesystem_result(
    session: &CarveScanSession,
    file_ids: &[String],
) -> Result<Arc<RecoveryScan>, AppError> {
    let scan = session
        .filesystem
        .as_ref()
        .and_then(|phase| phase.result.clone())
        .ok_or_else(|| AppError::not_found("该扫描没有可恢复的文件系统记录"))?;
    let wanted = file_ids.iter().collect::<HashSet<_>>();
    let selected = scan
        .candidates
        .iter()
        .filter(|candidate| wanted.contains(&candidate.id))
        .collect::<Vec<_>>();
    if selected.len() != wanted.len() {
        return Err(AppError::not_found("部分恢复文件不存在或已过期"));
    }
    if selected.iter().any(|candidate| !candidate.readable) {
        return Err(AppError::invalid_input("所选文件没有可读的数据映射"));
    }
    Ok(scan)
}

/// Exports filesystem candidates into a new `recovery-<UUID>` directory with a JSONL report.
pub(crate) fn recover_files(
    image: &Arc<EvidenceSession>,
    scan: &RecoveryScan,
    file_ids: &[String],
    destination: &Path,
    tasks: &TaskManager,
    task_id: &str,
    cancellation: &CancellationToken,
) -> Result<RecoveryExport, AppError> {
    let (result, faults) = image.collect_read_faults(|| {
        recovery::export::recover(
            image,
            scan,
            file_ids,
            destination,
            cancellation,
            &mut |done, total, message| {
                tasks.emit_progress(
                    task_id,
                    TaskProgressUnit::Entries,
                    done,
                    total,
                    Some(message.to_string()),
                );
            },
        )
    });
    match result {
        Ok(mut exported) => {
            exported.read_faults = faults;
            Ok(exported)
        }
        Err(_) if cancellation.is_cancelled() => Err(AppError::cancelled("文件恢复已取消")),
        Err(error) => Err(AppError::from_anyhow(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::image::ReadFaultReport;
    use crate::core::filesystem::recovery::{RecoveryCandidate, RecoveryOptions};
    use crate::core::filesystem::tree::fs_tree::{FsNode, Mactb, NodeKind};

    fn candidate(id: &str, readable: bool) -> RecoveryCandidate {
        RecoveryCandidate {
            id: id.to_string(),
            volume_index: 0,
            path: format!("/{id}.txt"),
            node: FsNode {
                id: format!("fat:0:{id}"),
                name: format!("{id}.txt"),
                parent_id: Some("part:0".to_string()),
                kind: NodeKind::File,
                size: 5,
                deleted: true,
                inode: None,
                timestamps: Mactb::default(),
                link_target: None,
                has_snapshot: false,
            },
            readable,
            initialized_size: None,
            source_path: None,
            source_node: None,
        }
    }

    #[test]
    fn file_selection_requires_a_finished_phase_and_readable_candidates() {
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
            "整个镜像".into(),
            1,
        )
        .with_filesystem_recovery(options, true);
        let ids = |values: &[&str]| {
            values
                .iter()
                .map(|value| value.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            filesystem_result(&session, &ids(&["0"])).unwrap_err().code,
            crate::app::error::ErrorCode::NotFound
        );
        session.filesystem.as_mut().unwrap().result = Some(Arc::new(RecoveryScan {
            scan_id: "scan".into(),
            image_id: "image".into(),
            options,
            ranges: Vec::new(),
            volumes: Vec::new(),
            candidates: vec![candidate("0", true), candidate("1", false)],
            directories: Vec::new(),
            truncated: false,
            warnings: Vec::new(),
            read_faults: ReadFaultReport::default(),
        }));
        assert!(filesystem_result(&session, &ids(&["0", "0"])).is_ok());
        assert!(filesystem_result(&session, &ids(&["0", "1"])).is_err());
        assert!(filesystem_result(&session, &ids(&["0", "9"])).is_err());
    }

    fn region(offset: u64, length: u64) -> CarveRegion {
        CarveRegion {
            offset,
            length,
            label: String::new(),
        }
    }

    #[test]
    fn sector_ranges_keep_whole_sectors_and_drop_slivers() {
        assert_eq!(
            sector_ranges(
                &[region(0, 4096), region(5000, 2000), region(8000, 100)],
                512
            ),
            vec![
                SectorRange {
                    start_sector: 0,
                    sector_count: 8,
                },
                SectorRange {
                    start_sector: 10,
                    sector_count: 3,
                },
            ]
        );
    }
}
