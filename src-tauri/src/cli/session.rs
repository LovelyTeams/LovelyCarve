//! `session`: JSONL requests on stdin, one JSON response per line on stdout.
//! Commands are the same `<collection>… <verb>` paths with arguments keyed by
//! API parameter name. Independent queries may run concurrently; anything
//! not marked concurrent in the catalogue is a barrier.
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, Write},
    path::Path,
};

use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::{sync::mpsc, task::JoinSet};

use super::{
    catalog::{self, Command},
    context::{Context, Exec, Request as Run},
    CliError, Output,
};

const MAX_LINE: usize = 16 * 1024 * 1024;
const MAX_REQUESTS: usize = 10_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Request {
    id: String,
    command: String,
    #[serde(default)]
    args: Map<String, Value>,
    /// Save the result to this file instead of returning it.
    #[serde(default)]
    output: Option<String>,
    /// Confirms a destructive command, like `--force`.
    #[serde(default)]
    force: bool,
    #[serde(default)]
    dry_run: bool,
}

/// Whether `command` may overlap other queries.
pub(super) fn concurrent(command: &str) -> bool {
    catalog::catalog()
        .ok()
        .and_then(|catalog| catalog.find_path(command))
        .is_some_and(|command| command.concurrent)
}

fn camel(name: &str) -> String {
    let mut output = String::new();
    let mut upper = false;
    for character in name.chars() {
        if character == '-' || character == '_' {
            upper = true;
        } else if upper {
            output.extend(character.to_uppercase());
            upper = false;
        } else {
            output.push(character);
        }
    }
    output
}

/// Accept API names, option names and snake_case alike.
fn normalize(command: &Command, args: Map<String, Value>) -> Result<Map<String, Value>, CliError> {
    let mut normalized = Map::new();
    for (key, value) in args {
        let name = command
            .params
            .iter()
            .find(|param| param.name == key || param.option == key)
            .map(|param| param.name.clone())
            .unwrap_or_else(|| camel(&key));
        if !command
            .params
            .iter()
            .any(|param| param.name == name && param.visible())
        {
            return Err(CliError::invalid(format!("未知参数: {key}")));
        }
        if normalized.insert(name.clone(), value).is_some() {
            return Err(CliError::invalid(format!("重复参数: {name}")));
        }
    }
    Ok(normalized)
}

/// Only structural references are expanded; literal evidence text stays exact.
fn resolve_refs(value: &mut Value, results: &HashMap<String, Value>) -> Result<(), CliError> {
    match value {
        Value::Object(object) if object.len() == 1 && object.contains_key("$ref") => {
            let reference = object["$ref"]
                .as_str()
                .ok_or_else(|| CliError::invalid("$ref 必须是字符串"))?;
            let (id, pointer) = reference.split_once('#').unwrap_or((reference, ""));
            *value = results
                .get(id)
                .and_then(|result| result.pointer(pointer))
                .cloned()
                .ok_or_else(|| CliError::invalid(format!("无效结果引用: {reference}")))?;
        }
        Value::Object(object) => {
            for value in object.values_mut() {
                resolve_refs(value, results)?;
            }
        }
        Value::Array(array) => {
            for value in array {
                resolve_refs(value, results)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn pending_ref(value: &Value, pending: &HashSet<String>) -> bool {
    match value {
        Value::Object(object) if object.len() == 1 && object.contains_key("$ref") => {
            object["$ref"].as_str().is_some_and(|reference| {
                pending.contains(reference.split_once('#').map_or(reference, |(id, _)| id))
            })
        }
        Value::Object(object) => object.values().any(|value| pending_ref(value, pending)),
        Value::Array(array) => array.iter().any(|value| pending_ref(value, pending)),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => false,
    }
}

fn print(response: &Value) -> Result<(), CliError> {
    let mut stdout = std::io::stdout().lock();
    serde_json::to_writer(&mut stdout, response)?;
    writeln!(stdout)?;
    stdout.flush()?;
    Ok(())
}

struct Completion {
    id: String,
    context: Context,
    result: Result<Value, CliError>,
}

#[derive(Default)]
struct Responses {
    results: HashMap<String, Value>,
    pending: HashSet<String>,
    failed: bool,
}

impl Responses {
    fn publish(
        &mut self,
        id: Option<String>,
        result: Result<Value, CliError>,
    ) -> Result<(), CliError> {
        let response = match result {
            Ok(value) => {
                if let Some(id) = &id {
                    self.results.insert(id.clone(), value.clone());
                }
                json!({"id": id, "result": value})
            }
            Err(error) => {
                self.failed = true;
                json!({"id": id, "error": error})
            }
        };
        print(&response)
    }

    async fn finish_one(&mut self, tasks: &mut JoinSet<Completion>) -> Result<(), CliError> {
        if let Some(completion) = tasks.join_next().await {
            let completion = completion.map_err(super::internal_error)?;
            self.pending.remove(&completion.id);
            self.publish(Some(completion.id), completion.result)?;
        }
        Ok(())
    }
}

fn launch<F>(tasks: &mut JoinSet<Completion>, context: Context, request: Request, execute: F)
where
    F: Fn(&mut Context, Request) -> Result<Value, CliError> + Send + 'static,
{
    tasks.spawn_blocking(move || {
        let mut context = context;
        let id = request.id.clone();
        // Keep the request ID even when a handler panics, and let other queries finish.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute(&mut context, request)
        }))
        .unwrap_or_else(|_| Err(super::internal_error("会话请求的工作线程异常退出")));
        Completion {
            id,
            context,
            result,
        }
    });
}

async fn run_lines<F>(
    context: &mut Context,
    mut lines: mpsc::Receiver<Result<String, CliError>>,
    jobs: usize,
    execute: F,
) -> Result<bool, CliError>
where
    F: Fn(&mut Context, Request) -> Result<Value, CliError> + Clone + Send + 'static,
{
    let mut responses = Responses::default();
    let mut request_ids = HashSet::new();
    let mut tasks = JoinSet::new();
    loop {
        let line = tokio::select! {
            completion = responses.finish_one(&mut tasks), if !responses.pending.is_empty() => {
                completion?;
                continue;
            }
            line = lines.recv() => line,
        };
        let Some(line) = line else {
            break;
        };
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                responses.publish(None, Err(error))?;
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(line.trim_start_matches('\u{feff}')) {
            Ok(value) => value,
            Err(error) => {
                responses.publish(None, Err(error.into()))?;
                continue;
            }
        };
        let id = value.get("id").and_then(Value::as_str).map(str::to_owned);
        let mut request: Request = match serde_json::from_value(value) {
            Ok(request) => request,
            Err(error) => {
                responses.publish(id, Err(error.into()))?;
                continue;
            }
        };
        let validation = if request.id.is_empty() || request_ids.contains(&request.id) {
            Err(CliError::invalid("请求 ID 必须非空且唯一"))
        } else if request_ids.len() >= MAX_REQUESTS {
            Err(CliError::invalid("单会话最多保留 10000 个结果引用"))
        } else {
            request_ids.insert(request.id.clone());
            Ok(())
        };
        if let Err(error) = validation {
            responses.publish(Some(request.id), Err(error))?;
            continue;
        }
        let mut args = Value::Object(std::mem::take(&mut request.args));
        let parallel = jobs > 1 && concurrent(&request.command);
        while (!parallel && !tasks.is_empty())
            || tasks.len() >= jobs
            || pending_ref(&args, &responses.pending)
        {
            responses.finish_one(&mut tasks).await?;
        }
        if let Err(error) = resolve_refs(&mut args, &responses.results) {
            responses.publish(Some(request.id), Err(error))?;
            continue;
        }
        let Value::Object(args) = args else {
            return Err(CliError::invalid("args 必须为对象"));
        };
        request.args = args;
        if parallel {
            responses.pending.insert(request.id.clone());
            launch(&mut tasks, context.clone(), request, execute.clone());
        } else {
            launch(&mut tasks, context.clone(), request, execute.clone());
            let completion = tasks
                .join_next()
                .await
                .ok_or_else(|| super::internal_error("会话请求未完成"))?
                .map_err(super::internal_error)?;
            *context = completion.context;
            responses.publish(Some(completion.id), completion.result)?;
        }
    }
    while !tasks.is_empty() {
        responses.finish_one(&mut tasks).await?;
    }
    Ok(responses.failed)
}

fn save(path: &Path, output: Output) -> Result<(), CliError> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    match output {
        Output::Json(value) => {
            serde_json::to_writer_pretty(&mut file, &value)?;
            writeln!(file)?;
        }
        Output::Bytes(bytes) => file.write_all(&bytes)?,
        Output::Written => {}
    }
    file.as_file().sync_all()?;
    file.persist_noclobber(path)
        .map_err(|error| CliError::from(error.error))?;
    Ok(())
}

fn read_line(reader: &mut impl std::io::BufRead) -> Result<Option<String>, CliError> {
    let mut text = String::new();
    let bytes = std::io::Read::take(&mut *reader, (MAX_LINE + 1) as u64).read_line(&mut text)?;
    if bytes > MAX_LINE {
        return Err(CliError::invalid("单行请求超过 16 MiB"));
    }
    Ok((bytes != 0).then_some(text))
}

pub(super) async fn json_lines(
    exec: Exec,
    context: &mut Context,
    jobs: usize,
) -> Result<bool, CliError> {
    // Blocking stdin runs outside the coordinator: results flush even while the
    // client keeps its pipe open and waits before submitting another request.
    let (sender, receiver) = mpsc::channel(jobs);
    std::thread::Builder::new()
        .name("lovelycarve-cli-input".into())
        .spawn(move || {
            let mut reader = std::io::stdin().lock();
            loop {
                let line = read_line(&mut reader);
                let stop = line.is_err();
                let item = match line {
                    Ok(Some(line)) => Ok(line),
                    Ok(None) => break,
                    Err(error) => Err(error),
                };
                if sender.blocking_send(item).is_err() || stop {
                    break;
                }
            }
        })?;
    let runtime = tokio::runtime::Handle::current();
    run_lines(context, receiver, jobs, move |context, request| {
        let catalog = catalog::catalog()?;
        let command = catalog.find_path(&request.command).ok_or_else(|| {
            CliError::invalid(match catalog.replacement(&request.command) {
                Some(replacement) => format!(
                    "未知命令: {}；现在的命令是 {}",
                    request.command,
                    replacement.trim_start_matches("lovelycarve-cli ")
                ),
                None => format!("未知命令: {}", request.command),
            })
        })?;
        if matches!(
            command.native.as_deref(),
            Some("session" | "shell" | "complete")
        ) {
            return Err(CliError::invalid(format!(
                "{} 不能在会话中运行",
                request.command
            )));
        }
        let values = normalize(command, request.args)?;
        let output = runtime.block_on(exec.execute(
            context,
            Run {
                command,
                values,
                ask: Vec::new(),
                force: request.force,
                dry_run: request.dry_run,
                text: false,
            },
        ))?;
        match request.output {
            Some(path) => {
                save(Path::new(&path), output)?;
                Ok(json!({"output": path}))
            }
            None => Ok(output.into_json()),
        }
    })
    .await
}

#[cfg(test)]
mod tests;
