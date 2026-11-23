//! `evidence`: what the workspace holds, each item under its alias.
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    cli::{
        exec::Exec,
        json_output, render,
        types::{parse_address, parse_host_path, parse_secret, Address, HostPath, Parts, Secret},
        workspace::{evidence_rows, LoadOptions},
        CliError, Output,
    },
    core::{common::hash::HashAlgo, evidence::image::RawReadMode},
};

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceArgs {
    #[command(subcommand)]
    pub command: Option<EvidenceCommand>,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EvidenceCommand {
    /// 列出检材与别名（默认）
    Ls,
    /// 打开检材；有案件时登记进案件并在后台建立卷索引
    Add(AddArgs),
    /// 检材的格式、大小、分区与采集元数据
    Show(AliasArg),
    /// 在当前进程中关闭检材（不改动案件）；关闭源检材也关闭由它派生的检材
    Close(AliasRequired),
    /// 从案件中移除检材
    Rm(AliasRequired),
    /// 计算整份检材的哈希
    Hash(HashEvidence),
    /// 导出整份检材为 RAW；加 --unlocked 时已解锁的分区写成明文
    Export(ExportEvidence),
    /// 读取时以零填充的坏区域
    BadRegions(AliasArg),
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadMode {
    /// 检查 RAW 分段完整性（默认）
    Strict,
    /// 只拼接连续前缀并容忍短读
    Force,
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Algo {
    Md5,
    Sha1,
    Sha256,
    Sm3,
}

impl From<Algo> for HashAlgo {
    fn from(algo: Algo) -> Self {
        match algo {
            Algo::Md5 => Self::Md5,
            Algo::Sha1 => Self::Sha1,
            Algo::Sha256 => Self::Sha256,
            Algo::Sm3 => Self::Sm3,
        }
    }
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddArgs {
    /// 检材路径：磁盘镜像、目录、归档、iOS 备份、AD1、物理盘（可列出多个）
    #[arg(value_parser = parse_host_path, value_name = "检材路径", required = true)]
    pub paths: Vec<HostPath>,
    /// 别名（只打开一份检材时可用）；默认取文件名
    #[arg(long = "as", value_name = "别名")]
    pub alias: Option<String>,
    /// RAW 读取模式
    #[arg(long, value_enum)]
    pub raw_read_mode: Option<ReadMode>,
    /// QCOW legacy AES 密码（@文件、- 或在终端中输入）
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_parser = parse_secret, value_name = "@文件")]
    pub qcow_password: Option<Secret>,
    /// QCOW legacy AES 密钥（16 字节十六进制）
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_parser = parse_secret, value_name = "@文件")]
    pub qcow_key: Option<Secret>,
    /// 等待后台的卷索引与识别任务完成，并报告每个卷的索引状态
    #[arg(long)]
    pub wait: bool,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasArg {
    /// 检材别名；只有一份检材时可省略
    #[arg(value_name = "别名")]
    pub alias: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AliasRequired {
    /// 检材别名
    #[arg(value_name = "别名")]
    pub alias: String,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HashEvidence {
    /// 检材别名；只有一份检材时可省略
    #[arg(value_name = "别名")]
    pub alias: Option<String>,
    /// 算法，逗号分隔
    #[arg(long, value_enum, value_delimiter = ',', default_values_t = [Algo::Md5, Algo::Sha1, Algo::Sha256])]
    pub algo: Vec<Algo>,
    /// 遇到读不出的扇区立即失败（默认补零并报告）
    #[arg(long)]
    pub strict: bool,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportEvidence {
    /// 检材别名
    #[arg(value_name = "检材")]
    pub target: String,
    /// 输出文件（必须不存在）
    #[arg(short = 'o', long, value_parser = parse_host_path, value_name = "文件")]
    pub output: HostPath,
    /// 把已解锁的分区（BitLocker、LUKS、VeraCrypt、FileVault）写成明文，其余字节保持源数据
    #[arg(long)]
    pub unlocked: bool,
    /// 遇到读不出的扇区立即失败（默认补零并报告）
    #[arg(long)]
    pub strict: bool,
}

pub fn path(args: &EvidenceArgs) -> &'static str {
    match &args.command {
        None | Some(EvidenceCommand::Ls) => "evidence ls",
        Some(EvidenceCommand::Add(_)) => "evidence add",
        Some(EvidenceCommand::Show(_)) => "evidence show",
        Some(EvidenceCommand::Close(_)) => "evidence close",
        Some(EvidenceCommand::Rm(_)) => "evidence rm",
        Some(EvidenceCommand::Hash(_)) => "evidence hash",
        Some(EvidenceCommand::Export(_)) => "evidence export",
        Some(EvidenceCommand::BadRegions(_)) => "evidence bad-regions",
    }
}

pub fn secrets_mut(args: &mut EvidenceArgs) -> Vec<(&'static str, &mut Secret)> {
    match &mut args.command {
        Some(EvidenceCommand::Add(add)) => {
            let mut secrets = Vec::new();
            if let Some(secret) = add.qcow_password.as_mut() {
                secrets.push(("qcow-password", secret));
            }
            if let Some(secret) = add.qcow_key.as_mut() {
                secrets.push(("qcow-key", secret));
            }
            secrets
        }
        _ => Vec::new(),
    }
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

fn no_clobber(path: &HostPath) -> Result<(), CliError> {
    if path.as_path().exists() {
        return Err(CliError::new(
            "alreadyExists",
            format!(
                "{} 已存在：导出从不覆盖已有文件，什么也没有写入；请换一个新的输出路径",
                path.text()
            ),
        ));
    }
    Ok(())
}

pub async fn run(exec: &Exec, args: EvidenceArgs) -> Result<Output, CliError> {
    match args.command.unwrap_or(EvidenceCommand::Ls) {
        EvidenceCommand::Ls => Ok(Output::json(evidence_rows(&exec.evidence().await?))),
        EvidenceCommand::Add(add) => add_evidence(exec, add).await,
        EvidenceCommand::Show(alias) => {
            let evidence = exec.resolve_default(alias.alias.as_deref()).await?;
            let info = crate::commands::evidence::image::image_info(
                exec.state(),
                evidence.image_id.clone(),
            )
            .await
            .map_err(operation("image_info"))?;
            let mut value = serde_json::to_value(crate::cli::wire::Camel(info))?;
            value["alias"] = json!(evidence.alias);
            value["address"] = json!(format!("/{}", evidence.alias));
            value["inCase"] = json!(evidence.in_case);
            Ok(Output::json(value))
        }
        EvidenceCommand::Close(alias) => {
            let evidence = exec.resolve(&alias.alias).await?;
            crate::commands::evidence::image::image_unload(exec.state(), evidence.image_id.clone())
                .await
                .map_err(operation("image_unload"))?;
            exec.workspace.forget(&evidence.image_id);
            Ok(Output::json(
                json!({"alias": evidence.alias, "closed": true}),
            ))
        }
        EvidenceCommand::Rm(alias) => {
            let evidence = exec.resolve(&alias.alias).await?;
            if !evidence.in_case {
                return Err(CliError::new(
                    "usage",
                    format!(
                        "{} 不是案件中的检材；在当前进程中关闭用 evidence close",
                        evidence.alias
                    ),
                ));
            }
            let case = crate::commands::casework::case::case_evidence_remove(
                exec.state(),
                evidence.image_id.clone(),
            )
            .await
            .map_err(operation("case_evidence_remove"))?;
            exec.workspace.forget(&evidence.image_id);
            Ok(Output::json(json!({
                "alias": evidence.alias,
                "removed": true,
                "case": serde_json::to_value(crate::cli::wire::Camel(case))?,
            })))
        }
        EvidenceCommand::Hash(hash) => {
            let evidence = exec.resolve_default(hash.alias.as_deref()).await?;
            let algorithms: Vec<HashAlgo> = hash.algo.iter().copied().map(HashAlgo::from).collect();
            let result = crate::commands::evidence::hash::image_hash_compute(
                exec.host.clone(),
                exec.state(),
                evidence.image_id.clone(),
                algorithms,
                super::task_id("evidence hash"),
                Some(policy(hash.strict)),
            )
            .await
            .map_err(operation("image_hash_compute"))?;
            let mut value = flattened(&result)?;
            value["alias"] = json!(evidence.alias);
            Ok(Output::json(value))
        }
        EvidenceCommand::Export(export) => export_evidence(exec, export).await,
        EvidenceCommand::BadRegions(alias) => {
            let evidence = exec.resolve_default(alias.alias.as_deref()).await?;
            json_output(
                crate::commands::evidence::image::image_bad_regions(
                    exec.state(),
                    evidence.image_id,
                )
                .await
                .map_err(operation("image_bad_regions"))?,
            )
        }
    }
}

fn policy(strict: bool) -> crate::core::evidence::image::ReadFaultPolicy {
    if strict {
        crate::core::evidence::image::ReadFaultPolicy::Strict
    } else {
        crate::core::evidence::image::ReadFaultPolicy::ZeroFillAndReport
    }
}

/// Results whose types flatten their audit fields: only the outer record's
/// keys become camelCase.
fn flattened(value: &impl Serialize) -> Result<Value, CliError> {
    match crate::cli::wire::flattened_output(value)? {
        Output::Json(json) => Ok(json.value().clone()),
        _ => Err(crate::cli::internal_error("结果不是 JSON")),
    }
}

async fn add_evidence(exec: &Exec, add: AddArgs) -> Result<Output, CliError> {
    if add.alias.is_some() && add.paths.len() > 1 {
        return Err(CliError::new("usage", "--as 只能用于打开一份检材"));
    }
    let mut added = Vec::new();
    for path in &add.paths {
        let options = LoadOptions {
            qcow_password: add
                .qcow_password
                .as_ref()
                .map(Secret::value)
                .transpose()?
                .map(str::to_owned),
            qcow_key_hex: add
                .qcow_key
                .as_ref()
                .map(Secret::value)
                .transpose()?
                .map(str::to_owned),
            raw_read_mode: add.raw_read_mode.map(|mode| match mode {
                ReadMode::Strict => RawReadMode::Strict,
                ReadMode::Force => RawReadMode::Force,
            }),
        };
        let (evidence, info) = exec
            .add(&path.text(), add.alias.as_deref(), options)
            .await?;
        let partitions = info.partitions.len();
        let mut value = serde_json::to_value(crate::cli::wire::Camel(info))?;
        value["alias"] = json!(evidence.alias);
        value["address"] = json!(format!("/{}", evidence.alias));
        value["inCase"] = json!(evidence.in_case);
        value["partitionCount"] = json!(partitions);
        added.push(value);
    }
    let mut result = json!({ "items": added });
    if add.wait {
        let report = exec.background.wait(&exec.state).await?.verdict(None)?;
        result["tasks"] = serde_json::to_value(&report)?;
        let evidence = exec.evidence().await?;
        result["indexes"] = json!(super::tasks::index_rows(exec, &evidence).await?);
    }
    Ok(Output::json(result))
}

async fn export_evidence(exec: &Exec, export: ExportEvidence) -> Result<Output, CliError> {
    no_clobber(&export.output)?;
    let address = if export.target.starts_with('/') {
        parse_address(&export.target).map_err(|message| CliError::new("usage", message))?
    } else {
        Address::new(format!("/{}", export.target))
    };
    match address.parts() {
        Parts::Path { path, .. } if !path.trim_matches('/').is_empty() => Err(CliError::new(
            "usage",
            format!(
                "{} 不是检材别名；导出整份检材写别名，已解锁的分区要写成明文时加 --unlocked",
                export.target
            ),
        )),
        Parts::Root => Err(CliError::new("usage", "需要检材别名")),
        Parts::Path { alias, .. } | Parts::Node { alias, .. } => {
            let evidence = exec.resolve(alias).await?;
            let result = crate::commands::evidence::image::image_export_raw(
                exec.host.clone(),
                exec.state(),
                evidence.image_id.clone(),
                export.output.text(),
                super::task_id("evidence export"),
                Some(policy(export.strict)),
                Some(if export.unlocked {
                    crate::core::filesystem::evidence_session::ImageExportMode::UnlockedPartitions
                } else {
                    crate::core::filesystem::evidence_session::ImageExportMode::Source
                }),
            )
            .await
            .map_err(operation("image_export_raw"))?;
            let mut value = flattened(&result)?;
            value["alias"] = json!(evidence.alias);
            Ok(Output::json(value))
        }
    }
}

pub fn render(args: &EvidenceArgs, value: &Value) -> Option<String> {
    let items = value["items"].as_array().map_or(&[][..], Vec::as_slice);
    match &args.command {
        None | Some(EvidenceCommand::Ls) => Some(if items.is_empty() {
            "（没有打开任何检材；用 evidence add <检材路径> 打开）\n".to_owned()
        } else {
            render::table(
                items,
                &[
                    ("别名", "path"),
                    ("格式", "sourceFormat"),
                    ("大小", "size"),
                    ("已打开", "loaded"),
                    ("案件", "inCase"),
                    ("检材路径", "evidencePath"),
                ],
            )
        }),
        Some(EvidenceCommand::Add(_)) => {
            let mut out = render::table(
                items,
                &[
                    ("别名", "address"),
                    ("格式", "sourceFormat"),
                    ("大小", "sizeBytes"),
                    ("分区", "partitionCount"),
                    ("检材路径", "path"),
                ],
            );
            if let Some(indexes) = value["indexes"].as_array() {
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
            }
            Some(out)
        }
        _ => None,
    }
}
