//! Analyzers and their results: running analyzers, published records, run
//! audits and BCP imports.
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::Command;
use crate::{
    cli::{
        browsing::LastLink,
        exec::Exec,
        render,
        types::{options_object, parse_address, parse_json_text, Address},
        CliError, Output,
    },
    core::common::ids::NodeId,
};

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalyzeArgs {
    #[command(subcommand)]
    pub command: AnalyzeCommand,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AnalyzeCommand {
    /// 列出分析器，以及检材上识别出的系统
    Ls(EvidenceOption),
    /// 创建插件任务：系统级分析器给 --system，文件级分析器给 --file
    Run(RunArgs),
}

#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceOption {
    /// 检材别名；只有一份检材时可省略
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunArgs {
    /// 分析器（analyze ls 返回的 function，如 org.lovelycarve.linuxinfo/linuxbaseinfo）
    #[arg(value_name = "分析器")]
    pub function: String,
    /// 检材别名；只有一份检材时可省略
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
    /// 目标系统（analyze ls 返回的系统 ID，如 linux@part:2）
    #[arg(long, value_name = "系统ID")]
    pub system: Option<String>,
    /// 文件级分析器的目标文件
    #[arg(long, value_parser = parse_address, value_name = "路径")]
    pub file: Option<Address>,
    /// 分析器选项 键=值（可重复）；值是 JSON 时保持类型
    #[arg(long = "option", value_name = "键=值")]
    pub options: Vec<String>,
    /// 分析器选项的 JSON 对象（文本或 @文件）；与 --option 合并
    #[arg(long, value_name = "JSON")]
    pub options_json: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultsArgs {
    #[command(subcommand)]
    pub command: Option<ResultsCommand>,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResultsCommand {
    /// 分析结果树：检材 → 分类 → 分析器运行 → 记录类型（默认）
    Ls,
    /// 分页读取一次运行中某种记录
    Rows(RowsArgs),
    /// 一条记录的详情
    Show(RecordArgs),
    /// 一次运行的审计：输入、选项、画像事实、告警与补零的源字节
    Run(RunId),
    /// 把 BCP 数据包（作为检材打开的 ZIP 或目录）发布为分析结果
    Import(ImportArgs),
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowsArgs {
    /// 运行 ID（results ls 中的 runId）
    #[arg(value_name = "运行ID")]
    pub run_id: String,
    /// 记录类型（results ls 中的 artifactType，如 linux.user）
    #[arg(value_name = "记录类型")]
    pub record_type: String,
    /// 检材别名；只有一份检材时可省略
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
    /// 关键词：匹配标题与文本类列（ASCII 不区分大小写）
    #[arg(long, value_name = "文本")]
    pub keyword: Option<String>,
    /// 排序列：列 id 或标题
    #[arg(long, value_name = "列")]
    pub sort: Option<String>,
    /// 降序
    #[arg(long)]
    pub desc: bool,
    /// 跳过的记录数
    #[arg(long, default_value_t = 0)]
    pub offset: i64,
    /// 每页记录数
    #[arg(long, default_value_t = 100)]
    pub limit: i64,
    /// 读取旧版本插件的结果（结果树中 legacy 为 true 时）
    #[arg(long)]
    pub legacy: bool,
    /// BCP：只看这些原始应用编码（可重复）
    #[arg(long = "bcp-app", value_name = "编码")]
    pub bcp_apps: Vec<String>,
    /// BCP：只看这个账号；空字符串表示无账号的记录
    #[arg(long, value_name = "账号")]
    pub bcp_account: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordArgs {
    /// 记录 ID（results rows 返回的 recordId）
    #[arg(value_name = "记录ID")]
    pub record_id: String,
    /// 检材别名；只有一份检材时可省略
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunId {
    /// 运行 ID
    #[arg(value_name = "运行ID")]
    pub run_id: String,
    /// 检材别名；只有一份检材时可省略
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportArgs {
    /// BCP 检材的别名；只有一份检材时可省略
    #[arg(value_name = "别名")]
    pub evidence: Option<String>,
    /// 重新读取源件，成功后替换上一次导入的结果
    #[arg(long)]
    pub replace: bool,
}

pub fn analyze_path(args: &AnalyzeArgs) -> &'static str {
    match args.command {
        AnalyzeCommand::Ls(_) => "analyze ls",
        AnalyzeCommand::Run(_) => "analyze run",
    }
}

pub fn results_path(args: &ResultsArgs) -> &'static str {
    match &args.command {
        None | Some(ResultsCommand::Ls) => "results ls",
        Some(ResultsCommand::Rows(_)) => "results rows",
        Some(ResultsCommand::Show(_)) => "results show",
        Some(ResultsCommand::Run(_)) => "results run",
        Some(ResultsCommand::Import(_)) => "results import",
    }
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

pub async fn analyze(exec: &Exec, args: AnalyzeArgs) -> Result<Output, CliError> {
    use crate::commands::extensions::task_scheduler as scheduler;
    match args.command {
        AnalyzeCommand::Ls(option) => {
            let functions = scheduler::sched_task_functions(exec.state())
                .await
                .map_err(operation("sched_task_functions"))?;
            let mut value =
                json!({ "functions": serde_json::to_value(crate::cli::wire::Camel(functions))? });
            let evidence = match option.evidence.as_deref() {
                Some(alias) => Some(exec.resolve(alias.trim_start_matches('/')).await?),
                None => exec.resolve_default(None).await.ok(),
            };
            if let Some(evidence) = evidence {
                let systems = scheduler::sched_systems_list(exec.state(), evidence.image_id)
                    .await
                    .map_err(operation("sched_systems_list"))?;
                value["evidence"] = json!(evidence.alias);
                value["systems"] = serde_json::to_value(crate::cli::wire::Camel(systems))?;
            }
            Ok(Output::json(value))
        }
        AnalyzeCommand::Run(run) => {
            let (evidence, node_id, file_name) = match &run.file {
                Some(address) => {
                    let target = exec.target(address, LastLink::Follow).await?;
                    let name = target.name().to_owned();
                    let node = NodeId::from(target.node_id());
                    (target.evidence, Some(node), Some(name))
                }
                None => (
                    exec.resolve_default(run.evidence.as_deref()).await?,
                    None,
                    None,
                ),
            };
            let mut options = match run.options_json.as_deref() {
                Some(text) => match parse_json_text(text)? {
                    Value::Object(object) => object,
                    _ => return Err(CliError::new("usage", "--options-json 必须是 JSON 对象")),
                },
                None => serde_json::Map::new(),
            };
            options.extend(options_object(&run.options)?);
            let created = scheduler::sched_task_create(
                exec.state(),
                run.function,
                evidence.image_id,
                node_id,
                file_name,
                run.system,
                (!options.is_empty()).then_some(Value::Object(options)),
            )
            .await
            .map_err(operation("sched_task_create"))?;
            crate::cli::json_output(created)
        }
    }
}

pub async fn results(exec: &Exec, args: ResultsArgs) -> Result<Output, CliError> {
    use crate::commands::analysis::{analysis_results, bcp};
    match args.command.unwrap_or(ResultsCommand::Ls) {
        ResultsCommand::Ls => crate::cli::json_output(
            analysis_results::analysis_result_tree(exec.state())
                .await
                .map_err(operation("analysis_result_tree"))?,
        ),
        ResultsCommand::Rows(rows) => {
            let evidence = exec.resolve_default(rows.evidence.as_deref()).await?;
            let scope = (!rows.bcp_apps.is_empty() || rows.bcp_account.is_some())
                .then(|| {
                    serde_json::from_value(
                        json!({"applications": rows.bcp_apps, "account": rows.bcp_account}),
                    )
                })
                .transpose()?;
            crate::cli::json_output(
                analysis_results::analysis_records_page(
                    exec.state(),
                    evidence.image_id,
                    rows.run_id,
                    rows.record_type,
                    Some(rows.legacy),
                    rows.keyword,
                    scope,
                    rows.sort,
                    Some(rows.desc),
                    rows.offset.max(0),
                    rows.limit.clamp(1, 10_000),
                )
                .await
                .map_err(operation("analysis_records_page"))?,
            )
        }
        ResultsCommand::Show(record) => {
            let evidence = exec.resolve_default(record.evidence.as_deref()).await?;
            crate::cli::json_output(
                analysis_results::analysis_record_detail(
                    exec.state(),
                    evidence.image_id,
                    record.record_id,
                )
                .await
                .map_err(operation("analysis_record_detail"))?,
            )
        }
        ResultsCommand::Run(run) => {
            let evidence = exec.resolve_default(run.evidence.as_deref()).await?;
            crate::cli::json_output(
                analysis_results::analysis_run_get(exec.state(), evidence.image_id, run.run_id)
                    .await
                    .map_err(operation("analysis_run_get"))?,
            )
        }
        ResultsCommand::Import(import) => {
            let evidence = exec.resolve_default(import.evidence.as_deref()).await?;
            crate::cli::json_output(
                bcp::bcp_import(exec.state(), evidence.image_id, Some(import.replace))
                    .await
                    .map_err(operation("bcp_import"))?,
            )
        }
    }
}

pub fn render(command: &Command, value: &Value) -> Option<String> {
    match command {
        Command::Analyze(AnalyzeArgs {
            command: AnalyzeCommand::Ls(_),
        }) => {
            let mut out = render::table(
                value["functions"].as_array().map_or(&[][..], Vec::as_slice),
                &[
                    ("分析器", "function"),
                    ("作用域", "scope"),
                    ("名称", "name"),
                ],
            );
            if let Some(systems) = value["systems"].as_array() {
                out.push('\n');
                out.push_str(&render::table(
                    systems,
                    &[("系统", "id"), ("类型", "family"), ("名称", "name")],
                ));
            }
            Some(out)
        }
        _ => None,
    }
}
