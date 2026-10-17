//! Hand-written commands: path-based browsing and reading over the index-first
//! directory pages, content search, context and task control. Each one is
//! declared in `cli/commands.toml` alongside the generated commands.
use std::{io::Write, sync::Arc, time::Duration};

use serde_json::{json, Map, Value};

use super::{
    browsing,
    catalog::{self, Command},
    context::{Context, Exec, Mode, Target},
    generated, search, strings, CliError, Output,
};
use crate::{
    core::filesystem::{evidence_session::EvidenceSession, tree::fs_tree::FsSession},
    state::AppState,
    task_manager::TaskKind,
};

/// Largest raw read returned inside a JSONL session response.
const MAX_SESSION_READ: u64 = 16 * 1024 * 1024;

fn number(values: &Map<String, Value>, name: &str) -> Option<u64> {
    values.get(name).and_then(Value::as_u64)
}

fn text(values: &Map<String, Value>, name: &str) -> Option<String> {
    values.get(name).and_then(Value::as_str).map(str::to_owned)
}

fn image_id(values: &Map<String, Value>) -> Result<String, CliError> {
    text(values, "imageId").ok_or_else(|| CliError::new("noImage", "需要检材"))
}

async fn loaded_image(state: &AppState, image_id: &str) -> Result<Arc<EvidenceSession>, CliError> {
    state
        .images
        .lock()
        .await
        .get(image_id)
        .cloned()
        .ok_or_else(|| CliError::invalid("检材已卸载；请重新 images open"))
}

/// Byte-level commands need the original evidence; index-only sessions can
/// still browse.
fn require_source(image: &EvidenceSession) -> Result<(), CliError> {
    if image.is_index_only() {
        image.require_source().map_err(super::internal_error)
    } else {
        image
            .validate_source_file_stable()
            .map_err(super::internal_error)
    }
}

/// Wait until no process task and no plugin task is active.
pub async fn wait_idle(state: &AppState) -> Result<(), CliError> {
    loop {
        let scheduler_busy = if state.active_case.lock().await.is_some() {
            state
                .scheduler
                .list_for_image(None, true)
                .map_err(super::internal_error)?
                .iter()
                .any(|task| task.status.is_active())
        } else {
            false
        };
        if state.tasks.list().is_empty() && !scheduler_busy {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub fn capabilities() -> Result<Value, CliError> {
    let catalog = catalog::catalog()?;
    Ok(json!({
        "version": catalog.version,
        "platform": std::env::consts::OS,
        "standalone": !cfg!(feature = "desktop"),
        "commands": catalog.commands.len(),
        "qrcode": cfg!(feature = "qrcode-wechat"),
        "ocr": cfg!(feature = "ocr"),
        "ocr_cuda": cfg!(feature = "ocr-cuda"),
        "dokan_mount": cfg!(target_os = "windows"),
        "session": {
            "protocol": "jsonl", "protocol_version": 2,
            "request": {"id": "string", "command": "<collection>… <verb>", "args": "object keyed by API parameter name", "output": "optional file path for raw content", "force": "boolean, confirms destructive commands", "dryRun": "boolean"},
            "default_jobs": 1, "max_jobs": 64,
            "response_order": "completion", "correlation_field": "id",
            "references": "prior-successful-result-json-pointer",
            "non_parallel_commands": "barrier",
            "stdout": "responses", "events": "stderr",
            "max_line_bytes": 16 * 1024 * 1024, "max_requests": 10000
        }
    }))
}

pub async fn run(
    exec: &Exec,
    context: &mut Context,
    command: &Command,
    values: Map<String, Value>,
    target: Option<Target>,
    text_output: bool,
) -> Result<Output, CliError> {
    match command.native.as_deref() {
        Some("cli-search") => {
            let query = text(&values, "query").unwrap_or_default();
            Ok(Output::Json(json!(search::search(
                catalog::catalog()?,
                &query
            ))))
        }
        Some("system-get") => Ok(Output::Json(capabilities()?)),
        Some("tasks-wait") => {
            wait_idle(&exec.state).await?;
            Ok(Output::Json(Value::Null))
        }
        Some("context-get") => context_get(exec, context).await,
        Some("context-set") => {
            if let Some(image) = text(&values, "imageId") {
                exec.resolve_image(context, &image).await?;
            }
            if let Some(dir) = text(&values, "dir") {
                let image_id = exec
                    .current_image(context)
                    .await?
                    .ok_or_else(|| CliError::new("noImage", "切换目录需要检材"))?;
                let location = exec.locate(context, &image_id, &dir).await?;
                if let Some(node) = node(exec, &image_id, &location).await? {
                    if node["kind"] != "dir" {
                        return Err(CliError::invalid("目标必须是目录；不会跟随符号链接"));
                    }
                }
                let parent = location
                    .last()
                    .map(|(_, id)| id.clone())
                    .unwrap_or_default();
                exec.page(&image_id, parent, None, 1).await?;
                context.location = location;
            }
            context_get(exec, context).await
        }
        Some("files-list") => files_list(exec, &values, target).await,
        Some("files-get") => files_get(exec, &values, target).await,
        Some("files-read") => files_read(exec, &values, target, text_output).await,
        Some("files-search") => {
            // Without the source bytes every file would fail to read and the
            // search would still "succeed" with no hits.
            let image_id = image_id(&values)?;
            let image = loaded_image(&exec.state, &image_id).await?;
            require_source(&image)?;
            let target = target.ok_or_else(|| CliError::invalid("需要路径"))?;
            super::grep::run(exec, &image_id, &target.node_id, &values).await
        }
        Some("files-strings") => {
            let image_id = image_id(&values)?;
            let image = loaded_image(&exec.state, &image_id).await?;
            require_source(&image)?;
            let target = target.ok_or_else(|| CliError::invalid("需要路径"))?;
            strings::read(&exec.state, image, target.node_id, &values).await
        }
        Some(other) => Err(CliError::invalid(format!(
            "{} 不能在 {} 中运行",
            command.command,
            match exec.mode {
                Mode::Session => "session",
                Mode::Shell => "shell",
                Mode::OneShot => other,
            }
        ))),
        None => Err(CliError::invalid(format!(
            "{} 不是手写命令",
            command.command
        ))),
    }
}

async fn context_get(exec: &Exec, context: &Context) -> Result<Output, CliError> {
    let case = match generated::dispatch(&exec.host, &exec.state, "case_active", Map::new()).await?
    {
        Output::Json(case) => case,
        Output::Bytes(_) | Output::Written => Value::Null,
    };
    let image = match &context.image_id {
        Some(id) => {
            let args = Map::from_iter([("imageId".to_owned(), json!(id))]);
            match generated::dispatch(&exec.host, &exec.state, "image_info", args).await {
                Ok(Output::Json(info)) => {
                    json!({"id": id, "path": info["path"], "source_format": info["source_format"]})
                }
                Ok(_) | Err(_) => json!({"id": id}),
            }
        }
        None => Value::Null,
    };
    Ok(Output::Json(json!({
        "case": case,
        "image": image,
        "scope": context.scope,
        "dir": context.pwd(),
    })))
}

/// The directory entry for the last node of `location`; `None` at `/`.
async fn node(
    exec: &Exec,
    image_id: &str,
    location: &[(String, String)],
) -> Result<Option<Value>, CliError> {
    let mut page = |parent_id, cursor, page_size| exec.page(image_id, parent_id, cursor, page_size);
    Ok(browsing::node(location, &mut page)
        .await?
        .map(|node| serde_json::to_value(node).unwrap_or(Value::Null)))
}

/// A location for a bare node ID, from the case's ancestor chain.
async fn locate_node(
    exec: &Exec,
    image_id: &str,
    node_id: &str,
) -> Result<Vec<(String, String)>, CliError> {
    let args = Map::from_iter([
        ("imageId".to_owned(), json!(image_id)),
        ("nodeId".to_owned(), json!(node_id)),
    ]);
    let Output::Json(located) =
        generated::dispatch(&exec.host, &exec.state, "fs_node_locate", args).await?
    else {
        return Err(super::internal_error(
            "files ancestors list 返回了非 JSON 结果",
        ));
    };
    let mut location: Vec<(String, String)> = located["frames"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|frame| {
            (
                frame["name"].as_str().unwrap_or_default().to_owned(),
                frame["id"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    if let Some(file) = located["file_id"].as_str() {
        location.push((String::new(), file.to_owned()));
    }
    Ok(location)
}

async fn files_list(
    exec: &Exec,
    values: &Map<String, Value>,
    target: Option<Target>,
) -> Result<Output, CliError> {
    let image_id = image_id(values)?;
    let target = target.ok_or_else(|| CliError::invalid("需要路径"))?;
    let state = &exec.state;
    let task = state.tasks.register(
        format!("cli-files-list-{}", uuid::Uuid::new_v4()),
        TaskKind::FlatList,
        Some(image_id.clone()),
    );
    let _permit = state
        .tasks
        .acquire_resources(&task)
        .await
        .map_err(crate::app::error::AppError::from)
        .map_err(super::command_error)?;
    let cancellation = task.token();
    let depth = number(values, "depth").unwrap_or(1).clamp(1, 128) as u32;
    let limit = number(values, "limit").unwrap_or(10_000).clamp(1, 100_000) as usize;
    let filter = text(values, "name").map(|name| name.to_lowercase());
    let mut page =
        |parent_id, cursor, page_size| exec.page(&image_id, parent_id, cursor, page_size);
    let (items, truncated) = browsing::walk(
        &target.node_id,
        depth,
        limit,
        filter.as_deref(),
        &cancellation,
        &mut page,
    )
    .await?;
    Ok(Output::Json(
        json!({"items": items, "truncated": truncated}),
    ))
}

async fn files_get(
    exec: &Exec,
    values: &Map<String, Value>,
    target: Option<Target>,
) -> Result<Output, CliError> {
    let image_id = image_id(values)?;
    let image = loaded_image(&exec.state, &image_id).await?;
    let target = target.ok_or_else(|| CliError::invalid("需要路径"))?;
    let location = match target.location {
        Some(location) => location,
        None => locate_node(exec, &image_id, &target.node_id).await?,
    };
    let mut value = match node(exec, &image_id, &location).await? {
        Some(node) => node,
        None => {
            json!({"id": "", "name": "/", "kind": "dir", "size": image.len(), "deleted": false})
        }
    };
    let names: Vec<String> = location
        .iter()
        .map(|(name, id)| {
            if name.is_empty() && id == &target.node_id {
                value["name"].as_str().unwrap_or_default().to_owned()
            } else {
                name.clone()
            }
        })
        .collect();
    value["node_id"] = json!(target.node_id);
    value["path"] = json!(format!("/{}", names.join("/")));
    value["index_only"] = json!(image.is_index_only());
    Ok(Output::Json(value))
}

/// Decodes UTF-8 across chunk boundaries, replacing invalid sequences.
#[derive(Default)]
struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    fn push(&mut self, chunk: &[u8], out: &mut String) {
        self.pending.extend_from_slice(chunk);
        let mut start = 0;
        loop {
            match std::str::from_utf8(&self.pending[start..]) {
                Ok(text) => {
                    out.push_str(text);
                    start = self.pending.len();
                    break;
                }
                Err(error) => {
                    let valid = start + error.valid_up_to();
                    out.push_str(&String::from_utf8_lossy(&self.pending[start..valid]));
                    match error.error_len() {
                        Some(length) => {
                            out.push('\u{FFFD}');
                            start = valid + length;
                        }
                        None => {
                            start = valid;
                            break;
                        }
                    }
                }
            }
        }
        self.pending.drain(..start);
    }

    fn finish(&mut self, out: &mut String) {
        if !self.pending.is_empty() {
            out.push_str(&String::from_utf8_lossy(&self.pending));
            self.pending.clear();
        }
    }
}

async fn files_read(
    exec: &Exec,
    values: &Map<String, Value>,
    target: Option<Target>,
    text_output: bool,
) -> Result<Output, CliError> {
    let image_id = image_id(values)?;
    let image = loaded_image(&exec.state, &image_id).await?;
    require_source(&image)?;
    let node_id = target.ok_or_else(|| CliError::invalid("需要路径"))?.node_id;
    let offset = number(values, "offset").unwrap_or(0);
    let requested = number(values, "length");
    let to_stdout = exec.mode != Mode::Session;
    tokio::task::spawn_blocking(move || -> Result<Output, CliError> {
        let mut session = FsSession::new(&image);
        let size = session
            .file_stat(&node_id)
            .map_err(super::internal_error)?
            .size;
        if offset > size {
            return Err(CliError::invalid(format!(
                "偏移 {offset} 超过文件大小 {size}"
            )));
        }
        let length = requested.unwrap_or(size - offset).min(size - offset);
        if !to_stdout {
            if length > MAX_SESSION_READ {
                return Err(CliError::invalid(
                    "会话中单次最多读取 16 MiB；用 offset/length 分段读取，或使用 files export",
                ));
            }
            let length = usize::try_from(length)
                .map_err(|_| CliError::invalid("当前平台无法表示该读取长度"))?;
            let bytes = crate::core::filesystem::tree::fs_tree::read_file_range(
                &image, &node_id, offset, length,
            )
            .map_err(super::internal_error)?;
            image
                .validate_source_file_stable()
                .map_err(super::internal_error)?;
            return Ok(Output::Bytes(bytes));
        }
        let mut stdout = std::io::stdout().lock();
        let mut decoder = Utf8Stream::default();
        let (result, faults) = image.collect_read_faults(|| {
            session.stream_file_range(&node_id, offset, length, 1024 * 1024, |chunk| {
                if text_output {
                    let mut decoded = String::new();
                    decoder.push(chunk, &mut decoded);
                    stdout.write_all(decoded.as_bytes())?;
                } else {
                    stdout.write_all(chunk)?;
                }
                Ok(())
            })
        });
        let read = result.map_err(super::internal_error)?;
        if text_output {
            let mut decoded = String::new();
            decoder.finish(&mut decoded);
            stdout.write_all(decoded.as_bytes())?;
        }
        stdout.flush()?;
        if read != length {
            return Err(CliError::new(
                "sourceChanged",
                format!("文件未完整读取：期望 {length} 字节，实际 {read} 字节"),
            ));
        }
        if faults.read_faults_detected {
            tracing::warn!(
                "files read: {} synthetic bytes from bad regions",
                faults.synthetic_bytes
            );
        }
        image
            .validate_source_file_stable()
            .map_err(super::internal_error)?;
        Ok(Output::Written)
    })
    .await
    .map_err(super::internal_error)?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_stream_survives_split_sequences() {
        let text = "取证 evidence ✓";
        let bytes = text.as_bytes();
        for split in 1..bytes.len() {
            let mut decoder = Utf8Stream::default();
            let mut out = String::new();
            decoder.push(&bytes[..split], &mut out);
            decoder.push(&bytes[split..], &mut out);
            decoder.finish(&mut out);
            assert_eq!(out, text, "split at {split}");
        }
        let mut decoder = Utf8Stream::default();
        let mut out = String::new();
        decoder.push(b"a\xffb\xe4", &mut out);
        decoder.finish(&mut out);
        assert_eq!(out, "a\u{FFFD}b\u{FFFD}");
    }
}
