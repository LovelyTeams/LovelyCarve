//! ID ancestry from current indexes, with live directory pages before indexing.
use std::{collections::HashSet, sync::Arc};

use super::{browsing, context::Exec, CliError};
use crate::{
    commands::filesystem::filesystem_snapshot_index::{index_targets, IndexCoverage},
    core::{
        catalog::filedb::{filedb_file, filedb_query},
        evidence::image::PartitionKind,
        filesystem::{
            evidence_session::EvidenceSession,
            volumes::apfs::{open_apfs_source, parse_apfs_id},
        },
    },
    task_manager::{TaskKind, TaskOutcome},
};

pub(super) async fn locate_node(
    exec: &Exec,
    image_id: &str,
    node_id: &str,
) -> Result<Vec<(String, String)>, CliError> {
    let task = exec.state.tasks.register(
        format!("cli-node-locate-{}", uuid::Uuid::new_v4()),
        TaskKind::FlatList,
        Some(image_id.to_owned()),
    );
    let cancellation = task.token();
    let located = async {
        let _permit = exec
            .state
            .tasks
            .acquire_resources(&task)
            .await
            .map_err(crate::app::error::AppError::from)
            .map_err(super::command_error)?;
        let image = exec
            .state
            .images
            .lock()
            .await
            .get(image_id)
            .cloned()
            .ok_or_else(|| CliError::invalid("检材已关闭"))?;
        let targets = index_targets(&exec.state, &image, image_id, IndexCoverage::Any).await;
        let target = node_id.to_owned();
        let indexed = tokio::task::spawn_blocking(move || {
            for index in targets {
                let connection = filedb_file::open_reader(&index.index)?;
                let mut parent = target.clone();
                let mut chain = Vec::new();
                let mut visited = HashSet::new();
                while let Some(row) = filedb_query::node_row(&connection, &parent)? {
                    anyhow::ensure!(visited.insert(row.nid.clone()), "节点索引父链存在循环");
                    anyhow::ensure!(chain.len() < 1024, "节点索引父链过深");
                    chain.push((row.name, row.nid));
                    parent = row.pid;
                }
                if !chain.is_empty() {
                    chain.reverse();
                    return Ok(Some((parent, chain)));
                }
            }
            anyhow::Ok(None)
        })
        .await
        .map_err(super::internal_error)?
        .map_err(super::internal_error)?;
        let mut page = |parent, cursor, limit| exec.page(image_id, parent, cursor, limit);
        match indexed {
            Some((parent, chain)) => {
                let mut location = browsing::locate_id(&parent, &cancellation, &mut page).await?;
                location.extend(chain);
                Ok(location)
            }
            None => browsing::locate_id(node_id, &cancellation, &mut page).await,
        }
    }
    .await;
    task.finish(match &located {
        Ok(_) => TaskOutcome::Succeeded,
        Err(_) if cancellation.is_cancelled() => TaskOutcome::Cancelled,
        Err(error) => TaskOutcome::Failed {
            error: error.message.clone(),
        },
    });
    located
}

/// Only known insensitive filesystems opt in. Synthetic roots and unknown or
/// offline Apple volumes retain exact matching; host OS rules never apply.
pub(super) async fn case_insensitive(
    image: Arc<EvidenceSession>,
    parent: String,
) -> Result<bool, CliError> {
    let session = image.filesystem_session(&parent);
    if parent.starts_with("ntfs:") || parent.starts_with("fat:") || parent.starts_with("exfat:") {
        return Ok(true);
    }
    if let Some((source, _)) = parse_apfs_id(&parent) {
        if session.is_index_only() {
            return Ok(false);
        }
        let image = Arc::clone(session.disk_image());
        return tokio::task::spawn_blocking(move || {
            open_apfs_source(&image, source)?.names_case_insensitive()
        })
        .await
        .map_err(super::internal_error)?
        .map_err(super::internal_error);
    }
    let parts = parent.split(':').collect::<Vec<_>>();
    let partition_id = if parts.first() == Some(&"part") {
        parts.get(1).and_then(|id| id.parse::<u8>().ok())
    } else if parts.first() == Some(&"hfsplus") && parts.get(1) == Some(&"part") {
        parts.get(2).and_then(|id| id.parse::<u8>().ok())
    } else {
        None
    };
    Ok(partition_id
        .and_then(|id| {
            session
                .partitions
                .iter()
                .find(|partition| partition.id == id)
        })
        .is_some_and(|partition| {
            matches!(
                partition.kind,
                PartitionKind::Ntfs
                    | PartitionKind::Fat12
                    | PartitionKind::Fat16
                    | PartitionKind::Fat32
                    | PartitionKind::ExFat
                    | PartitionKind::HfsPlus
            )
        }))
}
