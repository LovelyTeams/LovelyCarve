//! `tasks` (the task center), `index` (volume indexes) and `mount` /
//! `umount` (read-only mounts and SMB shares on Windows).
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    app::workflows::task_center,
    cli::{
        browsing::{volume_root, LastLink},
        exec::Exec,
        render,
        types::{parse_address, Address},
        workspace::{Evidence, Target},
        CliError, Output,
    },
    core::{common::ids::VolumeKey, filesystem::fs_all::ROOT_VOLUME_KEY},
};

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TasksArgs {
    #[command(subcommand)]
    pub command: Option<TasksCommand>,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TasksCommand {
    /// 列出任务（默认）：插件任务与本进程的内置任务
    Ls(TaskList),
    /// 查看一个任务
    Show(TaskId),
    /// 等待后台任务与插件任务结束；有任务失败时以 tasksFailed 失败
    Wait,
    /// 取消运行中的任务
    Cancel(TaskId),
    /// 从头重新运行插件任务
    Restart(TaskId),
    /// 删除插件任务及其发布的结果
    Rm(TaskId),
}

#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskList {
    /// 只列这份检材的任务
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskId {
    /// 任务 ID（tasks ls 返回；插件任务形如 plugin:12）
    #[arg(value_name = "任务ID")]
    pub task_id: String,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexArgs {
    #[command(subcommand)]
    pub command: Option<IndexCommand>,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IndexCommand {
    /// 列出案件中的卷索引（默认）
    Ls,
    /// 构建卷索引（后台运行；用 tasks wait 等待）
    Build(VolumeArg),
    /// 删除卷索引
    Rm(VolumeArg),
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeArg {
    /// 卷（/<别名>/part:0 等，见 volumes）
    #[arg(value_parser = parse_address, value_name = "卷")]
    pub volume: Address,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MountArgs {
    /// 要挂载的卷或目录；省略时列出挂载
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Option<Address>,
    /// 挂载点（盘符如 X: 或空目录）；默认自动选择
    #[arg(value_name = "挂载点")]
    pub mount_point: Option<String>,
    /// 同时以这个名称共享为 SMB
    #[arg(long, value_name = "共享名")]
    pub share: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UmountArgs {
    /// 挂载 ID（mount 列出）
    #[arg(value_name = "挂载ID")]
    pub mount_id: String,
}

pub fn tasks_path(args: &TasksArgs) -> &'static str {
    match &args.command {
        None | Some(TasksCommand::Ls(_)) => "tasks ls",
        Some(TasksCommand::Show(_)) => "tasks show",
        Some(TasksCommand::Wait) => "tasks wait",
        Some(TasksCommand::Cancel(_)) => "tasks cancel",
        Some(TasksCommand::Restart(_)) => "tasks restart",
        Some(TasksCommand::Rm(_)) => "tasks rm",
    }
}

pub fn index_path(args: &IndexArgs) -> &'static str {
    match &args.command {
        None | Some(IndexCommand::Ls) => "index ls",
        Some(IndexCommand::Build(_)) => "index build",
        Some(IndexCommand::Rm(_)) => "index rm",
    }
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

/// The number of a plugin task ID (`plugin:12`).
fn plugin_task(id: &str) -> Result<i64, CliError> {
    id.strip_prefix("plugin:")
        .unwrap_or(id)
        .parse()
        .map_err(|_| {
            CliError::new(
                "usage",
                format!("{id} 不是插件任务；插件任务 ID 形如 plugin:12"),
            )
        })
}

pub async fn tasks(exec: &Exec, args: TasksArgs) -> Result<Output, CliError> {
    use crate::commands::extensions::task_scheduler as scheduler;
    match args
        .command
        .unwrap_or(TasksCommand::Ls(TaskList::default()))
    {
        TasksCommand::Ls(list) => {
            let image_id = match list.evidence.as_deref() {
                Some(alias) => Some(exec.resolve(alias.trim_start_matches('/')).await?.image_id),
                None => None,
            };
            let tasks = crate::commands::system::system::task_list(exec.state(), image_id)
                .await
                .map_err(operation("task_list"))?;
            Ok(Output::json(
                json!({ "items": serde_json::to_value(crate::cli::wire::Camel(tasks))? }),
            ))
        }
        TasksCommand::Show(id) => crate::cli::json_output(
            crate::commands::system::system::task_status(exec.state(), id.task_id)
                .await
                .map_err(operation("task_status"))?,
        ),
        TasksCommand::Wait => {
            crate::cli::json_output(exec.background.wait(&exec.state).await?.verdict(None)?)
        }
        TasksCommand::Cancel(id) => cancel(exec, id.task_id).await,
        TasksCommand::Restart(id) => crate::cli::json_output(
            scheduler::sched_task_restart(exec.state(), plugin_task(&id.task_id)?)
                .await
                .map_err(operation("sched_task_restart"))?,
        ),
        TasksCommand::Rm(id) => crate::cli::json_output(
            scheduler::sched_task_remove(exec.state(), plugin_task(&id.task_id)?)
                .await
                .map_err(operation("sched_task_remove"))?,
        ),
    }
}

/// Only a running task can be cancelled; an unknown or ended task fails
/// instead of passing for a cancellation.
async fn cancel(exec: &Exec, id: String) -> Result<Output, CliError> {
    if id.starts_with("plugin:") {
        task_center::cancel(&exec.state, &id)
            .await
            .map_err(|error| crate::cli::command_error(error).in_operation("task_cancel"))?;
        return Ok(Output::json(json!({"id": id, "status": "cancelled"})));
    }
    if exec.state.tasks.cancel(&id) || exec.state.tasks.live(&id).is_some() {
        return Ok(Output::json(json!({"id": id, "status": "cancelling"})));
    }
    let ended = task_center::list(&exec.state, None)
        .await
        .map_err(|error| crate::cli::command_error(error).in_operation("task_list"))?
        .into_iter()
        .find(|entry| entry.id == id);
    Err(match ended {
        Some(entry) => CliError::new(
            "taskFinished",
            format!("任务已经结束（{}），无法取消：{id}", entry.status.as_str()),
        ),
        None => CliError::new(
            "notFound",
            match exec.mode {
                crate::cli::exec::Mode::OneShot => format!(
                    "找不到运行中的任务：{id}（没有案件后台进程时只看得到本进程的任务；用 case open 启动后台进程后再管理后台任务）"
                ),
                _ => format!("找不到运行中的任务：{id}"),
            },
        ),
    })
}

/// The volume an address names: the innermost volume root on its way; in a
/// folder or an archive, its one volume.
async fn volume_key(exec: &Exec, target: &Target) -> Result<VolumeKey, CliError> {
    if let Some((_, id)) = target.location.iter().rev().find(|(_, id)| volume_root(id)) {
        return Ok(VolumeKey::from(id.clone()));
    }
    let volumes = crate::commands::filesystem::filedb::filedb_volumes(
        exec.state(),
        target.image_id().clone(),
    )
    .await
    .map_err(operation("filedb_volumes"))?;
    if volumes
        .iter()
        .any(|volume| volume.volume_key == ROOT_VOLUME_KEY)
    {
        return Ok(VolumeKey::from(ROOT_VOLUME_KEY.to_owned()));
    }
    target
        .location
        .first()
        .map(|(_, id)| VolumeKey::from(id.clone()))
        .ok_or_else(|| {
            CliError::new(
                "usage",
                format!(
                    "{} 是检材根；需要其中的卷，例如 /{}/part:0",
                    target.address(),
                    target.evidence.alias
                ),
            )
        })
}

/// Where a volume's index is shown: the volume's address, the evidence
/// itself for a folder or an archive.
fn index_address(alias: &str, volume_key: &str) -> String {
    if volume_key == ROOT_VOLUME_KEY {
        format!("/{alias}")
    } else {
        format!("/{alias}/{volume_key}")
    }
}

/// The case's volume indexes, with the evidence alias in each address.
pub async fn index_rows(exec: &Exec, evidence: &[Evidence]) -> Result<Vec<Value>, CliError> {
    if exec.state.active_case.lock().await.is_none() {
        return Ok(Vec::new());
    }
    let list = crate::commands::casework::case_snapshot::snapshot_list(exec.state())
        .await
        .map_err(operation("snapshot_list"))?;
    let rows = match crate::cli::wire::flattened_output(&list)? {
        Output::Json(json) => json.value().as_array().cloned().unwrap_or_default(),
        _ => Vec::new(),
    };
    Ok(rows
        .into_iter()
        .map(|row| {
            let alias = evidence
                .iter()
                .find(|item| row["imageId"] == item.image_id.as_str())
                .map_or_else(|| render::cell(&row["imageId"]), |item| item.alias.clone());
            json!({
                "address": index_address(&alias, row["volumeKey"].as_str().unwrap_or_default()),
                "evidence": alias,
                "volumeKey": row["volumeKey"],
                "state": super::browse::index_state(&row),
                "fileCount": row["entryCount"],
                "builtAt": row["builtAt"],
                "truncated": row["truncated"],
                "stale": row["stale"],
            })
        })
        .collect())
}

pub async fn index(exec: &Exec, args: IndexArgs) -> Result<Output, CliError> {
    match args.command.unwrap_or(IndexCommand::Ls) {
        IndexCommand::Ls => {
            let evidence = exec.evidence().await?;
            Ok(Output::json(
                json!({ "items": index_rows(exec, &evidence).await? }),
            ))
        }
        IndexCommand::Build(volume) => {
            let target = exec.target(&volume.volume, LastLink::Follow).await?;
            let key = volume_key(exec, &target).await?;
            let task_id = super::task_id("index build");
            crate::commands::casework::case_snapshot::snapshot_build(
                exec.host.clone(),
                exec.state(),
                target.image_id().clone(),
                key.clone(),
                task_id.clone(),
            )
            .await
            .map_err(operation("snapshot_build"))?;
            Ok(Output::json(json!({
                "address": index_address(&target.evidence.alias, &key),
                "volumeKey": key,
                "taskId": task_id,
                "started": true,
            })))
        }
        IndexCommand::Rm(volume) => {
            let target = exec.target(&volume.volume, LastLink::Follow).await?;
            let key = volume_key(exec, &target).await?;
            crate::commands::casework::case_snapshot::snapshot_drop(
                exec.state(),
                target.image_id().clone(),
                key.clone(),
            )
            .await
            .map_err(operation("snapshot_drop"))?;
            Ok(Output::json(
                json!({"address": index_address(&target.evidence.alias, &key), "removed": true}),
            ))
        }
    }
}

pub async fn mount(exec: &Exec, args: MountArgs) -> Result<Output, CliError> {
    use crate::commands::system::mount as mounts;
    let Some(address) = args.path else {
        let list = mounts::fs_mounts(exec.state())
            .await
            .map_err(operation("fs_mounts"))?;
        return Ok(Output::json(
            json!({ "items": serde_json::to_value(crate::cli::wire::Camel(list))? }),
        ));
    };
    let target = exec.target(&address, LastLink::Follow).await?;
    let mount_point = match args.mount_point {
        Some(point) => point,
        None => mounts::fs_suggest_mount_point(exec.state())
            .await
            .map_err(operation("fs_suggest_mount_point"))?,
    };
    let view = mounts::fs_mount_readonly(
        exec.state(),
        target.image_id().clone(),
        target.node_id().into(),
        mount_point,
    )
    .await
    .map_err(operation("fs_mount_readonly"))?;
    let mut value = serde_json::to_value(crate::cli::wire::Camel(&view))?;
    value["path"] = json!(target.address());
    if let Some(name) = args.share {
        let share = mounts::fs_share_mount(exec.state(), view.id.clone(), Some(name))
            .await
            .map_err(operation("fs_share_mount"))?;
        value["share"] = serde_json::to_value(crate::cli::wire::Camel(share))?;
    }
    Ok(Output::json(value))
}

pub async fn umount(exec: &Exec, args: UmountArgs) -> Result<Output, CliError> {
    crate::commands::system::mount::fs_unmount_readonly(exec.state(), args.mount_id.clone())
        .await
        .map_err(operation("fs_unmount_readonly"))?;
    Ok(Output::json(
        json!({"mountId": args.mount_id, "unmounted": true}),
    ))
}

pub fn render_tasks(args: &TasksArgs, value: &Value) -> Option<String> {
    match &args.command {
        None | Some(TasksCommand::Ls(_)) => {
            let items = value["items"].as_array().map_or(&[][..], Vec::as_slice);
            if items.is_empty() {
                return Some("（没有任务）\n".to_owned());
            }
            let rows: Vec<Value> = items
                .iter()
                .map(|item| {
                    let progress = match (
                        item["progress"]["done"].as_u64(),
                        item["progress"]["total"].as_u64(),
                    ) {
                        (Some(done), Some(total)) if total > 0 => format!("{done}/{total}"),
                        _ => "-".to_owned(),
                    };
                    json!({
                        "id": item["id"],
                        "status": item["status"],
                        "kind": item["kind"],
                        "progress": progress,
                        "error": item["error"],
                    })
                })
                .collect();
            Some(render::table(
                &rows,
                &[
                    ("任务", "id"),
                    ("状态", "status"),
                    ("种类", "kind"),
                    ("进度", "progress"),
                    ("错误", "error"),
                ],
            ))
        }
        _ => None,
    }
}

pub fn render_index(args: &IndexArgs, value: &Value) -> Option<String> {
    match &args.command {
        None | Some(IndexCommand::Ls) => {
            let items = value["items"].as_array().map_or(&[][..], Vec::as_slice);
            Some(if items.is_empty() {
                "（案件中还没有卷索引）\n".to_owned()
            } else {
                render::table(
                    items,
                    &[
                        ("卷", "address"),
                        ("状态", "state"),
                        ("文件数", "fileCount"),
                        ("构建时间", "builtAt"),
                    ],
                )
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_task_ids_carry_their_number() {
        assert_eq!(plugin_task("plugin:12").unwrap(), 12);
        assert_eq!(plugin_task("7").unwrap(), 7);
        assert!(plugin_task("cli-tree-x").is_err());
    }
}
