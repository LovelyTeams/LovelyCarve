//! Recovery/export of a scan session: signature-carved artifacts plus a DFXML
//! manifest, and filesystem records with their JSONL provenance report.

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::host::State;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::app::access::loaded_image_app_error;
use crate::app::error::AppError;
use crate::commands::carving::carve::scan_session;
use crate::commands::carving::carve_filesystem::{filesystem_result, recover_files};
use crate::core::carving::carver::CarveArtifact;
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::recovery::export::RecoveryExport;
use crate::state::AppState;
use crate::task_manager::{TaskKind, TaskManager, TaskProgressUnit};

const RECOVERY_BUFFER_BYTES: usize = 4 * 1024 * 1024;
const MAX_RECOVERY_ERRORS: usize = 100;

#[derive(Debug, Clone, Deserialize)]
pub struct CarveRecoverOptions {
    /// `None` recovers every carved result in the scan; an explicit list
    /// recovers only those artifact ids. An empty list is rejected to avoid an
    /// accidental "all", unless `file_ids` selects filesystem records instead.
    #[serde(default)]
    pub artifact_ids: Option<Vec<String>>,
    /// Filesystem recovery candidates of the same session.
    #[serde(default)]
    pub file_ids: Vec<String>,
    pub destination: String,
    pub task_id: String,
}

/// Top-level counts describe carved artifacts; `files` describes filesystem records.
#[derive(Debug, Clone, Serialize)]
pub struct CarveRecoveryResult {
    pub recovered: usize,
    pub failed: usize,
    pub bytes_written: u64,
    pub synthetic_artifacts: usize,
    pub report_path: Option<String>,
    pub report_error: Option<String>,
    pub errors: Vec<String>,
    pub errors_truncated: bool,
    pub files: Option<RecoveryExport>,
}

#[derive(Debug)]
struct RecoveredArtifact {
    artifact: CarveArtifact,
    filename: String,
    sha256: String,
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn carve_recover(
    state: State<'_, AppState>,
    scan_id: String,
    options: CarveRecoverOptions,
) -> Result<CarveRecoveryResult, AppError> {
    if options.destination.trim().is_empty() {
        return Err(AppError::invalid_input("恢复目标目录不能为空"));
    }
    if options.task_id.trim().is_empty() {
        return Err(AppError::invalid_input("恢复任务 ID 不能为空"));
    }
    let session = scan_session(&state, &scan_id).await?;
    let (image_id, artifacts, files) = {
        let scan = session.read();
        let artifacts = match options.artifact_ids.as_deref() {
            Some([]) if !options.file_ids.is_empty() => Vec::new(),
            selected => select_artifacts(&scan.artifacts, selected)?,
        };
        let files = if options.file_ids.is_empty() {
            None
        } else {
            Some(filesystem_result(&scan, &options.file_ids)?)
        };
        (scan.image_id.clone(), artifacts, files)
    };
    if artifacts.is_empty() && files.is_none() {
        return Err(AppError::invalid_input("没有可恢复的雕刻结果"));
    }
    let image = loaded_image_app_error(&state, &image_id).await?;
    let task = state
        .tasks
        .register_if_absent(
            options.task_id.clone(),
            TaskKind::CarveRecovery,
            Some(image_id),
        )
        .ok_or_else(|| AppError::invalid_input("恢复任务 ID 正在使用中"))?;
    let resource_permit = state.tasks.acquire_resources(&task).await?;
    let token = task.token();
    let tasks = state.tasks.clone();
    let task_id = options.task_id;
    let destination = PathBuf::from(options.destination);
    let source_path = image.path.display().to_string();
    let scan_id_for_worker = scan_id.clone();
    let file_ids = options.file_ids;

    tokio::task::spawn_blocking(move || {
        let _resource_permit = resource_permit;
        let mut result = if artifacts.is_empty() {
            CarveRecoveryResult {
                recovered: 0,
                failed: 0,
                bytes_written: 0,
                synthetic_artifacts: 0,
                report_path: None,
                report_error: None,
                errors: Vec::new(),
                errors_truncated: false,
                files: None,
            }
        } else {
            recover_artifacts(
                &image,
                &source_path,
                &scan_id_for_worker,
                &destination,
                &artifacts,
                &task_id,
                &tasks,
                &token,
            )?
        };
        if let Some(scan) = files {
            result.files = Some(recover_files(
                &image,
                &scan,
                &file_ids,
                &destination,
                &tasks,
                &task_id,
                &token,
            )?);
        }
        Ok(result)
    })
    .await
    .map_err(|error| AppError::task_panicked("carve_recover", error))?
}

fn select_artifacts(
    artifacts: &[CarveArtifact],
    selected_ids: Option<&[String]>,
) -> Result<Vec<CarveArtifact>, AppError> {
    let Some(selected_ids) = selected_ids else {
        return Ok(artifacts.to_vec());
    };
    if selected_ids.is_empty() {
        return Err(AppError::invalid_input("恢复结果列表不能为空"));
    }
    let selected = selected_ids.iter().collect::<HashSet<_>>();
    let found = artifacts
        .iter()
        .filter(|artifact| selected.contains(&artifact.id))
        .cloned()
        .collect::<Vec<_>>();
    if found.len() != selected.len() {
        return Err(AppError::not_found("部分雕刻结果不存在或已过期"));
    }
    Ok(found)
}

#[allow(clippy::too_many_arguments)]
fn recover_artifacts(
    image: &Arc<EvidenceSession>,
    source_path: &str,
    scan_id: &str,
    destination: &Path,
    artifacts: &[CarveArtifact],
    task_id: &str,
    tasks: &TaskManager,
    cancellation: &CancellationToken,
) -> Result<CarveRecoveryResult, AppError> {
    image
        .validate_source_file_stable()
        .map_err(|error| AppError::source_changed(format!("雕刻恢复前校验失败: {error:#}")))?;
    std::fs::create_dir_all(destination)
        .map_err(|error| AppError::from_io(format!("create {}", destination.display()), error))?;
    if !destination.is_dir() {
        return Err(AppError::invalid_input("恢复目标不是目录"));
    }

    let bytes_total = artifacts.iter().fold(0u64, |total, artifact| {
        total.saturating_add(artifact.length)
    });
    let mut bytes_written = 0u64;
    let mut progress_done = 0u64;
    let mut recovered = Vec::new();
    let mut failed = 0usize;
    let mut errors = Vec::new();
    let mut errors_truncated = false;
    let mut synthetic_artifacts = 0usize;
    tasks.emit_progress(
        task_id,
        TaskProgressUnit::Bytes,
        0,
        Some(bytes_total),
        Some("准备恢复雕刻文件".to_string()),
    );

    for artifact in artifacts {
        if cancellation.is_cancelled() {
            return Err(AppError::cancelled("雕刻恢复已取消"));
        }
        let output_path = unique_artifact_path(destination, artifact);
        match recover_one(image, artifact, &output_path, cancellation, |written| {
            tasks.emit_progress(
                task_id,
                TaskProgressUnit::Bytes,
                progress_done.saturating_add(written),
                Some(bytes_total),
                Some(
                    output_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                ),
            );
        }) {
            Ok((written, sha256)) => {
                bytes_written = bytes_written.saturating_add(written);
                if artifact.read_fault
                    || image.has_bad_region_in_range(artifact.offset, artifact.length)
                {
                    synthetic_artifacts += 1;
                }
                recovered.push(RecoveredArtifact {
                    artifact: artifact.clone(),
                    filename: output_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    sha256,
                });
                progress_done = progress_done.saturating_add(artifact.length);
            }
            Err(error) if error.code == crate::app::error::ErrorCode::Cancelled => {
                return Err(error);
            }
            Err(error) => {
                failed += 1;
                if errors.len() < MAX_RECOVERY_ERRORS {
                    errors.push(format!("{}: {}", artifact.id, error.message));
                } else {
                    errors_truncated = true;
                }
                progress_done = progress_done.saturating_add(artifact.length);
            }
        }
    }

    image
        .validate_source_file_stable()
        .map_err(|error| AppError::source_changed(format!("雕刻恢复后校验失败: {error:#}")))?;
    let (report_path, report_error) =
        match write_dfxml_report(destination, source_path, scan_id, &recovered) {
            Ok(path) => (Some(path.display().to_string()), None),
            Err(error) => (None, Some(error.message)),
        };
    tasks.emit_progress(
        task_id,
        TaskProgressUnit::Bytes,
        bytes_total,
        Some(bytes_total),
        Some(format!("已恢复 {} 个文件", recovered.len())),
    );
    Ok(CarveRecoveryResult {
        recovered: recovered.len(),
        failed,
        bytes_written,
        synthetic_artifacts,
        report_path,
        report_error,
        errors,
        errors_truncated,
        files: None,
    })
}

fn recover_one(
    image: &EvidenceSession,
    artifact: &CarveArtifact,
    output_path: &Path,
    cancellation: &CancellationToken,
    mut progress: impl FnMut(u64),
) -> Result<(u64, String), AppError> {
    let partial_path = partial_path(output_path);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial_path)
        .map_err(|error| AppError::from_io(format!("create {}", partial_path.display()), error))?;
    let mut writer = BufWriter::with_capacity(RECOVERY_BUFFER_BYTES, file);
    let mut buffer = vec![0u8; RECOVERY_BUFFER_BYTES];
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    while written < artifact.length {
        if cancellation.is_cancelled() {
            return Err(AppError::cancelled("雕刻恢复已取消"));
        }
        let wanted = (artifact.length - written).min(RECOVERY_BUFFER_BYTES as u64) as usize;
        let read = image
            .read_into(artifact.offset + written, &mut buffer[..wanted])
            .map_err(|error| {
                AppError::from_anyhow(error.context(format!(
                    "read carved artifact {} at {}",
                    artifact.id,
                    artifact.offset + written
                )))
            })?;
        if read == 0 {
            return Err(AppError::from_message(format!(
                "雕刻结果 {} 在 {} 字节处提前结束",
                artifact.id, written
            )));
        }
        writer.write_all(&buffer[..read]).map_err(|error| {
            AppError::from_io(format!("write {}", partial_path.display()), error)
        })?;
        hasher.update(&buffer[..read]);
        written += read as u64;
        progress(written);
    }
    writer
        .flush()
        .map_err(|error| AppError::from_io(format!("flush {}", partial_path.display()), error))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|error| AppError::from_io(format!("sync {}", partial_path.display()), error))?;
    drop(writer);
    std::fs::rename(&partial_path, output_path).map_err(|error| {
        AppError::from_io(
            format!(
                "rename {} to {}",
                partial_path.display(),
                output_path.display()
            ),
            error,
        )
    })?;
    Ok((written, hex::encode(hasher.finalize())))
}

fn unique_artifact_path(destination: &Path, artifact: &CarveArtifact) -> PathBuf {
    let extension = if artifact
        .extension
        .chars()
        .all(|character| character.is_ascii_alphanumeric())
    {
        artifact.extension.as_str()
    } else {
        "bin"
    };
    let requested_name = artifact.name.as_str();
    let safe_name = !requested_name.is_empty()
        && requested_name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_')
        });
    let base_name = if safe_name {
        requested_name.to_string()
    } else {
        format!("f{}.{}", artifact.offset / 512, extension)
    };
    let (stem, extension) = base_name
        .rsplit_once('.')
        .map_or((base_name.as_str(), extension), |(stem, extension)| {
            (stem, extension)
        });
    let mut suffix = 0u64;
    loop {
        let filename = if suffix == 0 {
            base_name.clone()
        } else {
            format!("{stem}_{suffix}.{extension}")
        };
        let candidate = destination.join(filename);
        if !candidate.exists() && !partial_path(&candidate).exists() {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

fn partial_path(output_path: &Path) -> PathBuf {
    let filename = output_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    output_path.with_file_name(format!("{filename}.partial"))
}

fn write_dfxml_report(
    destination: &Path,
    source_path: &str,
    scan_id: &str,
    artifacts: &[RecoveredArtifact],
) -> Result<PathBuf, AppError> {
    let report_path = unique_report_path(destination, scan_id);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&report_path)
        .map_err(|error| AppError::from_io(format!("create {}", report_path.display()), error))?;
    let mut writer = BufWriter::new(file);
    writeln!(writer, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")
        .map_err(|error| AppError::from_io("write DFXML header", error))?;
    writeln!(
        writer,
        "<dfxml xmlns=\"http://www.forensicswiki.org/wiki/Category:Digital_Forensics_XML\" version=\"1.2.0\">"
    )
    .map_err(|error| AppError::from_io("write DFXML root", error))?;
    writeln!(
        writer,
        "  <creator><program>LovelyCarve</program><version>{}</version></creator>",
        env!("CARGO_PKG_VERSION")
    )
    .map_err(|error| AppError::from_io("write DFXML creator", error))?;
    writeln!(
        writer,
        "  <source><image_filename>{}</image_filename></source>",
        xml_escape(source_path)
    )
    .map_err(|error| AppError::from_io("write DFXML source", error))?;
    for recovered in artifacts {
        writeln!(writer, "  <fileobject>")
            .map_err(|error| AppError::from_io("write DFXML fileobject", error))?;
        writeln!(
            writer,
            "    <filename>{}</filename>",
            xml_escape(&recovered.filename)
        )
        .map_err(|error| AppError::from_io("write DFXML filename", error))?;
        writeln!(
            writer,
            "    <filesize>{}</filesize>",
            recovered.artifact.length
        )
        .map_err(|error| AppError::from_io("write DFXML filesize", error))?;
        writeln!(
            writer,
            "    <hashdigest type=\"SHA256\">{}</hashdigest>",
            recovered.sha256
        )
        .map_err(|error| AppError::from_io("write DFXML hash", error))?;
        writeln!(
            writer,
            "    <byte_runs><byte_run file_offset=\"0\" img_offset=\"{}\" len=\"{}\" /></byte_runs>",
            recovered.artifact.offset, recovered.artifact.length
        )
        .map_err(|error| AppError::from_io("write DFXML byte run", error))?;
        writeln!(writer, "  </fileobject>")
            .map_err(|error| AppError::from_io("write DFXML fileobject end", error))?;
    }
    writeln!(writer, "</dfxml>").map_err(|error| AppError::from_io("write DFXML end", error))?;
    writer
        .flush()
        .map_err(|error| AppError::from_io("flush DFXML report", error))?;
    writer
        .get_ref()
        .sync_all()
        .map_err(|error| AppError::from_io("sync DFXML report", error))?;
    Ok(report_path)
}

fn unique_report_path(destination: &Path, scan_id: &str) -> PathBuf {
    let short_scan_id = scan_id.get(..8).unwrap_or(scan_id);
    let mut suffix = 0u64;
    loop {
        let filename = if suffix == 0 {
            format!("lovelycarve-carve-{short_scan_id}.dfxml")
        } else {
            format!("lovelycarve-carve-{short_scan_id}-{suffix}.dfxml")
        };
        let candidate = destination.join(filename);
        if !candidate.exists() {
            return candidate;
        }
        suffix = suffix.saturating_add(1);
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_dfxml_text() {
        assert_eq!(xml_escape("a<&\"'>"), "a&lt;&amp;&quot;&apos;&gt;");
    }

    #[test]
    fn selected_artifacts_preserve_scan_order_and_dedupe_ids() {
        let artifact = |id: &str, sequence| CarveArtifact {
            id: id.to_string(),
            name: format!("f{sequence}.png"),
            format_id: "png".to_string(),
            extension: "png".to_string(),
            category: "image".to_string(),
            offset: sequence,
            length: 1,
            confidence: 99,
            region: "whole".to_string(),
            read_fault: false,
            sequence,
        };
        let artifacts = vec![artifact("a", 1), artifact("b", 2)];
        let selected = select_artifacts(
            &artifacts,
            Some(&["b".to_string(), "a".to_string(), "a".to_string()]),
        )
        .expect("selection");
        assert_eq!(
            selected
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            vec!["a", "b"]
        );
    }
}
