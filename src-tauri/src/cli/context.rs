//! Running one command: the session context it reads and updates, and the
//! steps every command goes through, in order: dry run, confirmation, values
//! and prompts, scope, then the call.
use std::{io::IsTerminal, path::Path, sync::Arc};

use serde::Serialize;
use serde_json::{json, Map, Value};

use super::{
    browsing,
    catalog::{Command, Kind},
    generated, native, output,
    progress::Reporter,
    prompt, CliError, Json, Output,
};
use crate::{commands::filesystem::filesystem_page::FsNodePage, host::AppHandle, state::AppState};

/// How a command reaches the user: a single invocation, the JSONL session,
/// or the interactive shell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    OneShot,
    Session,
    Shell,
}

/// What persists between commands in one process.
#[derive(Clone, Default, Debug)]
pub struct Context {
    /// The evidence scope as given (`--image`, `LOVELYCARVE_IMAGE`), opened on
    /// first use.
    pub scope: Option<String>,
    /// Where the scope came from when it was not `--image`, named in the
    /// notice printed when it is opened.
    pub scope_source: Option<String>,
    /// The current image.
    pub image_id: Option<String>,
    /// The working directory inside the image: names with their node IDs.
    pub location: Vec<(String, String)>,
}

impl Context {
    pub fn pwd(&self) -> String {
        format!(
            "/{}",
            self.location
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>()
                .join("/")
        )
    }
}

/// One command to run, with values keyed by API parameter name.
pub struct Request<'a> {
    pub command: &'a Command,
    pub values: Map<String, Value>,
    pub ask: Vec<String>,
    pub force: bool,
    pub dry_run: bool,
    pub text: bool,
}

/// A path inside the image, resolved to its node.
pub struct Target {
    pub node_id: String,
    /// The node's ancestors and itself, when the path named them.
    pub location: Option<Vec<(String, String)>>,
}

#[derive(Clone)]
pub struct Exec {
    pub host: AppHandle,
    pub state: Arc<AppState>,
    pub mode: Mode,
    pub quiet: bool,
    pub reporter: Reporter,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Preview<'a> {
    command: &'a str,
    ipc_command: &'a str,
    body: Value,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArgumentsOnly<'a> {
    command: &'a str,
    body: Value,
    dry_run: bool,
    executed: bool,
    validated: &'static str,
}

/// What `--dry-run` prints: the backend request, without running it or
/// touching evidence. Sensitive values are redacted.
pub fn preview(
    command: &Command,
    values: &Map<String, Value>,
    scope: Option<&str>,
) -> Result<Json, CliError> {
    let mut body = values.clone();
    for param in &command.params {
        if let Some(fixed) = &param.fixed {
            body.insert(param.name.clone(), fixed.clone());
        } else if let Some(default) = &param.default {
            body.entry(param.name.clone())
                .or_insert_with(|| default.clone());
        }
        if param.scope && !body.contains_key(&param.name) {
            body.insert(param.name.clone(), json!(scope.unwrap_or("<image>")));
        }
    }
    let mut body = Value::Object(body);
    for path in &command.sensitive_paths {
        redact(&mut body, path);
    }
    let Some(ipc_command) = command.ipc_command.as_deref() else {
        return Json::serialize(&ArgumentsOnly {
            command: &command.command,
            body,
            dry_run: true,
            executed: false,
            validated: "arguments only",
        });
    };
    Json::serialize(&Preview {
        command: &command.command,
        ipc_command,
        body,
    })
}

fn redact(value: &mut Value, path: &[String]) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    match value {
        Value::Array(items) => {
            for item in items {
                redact(item, path);
            }
        }
        Value::Object(map) => {
            if let Some(child) = map.get_mut(first) {
                if rest.is_empty() {
                    *child = json!("<redacted>");
                } else {
                    redact(child, rest);
                }
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn generated_id(command: &Command) -> String {
    format!(
        "cli-{}-{}",
        command.path().replace(' ', "-"),
        uuid::Uuid::new_v4()
    )
}

fn missing(value: Option<&Value>) -> bool {
    value.is_none_or(Value::is_null)
}

/// Every way to give a command its evidence, as cf lists the ways to give a
/// zone.
fn no_image(command: &Command, mode: Mode) -> CliError {
    let mut ways = Vec::new();
    if command
        .params
        .iter()
        .any(|param| param.scope && param.position.is_some())
    {
        ways.push(format!("位置参数：{} <检材路径或 ID>", command.command));
    }
    ways.push(format!(
        "--image（或 -i）：{} --image <检材路径或 ID>",
        command.command
    ));
    ways.push("环境变量 LOVELYCARVE_IMAGE（也可写在当前目录的 .env 或 .env.local）".to_owned());
    if mode != Mode::OneShot {
        ways.push("在当前会话中先运行 images open <检材路径>".to_owned());
    }
    let ways: Vec<String> = ways
        .iter()
        .enumerate()
        .map(|(index, way)| format!("  {}. {way}", index + 1))
        .collect();
    CliError::new(
        "noImage",
        format!("未指定检材。\n\n请提供以下任意一项：\n{}", ways.join("\n")),
    )
}

impl Exec {
    /// Whether missing values may be asked for (or, for secrets, read from
    /// piped stdin). A shell script on stdin is the shell's input, not data.
    pub fn interactive(&self) -> bool {
        match self.mode {
            Mode::OneShot => true,
            Mode::Shell => std::io::stdin().is_terminal(),
            Mode::Session => false,
        }
    }

    /// Load the image `reference` names unless it is already loaded, and
    /// return its ID. An image opened this way becomes the current image.
    pub async fn resolve_image(
        &self,
        context: &mut Context,
        reference: &str,
    ) -> Result<String, CliError> {
        if self.state.images.lock().await.contains_key(reference) {
            return Ok(reference.to_owned());
        }
        let args = Map::from_iter([("path".to_owned(), json!(reference))]);
        let output = generated::dispatch(&self.host, &self.state, "image_load", args).await?;
        let Output::Json(info) = output else {
            return Err(super::internal_error("images open 返回了非 JSON 结果"));
        };
        let id = info.value()["id"]
            .as_str()
            .ok_or_else(|| super::internal_error("images open 没有返回检材 ID"))?
            .to_owned();
        context.image_id = Some(id.clone());
        context.location.clear();
        Ok(id)
    }

    /// The current image, opening the scope on first use. A scope from the
    /// environment is announced when it is opened, unless quiet.
    pub async fn current_image(&self, context: &mut Context) -> Result<Option<String>, CliError> {
        if let Some(id) = &context.image_id {
            if self.state.images.lock().await.contains_key(id) {
                return Ok(Some(id.clone()));
            }
            context.image_id = None;
            context.location.clear();
        }
        let Some(scope) = context.scope.take() else {
            return Ok(None);
        };
        if let Some(source) = context.scope_source.take() {
            if !self.quiet && self.mode != Mode::Session {
                eprintln!("使用检材：{}（来自 {source}）", output::sanitize(&scope));
            }
        }
        self.resolve_image(context, &scope).await.map(Some)
    }

    pub async fn page(
        &self,
        image_id: &str,
        parent_id: String,
        cursor: Option<String>,
        page_size: usize,
    ) -> Result<FsNodePage, CliError> {
        crate::commands::filesystem::filesystem::fs_children_page(
            crate::host::State::new(&self.state),
            image_id.to_owned(),
            parent_id,
            cursor,
            Some(page_size),
            None,
        )
        .await
        .map_err(|error| super::command_error(error).in_operation("fs_children_page"))
    }

    /// Resolve `path` against the working directory.
    pub async fn locate(
        &self,
        context: &Context,
        image_id: &str,
        path: &str,
    ) -> Result<Vec<(String, String)>, CliError> {
        let mut page =
            |parent_id, cursor, page_size| self.page(image_id, parent_id, cursor, page_size);
        browsing::resolve(&context.location, path, &mut page).await
    }

    pub async fn execute(
        &self,
        context: &mut Context,
        request: Request<'_>,
    ) -> Result<Output, CliError> {
        let command = request.command;
        let mut values = request.values;
        if request.dry_run {
            return Ok(Output::Json(preview(
                command,
                &values,
                context.image_id.as_deref().or(context.scope.as_deref()),
            )?));
        }
        if command.destructive {
            if self.mode == Mode::Session {
                if !request.force {
                    return Err(CliError::new(
                        "aborted",
                        format!(
                            "{} 会删除数据；确认后在请求中加 \"force\": true",
                            command.command
                        ),
                    ));
                }
            } else if !prompt::confirm_delete(request.force, command.confirm.as_deref())? {
                eprintln!("Aborted.");
                return Ok(Output::Written);
            }
        }
        for param in &command.params {
            if let Some(fixed) = &param.fixed {
                values.insert(param.name.clone(), fixed.clone());
            } else if missing(values.get(&param.name)) {
                if let Some(default) = &param.default {
                    if !param.synthetic {
                        values.insert(param.name.clone(), default.clone());
                    }
                } else if param.generated {
                    values.insert(param.name.clone(), json!(generated_id(command)));
                }
            }
        }
        for name in &request.ask {
            let param = command
                .param(name)
                .ok_or_else(|| CliError::invalid(format!("未知参数：{name}")))?;
            let value = if self.interactive() {
                prompt::required_field(&param.option, &param.description, true)?
            } else {
                return Err(CliError::invalid(format!("缺少参数：{name}")));
            };
            values.insert(name.clone(), json!(value));
        }
        // A path is one of two ways to name the target node; `target` below
        // reports when neither was given. A subject `--body` should have
        // supplied is reported, not asked for.
        for param in &command.params {
            if !param.required || param.synthetic || !missing(values.get(&param.name)) {
                continue;
            }
            if param.position.is_some() {
                return Err(CliError::invalid(match self.mode {
                    Mode::Session => format!("缺少参数：{}", param.name),
                    Mode::OneShot | Mode::Shell => format!(
                        "缺少参数：<{}>（写在命令行上，或在 --body 中提供 {}）",
                        param.option, param.name
                    ),
                }));
            }
            if !self.interactive() {
                return Err(CliError::invalid(format!("缺少参数：{}", param.name)));
            }
            let value = match (&param.values, param.kind) {
                (Some(choices), Kind::Enum) => {
                    json!(prompt::required_choice(
                        &param.option,
                        &param.description,
                        choices
                    )?)
                }
                (_, Kind::String) => {
                    json!(prompt::required_field(
                        &param.option,
                        &param.description,
                        param.secret
                    )?)
                }
                (_, Kind::Number) => {
                    let raw = prompt::required_field(&param.option, &param.description, false)?;
                    raw.parse::<serde_json::Number>()
                        .map(Value::Number)
                        .map_err(|_| {
                            CliError::invalid(format!("--{} 需要数字：{raw}", param.option))
                        })?
                }
                _ => {
                    return Err(CliError::invalid(format!(
                        "缺少 --{0}（或在 --body 中提供 {1}）",
                        param.option, param.name
                    )))
                }
            };
            values.insert(param.name.clone(), value);
        }
        let mut monitor =
            (self.mode != Mode::Session).then(|| self.reporter.start(&command.path()));
        let outcome = self
            .execute_prepared(context, command, values, request.text)
            .await;
        if let Some(monitor) = &mut monitor {
            monitor.finish(&outcome);
        }
        outcome
    }

    async fn execute_prepared(
        &self,
        context: &mut Context,
        command: &Command,
        mut values: Map<String, Value>,
        text: bool,
    ) -> Result<Output, CliError> {
        let mut target = None;
        // RAID operates on all loaded images and has no imageId parameter,
        // but an explicitly configured initial evidence must still be opened.
        if command.ipc_command.as_deref() == Some("raid_auto_detect") {
            self.current_image(context).await?;
        }
        let image = command.params.iter().find(|param| param.scope);
        if image.is_some() || command.target.is_some() {
            let given = image
                .and_then(|image| values.get(&image.name))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let id = match given {
                Some(reference) => Some(self.resolve_image(context, &reference).await?),
                None => self.current_image(context).await?,
            };
            match id {
                Some(id) => {
                    values.insert("imageId".into(), json!(id));
                }
                None if image.is_some_and(|image| image.api_required)
                    || command.target.is_some() =>
                {
                    return Err(no_image(command, self.mode))
                }
                None => {}
            }
        }
        if let Some(name) = &command.target {
            target = Some(self.target(context, command, name, &mut values).await?);
        }
        for name in &command.no_clobber {
            if let Some(path) = values.get(name).and_then(Value::as_str) {
                if Path::new(path).exists() {
                    return Err(CliError::new(
                        "alreadyExists",
                        format!("{path} 已存在；为避免覆盖，请换一个输出路径"),
                    ));
                }
            }
        }
        let output = match command.ipc_command.as_deref() {
            Some(ipc) => generated::dispatch(&self.host, &self.state, ipc, values.clone()).await?,
            None => native::run(self, context, command, values.clone(), target, text).await?,
        };
        self.after(context, command, &values, &output);
        Ok(output)
    }

    /// Resolve the command's node parameter from `path` when it was not given.
    pub(super) async fn target(
        &self,
        context: &mut Context,
        command: &Command,
        name: &str,
        values: &mut Map<String, Value>,
    ) -> Result<Target, CliError> {
        let path = values
            .remove("path")
            .and_then(|value| value.as_str().map(str::to_owned))
            .or_else(|| {
                command
                    .param("path")
                    .and_then(|param| param.default.as_ref())
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            });
        if let Some(node_id) = values.get(name).and_then(Value::as_str).map(str::to_owned) {
            if command.ipc_command.is_none() {
                values.remove(name);
            }
            return Ok(Target {
                node_id,
                location: None,
            });
        }
        let Some(path) = path else {
            if command.param(name).is_some_and(|param| param.api_required)
                || command.ipc_command.is_none()
            {
                return Err(CliError::invalid(format!(
                    "需要镜像内路径 <path>，或用 --{} 指定节点 ID",
                    command
                        .param(name)
                        .map_or("node-id", |param| param.option.as_str())
                )));
            }
            return Ok(Target {
                node_id: String::new(),
                location: None,
            });
        };
        let image_id = values
            .get("imageId")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| CliError::new("noImage", "解析镜像内路径需要检材"))?;
        let location = self.locate(context, &image_id, &path).await?;
        let node_id = location
            .last()
            .map(|(_, id)| id.clone())
            .unwrap_or_default();
        if command.ipc_command.is_some() {
            values.insert(name.to_owned(), json!(node_id));
            if command.param("pathHint").is_some() && missing(values.get("pathHint")) {
                values.insert("pathHint".into(), json!(path));
            }
            if command.param("fileName").is_some() && missing(values.get("fileName")) {
                if let Some((file_name, _)) = location.last() {
                    values.insert("fileName".into(), json!(file_name));
                }
            }
        } else {
            values.insert("path".into(), json!(path));
        }
        Ok(Target {
            node_id,
            location: Some(location),
        })
    }

    /// Keep the context in step with the image and case the command changed.
    fn after(
        &self,
        context: &mut Context,
        command: &Command,
        values: &Map<String, Value>,
        output: &Output,
    ) {
        match command.ipc_command.as_deref() {
            Some("image_load") => {
                if let Output::Json(info) = output {
                    context.image_id = info.value()["id"].as_str().map(str::to_owned);
                    context.location.clear();
                }
            }
            Some("case_open" | "case_create" | "case_close") => {
                context.image_id = None;
                context.scope = None;
                context.scope_source = None;
                context.location.clear();
            }
            Some("image_unload")
                if values.get("imageId").and_then(Value::as_str) == context.image_id.as_deref() =>
            {
                context.image_id = None;
                context.location.clear();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::catalog::catalog;

    #[test]
    fn dry_run_shows_the_request_and_redacts_secrets() {
        let catalog = catalog().unwrap();
        let unlock = catalog.find(&["bitlocker", "unlock"]).unwrap();
        let values = Map::from_iter([
            ("partitionId".to_owned(), json!(1)),
            ("password".to_owned(), json!("hunter2")),
        ]);
        let shown = preview(unlock, &values, Some(r"D:\disk.E01")).unwrap();
        assert_eq!(
            shown.value(),
            &json!({
                "command": "lovelycarve-cli bitlocker unlock",
                "ipcCommand": "bitlocker_unlock",
                "body": {"partitionId": 1, "password": "<redacted>", "imageId": r"D:\disk.E01"},
            })
        );
        assert!(shown
            .text()
            .starts_with(r#"{"command":"lovelycarve-cli bitlocker unlock","ipcCommand":"#));
        let volumes = catalog.find(&["encrypted-volumes", "unlock"]).unwrap();
        let values = Map::from_iter([(
            "options".to_owned(),
            json!({"scheme": "luks", "password": "secret"}),
        )]);
        assert_eq!(
            preview(volumes, &values, None).unwrap().value()["body"],
            json!({"options": {"scheme": "luks", "password": "<redacted>"}, "imageId": "<image>"})
        );
        let docker = catalog.find(&["files", "docker", "get"]).unwrap();
        assert_eq!(
            preview(docker, &Map::new(), Some("x")).unwrap().value()["body"]["mode"],
            "status"
        );
        let list = catalog.find(&["files", "tree", "list"]).unwrap();
        assert_eq!(
            preview(list, &Map::new(), None).unwrap().value()["executed"],
            false
        );
    }

    #[test]
    fn a_missing_image_lists_every_way_to_give_one() {
        let catalog = catalog().unwrap();
        let get = catalog.find(&["images", "get"]).unwrap();
        let error = no_image(get, Mode::OneShot);
        assert_eq!(error.code, "noImage");
        assert_eq!(
            error.message,
            "未指定检材。\n\n请提供以下任意一项：\n  1. 位置参数：lovelycarve-cli images get <检材路径或 ID>\n  2. --image（或 -i）：lovelycarve-cli images get --image <检材路径或 ID>\n  3. 环境变量 LOVELYCARVE_IMAGE（也可写在当前目录的 .env 或 .env.local）"
        );
        let read = catalog.find(&["files", "read"]).unwrap();
        let shell = no_image(read, Mode::Shell).message;
        assert!(!shell.contains("位置参数") && shell.contains("3. 在当前会话中先运行 images open"));
    }
}
