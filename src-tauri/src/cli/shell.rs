use std::{io::Write, path::Path, sync::Arc};

use clap::ArgMatches;
use serde_json::{json, Map, Value};
use tauri::Manager;

use super::{catalog, generated, input, CliError, Output};
use crate::{
    core::{
        filesystem::evidence_session::EvidenceSession,
        filesystem::tree::fs_tree::{FsSession, NodeKind},
    },
    state::AppState,
};

#[derive(Clone, Default)]
pub(super) struct Shell {
    pub image_id: Option<String>,
    // Components store both names and opaque backend node IDs; IDs are never
    // reconstructed from user paths and symlinks are not followed implicitly.
    location: Vec<(String, String)>,
}

impl Shell {
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

    async fn image(&self, app: &tauri::AppHandle) -> Result<Arc<EvidenceSession>, CliError> {
        let id = self
            .image_id
            .as_ref()
            .ok_or_else(|| CliError::invalid("请先 open <检材路径>，或使用 --image"))?;
        app.state::<AppState>()
            .images
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| CliError::invalid("当前检材已卸载；请重新 open 或 use"))
    }

    pub async fn invoke(
        &mut self,
        app: &tauri::AppHandle,
        command: &str,
        mut args: Map<String, Value>,
    ) -> Result<Output, CliError> {
        let command = command.replace('-', "_");
        let catalog = catalog::commands()?;
        let descriptor = catalog
            .iter()
            .find(|entry| entry.name == command)
            .ok_or_else(|| CliError::invalid(format!("未知命令: {command}")))?;
        if descriptor
            .parameters
            .iter()
            .any(|parameter| parameter.name == "imageId")
            && !args.contains_key("imageId")
        {
            if let Some(id) = &self.image_id {
                args.insert("imageId".into(), json!(id));
            }
        }
        let result = generated::dispatch(app, &command, args).await?;
        if command == "image_load" {
            if let Output::Json(value) = &result {
                self.image_id = value.get("id").and_then(Value::as_str).map(str::to_owned);
                self.location.clear();
            }
        }
        if matches!(command.as_str(), "case_close" | "case_open" | "case_create") {
            self.image_id = None;
            self.location.clear();
        }
        Ok(result)
    }

    pub async fn execute(
        &mut self,
        app: &tauri::AppHandle,
        matches: &ArgMatches,
    ) -> Result<Option<Output>, CliError> {
        let Some((name, args)) = matches.subcommand() else {
            return Ok(None);
        };
        let string = |key: &str| args.get_one::<String>(key).cloned().unwrap_or_default();
        match name {
            "capabilities" => return super::runtime::capabilities().map(Some),
            "commands" => {
                let commands = catalog::commands()?;
                let value = match args.get_one::<String>("name") {
                    Some(name) => serde_json::to_value(
                        commands
                            .iter()
                            .find(|command| command.name == name.replace('-', "_"))
                            .ok_or_else(|| CliError::invalid("未知命令"))?,
                    )?,
                    None => serde_json::to_value(commands)?,
                };
                return Ok(Some(Output::Json(value)));
            }
            "call" => {
                let raw = match args.get_one::<String>("args-file") {
                    Some(path) => std::fs::read_to_string(path)?,
                    None => args
                        .get_one::<String>("args")
                        .cloned()
                        .unwrap_or_else(|| "{}".into()),
                };
                let output = self
                    .invoke(app, &string("command"), input::object(&raw)?)
                    .await?;
                if let Some(path) = args.get_one::<String>("output") {
                    super::workflow::save_output(Path::new(path), output)?;
                    return Ok(None);
                }
                return Ok(Some(output));
            }
            "open" => {
                return self
                    .invoke(
                        app,
                        "image_load",
                        input::object(&json!({"path":string("path")}).to_string())?,
                    )
                    .await
                    .map(Some)
            }
            "use" => {
                let id = string("image-id");
                if !app
                    .state::<AppState>()
                    .images
                    .lock()
                    .await
                    .contains_key(&id)
                {
                    return Err(CliError::invalid("检材 ID 未加载"));
                }
                self.image_id = Some(id);
                self.location.clear();
                return Ok(None);
            }
            "pwd" => return Ok(Some(Output::Json(json!(self.pwd())))),
            "list-devices" => {
                return self
                    .invoke(app, "physical_disks_list", Map::new())
                    .await
                    .map(Some)
            }
            "tasks" => return self.invoke(app, "task_list", Map::new()).await.map(Some),
            "wait" => {
                super::workflow::wait_idle(app).await?;
                return Ok(None);
            }
            _ => {}
        }
        if let Some(descriptor) = catalog::commands()?
            .into_iter()
            .find(|entry| entry.name.replace('_', "-") == name || entry.name == name)
        {
            let mut parameters = Map::new();
            for parameter in descriptor.parameters {
                if let Some(raw) = args.get_one::<String>(&parameter.name) {
                    parameters.insert(parameter.name, input::typed_value(raw, &parameter.r#type)?);
                }
            }
            return self
                .invoke(app, &descriptor.name, parameters)
                .await
                .map(Some);
        }
        let image = self.image(app).await?;
        if !image.is_index_only() {
            image
                .validate_source_file_stable()
                .map_err(super::internal_error)?;
        } else if !matches!(
            name,
            "info"
                | "list-partitions"
                | "select-partition"
                | "cd"
                | "ls"
                | "tree"
                | "find"
                | "stat"
        ) {
            image.require_source().map_err(super::internal_error)?;
        }
        if name == "info" {
            return Ok(Some(Output::Json(serde_json::to_value(image.info())?)));
        }
        if name == "list-partitions" {
            return Ok(Some(Output::Json(serde_json::to_value(&image.partitions)?)));
        }
        if name == "select-partition" {
            let id = format!("part:{}", string("id"));
            let root = crate::commands::filesystem::filesystem::fs_children(
                app.state(),
                self.image_id
                    .clone()
                    .ok_or_else(|| CliError::invalid("没有当前检材"))?,
                String::new(),
            )
            .await
            .map_err(super::command_error)?
            .into_iter()
            .find(|node| node.id == id)
            .ok_or_else(|| CliError::invalid("找不到分区 ID"))?;
            self.location = vec![(root.name, root.id)];
            return Ok(None);
        }
        let image_id = self
            .image_id
            .clone()
            .ok_or_else(|| CliError::invalid("没有当前检材"))?;
        let mut page = |parent_id, cursor, page_size| {
            let image_id = image_id.clone();
            async move {
                crate::commands::filesystem::filesystem::fs_children_page(
                    app.state(),
                    image_id,
                    parent_id,
                    cursor,
                    Some(page_size),
                    None,
                )
                .await
                .map_err(super::command_error)
            }
        };
        let location = super::browsing::resolve(&self.location, &string("path"), &mut page).await?;
        let node_id = location.last().map(|(_, id)| id.as_str()).unwrap_or("");
        match name {
            "cd" => {
                if let Some(node) = super::browsing::node(&location, &mut page).await? {
                    if node.kind != NodeKind::Dir {
                        return Err(CliError::invalid("cd 的目标必须是目录；不会跟随符号链接"));
                    }
                }
                page(node_id.to_owned(), None, 1).await?;
                self.location = location;
                Ok(None)
            }
            "ls" | "tree" | "find" => {
                let state = app.state::<AppState>();
                let task = state.tasks.register(
                    format!("cli-{name}-{}", uuid::Uuid::new_v4()),
                    crate::task_manager::TaskKind::FlatList,
                    Some(image_id.clone()),
                );
                let _resource_permit = state
                    .tasks
                    .acquire_resources(&task)
                    .await
                    .map_err(crate::app::error::AppError::from)
                    .map_err(super::command_error)?;
                let cancellation = task.token();
                let limit = *args.get_one::<u32>("limit").unwrap_or(&10000) as usize;
                let depth = if name == "ls" {
                    1
                } else {
                    *args.get_one::<u32>("depth").unwrap_or(&32)
                };
                let filter = args
                    .get_one::<String>("name")
                    .map(|name| name.to_lowercase());
                let (items, truncated) = super::browsing::walk(
                    node_id,
                    depth,
                    limit,
                    filter.as_deref(),
                    &cancellation,
                    &mut page,
                )
                .await?;
                Ok(Some(Output::Json(
                    json!({"items":items,"truncated":truncated}),
                )))
            }
            "grep" => super::search::grep(app, &image_id, node_id, args)
                .await
                .map(Some),
            "strings" => super::strings::read(app, image, node_id.to_owned(), args)
                .await
                .map(Some),
            "stat" => {
                let node = super::browsing::node(&location, &mut page).await?;
                let mut value = match node {
                    Some(node) => serde_json::to_value(node)?,
                    None => {
                        json!({"id":"", "name":"/", "kind":"dir", "size":image.len(), "deleted":false})
                    }
                };
                value["node_id"] = json!(node_id);
                value["path"] = json!(format!(
                    "/{}",
                    location
                        .iter()
                        .map(|(name, _)| name.as_str())
                        .collect::<Vec<_>>()
                        .join("/")
                ));
                value["index_only"] = json!(image.is_index_only());
                Ok(Some(Output::Json(value)))
            }
            "cat" | "hex" => {
                let mut session = FsSession::new(&image);
                let offset = *args.get_one::<u64>("offset").unwrap_or(&0);
                let length = *args.get_one::<u32>("length").unwrap_or(&4096) as usize;
                let bytes = session
                    .read_file_range(node_id, offset, length)
                    .map_err(super::internal_error)?;
                let total = session
                    .file_stat(node_id)
                    .map_err(super::internal_error)?
                    .size;
                let output = if name == "hex" {
                    hex::encode(&bytes)
                } else {
                    String::from_utf8_lossy(&bytes).into_owned()
                };
                Ok(Some(Output::Json(
                    json!({"offset":offset,"bytes_read":bytes.len(),"size":total,"truncated":offset.saturating_add(bytes.len() as u64)<total,"data":output,"encoding":if name == "hex" {"hex"} else {"utf-8"}}),
                )))
            }
            "export" => {
                let mut session = FsSession::new(&image);
                let destination = string("destination");
                let path = Path::new(&destination);
                let parent = path
                    .parent()
                    .filter(|path| !path.as_os_str().is_empty())
                    .unwrap_or(Path::new("."));
                let mut output = tempfile::NamedTempFile::new_in(parent)?;
                let expected = session
                    .file_stat(node_id)
                    .map_err(super::internal_error)?
                    .size;
                let (result, faults) = image.collect_read_faults(|| {
                    session.stream_file(node_id, 1024 * 1024, |chunk| {
                        output.write_all(chunk)?;
                        Ok(())
                    })
                });
                let bytes = result.map_err(super::internal_error)?;
                if bytes != expected {
                    return Err(CliError::invalid(format!(
                        "文件未完整读取：期望 {expected} 字节，实际 {bytes} 字节"
                    )));
                }
                image
                    .validate_source_file_stable()
                    .map_err(super::internal_error)?;
                output.as_file().sync_all()?;
                output
                    .persist_noclobber(path)
                    .map_err(|error| CliError::from(error.error))?;
                Ok(Some(Output::Json(
                    json!({"bytes_written":bytes,"destination":destination,"read_faults":faults}),
                )))
            }
            "file" | "hash" | "md5sum" | "sha1sum" | "sha256sum" => {
                let mut params = Map::from_iter([("nodeId".into(), json!(node_id))]);
                let command = match name {
                    "file" => "fs_identify_type",
                    "hash" | "md5sum" | "sha1sum" | "sha256sum" => {
                        params.insert(
                            "algos".into(),
                            if name == "hash" {
                                json!(["md5", "sha1", "sha256", "sm3"])
                            } else {
                                json!([name.trim_end_matches("sum")])
                            },
                        );
                        "fs_digest"
                    }
                    _ => return Err(CliError::invalid("未知文件命令")),
                };
                self.invoke(app, command, params).await.map(Some)
            }
            _ => Err(CliError::invalid(format!("未知命令: {name}"))),
        }
    }
}
