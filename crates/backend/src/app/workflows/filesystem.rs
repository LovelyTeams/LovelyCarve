//! Lazy provider probing and filesystem session installation.
use crate::core::{
    evidence::image::PartitionKind, evidence::partitions::SectorSource,
    filesystem::evidence_session::EvidenceSession,
};
use crate::{app::error::AppError, state::AppState};
use std::sync::Arc;

pub(crate) async fn prepare_remote_partition(
    state: &AppState,
    image: &Arc<EvidenceSession>,
    parent_id: &str,
) -> Result<(), AppError> {
    let Some(partition_id) = parent_id.strip_prefix("part:") else {
        return Ok(());
    };
    let Ok(partition_id) = partition_id.parse::<u8>() else {
        return Ok(());
    };
    let Some(partition) = image
        .partitions
        .iter()
        .find(|partition| partition.id == partition_id && partition.kind == PartitionKind::Unknown)
        .cloned()
    else {
        return Ok(());
    };
    if image.external_volume(parent_id).is_some() {
        return Ok(());
    }
    let manager = state.plugins.read().clone();
    let Some(manager) = manager else {
        return Ok(());
    };

    let _open_guard = state.plugin_open_lock.lock().await;
    if image.external_volume(parent_id).is_some() {
        return Ok(());
    }
    if let Some(stale) = image.remove_external_volume(parent_id) {
        manager.sources().remove(stale.source_id());
    }
    let sector_size = u32::try_from(image.sector_size)
        .map_err(|_| AppError::resource_limit("镜像扇区大小超出插件协议范围"))?;
    let base = partition
        .start_lba
        .checked_mul(image.sector_size as u64)
        .ok_or_else(|| AppError::resource_limit("插件分区起始偏移溢出"))?;
    let length = partition
        .length_lba
        .checked_mul(image.sector_size as u64)
        .ok_or_else(|| AppError::resource_limit("插件分区长度溢出"))?;
    let parent: Arc<dyn SectorSource + Send + Sync> = image.clone();
    let source = manager
        .sources()
        .register_slice(parent, base, length, sector_size)
        .map_err(|error| AppError::from_anyhow(anyhow::anyhow!(error)))?;
    match manager.open_best(&source, parent_id.to_owned()).await {
        Ok(Some(volume)) => image.install_external_volume(volume),
        Ok(None) => {
            manager.sources().remove(&source.source_id);
        }
        Err(error) => {
            manager.sources().remove(&source.source_id);
            return Err(AppError::from_anyhow(anyhow::anyhow!(
                "插件无法打开未知分区 {partition_id}：{error}"
            )));
        }
    }
    Ok(())
}
