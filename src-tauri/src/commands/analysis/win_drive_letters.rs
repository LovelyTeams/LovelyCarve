//! 原始盘符还原 IPC。读 Windows 卷的 SYSTEM 配置单元、解 `MountedDevices`、
//! 按磁盘签名/偏移（MBR）或唯一 GUID（GPT）关联到分区，返回 `分区 id → 盘符`。
//! 解析较重（NTFS 读 + 多 MiB 注册表配置单元），放 `spawn_blocking`，不占用
//! tokio 运行时。前端在目录树根列表渲染后异步调用，把盘符叠加到分区根节点。
//!
//! 有打开的案件时结果持久化到案件 SQLite（`core::casework::drive_letter_store`）：证据不可
//! 变，盘符是确定值，命中缓存即直接返回、连镜像配置单元都不必再读；旧 JSON sidecar
//! 仍作为兼容回退。仅缓存**非空**报告——空结果（非 Windows / BitLocker 仍上锁 /
//! 无 MountedDevices）保持可重算，否则解锁 BitLocker 后会被钉死在「什么都没找到」
//! 上。无案件时退回会话级实时解析。

use crate::host::State;

use crate::core::analysis::identify::win_drive_letters::{
    read_image_drive_letters, DriveLetterReport,
};
use crate::core::casework::drive_letter_store;
use crate::state::AppState;
use {crate::app::access::loaded_image_app_error, crate::app::error::AppError};

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn win_drive_letters(
    state: State<'_, AppState>,
    image_id: String,
) -> Result<DriveLetterReport, AppError> {
    // 快路径：案件里已缓存即直接返回（不读镜像、不解析配置单元）。锁只在取
    // 案件目录时短暂持有，绝不跨 spawn_blocking——否则重解析期间会卡住整个
    // 案件状态。
    let case_dir = {
        let guard = state.active_case.lock().await;
        guard.as_ref().map(|case| case.dir.clone())
    };
    if let Some(dir) = &case_dir {
        match drive_letter_store::load(dir, &image_id) {
            Ok(Some(report)) => return Ok(report),
            Ok(None) => {}
            Err(err) => {
                // 缓存损坏/版本过新：记一笔并退回实时解析，不打断浏览。
                tracing::warn!("drive-letter store: load image={image_id} failed: {err}");
            }
        }
    }

    // 缓存未命中：实时解析。
    let image = loaded_image_app_error(&state, &image_id).await?;
    let report = tokio::task::spawn_blocking(move || read_image_drive_letters(&image))
        .await
        .map_err(|err| AppError::task_panicked("win_drive_letters", err))?;

    // 仅在仍有案件且报告非空时落盘（重新取锁：重解析期间案件可能刚被打开）。
    if !report.assignments.is_empty() {
        let dir = {
            let guard = state.active_case.lock().await;
            guard.as_ref().map(|case| case.dir.clone())
        };
        if let Some(dir) = dir {
            if let Err(err) = drive_letter_store::save(&dir, &image_id, &report) {
                tracing::warn!("drive-letter store: persist image={image_id} failed: {err}");
            } else {
                tracing::info!(
                    "drive-letter store: persisted image={image_id} count={}",
                    report.assignments.len()
                );
            }
        }
    }

    Ok(report)
}
