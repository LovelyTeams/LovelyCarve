//! Bounded JSONL queries, with ordered barriers for shared-context operations.
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::{sync::mpsc, task::JoinSet};

use super::{input, shell::Shell, workflow, CliError, Output};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: String,
    command: String,
    #[serde(default)]
    args: Map<String, Value>,
    #[serde(default)]
    output: Option<String>,
}

// Explicitly reviewed queries only. New commands default to a barrier so that
// adding a command cannot accidentally race an unload or mutate shared context.
pub(super) fn concurrent(command: &str) -> bool {
    matches!(
        command.replace('-', "_").as_str(),
        "pwd"
            | "ls"
            | "tree"
            | "find"
            | "stat"
            | "hex"
            | "cat"
            | "strings"
            | "file"
            | "grep"
            | "hash"
            | "md5sum"
            | "sha1sum"
            | "sha256sum"
            | "info"
            | "list_partitions"
            | "list_devices"
            | "tasks"
            | "image_info"
            | "image_list"
            | "image_bad_regions"
            | "physical_disks_list"
            | "fs_children"
            | "fs_children_page"
            | "fs_child_counts"
            | "fs_subtree_empty_dirs"
            | "fs_node_locate"
            | "fs_flat_page"
            | "fs_flat_ext_counts"
            | "fs_dir_stats"
            | "fs_source_locate"
            | "fs_read"
            | "fs_read_range"
            | "fs_read_large"
            | "fs_preview"
            | "fs_preview_text"
            | "fs_stat"
            | "fs_streams"
            | "fs_stream_preview"
            | "fs_digest"
            | "fs_digest_load"
            | "fs_digest_load_nodes"
            | "fs_identify_type"
            | "fs_tags_load"
            | "fs_tags_load_nodes"
            | "fs_ftype_load"
            | "fs_ftype_load_nodes"
            | "fs_sqlite_preview"
            | "fs_sqlite_deleted_records"
            | "fs_sqlite_table_rows"
            | "fs_realm_preview"
            | "fs_realm_table_rows"
            | "fs_binary_preview"
            | "fs_gguf_preview"
            | "fs_zip_preview"
            | "fs_zip_member_read"
            | "fs_csv_preview"
            | "fs_vim_swap_preview"
            | "fs_ext4_journal"
            | "fs_ext4_journal_blocks_page"
            | "fs_usn_journal"
            | "fs_usn_journal_page"
            | "filedb_volumes"
            | "filedb_query_files"
            | "filedb_resolve_meta_addr"
            | "global_search_start"
            | "global_search_hits_page"
            | "global_search_history_list"
            | "timeline_status"
            | "timeline_types"
            | "timeline_count"
            | "timeline_datas"
            | "timeline_range"
            | "analysis_result_tree"
            | "analysis_records_page"
            | "analysis_record_detail"
            | "bcp_catalog"
            | "bcp_rows"
            | "bitlocker_metadata"
            | "bitlocker_status"
            | "filevault_inspect"
            | "shadow_copy_list"
            | "carve_formats"
            | "carve_scan_status"
            | "carve_latest_scan"
            | "carve_results_page"
            | "carve_preview"
            | "carve_scan_list"
            | "case_active"
            | "case_list"
            | "case_option_query"
            | "case_evidence_list"
            | "snapshot_list"
            | "plugin_list"
            | "plugin_analysis_list"
            | "sched_task_functions"
            | "sched_task_list"
            | "module_summary"
            | "system_path_info"
            | "task_list"
            | "task_status"
            | "task_cancel"
    )
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

struct Completion {
    id: String,
    shell: Shell,
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
                json!({"id":id,"result":value})
            }
            Err(error) => {
                self.failed = true;
                json!({"id":id,"error":error})
            }
        };
        workflow::print_output(Output::Json(response), true)
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

fn launch<F>(tasks: &mut JoinSet<Completion>, shell: Shell, request: Request, execute: F)
where
    F: Fn(&mut Shell, Request) -> Result<Value, CliError> + Send + 'static,
{
    tasks.spawn_blocking(move || {
        let mut shell = shell;
        let id = request.id.clone();
        // Keep the request ID even when a handler panics, and let other queries finish.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            execute(&mut shell, request)
        }))
        .unwrap_or_else(|_| Err(super::internal_error("CLI 查询工作线程异常退出")));
        Completion { id, shell, result }
    });
}

async fn run_lines<F>(
    shell: &mut Shell,
    mut lines: mpsc::Receiver<Result<String, CliError>>,
    jobs: usize,
    execute: F,
) -> Result<bool, CliError>
where
    F: Fn(&mut Shell, Request) -> Result<Value, CliError> + Clone + Send + 'static,
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
        } else if request_ids.len() >= 10000 {
            Err(CliError::invalid("单会话最多保留 10000 个结果引用"))
        } else {
            request_ids.insert(request.id.clone());
            Ok(())
        };
        if let Err(error) = validation {
            responses.publish(Some(request.id), Err(error))?;
            continue;
        }
        let mut args = Value::Object(request.args);
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
            launch(&mut tasks, shell.clone(), request, execute.clone());
        } else {
            launch(&mut tasks, shell.clone(), request, execute.clone());
            let completion = tasks
                .join_next()
                .await
                .ok_or_else(|| super::internal_error("CLI 请求未完成"))?
                .map_err(super::internal_error)?;
            *shell = completion.shell;
            responses.publish(Some(completion.id), completion.result)?;
        }
    }
    while !tasks.is_empty() {
        responses.finish_one(&mut tasks).await?;
    }
    Ok(responses.failed)
}

pub(super) async fn json_lines(
    app: &tauri::AppHandle,
    shell: &mut Shell,
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
                let line = workflow::line(&mut reader);
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
    let app = app.clone();
    run_lines(shell, receiver, jobs, move |shell, request| {
        tauri::async_runtime::block_on(async {
            let output = if request.command == "wait" {
                if !request.args.is_empty() {
                    return Err(CliError::invalid("wait 不接受参数"));
                }
                workflow::wait_idle(&app).await?;
                Output::Json(Value::Null)
            } else if let Some(matches) = input::shell_matches(&request.command, &request.args)? {
                shell
                    .execute(&app, &matches)
                    .await?
                    .unwrap_or(Output::Json(Value::Null))
            } else {
                shell.invoke(&app, &request.command, request.args).await?
            };
            if let Some(path) = request.output {
                workflow::save_output(Path::new(&path), output)?;
                Ok(json!({"output":path}))
            } else {
                Ok(output.json())
            }
        })
    })
    .await
}

#[cfg(test)]
mod tests;
