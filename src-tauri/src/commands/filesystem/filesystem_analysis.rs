//! Single-file analysis IPC commands.
//!
//! These commands read one filesystem node and derive metadata from its
//! contents: cryptographic digests and Magika type identification.
//!
//! Each read runs as a task under the caller's task id. The blocking worker
//! owns the registration, so the task stays registered and cancellable for
//! as long as the read runs, also when the command's caller went away (an
//! interrupted CLI client); the read checks the token between chunks.

use crate::core::common::ids::{ImageId, NodeId};
use std::sync::Arc;
use std::time::Instant;

use crate::host::State;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::analysis::identify::magika_ident;
use crate::core::common::error::Failure;
use crate::core::common::hash::{HashAlgo, MultiHasher};
use crate::core::evidence::image::ReadFaultReport;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree;
use crate::state::AppState;
use crate::task_manager::TaskKind;
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

/// Per-file digest result. Only the algorithms the caller requested are
/// `Some` — the file-list context menu computes hash types on demand.
#[derive(Debug, Serialize, Deserialize)]
pub struct FsDigest {
    pub md5: Option<String>,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
    pub sm3: Option<String>,
    pub bytes_hashed: u64,
    pub truncated: bool,
    #[serde(flatten)]
    pub read_faults: ReadFaultReport,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_digest(
    state: State<'_, AppState>,
    image_id: ImageId,
    node_id: NodeId,
    algos: Vec<HashAlgo>,
    task_id: String,
) -> Result<FsDigest, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let node_for_log = node_id.clone();
    let started = Instant::now();
    tracing::debug!(
        "fs_digest begin image={image_id} node={node_for_log} algos={algos:?} task={task_id}"
    );
    let task = state
        .tasks
        .register(task_id, TaskKind::FileHash, Some(image_id.clone()));
    let resource_permit = state.tasks.acquire_resources(&task).await?;

    let result = tokio::task::spawn_blocking(move || -> Result<FsDigest, AppError> {
        let _resource_permit = resource_permit;
        let digest = digest_file(&image, &node_id, algos, &task.token());
        task.finish_with(&digest);
        digest
    })
    .await
    .map_err(|err| AppError::task_panicked("fs_digest", err))?;
    match &result {
        Ok(digest) => tracing::debug!(
            "fs_digest done image={image_id} node={node_for_log} elapsed={:.3?} bytes={} md5={} sha1={} sha256={} sm3={}",
            started.elapsed(),
            digest.bytes_hashed,
            digest.md5.as_deref().unwrap_or("-"),
            digest.sha1.as_deref().unwrap_or("-"),
            digest.sha256.as_deref().unwrap_or("-"),
            digest.sm3.as_deref().unwrap_or("-"),
        ),
        Err(err) => tracing::warn!(
            "fs_digest err image={image_id} node={node_for_log} elapsed={:.3?} err={}",
            started.elapsed(),
            err.message,
        ),
    }
    result
}

/// Hashes one file, stopping at the next chunk once `cancel` fires.
fn digest_file(
    image: &Arc<EvidenceSession>,
    node_id: &str,
    algos: Vec<HashAlgo>,
    cancel: &CancellationToken,
) -> Result<FsDigest, AppError> {
    // Defensive default: an empty selection means "everything" rather
    // than a digest of nothing.
    let algos = if algos.is_empty() {
        vec![
            HashAlgo::Md5,
            HashAlgo::Sha1,
            HashAlgo::Sha256,
            HashAlgo::Sm3,
        ]
    } else {
        algos
    };
    let mut hasher = MultiHasher::new(&algos);
    let mut session = fs_tree::FsSession::new(image);
    let (result, read_faults) = image.collect_read_faults(|| {
        session.stream_file(node_id, 1024 * 1024, |chunk| {
            anyhow::ensure!(!cancel.is_cancelled(), Failure::cancelled("文件哈希已取消"));
            hasher.update(chunk);
            Ok(())
        })
    });
    let bytes_hashed = result.map_err(AppError::from)?;
    image
        .validate_source_file_stable()
        .map_err(AppError::from)?;

    let out = hasher.finalize();
    Ok(FsDigest {
        md5: out.md5,
        sha1: out.sha1,
        sha256: out.sha256,
        sm3: out.sm3,
        bytes_hashed,
        truncated: false,
        read_faults,
    })
}

/// AI 文件类型识别（Magika）。读盘 + 推理都在阻塞线程里做；模型只在首次
/// 调用时加载一次。
#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn fs_identify_type(
    state: State<'_, AppState>,
    image_id: ImageId,
    node_id: NodeId,
    task_id: String,
) -> Result<magika_ident::TypeIdentReport, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let node_for_log = node_id.clone();
    let started = Instant::now();
    let task = state
        .tasks
        .register(task_id, TaskKind::TypeIdentify, Some(image_id.clone()));
    let resource_permit = state.tasks.acquire_resources(&task).await?;
    let result =
        tokio::task::spawn_blocking(move || -> Result<magika_ident::TypeIdentReport, AppError> {
            let _resource_permit = resource_permit;
            let report = magika_ident::identify_file(&image, &node_id, &task.token())
                .map_err(AppError::from);
            task.finish_with(&report);
            report
        })
        .await
        .map_err(|err| AppError::task_panicked("fs_identify_type", err))?;
    match &result {
        Ok(report) => tracing::debug!(
            "fs_identify_type done image={image_id} node={node_for_log} elapsed={:.3?} label={} score={:.3}",
            started.elapsed(),
            report.label,
            report.score,
        ),
        Err(err) => tracing::warn!(
            "fs_identify_type err image={image_id} node={node_for_log} elapsed={:.3?} err={}",
            started.elapsed(),
            err.message,
        ),
    }
    result
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::app::error::ErrorCode;

    /// `hello.txt`, as a folder evidence node.
    const HELLO: &str = "folder:68656c6c6f2e747874";

    async fn folder_state(content: &[u8]) -> (tempfile::TempDir, AppState) {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("hello.txt"), content).unwrap();
        let state = AppState::new();
        let image = Arc::new(EvidenceSession::open_with_id("img", directory.path()).unwrap());
        state.images.lock().await.insert("img".into(), image);
        (directory, state)
    }

    #[tokio::test]
    async fn a_digest_runs_as_its_callers_task() {
        let (_directory, state) = folder_state(b"abc").await;

        let digest = fs_digest(
            State::new(&state),
            "img".into(),
            HELLO.into(),
            vec![HashAlgo::Sha256],
            "hash-1".into(),
        )
        .await
        .unwrap();

        assert_eq!(
            digest.sha256.as_deref(),
            Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
        assert!(state.tasks.live("hash-1").is_none());
    }

    #[test]
    fn a_cancelled_digest_stops_reading() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("hello.txt"), b"abc").unwrap();
        let image = Arc::new(EvidenceSession::open(directory.path()).unwrap());
        let cancel = CancellationToken::new();
        cancel.cancel();

        let error = digest_file(&image, HELLO, vec![HashAlgo::Sm3], &cancel).unwrap_err();

        assert_eq!(error.code, ErrorCode::Cancelled);
    }

    /// An interrupted CLI drops the command before it cancels anything: the
    /// worker still holds the task, so the cancellation reaches the read.
    #[tokio::test]
    async fn a_dropped_digest_stays_cancellable_until_its_read_stops() {
        let (_directory, state) = folder_state(&vec![0; 64 << 20]).await;
        let digest = fs_digest(
            State::new(&state),
            "img".into(),
            HELLO.into(),
            vec![HashAlgo::Sm3],
            "hash-1".into(),
        );
        let running = async {
            while state.tasks.live("hash-1").is_none() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        };
        tokio::select! {
            result = digest => panic!("the digest ended before it was dropped: {result:?}"),
            () = running => {}
        }

        assert!(state.tasks.cancel("hash-1"));
        assert_eq!(
            state
                .tasks
                .wait_image_stopped("img", Duration::from_secs(30))
                .await,
            Vec::<String>::new()
        );
    }
}
