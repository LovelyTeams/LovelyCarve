//! Filesystem-tree IPC: directory browsing and snapshot-index assisted
//! child queries across every detected filesystem.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use crate::host::State;
use serde::Serialize;

use crate::app::workflows::filesystem::prepare_remote_partition;
use crate::commands::filesystem::filesystem_page::{
    fs_node_page_from_filedb, fs_node_page_from_sorted_nodes, normalize_fs_node_page_size,
    FsNodePage,
};
use crate::commands::filesystem::filesystem_snapshot_index::{
    case_index_targets, index_targets, query_snapshot_index_child_counts,
    query_snapshot_index_children, query_snapshot_index_subtree_empties, IndexCoverage,
    SnapshotIndexTarget,
};
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::fs_dispatch::Volume as _;
use crate::core::filesystem::tree::fs_tree::{self, FsNode};
use crate::core::{catalog::filedb::filedb_file, catalog::filedb::filedb_query};
use crate::state::AppState;
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

pub use crate::commands::filesystem::filesystem_journal::{
    fs_ext4_journal, fs_ext4_journal_blocks_page, fs_usn_journal, fs_usn_journal_page,
    Ext4JournalBlocksPage, UsnJournalPage,
};

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_children(
    state: State<'_, AppState>,
    image_id: String,
    parent_id: String,
) -> Result<Vec<FsNode>, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    prepare_remote_partition(&state, &image, &parent_id).await?;
    // Volume index fast path: the case's published indexes, or the caseless
    // session indexes.
    let targets = index_targets(&state, &image, &image_id, IndexCoverage::Any).await;
    if !targets.is_empty() {
        let parent_id_for_index = parent_id.clone();
        let indexed = tokio::task::spawn_blocking(move || {
            query_snapshot_index_children(&targets, &parent_id_for_index)
        })
        .await;
        match indexed {
            Ok(Ok(Some(nodes))) => {
                tracing::debug!(
                    "fs_children index-hit image={image_id} parent={parent_id} nodes={}",
                    nodes.len()
                );
                return Ok(nodes);
            }
            Ok(Ok(None)) => {}
            Ok(Err(err)) => {
                tracing::warn!("fs_children index err image={image_id} parent={parent_id}: {err}");
            }
            Err(join_err) => {
                tracing::warn!(
                    "fs_children index task crashed image={image_id} parent={parent_id}: {join_err}"
                );
            }
        }
    }
    // Fast path: serve directly from the active case's volume snapshot when
    // one covers this parent. Snapshot bodies are only ever installed after a
    // freshness check, so a hit is byte-for-byte what a live listing returns —
    // without re-walking the filesystem. A miss (parent not in the snapshot,
    // e.g. beyond a truncation cap or no case open) falls through to live.
    {
        let active = state.active_case.lock().await;
        if let Some(case) = active.as_ref() {
            if let Some(snapshot) = case.covering_snapshot(&image_id, &parent_id) {
                if let Some(nodes) = snapshot.children(&parent_id) {
                    tracing::debug!(
                        "fs_children snapshot-hit image={image_id} parent={parent_id} nodes={}",
                        nodes.len()
                    );
                    return Ok(nodes);
                }
            }
        }
    }
    // Caseless fast path: serve from the in-memory session snapshot built on
    // image load when no case is open (or the case didn't cover this parent).
    {
        let sessions = state.session_snapshots.lock().await;
        if let Some(snap) =
            crate::app::workflows::snapshot::snapshot_session::covering_session_snapshot(
                &sessions, &image_id, &parent_id,
            )
        {
            if let Some(nodes) = snap
                .body
                .as_ref()
                .and_then(|body| body.children(&parent_id))
            {
                tracing::debug!(
                    "fs_children session-hit image={image_id} parent={parent_id} nodes={}",
                    nodes.len()
                );
                return Ok(nodes);
            }
        }
    }
    let parent_for_log = parent_id.clone();
    let started = std::time::Instant::now();
    tracing::debug!("fs_children begin image={image_id} parent={parent_for_log}");
    // children_for_image can issue real disk I/O the first time a partition
    // is opened (volume headers, MFT bootstrap, ext4 superblock). Keep that
    // off the tokio worker so other IPCs stay responsive — and so a slow
    // physical-disk read doesn't make the whole UI feel unresponsive.
    let result = tokio::task::spawn_blocking(move || {
        fs_tree::children_for_image(&image, &parent_id).map_err(AppError::from_anyhow)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_children", err))?;
    match &result {
        Ok(nodes) => tracing::debug!(
            "fs_children done image={image_id} parent={parent_for_log} elapsed={:.3?} nodes={}",
            started.elapsed(),
            nodes.len()
        ),
        Err(err) => tracing::warn!(
            "fs_children err image={image_id} parent={parent_for_log} elapsed={:.3?} err={}",
            started.elapsed(),
            err.message
        ),
    }
    result
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_children_page(
    state: State<'_, AppState>,
    image_id: String,
    parent_id: String,
    cursor: Option<String>,
    page_size: Option<usize>,
    query: Option<crate::core::catalog::directory_query::DirectoryQuery>,
) -> Result<FsNodePage, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    prepare_remote_partition(&state, &image, &parent_id).await?;
    if let Some(query) = query {
        return crate::commands::filesystem::filesystem_directory::page(
            state, image_id, parent_id, query, cursor, page_size,
        )
        .await
        .map(fs_node_page_from_filedb);
    }
    // Volume index fast path. This avoids cloning and sorting a whole
    // in-memory directory vector just to return one page. A miss falls through
    // to the in-memory/live paths.
    let targets = index_targets(&state, &image, &image_id, IndexCoverage::Any).await;
    if !targets.is_empty() {
        let parent_id_for_index = parent_id.clone();
        let cursor_for_index = cursor.clone();
        let indexed = tokio::task::spawn_blocking(move || {
            for target in targets {
                if let Some(page) = filedb_query::children_page(
                    &target.index,
                    &parent_id_for_index,
                    cursor_for_index.as_deref(),
                    page_size,
                )? {
                    return Ok(Some(page));
                }
            }
            anyhow::Ok(None)
        })
        .await;
        match indexed {
            Ok(Ok(Some(page))) => {
                tracing::debug!(
                    "fs_children_page index-hit image={image_id} parent={parent_id} items={} next_cursor={}",
                    page.items.len(),
                    page.next_cursor.is_some(),
                );
                return Ok(fs_node_page_from_filedb(page));
            }
            Ok(Ok(None)) => {}
            Ok(Err(err)) => {
                tracing::warn!(
                    "fs_children_page index err image={image_id} parent={parent_id}: {err}"
                );
            }
            Err(join_err) => {
                tracing::warn!(
                    "fs_children_page index task crashed image={image_id} parent={parent_id}: {join_err}"
                );
            }
        }
    }
    // Keep the same snapshot-first behaviour as `fs_children`: the paged
    // command should not re-open the filesystem when a fresh case/session
    // snapshot already covers this parent.
    {
        let active = state.active_case.lock().await;
        if let Some(case) = active.as_ref() {
            if let Some(snapshot) = case.covering_snapshot(&image_id, &parent_id) {
                if let Some(nodes) = snapshot.children(&parent_id) {
                    tracing::debug!(
                        "fs_children_page snapshot-hit image={image_id} parent={parent_id} nodes={}",
                        nodes.len()
                    );
                    return fs_node_page_from_sorted_nodes(nodes, cursor.as_deref(), page_size);
                }
            }
        }
    }
    {
        let sessions = state.session_snapshots.lock().await;
        if let Some(snap) =
            crate::app::workflows::snapshot::snapshot_session::covering_session_snapshot(
                &sessions, &image_id, &parent_id,
            )
        {
            if let Some(nodes) = snap
                .body
                .as_ref()
                .and_then(|body| body.children(&parent_id))
            {
                tracing::debug!(
                    "fs_children_page session-hit image={image_id} parent={parent_id} nodes={}",
                    nodes.len()
                );
                return fs_node_page_from_sorted_nodes(nodes, cursor.as_deref(), page_size);
            }
        }
    }

    let parent_for_log = parent_id.clone();
    let started = Instant::now();
    let normalized_page_size = normalize_fs_node_page_size(page_size);
    tracing::debug!("fs_children_page begin image={image_id} parent={parent_for_log}");
    let result = tokio::task::spawn_blocking(move || {
        if let Some(volume) = image.external_volume(&parent_id) {
            let options = fs_tree::ChildrenOptions {
                include_deleted_ntfs: true,
                include_unallocated_nodes: true,
            };
            if let Some(page) = volume
                .children_page(
                    &image,
                    &parent_id,
                    options,
                    cursor.as_deref(),
                    normalized_page_size,
                )
                .map_err(AppError::from_anyhow)?
            {
                return Ok(FsNodePage {
                    items: page.items,
                    next_cursor: page.next_cursor,
                    total: page.total,
                    truncated: false,
                });
            }
        }
        let nodes =
            fs_tree::children_for_image(&image, &parent_id).map_err(AppError::from_anyhow)?;
        fs_node_page_from_sorted_nodes(nodes, cursor.as_deref(), page_size)
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_children_page", err))?;
    match &result {
        Ok(page) => tracing::debug!(
            "fs_children_page done image={image_id} parent={parent_for_log} elapsed={:.3?} items={} next_cursor={}",
            started.elapsed(),
            page.items.len(),
            page.next_cursor.is_some(),
        ),
        Err(err) => tracing::warn!(
            "fs_children_page err image={image_id} parent={parent_for_log} elapsed={:.3?} err={}",
            started.elapsed(),
            err.message
        ),
    }
    result
}

/// Batch directory child-counts served from the active case's volume snapshot.
///
/// For each `parent_id` covered by an installed snapshot, returns the exact
/// number of children the snapshot recorded (a `by_parent` length lookup, no
/// node cloning, no filesystem I/O). Parents with no covering snapshot — no
/// case open, still building, or beyond a truncation cap — are simply omitted
/// from the map, so the frontend can fall back to a bounded live probe only
/// for those. This collapses the per-directory count fan-out (one `fs_children`
/// per subdir) into a single call whenever a snapshot exists.
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_child_counts(
    state: State<'_, AppState>,
    image_id: String,
    parent_ids: Vec<String>,
) -> Result<HashMap<String, u64>, AppError> {
    let image = {
        let images = state.images.lock().await;
        images.get(&image_id).cloned()
    };
    let mut counts = HashMap::with_capacity(parent_ids.len());
    // In-memory case bodies first (volumes whose index could not be built).
    let mut missing_parent_ids = Vec::with_capacity(parent_ids.len());
    let has_case = {
        let active = state.active_case.lock().await;
        match active.as_ref() {
            Some(case) => {
                for parent_id in parent_ids {
                    match case.snapshot_child_count(&image_id, &parent_id) {
                        Some(count) => {
                            counts.insert(parent_id, count as u64);
                        }
                        None => missing_parent_ids.push(parent_id),
                    }
                }
                true
            }
            None => {
                missing_parent_ids = parent_ids;
                false
            }
        }
    };
    // Then the volume indexes (case or caseless session).
    if let Some(image) = image.as_deref() {
        let targets = index_targets(&state, image, &image_id, IndexCoverage::Any).await;
        if !targets.is_empty() && !missing_parent_ids.is_empty() {
            let parent_ids_for_index = missing_parent_ids.clone();
            let indexed_counts = tokio::task::spawn_blocking(move || {
                query_snapshot_index_child_counts(&targets, &parent_ids_for_index)
            })
            .await
            .map_err(|err| AppError::task_panicked("fs_child_counts index", err))?;
            match indexed_counts {
                Ok(indexed_counts) => {
                    counts.extend(indexed_counts);
                    missing_parent_ids.retain(|parent_id| !counts.contains_key(parent_id));
                }
                Err(err) => {
                    tracing::warn!("fs_child_counts index err image={image_id}: {err}");
                }
            }
        }
    }
    // Caseless in-memory session bodies last.
    if !has_case {
        let sessions = state.session_snapshots.lock().await;
        for parent_id in missing_parent_ids {
            if let Some(count) =
                crate::app::workflows::snapshot::snapshot_session::session_child_count(
                    &sessions, &image_id, &parent_id,
                )
            {
                counts.insert(parent_id, count as u64);
            }
        }
    }
    Ok(counts)
}

/// Batch "is this directory's subtree empty of files?" served from the active
/// case's volume snapshot (or the caseless session snapshot).
///
/// For each `parent_id` the snapshot can decide, returns whether its subtree
/// holds nothing but (recursively) empty directories: `true` = empty of files
/// (the directory tree's "hide empty directories" filter removes it), `false` =
/// a real file exists somewhere beneath it. Parents the snapshot can't decide —
/// no case open, still building, or beyond a truncation cap — are omitted, so
/// the frontend falls back to its direct child-count signal for those. Reads
/// snapshot indices only (no filesystem I/O); see
/// [`crate::core::catalog::snapshot::volume_snapshot::VolumeSnapshot::subtree_has_file`].
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_subtree_empty_dirs(
    state: State<'_, AppState>,
    image_id: String,
    parent_ids: Vec<String>,
) -> Result<HashMap<String, bool>, AppError> {
    let image = {
        let images = state.images.lock().await;
        images.get(&image_id).cloned()
    };
    let mut empties = HashMap::with_capacity(parent_ids.len());
    // In-memory case bodies first (volumes whose index could not be built).
    let mut missing_parent_ids = Vec::with_capacity(parent_ids.len());
    let has_case = {
        let active = state.active_case.lock().await;
        match active.as_ref() {
            Some(case) => {
                for parent_id in parent_ids {
                    match case.snapshot_subtree_empty(&image_id, &parent_id) {
                        Some(empty) => {
                            empties.insert(parent_id, empty);
                        }
                        None => missing_parent_ids.push(parent_id),
                    }
                }
                true
            }
            None => {
                missing_parent_ids = parent_ids;
                false
            }
        }
    };
    // Then the volume indexes (case or caseless session).
    if let Some(image) = image.as_deref() {
        let targets = index_targets(&state, image, &image_id, IndexCoverage::Any).await;
        if !targets.is_empty() && !missing_parent_ids.is_empty() {
            let parent_ids_for_index = missing_parent_ids.clone();
            let indexed_empties = tokio::task::spawn_blocking(move || {
                query_snapshot_index_subtree_empties(&targets, &parent_ids_for_index)
            })
            .await
            .map_err(|err| AppError::task_panicked("fs_subtree_empty_dirs index", err))?;
            match indexed_empties {
                Ok(indexed_empties) => {
                    empties.extend(indexed_empties);
                    missing_parent_ids.retain(|parent_id| !empties.contains_key(parent_id));
                }
                Err(err) => {
                    tracing::warn!("fs_subtree_empty_dirs index err image={image_id}: {err}");
                }
            }
        }
    }
    // Caseless in-memory session bodies last.
    if !has_case {
        let sessions = state.session_snapshots.lock().await;
        for parent_id in missing_parent_ids {
            if let Some(empty) =
                crate::app::workflows::snapshot::snapshot_session::session_subtree_empty(
                    &sessions, &image_id, &parent_id,
                )
            {
                empties.insert(parent_id, empty);
            }
        }
    }
    Ok(empties)
}

// ── 节点定位（全局搜索跳转 / jump2Page 支撑）─────────────────────────

/// 祖先帧：根→目标所在目录链上的一级（与文件列表 dirStack 帧同构）。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FsLocateNode {
    pub id: String,
    pub name: String,
    pub kind: String,
}

/// `fs_node_locate` 的返回：`frames` 是根相对祖先帧链（不含目标本身），
/// `file_id` 是目标文件节点（目标为目录时为 None——frames 末位即目标）。
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct FsLocateResult {
    pub node_id: String,
    pub volume_key: String,
    pub frames: Vec<FsLocateNode>,
    pub file_id: Option<String>,
}

/// 把一个节点解析为可导航的祖先帧链，用于跳回文件页定位目录树。
/// 文件走所在卷索引的 pid 链；分区/未分配/系统区/
/// 磁盘间隙虚拟节点按 id 前缀合成。解析不到（索引缺行）时报错，前端回退
/// 为提示而不跳转。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_node_locate(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
) -> Result<FsLocateResult, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let targets = {
        let active = state.active_case.lock().await;
        let case = active
            .as_ref()
            .ok_or_else(|| AppError::invalid_input("定位节点需要先打开案件"))?;
        case_index_targets(case, &image, &image_id, IndexCoverage::Any)
    };
    let image_for_locate = Arc::clone(&image);
    let locate_node_id = node_id.clone();
    tokio::task::spawn_blocking(move || locate_node(&image_for_locate, &targets, &locate_node_id))
        .await
        .map_err(|err| AppError::task_panicked("fs_node_locate", err))?
        .map_err(|err| AppError::not_found(format!("无法定位节点 {node_id}：{err:#}")))
}

/// 阻塞侧定位实现。
fn locate_node(
    image: &Arc<EvidenceSession>,
    targets: &[SnapshotIndexTarget],
    node_id: &str,
) -> anyhow::Result<FsLocateResult> {
    let root_frame = || FsLocateNode {
        id: "root".to_string(),
        name: "/".to_string(),
        kind: "dir".to_string(),
    };
    let partition_frame = |partition_id: u8| -> Option<FsLocateNode> {
        let partition = image
            .partitions
            .iter()
            .find(|partition| partition.id == partition_id)?;
        Some(FsLocateNode {
            id: format!("part:{partition_id}"),
            name: fs_tree::partition_node(image, partition).name,
            kind: "dir".to_string(),
        })
    };

    // 虚拟节点族：id 前缀直接给出父级。
    if let Some(rest) = node_id
        .strip_prefix("unalloc:part:")
        .and_then(|rest| rest.split(':').next())
    {
        if let Ok(partition_id) = rest.parse::<u8>() {
            let mut frames = vec![root_frame()];
            if let Some(frame) = partition_frame(partition_id) {
                frames.push(frame);
            }
            return Ok(FsLocateResult {
                node_id: node_id.to_string(),
                volume_key: format!("part:{partition_id}"),
                frames,
                file_id: Some(node_id.to_string()),
            });
        }
    }
    if let Some(rest) = node_id
        .strip_prefix("sysfile:part:")
        .and_then(|rest| rest.split(':').next())
    {
        if let Ok(partition_id) = rest.parse::<u8>() {
            let mut frames = vec![root_frame()];
            if let Some(frame) = partition_frame(partition_id) {
                frames.push(frame);
            }
            return Ok(FsLocateResult {
                node_id: node_id.to_string(),
                volume_key: format!("part:{partition_id}"),
                frames,
                file_id: Some(node_id.to_string()),
            });
        }
    }
    if node_id.starts_with("diskgap:") {
        return Ok(FsLocateResult {
            node_id: node_id.to_string(),
            volume_key: String::new(),
            frames: vec![root_frame()],
            file_id: Some(node_id.to_string()),
        });
    }
    if let Some(rest) = node_id
        .strip_prefix("part:")
        .or_else(|| node_id.strip_prefix("unsupported:"))
        .and_then(|rest| rest.split(':').next())
    {
        if let Ok(partition_id) = rest.parse::<u8>() {
            let mut frames = vec![root_frame()];
            if let Some(frame) = partition_frame(partition_id) {
                frames.push(frame);
            }
            return Ok(FsLocateResult {
                node_id: node_id.to_string(),
                volume_key: format!("part:{partition_id}"),
                frames,
                file_id: None,
            });
        }
    }

    // 索引 pid 链：在目标所在卷的索引里，目标行 → 父行 → … 直到链顶（父为
    // 分区/根节点，不在索引中）。
    let mut located = None;
    for target in targets {
        let conn = filedb_file::open_reader(&target.index)?;
        if let Some(row) = filedb_query::node_row(&conn, node_id)? {
            located = Some((target, conn, row));
            break;
        }
    }
    let Some((target, conn, row)) = located else {
        anyhow::bail!("节点不在文件索引中");
    };
    let volume_key = target.volume_key.clone();
    let is_dir = row.kind == "dir";
    let mut top_parent = row.pid.clone();
    let mut chain = vec![FsLocateNode {
        id: row.nid,
        name: row.name,
        kind: row.kind,
    }];
    for _ in 0..64 {
        let Some(parent) = filedb_query::node_row(&conn, &top_parent)? else {
            break;
        };
        chain.push(FsLocateNode {
            id: parent.nid,
            name: parent.name,
            kind: parent.kind,
        });
        top_parent = parent.pid;
    }
    // 目标为目录：目标帧留在链上（file_id=None）；目标为文件：链去尾。
    let mut frames = vec![root_frame()];
    // 链顶父节点是分区帧（part:N / unsupported:N）时补上；'root'/空则不补。
    if let Some(partition_id) = top_parent
        .strip_prefix("part:")
        .or_else(|| top_parent.strip_prefix("unsupported:"))
        .and_then(|id| id.parse::<u8>().ok())
    {
        if let Some(frame) = partition_frame(partition_id) {
            frames.push(frame);
        }
    }
    chain.reverse();
    if is_dir {
        frames.extend(chain);
        return Ok(FsLocateResult {
            node_id: node_id.to_string(),
            volume_key,
            frames,
            file_id: None,
        });
    }
    let file_frame = chain.pop().unwrap_or_else(|| FsLocateNode {
        id: node_id.to_string(),
        name: String::new(),
        kind: "file".to_string(),
    });
    frames.extend(chain);
    Ok(FsLocateResult {
        node_id: node_id.to_string(),
        volume_key,
        frames,
        file_id: Some(file_frame.id),
    })
}

#[cfg(test)]
#[path = "filesystem_location_tests.rs"]
mod location_tests;
