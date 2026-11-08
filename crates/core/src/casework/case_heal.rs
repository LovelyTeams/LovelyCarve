//! 打开案件时的自愈(幂等,干净的案件是空操作):
//!
//! - 清空 `staging/`(上次崩溃留下的半成品);
//! - 删除不属于任何检材的检材目录;
//! - 快照文件缺失的卷记录删除;索引文件缺失的卷记录下线索引;
//! - 回收卷目录里记录之外的旧代文件,删除没有记录的卷目录。
//!
//! 每一步失败都只记日志:自愈不能让案件打不开,残留留到下次打开再清。

use std::collections::{HashMap, HashSet};

use super::{remove_file_quietly, CaseState};
use crate::casework::case_ledger;
use crate::casework::case_ledger_volumes::{self as ledger_volumes, VolumeRecord};
use crate::casework::case_paths;

pub(super) fn heal_case(case: &mut CaseState) {
    let staging = case_paths::staging_dir(&case.dir);
    if let Err(error) = case_paths::remove_dir_if_exists(&staging) {
        tracing::warn!("case: heal: clear staging {}: {error}", staging.display());
    }
    if let Err(error) = std::fs::create_dir_all(&staging) {
        tracing::warn!("case: heal: create staging {}: {error}", staging.display());
    }
    remove_orphan_evidence_dirs(case);
    repair_volume_records(case);
    collect_retired_volume_files(case);
}

fn remove_orphan_evidence_dirs(case: &CaseState) {
    let live: HashSet<String> = case
        .evidence
        .iter()
        .map(|record| case_paths::path_segment(&record.uid, "evidence"))
        .collect();
    let Ok(entries) = std::fs::read_dir(case_paths::evidence_root(&case.dir)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !live.contains(&name) {
            tracing::info!("case: heal: removing orphan evidence directory {name}");
            if let Err(error) = case_paths::remove_dir_if_exists(&entry.path()) {
                tracing::warn!("case: heal: remove {}: {error}", entry.path().display());
            }
        }
    }
}

fn repair_volume_records(case: &mut CaseState) {
    let mut missing_snapshots = Vec::new();
    let mut missing_indexes = Vec::new();
    for record in &case.volumes {
        if !case.snapshot_path(record).is_file() {
            missing_snapshots.push((record.image_id.clone(), record.volume_key.clone()));
        } else if case
            .index_path(record)
            .is_some_and(|index_path| !index_path.is_file())
        {
            missing_indexes.push((record.image_id.clone(), record.volume_key.clone()));
        }
    }
    if missing_snapshots.is_empty() && missing_indexes.is_empty() {
        return;
    }
    let repaired = case_ledger::write_transaction(&case.ledger).and_then(|tx| {
        for (image_id, volume_key) in &missing_snapshots {
            tracing::warn!("case: heal: snapshot file missing for {image_id}/{volume_key}");
            if let Err(error) = ledger_volumes::delete(&tx, image_id, volume_key) {
                tracing::warn!("case: heal: drop volume {image_id}/{volume_key}: {error:#}");
            }
        }
        for (image_id, volume_key) in &missing_indexes {
            tracing::warn!("case: heal: index file missing for {image_id}/{volume_key}");
            if let Err(error) = ledger_volumes::clear_index(&tx, image_id, volume_key) {
                tracing::warn!("case: heal: clear index {image_id}/{volume_key}: {error:#}");
            }
        }
        tx.commit()
    });
    if let Err(error) = repaired {
        tracing::warn!("case: heal: repair volume records: {error}");
    }
    match ledger_volumes::list(&case.ledger) {
        Ok(volumes) => case.volumes = volumes,
        Err(error) => tracing::warn!("case: heal: reload volume records: {error:#}"),
    }
}

fn collect_retired_volume_files(case: &CaseState) {
    for record in &case.evidence {
        let volumes_dir = case_paths::volumes_dir(&case.dir, &record.uid);
        let Ok(entries) = std::fs::read_dir(&volumes_dir) else {
            continue;
        };
        let owned: HashMap<String, &VolumeRecord> = case
            .volumes
            .iter()
            .filter(|volume| volume.image_id == record.uid)
            .map(|volume| (case_paths::volume_segment(&volume.volume_key), volume))
            .collect();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(volume) = owned.get(&name) else {
                if let Err(error) = case_paths::remove_dir_if_exists(&entry.path()) {
                    tracing::warn!("case: heal: remove {}: {error}", entry.path().display());
                }
                continue;
            };
            let referenced: HashSet<&str> = std::iter::once(volume.snapshot_file.as_str())
                .chain(volume.index_file.as_deref())
                .collect();
            let Ok(files) = std::fs::read_dir(entry.path()) else {
                continue;
            };
            for file in files.flatten() {
                let file_name = file.file_name().to_string_lossy().into_owned();
                let owner = file_name
                    .strip_suffix("-wal")
                    .or_else(|| file_name.strip_suffix("-shm"))
                    .or_else(|| file_name.strip_suffix("-journal"))
                    .unwrap_or(&file_name);
                if !referenced.contains(owner) {
                    remove_file_quietly(&file.path(), "retired volume file");
                }
            }
        }
    }
}
