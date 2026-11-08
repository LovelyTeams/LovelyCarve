//! `filedb` IPC: metadata queries and the MetaAddr
//! addressing channel.
//!
//! * `filedb_volumes` — the image's filesystem array (`fs_all`): the mapping
//!   between `vol_id` (array position) and the walker's volume keys.
//! * `filedb_query_files` — the generic filter path over the lowercase copy
//!   columns (name glob / name or path prefix or substring, extension, size range, kind,
//!   delete state), naturally ordered and `LIMIT/OFFSET` paged.
//! * `filedb_resolve_meta_addr` — the `QueryFileSt` channel: resolve
//!   `(image, volId, metaAddr, metaAddrHigh)` back to the node id the read
//!   commands accept (`fs_read*`, `fs_preview*`, …).

use std::sync::Arc;

use crate::host::State;
use serde::Serialize;

use crate::app::access::loaded_image_app_error;
use crate::app::error::AppError;
use crate::commands::filesystem::filesystem_snapshot_index::{
    index_targets, IndexCoverage, SnapshotIndexTarget,
};
use crate::core::catalog::filedb::filedb_file;
use crate::core::catalog::filedb::filedb_query::{self, FileFilter};
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::fs_all::{self, FsEntry};
use crate::core::filesystem::meta_addr::FileQuery;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FiledbVolume {
    pub vol_id: i64,
    pub volume_key: String,
    pub fs_prefix: String,
    pub kind: String,
    pub size: u64,
    pub indexes: Vec<FiledbIndexCoverage>,
}

/// Published index and the subtree containing this filesystem's entries.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FiledbIndexCoverage {
    pub volume_key: String,
    pub path_prefix: Option<String>,
    pub truncated: bool,
}

/// The image's filesystem array: `volId` is the index.
pub async fn filedb_volumes(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<Vec<FiledbVolume>, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    if image.is_index_only() {
        let active = state.active_case.lock().await;
        let case = active
            .as_ref()
            .ok_or_else(|| AppError::not_found("离线索引所属案件已关闭"))?;
        let targets = crate::commands::filesystem::filesystem_snapshot_index::case_index_targets(
            case,
            &image,
            &image_id,
            IndexCoverage::Any,
        );
        return Ok(targets
            .iter()
            .filter_map(|target| {
                case.volume(&image_id, &target.volume_key)
                    .map(|record| FiledbVolume {
                        vol_id: record.vol_id,
                        volume_key: record.volume_key.clone(),
                        fs_prefix: "indexed".to_string(),
                        kind: "indexed".to_string(),
                        size: record.source_size,
                        indexes: vec![FiledbIndexCoverage {
                            volume_key: record.volume_key.clone(),
                            path_prefix: None,
                            truncated: record.truncated,
                        }],
                    })
            })
            .collect());
    }
    let targets = index_targets(&state, &image, &image_id, IndexCoverage::Any).await;
    let volumes = tokio::task::spawn_blocking(move || {
        let mut volumes = fs_all_volumes(&image)?;
        add_index_coverage(&mut volumes, &targets, &image_id)?;
        anyhow::Ok(volumes)
    })
    .await
    .map_err(|err| AppError::task_panicked("filedb_volumes fs_all", err))??;
    Ok(volumes)
}

fn add_index_coverage(
    volumes: &mut [FiledbVolume],
    targets: &[SnapshotIndexTarget],
    image_id: &str,
) -> anyhow::Result<()> {
    for target in targets {
        let connection = filedb_file::open_reader(&target.index)?;
        let metadata = filedb_file::read_meta(&connection)?;
        anyhow::ensure!(
            metadata.image_id == image_id && metadata.volume_key == target.volume_key,
            "published index identity mismatch"
        );
        for volume in volumes.iter_mut() {
            let path_prefix = if volume.volume_key == target.volume_key {
                None
            } else if let Some(row) = filedb_query::node_row(&connection, &volume.volume_key)? {
                Some(format!("{}/", row.full_path.trim_end_matches('/')))
            } else {
                continue;
            };
            volume.indexes.push(FiledbIndexCoverage {
                volume_key: target.volume_key.clone(),
                path_prefix,
                truncated: metadata.truncated,
            });
        }
    }
    Ok(())
}

fn fs_all_volumes(image: &Arc<EvidenceSession>) -> anyhow::Result<Vec<FiledbVolume>> {
    Ok(fs_all::FsAll::for_image(image)
        .entries()
        .iter()
        .map(
            |FsEntry {
                 vol_id,
                 volume_key,
                 kind,
                 fs_prefix,
                 size,
                 ..
             }| FiledbVolume {
                vol_id: *vol_id,
                volume_key: volume_key.clone(),
                fs_prefix: (*fs_prefix).to_string(),
                kind: format!("{kind:?}").to_lowercase(),
                size: *size,
                indexes: Vec::new(),
            },
        )
        .collect())
}

/// Query the metadata index with structured filters. With no
/// `volume_key`, every indexed volume of the image is scanned and the results
/// concatenated (natural order within each volume).
pub async fn filedb_query_files(
    state: State<'_, AppState>,
    image_id: String,
    volume_key: Option<String>,
    filter: FileFilter,
) -> Result<Vec<filedb_query::FileHit>, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let filter = filter.normalized();
    let available = index_targets(&state, &image, &image_id, IndexCoverage::Any).await;
    let targets: Vec<_> = available
        .iter()
        .filter(|target| {
            volume_key
                .as_deref()
                .is_none_or(|wanted| wanted == target.volume_key)
        })
        .cloned()
        .collect();
    if targets.is_empty() {
        return Err(AppError::not_found("没有可查询的已发布索引；这不表示文件不存在。用 indexes volumes list 查看 indexes 中的实际索引键和 pathPrefix，再用 indexes list 核验状态")
            .with_context_value(serde_json::json!({"reason":"indexUnavailable", "imageId":image_id,
                "volumeKey":volume_key, "availableIndexKeys":available.iter().map(|target| &target.volume_key).collect::<Vec<_>>()})));
    }
    let hits = tokio::task::spawn_blocking(move || query_indexes(&image_id, &targets, &filter))
        .await
        .map_err(|err| AppError::task_panicked("filedb_query_files", err))??;
    Ok(hits)
}

fn query_indexes(
    image_id: &str,
    targets: &[SnapshotIndexTarget],
    filter: &FileFilter,
) -> Result<Vec<filedb_query::FileHit>, AppError> {
    let mut hits = Vec::new();
    for target in targets {
        let connection = filedb_file::open_reader(&target.index).map_err(AppError::from)?;
        let metadata = filedb_file::read_meta(&connection).map_err(AppError::from)?;
        if metadata.image_id != image_id || metadata.volume_key != target.volume_key {
            return Err(AppError::source_changed("索引来源身份与当前查询不一致"));
        }
        if metadata.truncated {
            return Err(AppError::resource_limit("索引覆盖不完整；请重新构建完整索引后查询，不能据此判断文件不存在")
                .with_context_value(serde_json::json!({"reason":"incompleteIndex", "imageId":image_id, "volumeKey":target.volume_key})));
        }
        hits.extend(filedb_query::query_files(&target.index, filter).map_err(AppError::from)?);
    }
    Ok(hits)
}

/// `QueryFileSt` channel: map a MetaAddr four-tuple back to the node id that
/// `fs_read*` / `fs_stat` accept. Path-addressed entries (`metaAddr = 0`)
/// require `full_path`.
pub async fn filedb_resolve_meta_addr(
    state: State<'_, AppState>,
    query: FileQuery,
) -> Result<Option<String>, AppError> {
    let image_id = query.image_id.clone();
    let image = {
        let images = state.images.lock().await;
        images.get(&image_id).cloned()
    };
    let targets = targets_for(&state, image.as_deref(), &image_id, None).await;
    let full_path = query.full_path.clone();
    let meta_addr = query.meta_addr;
    let meta_addr_high = query.meta_addr_high;
    let nid = tokio::task::spawn_blocking(move || -> anyhow::Result<Option<String>> {
        for target in targets
            .iter()
            .filter(|target| target.vol_id == query.vol_id)
        {
            if let Some(nid) = filedb_query::node_id_for_meta_addr(
                &target.index,
                meta_addr,
                meta_addr_high,
                full_path.as_deref(),
            )? {
                return Ok(Some(nid));
            }
        }
        Ok(None)
    })
    .await
    .map_err(|err| AppError::task_panicked("filedb_resolve_meta_addr", err))??;
    Ok(nid)
}

/// Index targets: the active case's published volume indexes, or the caseless
/// session indexes — same dual source the browsing commands use.
async fn targets_for(
    state: &AppState,
    image: Option<&EvidenceSession>,
    image_id: &str,
    volume_key: Option<&str>,
) -> Vec<SnapshotIndexTarget> {
    let Some(image) = image else {
        return Vec::new();
    };
    index_targets(state, image, image_id, IndexCoverage::Any)
        .await
        .into_iter()
        .filter(|target| volume_key.is_none_or(|wanted| wanted == target.volume_key))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::error::ErrorCode;
    use crate::core::catalog::filedb::filedb_store::{self, test_support};
    use crate::core::filesystem::tree::fs_tree::NodeKind;

    #[test]
    fn logical_volume_reports_parent_index_and_bounded_subtree() {
        let directory = tempfile::tempdir().unwrap();
        let entries = [
            test_support::entry("part:2", "lvm:2:1", "root", NodeKind::Dir, "/root"),
            test_support::entry(
                "lvm:2:1",
                "file-a",
                "a.conf",
                NodeKind::File,
                "/root/a.conf",
            ),
        ];
        let index = test_support::build_test_index(directory.path(), "part:2", &entries);
        let targets = [SnapshotIndexTarget {
            index,
            volume_key: "part:2".into(),
            vol_id: 2,
        }];
        let mut volumes = vec![FiledbVolume {
            vol_id: 4,
            volume_key: "lvm:2:1".into(),
            fs_prefix: "ext4".into(),
            kind: "ext4".into(),
            size: 1024,
            indexes: Vec::new(),
        }];
        add_index_coverage(&mut volumes, &targets, "img").unwrap();
        assert_eq!(
            serde_json::to_value(&volumes[0].indexes).unwrap(),
            serde_json::json!([
                {"volumeKey":"part:2", "pathPrefix":"/root/", "truncated":false}
            ])
        );
        let hits = query_indexes(
            "img",
            &targets,
            &FileFilter {
                name_glob: Some("*.conf".into()),
                path_prefix: Some("/root/".into()),
                limit: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            hits,
            vec![filedb_query::FileHit {
                nid: "file-a".into(),
                name: "a.conf".into(),
                full_path: "/root/a.conf".into(),
                kind: "file".into(),
                size: 123,
                is_delete: 2
            }]
        );
        assert_eq!(
            query_indexes("different-image", &targets, &FileFilter::default())
                .unwrap_err()
                .code,
            ErrorCode::SourceChanged
        );
    }

    #[test]
    fn truncated_index_is_not_a_complete_negative_query() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial.db");
        let source = filedb_store::IndexSource {
            truncated: true,
            ..test_support::source("part:0")
        };
        filedb_store::build_index_from_entries(&test_support::entries(), &path, &source).unwrap();
        let targets = [SnapshotIndexTarget {
            index: path,
            volume_key: "part:0".into(),
            vol_id: 0,
        }];
        let error = query_indexes(
            "img",
            &targets,
            &FileFilter {
                name_glob: Some("absent".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimit);
        assert_eq!(error.context.unwrap()["reason"], "incompleteIndex");
    }

    #[tokio::test]
    async fn missing_index_and_missing_image_are_errors_instead_of_empty_hits() {
        let directory = tempfile::tempdir().unwrap();
        let state = AppState::new();
        let image =
            Arc::new(EvidenceSession::open_with_id("img".into(), directory.path()).unwrap());
        state.images.lock().await.insert("img".into(), image);
        let error = filedb_query_files(
            State::new(&state),
            "img".into(),
            None,
            FileFilter::default(),
        )
        .await
        .unwrap_err();
        assert_eq!(error.context.unwrap()["reason"], "indexUnavailable");
        assert_eq!(
            filedb_query_files(
                State::new(&state),
                "missing".into(),
                None,
                FileFilter::default()
            )
            .await
            .unwrap_err()
            .code,
            ErrorCode::NotFound
        );
    }
}
