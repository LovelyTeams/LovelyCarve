//! Current-image carving history. Sessions are retained in memory by `carve_start`.

use std::{collections::HashMap, sync::Arc};

use crate::host::State;
use parking_lot::RwLock;

use crate::{
    commands::app_error::AppError,
    core::carving::carve_session::{CarveScanSession, CarveScanSnapshot},
    state::AppState,
};

type Sessions = HashMap<String, Arc<RwLock<CarveScanSession>>>;

fn snapshots_for_image(sessions: &Sessions, image_id: &str) -> Vec<CarveScanSnapshot> {
    let mut snapshots: Vec<_> = sessions
        .values()
        .filter_map(|session| {
            let scan = session.read();
            (scan.image_id == image_id).then(|| scan.snapshot())
        })
        .collect();
    snapshots.sort_by(|left, right| {
        right
            .created_unix_ms
            .cmp(&left.created_unix_ms)
            .then_with(|| left.scan_id.cmp(&right.scan_id))
    });
    snapshots
}

fn remove_finished_scan(
    sessions: &mut Sessions,
    image_id: &str,
    scan_id: &str,
) -> Result<(), AppError> {
    {
        let session = sessions
            .get(scan_id)
            .ok_or_else(|| AppError::not_found("雕刻记录不存在"))?;
        let scan = session.read();
        if scan.image_id != image_id {
            return Err(AppError::not_found("雕刻记录不属于当前检材"));
        }
        if !scan.status.is_finished() {
            return Err(AppError::invalid_input("请先停止扫描，再删除记录"));
        }
    }
    sessions.remove(scan_id);
    Ok(())
}

/// Lists retained scans for this image; does not include other evidence or emit events.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn carve_scan_list(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<Vec<CarveScanSnapshot>, AppError> {
    Ok(snapshots_for_image(
        &*state.carve_scans.lock().await,
        &image_id,
    ))
}

/// Removes a finished session and its in-memory results, leaving recovered files untouched.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn carve_scan_remove(
    state: State<'_, AppState>,
    image_id: String,
    scan_id: String,
) -> Result<(), AppError> {
    remove_finished_scan(&mut *state.carve_scans.lock().await, &image_id, &scan_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::carving::carve_session::CarveSessionStatus;

    fn session(
        id: &str,
        image: &str,
        time: i64,
        status: CarveSessionStatus,
    ) -> (String, Arc<RwLock<CarveScanSession>>) {
        let mut session = CarveScanSession::new(
            id.into(),
            format!("task-{id}"),
            image.into(),
            "整个镜像".into(),
            1024,
        );
        session.created_unix_ms = time;
        session.status = status;
        (id.into(), Arc::new(RwLock::new(session)))
    }

    #[test]
    fn carve_history_lists_only_current_image_newest_first() {
        let sessions = HashMap::from([
            session("old", "a", 1, CarveSessionStatus::Completed),
            session("new", "a", 3, CarveSessionStatus::Running),
            session("other", "b", 4, CarveSessionStatus::Completed),
        ]);
        assert_eq!(
            snapshots_for_image(&sessions, "a")
                .into_iter()
                .map(|scan| (scan.scan_id, scan.status))
                .collect::<Vec<_>>(),
            vec![
                ("new".into(), CarveSessionStatus::Running),
                ("old".into(), CarveSessionStatus::Completed)
            ]
        );
    }

    #[test]
    fn carve_history_removal_rejects_active_and_other_evidence() {
        for status in [
            CarveSessionStatus::Queued,
            CarveSessionStatus::Running,
            CarveSessionStatus::Paused,
        ] {
            let mut sessions = HashMap::from([session("scan", "a", 1, status)]);
            assert!(remove_finished_scan(&mut sessions, "a", "scan").is_err());
            assert_eq!(sessions.len(), 1);
        }
        for status in [
            CarveSessionStatus::Completed,
            CarveSessionStatus::Cancelled,
            CarveSessionStatus::Failed,
        ] {
            let mut sessions = HashMap::from([session("scan", "a", 1, status)]);
            assert!(remove_finished_scan(&mut sessions, "b", "scan").is_err());
            assert_eq!(sessions.len(), 1);
            assert!(remove_finished_scan(&mut sessions, "a", "scan").is_ok());
            assert!(sessions.is_empty());
            assert!(remove_finished_scan(&mut sessions, "a", "scan").is_err());
        }
    }
}
