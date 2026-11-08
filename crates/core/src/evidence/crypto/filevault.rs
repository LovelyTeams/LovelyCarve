//! CoreStorage inventory and upstream forensic observations over read-only evidence.
use super::encrypted_image::EvidenceReader;
use crate::evidence::image::DiskImage;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InspectOptions {
    pub partition_id: Option<u8>,
    pub offset: Option<u64>,
    pub length: Option<u64>,
    pub encrypted_root_plist_path: Option<String>,
}
#[derive(Serialize)]
pub struct Finding {
    pub code: String,
    pub severity: String,
    pub note: String,
}
#[derive(Serialize)]
pub struct LogicalVolume {
    pub index: usize,
    pub name: Option<String>,
    pub identifier: Option<String>,
    pub family_identifier: String,
    pub size: u64,
    pub pbkdf2_iterations: u32,
    pub protectors: Vec<String>,
    pub conversion_status: Option<String>,
    pub findings: Vec<Finding>,
    pub object_identifier: String,
    pub transaction: String,
}
#[derive(Serialize)]
pub struct Inventory {
    pub volumes: Vec<LogicalVolume>,
    pub header_offset: u64,
    pub source_offset: u64,
    pub source_length: u64,
    pub physical_volume_identifier: String,
    pub volume_group_identifier: String,
    pub volume_group_name: Option<String>,
    pub metadata_pointer_offset: u64,
    pub encrypted_metadata_offset: u64,
}
pub(crate) fn read_external_context(path: Option<&str>) -> Result<Option<Vec<u8>>> {
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return Ok(None);
    };
    let file = std::fs::File::open(path).context("读取 FileVault 外部加密上下文失败")?;
    ensure!(
        file.metadata()?.len() <= 16 * 1024 * 1024,
        "FileVault 外部加密上下文超过 16 MiB"
    );
    let mut bytes = Vec::new();
    file.take(16 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= 16 * 1024 * 1024,
        "FileVault 外部加密上下文超过 16 MiB"
    );
    Ok(Some(bytes))
}
pub fn inspect(
    source: &Arc<DiskImage>,
    options: InspectOptions,
    cancellation: CancellationToken,
) -> Result<Inventory> {
    let reader = EvidenceReader::new(
        source,
        options.partition_id,
        options.offset,
        options.length,
        Arc::new(Mutex::new(cancellation.clone())),
    )?;
    let (source_offset, source_length) = reader.bounds();
    let external = read_external_context(options.encrypted_root_plist_path.as_deref())?;
    let container = filevault::storage::Container::open(reader, external.as_deref())?;
    ensure!(!cancellation.is_cancelled(), "FileVault 元数据读取已取消");
    let volumes = container
        .volumes()
        .map(|volume| {
            let info = &volume.info;
            let findings = filevault_forensic::audit_info(info)
                .into_iter()
                .map(|finding| Finding {
                    code: finding.code.to_string(),
                    severity: format!("{:?}", finding.severity).to_lowercase(),
                    note: finding.note,
                })
                .collect();
            LogicalVolume {
                index: volume.index,
                name: info.lv_name.clone(),
                identifier: info.lv_identifier.clone(),
                family_identifier: info.family_uuid.clone(),
                size: info.lv_size,
                pbkdf2_iterations: info.pbkdf2_iterations,
                protectors: info
                    .protectors
                    .iter()
                    .map(|protector| protector.kind.label().to_string())
                    .collect(),
                conversion_status: info.conversion_status.clone(),
                findings,
                object_identifier: format!("0x{:x}", volume.object_identifier),
                transaction: volume.transaction.to_string(),
            }
        })
        .collect();
    Ok(Inventory {
        volume_group_name: container.volume_group_name.clone(),
        source_offset,
        source_length,
        physical_volume_identifier: uuid::Uuid::from_bytes(
            container.header().physical_volume_identifier,
        )
        .to_string(),
        volume_group_identifier: uuid::Uuid::from_bytes(
            container.header().logical_volume_group_identifier,
        )
        .to_string(),
        volumes,
        header_offset: container.header_offset,
        metadata_pointer_offset: container.metadata_pointer_offset,
        encrypted_metadata_offset: container.encrypted_metadata_offset,
    })
}
