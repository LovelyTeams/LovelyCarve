//! Hand-written commands: path-based browsing and reading over the index-first
//! directory pages, content search, context and task control, and the
//! discovery commands that need no evidence. Each one is declared in
//! `cli/commands.toml` alongside the generated commands.
use std::{io::Write, sync::Arc, time::Duration};

use serde::Serialize;
use serde_json::{json, Map, Value};

use super::{
    browsing::{self, Entry},
    catalog::{self, Command},
    context::{Context, Exec, Mode, Target},
    docs, generated, json_output, schema, search, strings, tools, CliError, Output,
};
use crate::{
    core::filesystem::{
        evidence_session::EvidenceSession,
        tree::fs_tree::{FsNode, FsSession},
    },
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
        .ok_or_else(|| CliError::invalid("检材已关闭；请重新运行 images open"))
}

/// Byte-level commands need the original evidence; index-only sessions can
/// still browse.
pub(super) fn require_source(image: &EvidenceSession) -> Result<(), CliError> {
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
        "ocrCuda": cfg!(feature = "ocr-cuda"),
        "dokanMount": cfg!(target_os = "windows"),
        "residentSessions": {
            "open": "sessions open <name>", "attach": "sessions attach <name>",
            "close": "sessions close <name>", "option": "--session <name>",
            "environment": "LOVELYCARVE_SESSION", "execution": "serial",
            "rawRead": "stream", "retains": ["case", "images", "assembledVolumes", "directory", "tasks"]
        },
        "session": {
            "protocol": "jsonl", "protocolVersion": 3,
            "request": {"id": "string", "command": "<collection>… <verb>", "args": "object keyed by API parameter name", "output": "optional file path for raw content", "force": "boolean, confirms destructive commands", "dryRun": "boolean"},
            "defaultJobs": 1, "maxJobs": 64,
            "responseOrder": "completion", "correlationField": "id",
            "references": "prior-successful-result-json-pointer",
            "nonParallelCommands": "barrier",
            "stdout": "responses", "events": "stderr",
            "maxLineBytes": 16 * 1024 * 1024, "maxRequests": 10000
        }
    }))
}

/// Hand-written commands that need neither the backend nor evidence, run
/// before anything starts; `None` for every other command.
pub fn offline(
    command: &Command,
    values: &Map<String, Value>,
    mode: Mode,
) -> Option<Result<Output, CliError>> {
    let result = match command.native.as_deref()? {
        "cli-search" => catalog::catalog().and_then(|catalog| {
            json_output(search::search(
                catalog,
                &text(values, "query").unwrap_or_default(),
            ))
        }),
        "docs-search" => json_output(docs::search(&text(values, "query").unwrap_or_default())),
        "docs-list" => json_output(docs::list()),
        "docs-get" => text(values, "page")
            .ok_or_else(|| CliError::invalid("缺少参数：page"))
            .and_then(|page| docs::get(&page, text(values, "section").as_deref()))
            .and_then(|reading| match mode {
                Mode::Session => json_output(reading),
                Mode::OneShot | Mode::Shell => Ok(Output::Bytes(reading.markdown.into_bytes())),
            }),
        "tools" => catalog::catalog().and_then(|catalog| json_output(tools::definitions(catalog))),
        "schema" => catalog::catalog().and_then(|catalog| {
            let words: Vec<String> = values
                .get("command")
                .and_then(Value::as_array)
                .map(|words| {
                    words
                        .iter()
                        .filter_map(|word| word.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            let list = values.get("list") == Some(&Value::Bool(true));
            schema::run(catalog, &words, list, mode)
        }),
        "system-get" => capabilities().map(Output::json),
        _ => return None,
    };
    Some(result)
}

pub async fn run(
    exec: &Exec,
    context: &mut Context,
    command: &Command,
    values: Map<String, Value>,
    target: Option<Target>,
    text_output: bool,
) -> Result<Output, CliError> {
    if let Some(result) = offline(command, &values, exec.mode) {
        return result;
    }
    match command.native.as_deref() {
        Some("tasks-wait") => {
            wait_idle(&exec.state).await?;
            Ok(Output::json(Value::Null))
        }
        Some("context-get") => context_get(exec, context).await,
        Some("context-set" | "context-use") => {
            if let Some(image) = text(&values, "imageId") {
                let image_id = exec.resolve_image(context, &image).await?;
                if context.image_id.as_deref() != Some(&image_id)
                    || command.native.as_deref() == Some("context-use")
                {
                    context.location.clear();
                }
                context.image_id = Some(image_id);
                context.scope = None;
                context.scope_source = None;
            }
            if let Some(dir) = text(&values, "dir") {
                let image_id = exec
                    .current_image(context)
                    .await?
                    .ok_or_else(|| CliError::new("noImage", "切换目录需要检材"))?;
                let location = exec.locate(context, &image_id, &dir).await?;
                if let Some(node) = node(exec, &image_id, &location).await? {
                    if node.kind != crate::core::filesystem::tree::fs_tree::NodeKind::Dir {
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
        Some("files-tree-list") => files_tree_list(exec, &values, target).await,
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

#[derive(Serialize)]
struct ContextInfo {
    case: Value,
    image: Value,
    scope: Option<String>,
    dir: String,
}

async fn context_get(exec: &Exec, context: &Context) -> Result<Output, CliError> {
    let case = match generated::dispatch(&exec.host, &exec.state, "case_active", Map::new()).await?
    {
        Output::Json(case) => case.value().clone(),
        Output::Bytes(_) | Output::Written => Value::Null,
    };
    let image = match &context.image_id {
        Some(id) => {
            let args = Map::from_iter([("imageId".to_owned(), json!(id))]);
            match generated::dispatch(&exec.host, &exec.state, "image_info", args).await {
                Ok(Output::Json(info)) => {
                    let info = info.value();
                    json!({"id": id, "path": info["path"], "sourceFormat": info["sourceFormat"]})
                }
                Ok(_) | Err(_) => json!({"id": id}),
            }
        }
        None => Value::Null,
    };
    json_output(ContextInfo {
        case,
        image,
        scope: context.scope.clone(),
        dir: context.pwd(),
    })
}

/// The directory entry for the last node of `location`; `None` at `/`.
async fn node(
    exec: &Exec,
    image_id: &str,
    location: &[(String, String)],
) -> Result<Option<FsNode>, CliError> {
    let mut page = |parent_id, cursor, page_size| exec.page(image_id, parent_id, cursor, page_size);
    browsing::node(location, &mut page).await
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
    let located = located.value();
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
    if let Some(file) = located["fileId"].as_str() {
        location.push((String::new(), file.to_owned()));
    }
    Ok(location)
}

#[derive(Serialize)]
struct Listing {
    items: Vec<Entry>,
    truncated: bool,
}

async fn files_tree_list(
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
    json_output(Listing { items, truncated })
}

/// The synthetic root of an image.
#[derive(Serialize)]
struct Root {
    id: &'static str,
    name: &'static str,
    kind: &'static str,
    size: u64,
    deleted: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NodeInfo<N> {
    #[serde(flatten)]
    node: N,
    node_id: String,
    path: String,
    index_only: bool,
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
    let found = node(exec, &image_id, &location).await?;
    let own_name = found.as_ref().map_or("/", |node| node.name.as_str());
    let names: Vec<&str> = location
        .iter()
        .map(|(name, id)| {
            if name.is_empty() && id == &target.node_id {
                own_name
            } else {
                name.as_str()
            }
        })
        .collect();
    let path = format!("/{}", names.join("/"));
    let index_only = image.is_index_only();
    match found {
        Some(node) => json_output(NodeInfo {
            node: serde_json::to_value(super::wire::Camel(node))?,
            node_id: target.node_id,
            path,
            index_only,
        }),
        None => json_output(NodeInfo {
            node: Root {
                id: "",
                name: "/",
                kind: "dir",
                size: image.len(),
                deleted: false,
            },
            node_id: target.node_id,
            path,
            index_only,
        }),
    }
}

/// Decodes UTF-8 across chunk boundaries, replacing invalid sequences.
#[derive(Default)]
pub(super) struct Utf8Stream {
    pending: Vec<u8>,
}

impl Utf8Stream {
    pub(super) fn push(&mut self, chunk: &[u8], out: &mut String) {
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

    pub(super) fn finish(&mut self, out: &mut String) {
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

    #[test]
    fn discovery_commands_run_without_the_backend() {
        let catalog = catalog::catalog().unwrap();
        let docs = catalog.find(&["docs", "get"]).unwrap();
        let values = Map::from_iter([("page".to_owned(), json!("CLI.md"))]);
        match offline(docs, &values, Mode::OneShot) {
            Some(Ok(Output::Bytes(markdown))) => {
                assert!(markdown.starts_with("# LovelyCarve CLI".as_bytes()))
            }
            other => panic!("{other:?}"),
        }
        match offline(docs, &values, Mode::Session) {
            Some(Ok(Output::Json(reading))) => assert_eq!(reading.value()["page"], "CLI.md"),
            other => panic!("{other:?}"),
        }
        let tools = catalog.find(&["tools"]).unwrap();
        assert!(matches!(
            offline(tools, &Map::new(), Mode::Session),
            Some(Ok(Output::Json(_)))
        ));
        let schema = catalog.find(&["schema"]).unwrap();
        let values = Map::from_iter([("command".to_owned(), json!(["images", "open"]))]);
        match offline(schema, &values, Mode::Session) {
            Some(Ok(Output::Json(info))) => assert_eq!(info.value()["ipcCommand"], "image_load"),
            other => panic!("{other:?}"),
        }
        let wait = catalog.find(&["tasks", "wait"]).unwrap();
        assert!(offline(wait, &Map::new(), Mode::OneShot).is_none());
    }
}
