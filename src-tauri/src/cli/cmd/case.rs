//! `case`: the case is the workspace. Its directory names the backend
//! process that keeps evidence open between commands.
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli::{
    browsing::LastLink,
    exec::Exec,
    json_output, render,
    types::{parse_address, parse_host_path, Address, HostPath, Parts},
    CliError, Output,
};

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseArgs {
    #[command(subcommand)]
    pub command: Option<CaseCommand>,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaseCommand {
    /// 当前案件：后台进程、检材、卷索引与运行中的任务（默认）
    Status,
    /// 创建案件；--open 同时启动它的后台进程
    New(NewCase),
    /// 启动案件后台进程：之后指向该案件的命令复用已打开的检材、解锁与组装结果
    Open(CaseDir),
    /// 停止案件后台进程：取消任务、释放检材后退出
    Close(CaseDir),
    /// 列出本机登记的案件
    Ls,
    /// 锁定当前案件：拒绝添加或移除检材、构建或删除卷索引等修改
    Lock,
    /// 解除案件锁定
    Unlock,
    /// 当前案件或其中文件的 lovelycarve:// 链接：桌面端打开后定位并高亮
    Link(LinkArgs),
    /// 删除案件：移除案件目录中由 LovelyCarve 创建的台账、检材数据与暂存区
    Rm(RequiredDir),
    /// 运行案件后台进程（由 case open 启动）
    #[command(hide = true)]
    Serve(RequiredDir),
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewCase {
    /// 新案件目录
    #[arg(value_parser = parse_host_path, value_name = "目录")]
    pub dir: HostPath,
    /// 案件名称；默认取目录名
    #[arg(long)]
    pub name: Option<String>,
    /// 案件编号
    #[arg(long)]
    pub number: Option<String>,
    /// 案件说明
    #[arg(long)]
    pub description: Option<String>,
    /// 检验人
    #[arg(long)]
    pub examiner: Option<String>,
    /// 创建后启动案件后台进程
    #[arg(long)]
    pub open: bool,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaseDir {
    /// 案件目录；默认取 -C、LOVELYCARVE_CASE 或当前目录所在的案件
    #[arg(value_parser = parse_host_path, value_name = "目录")]
    pub dir: Option<HostPath>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkArgs {
    /// 检材内的文件或目录；省略时链接到案件本身
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Option<Address>,
    /// 要高亮的内容起点（字节）
    #[arg(long, value_name = "字节")]
    pub offset: Option<u64>,
    /// 要高亮的字节数，默认 1
    #[arg(long, value_name = "字节", requires = "offset")]
    pub length: Option<u64>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequiredDir {
    /// 案件目录
    #[arg(value_parser = parse_host_path, value_name = "目录")]
    pub dir: HostPath,
}

pub fn path(args: &CaseArgs) -> &'static str {
    match &args.command {
        None | Some(CaseCommand::Status) => "case status",
        Some(CaseCommand::New(_)) => "case new",
        Some(CaseCommand::Open(_)) => "case open",
        Some(CaseCommand::Close(_)) => "case close",
        Some(CaseCommand::Ls) => "case ls",
        Some(CaseCommand::Lock) => "case lock",
        Some(CaseCommand::Unlock) => "case unlock",
        Some(CaseCommand::Link(_)) => "case link",
        Some(CaseCommand::Rm(_)) => "case rm",
        Some(CaseCommand::Serve(_)) => "case serve",
    }
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

pub async fn run(exec: &Exec, args: CaseArgs) -> Result<Output, CliError> {
    match args.command.unwrap_or(CaseCommand::Status) {
        CaseCommand::Status => status(exec).await,
        CaseCommand::New(new) => {
            let name = new.name.clone().unwrap_or_else(|| {
                new.dir
                    .as_path()
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| new.dir.text())
            });
            let info = crate::commands::casework::case::case_create(
                exec.host.clone(),
                exec.state(),
                new.dir.text(),
                name,
                new.number,
                new.description,
                new.examiner,
            )
            .await
            .map_err(operation("case_create"))?;
            json_output(info)
        }
        CaseCommand::Ls => {
            let cases = crate::commands::casework::case::case_list(exec.host.clone())
                .await
                .map_err(operation("case_list"))?;
            let mut items = serde_json::to_value(crate::cli::wire::Camel(cases))?;
            if let Some(items) = items.as_array_mut() {
                for item in items {
                    let running = item["dir"]
                        .as_str()
                        .is_some_and(crate::cli::backend::running);
                    item["backend"] = json!(running);
                }
            }
            Ok(Output::json(json!({ "items": items })))
        }
        CaseCommand::Lock => json_output(
            crate::commands::casework::case::case_lock(exec.state())
                .await
                .map_err(operation("case_lock"))?,
        ),
        CaseCommand::Unlock => json_output(
            crate::commands::casework::case::case_unlock(exec.state())
                .await
                .map_err(operation("case_unlock"))?,
        ),
        CaseCommand::Link(args) => link(exec, args).await,
        CaseCommand::Rm(dir) => {
            if crate::cli::backend::running(&dir.dir.text()) {
                return Err(CliError::new(
                    "caseBackendRunning",
                    format!(
                        "案件后台进程仍在运行；先运行 case close \"{}\"",
                        dir.dir.text()
                    ),
                ));
            }
            crate::commands::casework::case::case_remove(
                exec.host.clone(),
                exec.state(),
                dir.dir.text(),
            )
            .await
            .map_err(operation("case_remove"))?;
            Ok(Output::json(
                json!({"dir": dir.dir.text(), "removed": true}),
            ))
        }
        CaseCommand::Open(_) | CaseCommand::Close(_) | CaseCommand::Serve(_) => Err(
            crate::cli::internal_error("case open/close/serve 由进程入口处理"),
        ),
    }
}

/// The link of the active case, or of a node in its evidence with an
/// optional byte range to highlight.
async fn link(exec: &Exec, args: LinkArgs) -> Result<Output, CliError> {
    if exec.case_dir().await.is_none() {
        return Err(CliError::new(
            "usage",
            "链接指向案件中的位置：用 -C <案件目录>，或在案件目录中运行",
        ));
    }
    let target = match &args.path {
        Some(address) if address.parts() != Parts::Root => {
            Some(exec.target(address, LastLink::Keep).await?)
        }
        _ => None,
    };
    let node_id = target.as_ref().map_or("", |target| target.node_id());
    if args.offset.is_some() && node_id.is_empty() {
        return Err(CliError::new(
            "usage",
            "--offset 需要检材内的文件或目录：case link /<别名>/<路径> --offset <字节>",
        ));
    }
    let place = target
        .as_ref()
        .map(|target| crate::commands::casework::case::LinkPlace {
            image_id: target.image_id().clone(),
            node_id: (!node_id.is_empty()).then(|| node_id.into()),
            root_id: None,
            path: None,
            offset: args.offset,
            length: args.length,
        });
    let link = crate::commands::casework::case::case_link(exec.state(), place)
        .await
        .map_err(operation("case_link"))?;
    Ok(Output::json(json!({
        "link": link,
        "path": target.as_ref().map(crate::cli::workspace::Target::address),
        "nodeId": (!node_id.is_empty()).then_some(node_id),
        "offset": args.offset,
        "length": args.offset.map(|_| args.length.unwrap_or(1)),
    })))
}

/// The case, this process, the evidence, the indexes and the active tasks.
async fn status(exec: &Exec) -> Result<Output, CliError> {
    let case = crate::commands::casework::case::case_active(exec.state())
        .await
        .map_err(operation("case_active"))?;
    let evidence = exec.evidence().await?;
    let indexes = if case.is_some() {
        super::tasks::index_rows(exec, &evidence).await?
    } else {
        Vec::new()
    };
    let tasks = crate::commands::system::system::task_list(exec.state(), None)
        .await
        .map_err(operation("task_list"))?;
    let tasks = serde_json::to_value(crate::cli::wire::Camel(tasks))?;
    let active: Vec<Value> = tasks
        .as_array()
        .map(|tasks| {
            tasks
                .iter()
                .filter(|task| {
                    matches!(
                        task["status"].as_str(),
                        Some("running" | "created" | "queued" | "pause")
                    )
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let case = match case {
        Some(case) => {
            let mut case = serde_json::to_value(crate::cli::wire::Camel(case))?;
            if let Some(fields) = case.as_object_mut() {
                // Listed below with their aliases.
                fields.remove("evidence");
                fields.remove("snapshots");
            }
            case
        }
        None => Value::Null,
    };
    Ok(Output::json(json!({
        "case": case,
        "process": {"pid": std::process::id(), "mode": format!("{:?}", exec.mode)},
        "evidence": evidence,
        "indexes": indexes,
        "activeTasks": active,
    })))
}

pub fn render(args: &CaseArgs, value: &Value) -> Option<String> {
    match &args.command {
        None | Some(CaseCommand::Status) => {
            let mut out = String::new();
            match value["case"].as_object() {
                Some(case) => {
                    out.push_str(&format!(
                        "案件  {}  {}{}\n",
                        render::cell(&case["name"].clone()),
                        render::cell(&case["dir"].clone()),
                        if case["locked"] == Value::Bool(true) {
                            "  [已锁定]"
                        } else {
                            ""
                        }
                    ));
                }
                None => out.push_str("案件  （未打开案件）\n"),
            }
            let backend = value["process"]["mode"].as_str() == Some("Backend");
            out.push_str(&format!(
                "进程  {}\n",
                if backend {
                    format!("案件后台进程 pid {}", value["process"]["pid"])
                } else {
                    "本次命令（没有案件后台进程时每条命令重新打开检材）".to_owned()
                }
            ));
            let evidence = value["evidence"].as_array().map_or(&[][..], Vec::as_slice);
            out.push('\n');
            if evidence.is_empty() {
                out.push_str("检材  （无）\n");
            } else {
                out.push_str(&render::table(
                    evidence,
                    &[
                        ("检材", "alias"),
                        ("格式", "sourceFormat"),
                        ("大小", "sizeBytes"),
                        ("已打开", "loaded"),
                        ("路径", "path"),
                    ],
                ));
            }
            let indexes = value["indexes"].as_array().map_or(&[][..], Vec::as_slice);
            if !indexes.is_empty() {
                out.push('\n');
                out.push_str(&render::table(
                    indexes,
                    &[
                        ("卷", "address"),
                        ("索引", "state"),
                        ("文件数", "fileCount"),
                    ],
                ));
            }
            let tasks = value["activeTasks"]
                .as_array()
                .map_or(&[][..], Vec::as_slice);
            if !tasks.is_empty() {
                out.push('\n');
                out.push_str(&render::table(
                    tasks,
                    &[
                        ("任务", "id"),
                        ("状态", "status"),
                        ("种类", "kind"),
                        ("对象", "subject"),
                    ],
                ));
            }
            Some(out)
        }
        Some(CaseCommand::Link(_)) => value["link"]
            .as_str()
            .map(|link| format!("{}\n", crate::cli::output::sanitize(link))),
        Some(CaseCommand::Ls) => Some(render::table(
            value["items"].as_array().map_or(&[][..], Vec::as_slice),
            &[
                ("名称", "name"),
                ("编号", "caseNumber"),
                ("锁定", "locked"),
                ("后台", "backend"),
                ("目录", "dir"),
            ],
        )),
        _ => None,
    }
}
