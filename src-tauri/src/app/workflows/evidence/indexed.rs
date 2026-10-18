//! Restore a case's published filesystem indexes when its source is missing.

use std::{path::PathBuf, sync::Arc};

use crate::app::error::AppError;
use crate::core::casework::{case::EvidenceRef, case_ledger_volumes::VolumeRecord};
use crate::core::catalog::filedb::filedb_file;
use crate::core::evidence::image::DiskImage;
use crate::core::filesystem::{
    evidence_session::EvidenceSession,
    tree::fs_tree::{FsNode, Mactb, NodeKind},
};
use crate::state::AppState;

pub(super) async fn open_indexed_evidence(
    state: &AppState,
    image_id: &str,
    path: &str,
) -> Result<Option<Arc<EvidenceSession>>, AppError> {
    let (case_dir, evidence, records) = {
        let active = state.active_case.lock().await;
        let Some(case) = active.as_ref() else {
            return Ok(None);
        };
        if case.evidence_image_id_for_path(path).as_deref() != Some(image_id) {
            return Ok(None);
        }
        let Some(evidence) = case.evidence_ref(image_id) else {
            return Ok(None);
        };
        let records = case
            .volumes()
            .iter()
            .filter(|record| record.image_id == image_id && record.index_is_current())
            .filter_map(|record| case.index_path(record).map(|index| (record.clone(), index)))
            .collect::<Vec<_>>();
        (case.dir.clone(), evidence, records)
    };
    let checked_records = records.clone();
    let session =
        tokio::task::spawn_blocking(move || restore_indexed_session(evidence, &checked_records))
            .await
            .map_err(|error| AppError::task_panicked("open offline indexes", error))??;
    let Some(session) = session else {
        return Ok(None);
    };
    // Index validation may outlive closing the case or replacing a publication.
    let active = state.active_case.lock().await;
    let valid = active.as_ref().is_some_and(|case| {
        case.dir == case_dir
            && case.evidence_image_id_for_path(path).as_deref() == Some(image_id)
            && records
                .iter()
                .all(|(record, _)| case.volume(image_id, &record.volume_key) == Some(record))
    });
    if !valid {
        return Err(AppError::not_found(
            "离线索引所属案件已关闭或索引已更新，请重新打开检材",
        ));
    }
    let session = Arc::new(session);
    state
        .images
        .lock()
        .await
        .insert(image_id.to_string(), Arc::clone(&session));
    Ok(Some(session))
}

fn restore_indexed_session(
    evidence: EvidenceRef,
    records: &[(VolumeRecord, PathBuf)],
) -> anyhow::Result<Option<EvidenceSession>> {
    let mut roots = Vec::new();
    for (record, index) in records {
        match verified_index(record, index) {
            Ok(()) => roots.push(FsNode {
                id: record.volume_key.clone(),
                name: record.volume_key.strip_prefix("part:").map_or_else(
                    || record.volume_key.clone(),
                    |number| format!("分区{number}"),
                ),
                parent_id: None,
                kind: NodeKind::Dir,
                size: record.source_size,
                deleted: false,
                inode: None,
                timestamps: Mactb::default(),
                link_target: None,
                has_snapshot: false,
            }),
            Err(error) => {
                tracing::warn!("offline index {} unavailable: {error:#}", index.display())
            }
        }
    }
    if roots.is_empty() {
        return Ok(None);
    }
    let image = DiskImage::from_index_metadata(
        evidence.image_id,
        PathBuf::from(evidence.path),
        evidence.source_format,
        evidence.size_bytes,
    )?;
    Ok(Some(EvidenceSession::from_indexed_image(image, roots)))
}

fn verified_index(record: &VolumeRecord, path: &std::path::Path) -> anyhow::Result<()> {
    let connection = filedb_file::open_reader(path)?;
    let meta = filedb_file::read_meta(&connection)?;
    anyhow::ensure!(
        record.index_is_current()
            && meta.image_id == record.image_id
            && meta.volume_key == record.volume_key
            && meta.vol_id == record.vol_id
            && meta.built_at == record.built_at
            && meta.fingerprint == record.fingerprint
            && meta.entry_count == record.entry_count
            && meta.truncated == record.truncated,
        "索引与案件中的当前卷记录不匹配"
    );
    Ok(())
}

#[cfg(test)]
#[path = "indexed_tests.rs"]
mod tests;
