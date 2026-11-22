//! `batch`: JSONL requests on stdin, one JSON response per line on stdout.
//! A request is the command line itself, `{"id":…,"argv":[…]}`, so scripts,
//! agents and people share one set of commands and options. An argv element
//! may be `{"$ref":"<id>#/<JSON pointer>"}`, a value from an earlier result.
//! Read-only queries may run concurrently (`--jobs`); anything else is a
//! barrier. With a case backend running, every request goes to it.
use std::{
    collections::{HashMap, HashSet},
    io::{BufRead, Write},
    path::Path,
};

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{sync::mpsc, task::JoinSet};

use super::{
    cmd::{CaseCommand, Command},
    exec::{Exec, RequestOptions},
    incomplete::{self, Incomplete, EXIT_INCOMPLETE},
    output,
    types::Secret,
    CliError, Json, Output,
};

const MAX_LINE: usize = 16 * 1024 * 1024;
const MAX_REQUESTS: usize = 10_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Request {
    id: String,
    argv: Vec<Value>,
    /// Save the result to this new file instead of returning it.
    #[serde(default)]
    output: Option<String>,
}

/// Only structural references are expanded; literal evidence text stays exact.
fn resolve_refs(value: &mut Value, results: &HashMap<String, Json>) -> Result<(), CliError> {
    match value {
        Value::Object(object) if object.len() == 1 && object.contains_key("$ref") => {
            let reference = object["$ref"]
                .as_str()
                .ok_or_else(|| CliError::invalid("$ref 必须是字符串"))?;
            let (id, pointer) = reference.split_once('#').unwrap_or((reference, ""));
            *value = results
                .get(id)
                .and_then(|result| result.value().pointer(pointer))
                .cloned()
                .ok_or_else(|| CliError::invalid(format!("无效结果引用：{reference}")))?;
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

/// The words of a resolved argv: strings as they are, other JSON as text.
fn words(argv: Vec<Value>) -> Result<Vec<String>, CliError> {
    argv.into_iter()
        .map(|value| match value {
            Value::String(text) => Ok(text),
            Value::Number(number) => Ok(number.to_string()),
            Value::Bool(flag) => Ok(flag.to_string()),
            Value::Null => Err(CliError::invalid("argv 中不能有 null")),
            other => Ok(other.to_string()),
        })
        .collect()
}

/// A successful response: the result, and why it is incomplete when it is.
pub(super) struct Answer {
    json: Json,
    incomplete: Option<Incomplete>,
}

impl From<Json> for Answer {
    fn from(json: Json) -> Self {
        Self {
            json,
            incomplete: None,
        }
    }
}

/// One response line: `{"id":…,"result":…}` with the result's JSON text as
/// the command wrote it, followed by `"warning":…` when the result is
/// incomplete, or `{"id":…,"error":…}`.
fn response(id: Option<&str>, result: &Result<Answer, CliError>) -> Result<String, CliError> {
    let id = serde_json::to_string(&id)?;
    Ok(match result {
        Ok(Answer {
            json,
            incomplete: None,
        }) => format!("{{\"id\":{id},\"result\":{}}}", json.text()),
        Ok(Answer {
            json,
            incomplete: Some(incomplete),
        }) => format!(
            "{{\"id\":{id},\"result\":{},\"warning\":{}}}",
            json.text(),
            serde_json::to_string(incomplete)?
        ),
        Err(error) => format!(
            "{{\"id\":{id},\"error\":{}}}",
            serde_json::to_string(error)?
        ),
    })
}

struct Completion {
    id: String,
    result: Result<Answer, CliError>,
}

/// The responses written so far, to `out`, and what later requests may
/// refer to.
struct Responses<W> {
    out: W,
    results: HashMap<String, Json>,
    pending: HashSet<String>,
    failed: bool,
    incomplete: bool,
}

impl<W: Write> Responses<W> {
    fn new(out: W) -> Self {
        Self {
            out,
            results: HashMap::new(),
            pending: HashSet::new(),
            failed: false,
            incomplete: false,
        }
    }

    fn publish(
        &mut self,
        id: Option<String>,
        result: Result<Answer, CliError>,
    ) -> Result<(), CliError> {
        writeln!(self.out, "{}", response(id.as_deref(), &result)?)?;
        self.out.flush()?;
        match result {
            Ok(answer) => {
                self.incomplete |= answer.incomplete.is_some();
                if let Some(id) = id {
                    self.results.insert(id, answer.json);
                }
            }
            Err(_) => self.failed = true,
        }
        Ok(())
    }

    /// 1 when a request failed, else 2 when a result was incomplete.
    fn status(&self) -> i32 {
        if self.failed {
            1
        } else if self.incomplete {
            EXIT_INCOMPLETE
        } else {
            0
        }
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

/// A parsed request, ready to run.
pub(super) struct Prepared {
    id: String,
    command: Command,
    options: RequestOptions,
    output: Option<String>,
    /// `--json=fields`: the list fields its result keeps.
    fields: Vec<Vec<String>>,
}

/// Parse one request's argv: the same command line as `lovelycarve-cli`,
/// minus what only a process can do.
fn prepare(
    id: String,
    words: &[String],
    output: Option<String>,
) -> Result<Result<Prepared, Answer>, CliError> {
    let cli = super::runtime::parse(words)?;
    let globals = cli.globals;
    if globals.case.is_some() || !globals.images.is_empty() {
        return Err(CliError::new(
            "usage",
            "batch 请求中不能用 -C / --image：案件在启动 batch 时指定，检材用 evidence add 请求打开",
        ));
    }
    let mut command = cli
        .command
        .ok_or_else(|| CliError::new("usage", "argv 需要命令，例如 [\"ls\", \"/\"]"))?;
    match &command {
        Command::Shell | Command::Batch(_) | Command::Complete(_) | Command::Completion(_) => {
            return Err(CliError::new(
                "usage",
                format!("{} 不能在 batch 中运行", command.path()),
            ))
        }
        Command::Case(super::cmd::case::CaseArgs {
            command: Some(CaseCommand::Open(_) | CaseCommand::Close(_) | CaseCommand::Serve(_)),
        }) => {
            return Err(CliError::new(
                "usage",
                "case open / close 管理进程，不能在 batch 中运行",
            ))
        }
        Command::Schema(args) => {
            return Ok(Err(Json::from(super::schema::describe(&args.words)?).into()))
        }
        Command::System => return Ok(Err(Json::from(super::cmd::system::capabilities()).into())),
        _ => {}
    }
    super::runtime::check_projection(&command, &globals)?;
    let fields = super::projection::parse(globals.json.as_deref())?;
    if globals.dry_run {
        let mut value = serde_json::to_value(&command)?;
        value = json!({"command": command.path(), "request": value, "executed": false});
        return Ok(Err(Json::from(value).into()));
    }
    for (name, secret) in command.secrets_mut() {
        if matches!(secret, Secret::Stdin | Secret::Ask) {
            return Err(CliError::new(
                "usage",
                format!("batch 中 --{name} 只能写成 @文件：标准输入是请求流"),
            ));
        }
        secret.resolve(name, false)?;
    }
    Ok(Ok(Prepared {
        id,
        command,
        options: RequestOptions {
            tz: globals.tz,
            yes: globals.yes,
        },
        output,
        fields,
    }))
}

/// Runs every request, writes one response line each to `out` and returns
/// the batch's status (see [`Responses::status`]).
async fn run_lines<F, W>(
    mut lines: mpsc::Receiver<Result<String, CliError>>,
    jobs: usize,
    out: W,
    execute: F,
) -> Result<i32, CliError>
where
    F: Fn(Prepared) -> Result<Answer, CliError> + Clone + Send + 'static,
    W: Write,
{
    let mut responses = Responses::new(out);
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
        let named_command = value.get("command").is_some() && value.get("argv").is_none();
        let request: Request = match serde_json::from_value(value) {
            Ok(request) => request,
            Err(error) => {
                let error = if named_command {
                    CliError::new(
                        "usage",
                        "batch 请求是一条命令行：{\"id\":\"1\",\"argv\":[\"ls\",\"/\"]}；没有 command 与 args 字段",
                    )
                } else {
                    error.into()
                };
                responses.publish(id, Err(error))?;
                continue;
            }
        };
        let validation = if request.id.is_empty() || request_ids.contains(&request.id) {
            Err(CliError::invalid("请求 ID 必须非空且唯一"))
        } else if request_ids.len() >= MAX_REQUESTS {
            Err(CliError::invalid("一次 batch 最多 10000 个请求"))
        } else {
            request_ids.insert(request.id.clone());
            Ok(())
        };
        if let Err(error) = validation {
            responses.publish(Some(request.id), Err(error))?;
            continue;
        }
        let mut argv = Value::Array(request.argv);
        while pending_ref(&argv, &responses.pending) {
            responses.finish_one(&mut tasks).await?;
        }
        let prepared = resolve_refs(&mut argv, &responses.results)
            .and_then(|()| match argv {
                Value::Array(argv) => words(argv),
                _ => Err(CliError::invalid("argv 必须是数组")),
            })
            .and_then(|words| prepare(request.id.clone(), &words, request.output));
        let prepared = match prepared {
            Ok(Ok(prepared)) => prepared,
            Ok(Err(answer)) => {
                responses.publish(Some(request.id), Ok(answer))?;
                continue;
            }
            Err(error) => {
                responses.publish(Some(request.id), Err(error))?;
                continue;
            }
        };
        let parallel = jobs > 1 && prepared.command.meta().concurrent;
        while (!parallel && !tasks.is_empty()) || tasks.len() >= jobs {
            responses.finish_one(&mut tasks).await?;
        }
        let id = prepared.id.clone();
        let execute = execute.clone();
        tasks.spawn_blocking(move || {
            // Keep the request ID even when a handler panics.
            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| execute(prepared)))
                    .unwrap_or_else(|_| Err(super::internal_error("batch 请求的工作线程异常退出")));
            Completion { id, result }
        });
        if parallel {
            responses.pending.insert(request.id);
        } else {
            responses.finish_one(&mut tasks).await?;
        }
    }
    while !tasks.is_empty() {
        responses.finish_one(&mut tasks).await?;
    }
    Ok(responses.status())
}

fn save(path: &Path, output: &Output) -> Result<(), CliError> {
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    match output {
        Output::Json(json) => writeln!(file, "{}", output::plain_json(json))?,
        Output::Bytes(bytes) => file.write_all(bytes)?,
        Output::Written | Output::Stream(_) => {}
    }
    file.as_file().sync_all()?;
    file.persist_noclobber(path)
        .map_err(|error| CliError::from(error.error))?;
    Ok(())
}

/// The response for a finished command; `fields` project a list result.
fn answer(
    command: &Command,
    output: Output,
    destination: Option<String>,
    fields: &[Vec<String>],
) -> Result<Answer, CliError> {
    let incomplete = incomplete::check(command.meta().incomplete, &output);
    let output = super::projection::apply(output, fields)?;
    let json = match destination {
        Some(path) => {
            save(Path::new(&path), &output)?;
            Json::from(json!({"output": path}))
        }
        None => match output {
            Output::Json(json) => json,
            Output::Bytes(bytes) => {
                Json::from(json!({"encoding": "hex", "data": hex::encode(bytes)}))
            }
            Output::Written | Output::Stream(_) => Json::from(Value::Null),
        },
    };
    Ok(Answer { json, incomplete })
}

fn read_line(reader: &mut impl BufRead) -> Result<Option<String>, CliError> {
    let mut text = String::new();
    let bytes = std::io::Read::take(&mut *reader, (MAX_LINE + 1) as u64).read_line(&mut text)?;
    if bytes > MAX_LINE {
        return Err(CliError::invalid("单行请求超过 16 MiB"));
    }
    Ok((bytes != 0).then_some(text))
}

/// Read stdin on its own thread: results flush even while the client keeps
/// its pipe open and waits before submitting another request.
fn spawn_reader(jobs: usize) -> Result<mpsc::Receiver<Result<String, CliError>>, CliError> {
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
    Ok(receiver)
}

/// Requests run in this process.
pub(super) async fn run(exec: Exec, jobs: usize) -> Result<i32, CliError> {
    let receiver = spawn_reader(jobs)?;
    let runtime = tokio::runtime::Handle::current();
    run_lines(receiver, jobs, std::io::stdout(), move |prepared| {
        let Prepared {
            command,
            options,
            output,
            fields,
            ..
        } = prepared;
        let result: Result<Output, CliError> = runtime.block_on(async {
            let executed = exec.run(command.clone(), &options).await?;
            match executed.output {
                Output::Stream(plan) => {
                    Ok(Output::Bytes(super::stream::buffer(&exec, plan).await?))
                }
                other => Ok(other),
            }
        });
        answer(&command, result?, output, &fields)
    })
    .await
}

/// Requests sent to the case backend of `case_dir`.
pub(super) fn run_client(case_dir: String, jobs: usize) -> Result<i32, CliError> {
    super::runtime::runtime()?.block_on(async move {
        let receiver = spawn_reader(jobs)?;
        run_lines(receiver, jobs, std::io::stdout(), move |prepared| {
            let mut bytes = super::stream::Bounded::default();
            let result = super::backend::forward_into(
                &case_dir,
                &prepared.command,
                prepared.options,
                None,
                &mut bytes,
            )?;
            let result = match result {
                Output::Written
                    if prepared.command.meta().output == super::cmd::OutputKind::Bytes =>
                {
                    Output::Bytes(bytes.into_inner())
                }
                other => other,
            };
            answer(&prepared.command, result, prepared.output, &prepared.fields)
        })
        .await
    })
}

#[cfg(test)]
mod tests;
