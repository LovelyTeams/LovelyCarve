//! `carve`: file system record recovery and signature carving, one scan
//! session per run, with its results and their recovery.
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli::{
    exec::Exec,
    render,
    types::{parse_host_path, HostPath},
    CliError, Output,
};

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CarveArgs {
    #[command(subcommand)]
    pub command: Option<CarveCommand>,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CarveCommand {
    /// 列出检材的恢复扫描（默认）
    Ls(EvidenceOption),
    /// 可雕刻的文件格式
    Formats,
    /// 开始扫描：文件系统记录恢复与（或）签名雕刻，在后台运行
    Start(StartArgs),
    /// 扫描的阶段、进度与摘要
    Status(ScanId),
    /// 扫描结果：雕刻出的文件，或 --fs 文件系统恢复出的文件
    Results(ResultsArgs),
    /// 输出一个雕刻结果的内容（原始字节）
    Cat(CatArgs),
    /// 暂停扫描
    Pause(ScanId),
    /// 继续暂停的扫描
    Resume(ScanId),
    /// 把恢复出的文件导出到新目录
    Export(ExportArgs),
    /// 删除已结束的扫描及其结果
    Rm(ScanRemove),
}

#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceOption {
    /// 检材别名；只有一份检材时可省略
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    Whole,
    Unallocated,
    Partition,
    Range,
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Filesystem {
    Fat,
    Ntfs,
    Exfat,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartArgs {
    /// 检材别名；只有一份检材时可省略
    #[arg(value_name = "别名")]
    pub evidence: Option<String>,
    /// 扫描范围
    #[arg(long, value_enum, default_value_t = Scope::Whole)]
    pub scope: Scope,
    /// --scope partition 的分区序号
    #[arg(long, value_name = "序号")]
    pub partition: Option<u8>,
    /// --scope range 的起始扇区
    #[arg(long, value_name = "扇区")]
    pub start_sector: Option<u64>,
    /// --scope range 的扇区数
    #[arg(long, value_name = "扇区数")]
    pub sectors: Option<u64>,
    /// 雕刻的格式，逗号分隔（carve formats 列出）；默认全部
    #[arg(long, value_delimiter = ',', value_name = "格式")]
    pub types: Vec<String>,
    /// 同时恢复这些文件系统的记录，逗号分隔
    #[arg(long, value_enum, value_delimiter = ',', value_name = "文件系统")]
    pub fs: Vec<Filesystem>,
    /// 文件系统恢复也列出已知分区中的现存文件
    #[arg(long)]
    pub existing: bool,
    /// 不做签名雕刻，只恢复文件系统记录
    #[arg(long)]
    pub no_signatures: bool,
    /// 单个雕刻结果的最大字节数
    #[arg(long, value_parser = super::parse_size, value_name = "大小")]
    pub max_file_size: Option<u64>,
    /// 最多保留的雕刻结果数
    #[arg(long)]
    pub max_results: Option<usize>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanId {
    /// 扫描 ID（carve start 返回的 scanId）
    #[arg(value_name = "扫描ID")]
    pub scan_id: String,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRemove {
    /// 扫描 ID
    #[arg(value_name = "扫描ID")]
    pub scan_id: String,
    /// 检材别名；只有一份检材时可省略
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultsArgs {
    /// 扫描 ID
    #[arg(value_name = "扫描ID")]
    pub scan_id: String,
    /// 列出文件系统恢复出的文件（在该阶段结束前为空）
    #[arg(long)]
    pub fs: bool,
    /// 只看这种格式
    #[arg(long, value_name = "格式")]
    pub format: Option<String>,
    /// 只看这个类别
    #[arg(long, value_name = "类别")]
    pub category: Option<String>,
    /// 最低置信度（0–100）
    #[arg(long)]
    pub min_confidence: Option<u8>,
    /// 上一页给出的游标
    #[arg(long, value_name = "游标")]
    pub cursor: Option<String>,
    /// 每页条数
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatArgs {
    /// 扫描 ID
    #[arg(value_name = "扫描ID")]
    pub scan_id: String,
    /// 雕刻结果 ID
    #[arg(value_name = "结果ID")]
    pub artifact_id: String,
    /// 最多读取的字节数
    #[arg(long)]
    pub max_bytes: Option<u64>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportArgs {
    /// 扫描 ID
    #[arg(value_name = "扫描ID")]
    pub scan_id: String,
    /// 输出目录（必须不存在）
    #[arg(short = 'o', long, value_parser = parse_host_path, value_name = "目录")]
    pub output: HostPath,
    /// 只导出这些雕刻结果，逗号分隔；默认全部
    #[arg(long, value_delimiter = ',', value_name = "ID")]
    pub artifacts: Vec<String>,
    /// 不导出雕刻结果
    #[arg(long, conflicts_with = "artifacts")]
    pub no_artifacts: bool,
    /// 同时导出这些文件系统恢复结果（候选编号，逗号分隔）
    #[arg(long, value_delimiter = ',', value_name = "编号")]
    pub files: Vec<String>,
}

pub fn path(args: &CarveArgs) -> &'static str {
    match &args.command {
        None | Some(CarveCommand::Ls(_)) => "carve ls",
        Some(CarveCommand::Formats) => "carve formats",
        Some(CarveCommand::Start(_)) => "carve start",
        Some(CarveCommand::Status(_)) => "carve status",
        Some(CarveCommand::Results(_)) => "carve results",
        Some(CarveCommand::Cat(_)) => "carve cat",
        Some(CarveCommand::Pause(_)) => "carve pause",
        Some(CarveCommand::Resume(_)) => "carve resume",
        Some(CarveCommand::Export(_)) => "carve export",
        Some(CarveCommand::Rm(_)) => "carve rm",
    }
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

pub async fn run(exec: &Exec, args: CarveArgs) -> Result<Output, CliError> {
    use crate::commands::carving::{carve, carve_filesystem, carve_history, carve_recovery};
    match args
        .command
        .unwrap_or(CarveCommand::Ls(EvidenceOption::default()))
    {
        CarveCommand::Ls(option) => {
            let evidence = exec.resolve_default(option.evidence.as_deref()).await?;
            let scans = carve_history::carve_scan_list(exec.state(), evidence.image_id)
                .await
                .map_err(operation("carve_scan_list"))?;
            Ok(Output::json(
                json!({ "items": serde_json::to_value(crate::cli::wire::Camel(scans))? }),
            ))
        }
        CarveCommand::Formats => Ok(Output::json(json!({
            "items": serde_json::to_value(crate::cli::wire::Camel(carve::carve_formats()))?,
        }))),
        CarveCommand::Start(start) => {
            let evidence = exec.resolve_default(start.evidence.as_deref()).await?;
            let mut options = json!({
                "format_ids": start.types,
                "scope": match start.scope {
                    Scope::Whole => "whole",
                    Scope::Unallocated => "unallocated",
                    Scope::Partition => "partition",
                    Scope::Range => "range",
                },
                "partition_id": start.partition,
                "start_sector": start.start_sector,
                "sector_count": start.sectors,
                "filesystems": {
                    "fat": start.fs.contains(&Filesystem::Fat),
                    "ntfs": start.fs.contains(&Filesystem::Ntfs),
                    "exfat": start.fs.contains(&Filesystem::Exfat),
                    "include_existing": start.existing,
                },
                "signatures": !start.no_signatures,
            });
            if let Some(size) = start.max_file_size {
                options["max_file_size"] = json!(size);
            }
            if let Some(results) = start.max_results {
                options["max_results"] = json!(results);
            }
            let result = carve::carve_start(
                exec.host.clone(),
                exec.state(),
                evidence.image_id,
                serde_json::from_value(options)?,
            )
            .await
            .map_err(operation("carve_start"))?;
            crate::cli::json_output(result)
        }
        CarveCommand::Status(scan) => crate::cli::json_output(
            carve::carve_scan_status(exec.state(), scan.scan_id)
                .await
                .map_err(operation("carve_scan_status"))?,
        ),
        CarveCommand::Results(results) => {
            if results.fs {
                return crate::cli::json_output(
                    carve_filesystem::carve_filesystem_results(exec.state(), results.scan_id)
                        .await
                        .map_err(operation("carve_filesystem_results"))?,
                );
            }
            let filter = serde_json::from_value(json!({
                "format_id": results.format,
                "category": results.category,
                "min_confidence": results.min_confidence,
            }))?;
            crate::cli::json_output(
                carve::carve_results_page(
                    exec.state(),
                    results.scan_id,
                    results.cursor,
                    Some(results.limit.clamp(1, 10_000)),
                    Some(filter),
                )
                .await
                .map_err(operation("carve_results_page"))?,
            )
        }
        CarveCommand::Cat(cat) => {
            let response =
                carve::carve_preview(exec.state(), cat.scan_id, cat.artifact_id, cat.max_bytes)
                    .await
                    .map_err(operation("carve_preview"))?;
            Ok(Output::Bytes(response.into_bytes()))
        }
        CarveCommand::Pause(scan) => crate::cli::json_output(
            carve::carve_scan_pause(exec.host.clone(), exec.state(), scan.scan_id)
                .await
                .map_err(operation("carve_scan_pause"))?,
        ),
        CarveCommand::Resume(scan) => crate::cli::json_output(
            carve::carve_scan_resume(exec.host.clone(), exec.state(), scan.scan_id)
                .await
                .map_err(operation("carve_scan_resume"))?,
        ),
        CarveCommand::Export(export) => {
            if export.output.as_path().exists() {
                return Err(CliError::new(
                    "alreadyExists",
                    format!(
                        "{} 已存在：导出从不覆盖已有内容；请换一个新的输出目录",
                        export.output.text()
                    ),
                ));
            }
            let options = serde_json::from_value(json!({
                "artifact_ids": if export.no_artifacts {
                    Some(Vec::new())
                } else if export.artifacts.is_empty() {
                    None
                } else {
                    Some(export.artifacts)
                },
                "file_ids": export.files,
                "destination": export.output.text(),
                "task_id": super::task_id("carve export"),
            }))?;
            crate::cli::json_output(
                carve_recovery::carve_recover(exec.state(), export.scan_id, options)
                    .await
                    .map_err(operation("carve_recover"))?,
            )
        }
        CarveCommand::Rm(remove) => {
            let evidence = exec.resolve_default(remove.evidence.as_deref()).await?;
            carve_history::carve_scan_remove(
                exec.state(),
                evidence.image_id,
                remove.scan_id.clone(),
            )
            .await
            .map_err(operation("carve_scan_remove"))?;
            Ok(Output::json(
                json!({"scanId": remove.scan_id, "removed": true}),
            ))
        }
    }
}

pub fn render(args: &CarveArgs, value: &Value) -> Option<String> {
    let items = value["items"].as_array().map_or(&[][..], Vec::as_slice);
    match &args.command {
        Some(CarveCommand::Formats) => Some(render::table(
            items,
            &[
                ("格式", "id"),
                ("类别", "category"),
                ("说明", "description"),
            ],
        )),
        _ => None,
    }
}
