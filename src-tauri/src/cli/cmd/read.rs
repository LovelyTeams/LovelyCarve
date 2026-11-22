//! Reading files: raw bytes, decoded text, strings, hashes, content type,
//! named streams, and exports to the examiner's machine.
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::evidence::Algo;
use crate::{
    cli::{
        browsing::LastLink,
        exec::Exec,
        render,
        stream::{require_source, ReadPlan},
        types::{parse_address, parse_host_path, Address, HostPath},
        workspace::Target,
        CliError, Output,
    },
    commands::filesystem::filesystem_export::ExportDirMode,
    core::{common::hash::HashAlgo, filesystem::tree::fs_tree::NodeKind},
};

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatArgs {
    /// 文件
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// 起始字节偏移
    #[arg(long, default_value_t = 0)]
    pub offset: u64,
    /// 读取的字节数；默认到文件末尾
    #[arg(long)]
    pub length: Option<u64>,
    /// 读取 ZIP 中的成员（最多 --length 或 16 MiB）
    #[arg(long, value_name = "成员路径", conflicts_with = "stream")]
    pub member: Option<String>,
    /// 读取命名数据流（最多 --length 或 16 MiB）
    #[arg(long, value_name = "流名")]
    pub stream: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextArgs {
    /// 文件
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// 按此编码解码（WHATWG 标签：utf-8、gbk、gb18030、big5、shift_jis、utf-16le…）；默认自动识别
    #[arg(long)]
    pub encoding: Option<String>,
    /// 起始字节偏移；继续读下一段时传上一段的 nextOffset
    #[arg(long)]
    pub offset: Option<u64>,
    /// 本段最多读取的字节数（4–2097152）
    #[arg(long, default_value_t = 2_097_152)]
    pub max_bytes: usize,
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StringEncoding {
    Ascii,
    #[value(name = "utf-8")]
    #[serde(rename = "utf-8")]
    Utf8,
    #[value(name = "utf-16le")]
    #[serde(rename = "utf-16le")]
    Utf16le,
    #[value(name = "utf-16be")]
    #[serde(rename = "utf-16be")]
    Utf16be,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StringsArgs {
    /// 文件
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// 字符编码
    #[arg(long, value_enum, default_value_t = StringEncoding::Ascii)]
    pub encoding: StringEncoding,
    /// 最少字符数（1–4096）
    #[arg(long, default_value_t = 4)]
    pub min_length: u64,
    /// 起始字节偏移
    #[arg(long, default_value_t = 0)]
    pub offset: u64,
    /// 扫描的字节数；默认到文件末尾
    #[arg(long)]
    pub length: Option<u64>,
    /// 最多返回的字符串数（1–100000）
    #[arg(long, default_value_t = 10_000)]
    pub limit: u64,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HashArgs {
    /// 文件（可列出多个）
    #[arg(value_parser = parse_address, value_name = "路径", required = true)]
    pub paths: Vec<Address>,
    /// 算法，逗号分隔
    #[arg(long, value_enum, value_delimiter = ',', default_values_t = [Algo::Md5, Algo::Sha1, Algo::Sha256])]
    pub algo: Vec<Algo>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentifyArgs {
    /// 文件
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamsArgs {
    /// 文件或目录
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportArgs {
    /// 要导出的文件或目录（可列出多个）
    #[arg(value_parser = parse_address, value_name = "路径", required = true)]
    pub paths: Vec<Address>,
    /// 目标：一个文件或目录时为新的输出路径；多个时为新目录，各项按名称放入
    #[arg(short = 'o', long, value_parser = parse_host_path, value_name = "输出路径")]
    pub output: HostPath,
    /// 目录导出为 ZIP 文件
    #[arg(long)]
    pub zip: bool,
    /// 导出命名数据流而不是主数据
    #[arg(long, value_name = "流名")]
    pub stream: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyfileArgs {
    /// 卷或目录
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// 输出的 body 文件（必须不存在）
    #[arg(short = 'o', long, value_parser = parse_host_path, value_name = "文件")]
    pub output: HostPath,
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

/// A file address resolved, refusing the evidence root.
async fn file(exec: &Exec, address: &Address) -> Result<Target, CliError> {
    let target = exec.target(address, LastLink::Follow).await?;
    if target.location.is_empty() {
        return Err(CliError::new(
            "usage",
            format!("{} 是检材根；需要其中的文件", target.address()),
        ));
    }
    Ok(target)
}

fn no_clobber(path: &HostPath) -> Result<(), CliError> {
    if path.as_path().exists() {
        return Err(CliError::new(
            "alreadyExists",
            format!(
                "{} 已存在：导出从不覆盖已有文件，什么也没有写入；请换一个新的输出路径（重跑批量导出时用新的目录）",
                path.text()
            ),
        ));
    }
    Ok(())
}

pub async fn cat(exec: &Exec, args: CatArgs) -> Result<Output, CliError> {
    let target = file(exec, &args.path).await?;
    let max_bytes = args
        .length
        .unwrap_or(crate::cli::stream::MAX_BUFFERED)
        .min(crate::cli::stream::MAX_BUFFERED) as usize;
    if let Some(member) = args.member {
        let response = crate::commands::preview::filesystem_native_preview::fs_zip_member_read(
            exec.state(),
            target.image_id().clone(),
            target.node_id().into(),
            member,
            max_bytes,
        )
        .await
        .map_err(operation("fs_zip_member_read"))?;
        return Ok(Output::Bytes(response.into_bytes()));
    }
    if let Some(stream) = args.stream {
        let response = crate::commands::filesystem::filesystem_streams::fs_stream_preview(
            exec.state(),
            target.image_id().clone(),
            target.node_id().into(),
            stream,
            max_bytes,
        )
        .await
        .map_err(operation("fs_stream_preview"))?;
        return Ok(Output::Bytes(response.into_bytes()));
    }
    Ok(Output::Stream(ReadPlan {
        image_id: target.image_id().clone(),
        node_id: target.node_id().to_owned(),
        offset: args.offset,
        length: args.length,
    }))
}

pub async fn text(exec: &Exec, args: TextArgs) -> Result<Output, CliError> {
    let target = file(exec, &args.path).await?;
    let page = crate::commands::preview::filesystem_preview::fs_preview_text(
        exec.state(),
        target.image_id().clone(),
        target.node_id().into(),
        args.max_bytes.clamp(4, 2_097_152),
        args.offset,
        args.encoding,
    )
    .await
    .map_err(operation("fs_preview_text"))?;
    let mut value = serde_json::to_value(crate::cli::wire::Camel(page))?;
    value["path"] = json!(target.address());
    Ok(Output::json(value))
}

pub async fn strings(exec: &Exec, args: StringsArgs) -> Result<Output, CliError> {
    let target = file(exec, &args.path).await?;
    let image = exec.image(target.image_id()).await?;
    require_source(&image)?;
    let mut values = Map::new();
    values.insert("encoding".into(), serde_json::to_value(args.encoding)?);
    values.insert("minLength".into(), json!(args.min_length));
    values.insert("offset".into(), json!(args.offset));
    values.insert("limit".into(), json!(args.limit));
    if let Some(length) = args.length {
        values.insert("length".into(), json!(length));
    }
    let output =
        crate::cli::strings::read(&exec.state, image, target.node_id().to_owned(), &values).await?;
    with_path(output, &target.address())
}

/// Adds the address the result is about.
fn with_path(output: Output, address: &str) -> Result<Output, CliError> {
    match output {
        Output::Json(json) => {
            let mut value = json.value().clone();
            if let Some(fields) = value.as_object_mut() {
                fields.insert("path".into(), json!(address));
            }
            Ok(Output::json(value))
        }
        other => Ok(other),
    }
}

pub async fn hash(exec: &Exec, args: HashArgs) -> Result<Output, CliError> {
    let algos: Vec<HashAlgo> = args.algo.iter().copied().map(HashAlgo::from).collect();
    let mut items = Vec::new();
    for address in &args.paths {
        let target = file(exec, address).await?;
        let digest = crate::commands::filesystem::filesystem_analysis::fs_digest(
            exec.state(),
            target.image_id().clone(),
            target.node_id().into(),
            algos.clone(),
        )
        .await
        .map_err(operation("fs_digest"))?;
        let mut value = match crate::cli::wire::flattened_output(&digest)? {
            Output::Json(json) => json.value().clone(),
            _ => Value::Null,
        };
        value["path"] = json!(target.address());
        items.push(value);
    }
    Ok(Output::json(json!({ "items": items })))
}

pub async fn identify(exec: &Exec, args: IdentifyArgs) -> Result<Output, CliError> {
    let target = file(exec, &args.path).await?;
    let report = crate::commands::filesystem::filesystem_analysis::fs_identify_type(
        exec.state(),
        target.image_id().clone(),
        target.node_id().into(),
    )
    .await
    .map_err(operation("fs_identify_type"))?;
    with_path(crate::cli::json_output(report)?, &target.address())
}

pub async fn streams(exec: &Exec, args: StreamsArgs) -> Result<Output, CliError> {
    let target = file(exec, &args.path).await?;
    let streams = crate::commands::filesystem::filesystem_streams::fs_streams(
        exec.state(),
        target.image_id().clone(),
        target.node_id().into(),
    )
    .await
    .map_err(operation("fs_streams"))?;
    Ok(Output::json(json!({
        "path": target.address(),
        "items": serde_json::to_value(crate::cli::wire::Camel(streams))?,
    })))
}

/// The kind of the last node of `target`.
async fn kind(exec: &Exec, target: &Target) -> Result<NodeKind, CliError> {
    let mut page = |parent_id: String, cursor, page_size| {
        exec.page(target.image_id(), parent_id, cursor, page_size)
    };
    Ok(crate::cli::browsing::node(&target.location, &mut page)
        .await?
        .map_or(NodeKind::Dir, |node| node.kind))
}

async fn export_one(
    exec: &Exec,
    target: &Target,
    destination: &HostPath,
    zip: bool,
    stream: Option<&str>,
) -> Result<Value, CliError> {
    no_clobber(destination)?;
    let mut value = if let Some(stream) = stream {
        let bytes = crate::commands::filesystem::filesystem_streams::fs_stream_export(
            exec.state(),
            target.image_id().clone(),
            target.node_id().into(),
            stream.to_owned(),
            destination.text(),
        )
        .await
        .map_err(operation("fs_stream_export"))?;
        json!({"stream": stream, "bytesWritten": bytes})
    } else if target.location.is_empty() || kind(exec, target).await? == NodeKind::Dir {
        let summary = crate::commands::filesystem::filesystem_export::fs_export_dir(
            exec.host.clone(),
            exec.state(),
            target.image_id().clone(),
            target.node_id().into(),
            destination.text(),
            super::task_id("export"),
            if zip {
                ExportDirMode::Zip
            } else {
                ExportDirMode::Tree
            },
        )
        .await
        .map_err(operation("fs_export_dir"))?;
        flattened(&summary)?
    } else {
        if zip {
            return Err(CliError::new("usage", "--zip 只用于目录"));
        }
        let result = crate::commands::filesystem::filesystem_export::fs_export(
            exec.state(),
            target.image_id().clone(),
            target.node_id().into(),
            destination.text(),
        )
        .await
        .map_err(operation("fs_export"))?;
        flattened(&result)?
    };
    if let Some(fields) = value.as_object_mut() {
        fields.insert("path".into(), json!(target.address()));
        fields.insert("destination".into(), json!(destination.text()));
    }
    Ok(value)
}

fn flattened(value: &impl Serialize) -> Result<Value, CliError> {
    match crate::cli::wire::flattened_output(value)? {
        Output::Json(json) => Ok(json.value().clone()),
        _ => Err(crate::cli::internal_error("结果不是 JSON")),
    }
}

pub async fn export(exec: &Exec, args: ExportArgs) -> Result<Output, CliError> {
    no_clobber(&args.output)?;
    let mut targets = Vec::new();
    for address in &args.paths {
        targets.push(exec.target(address, LastLink::Follow).await?);
    }
    if let [target] = targets.as_slice() {
        let item = export_one(exec, target, &args.output, args.zip, args.stream.as_deref()).await?;
        return Ok(Output::json(json!({ "items": [item] })));
    }
    // Several items go into a new directory, each under its own name.
    let mut names = std::collections::HashSet::new();
    let mut planned = Vec::new();
    for target in &targets {
        let name =
            crate::core::filesystem::tree::fs_tree_export::sanitize_export_component(target.name());
        let mut name = if args.zip
            && (target.location.is_empty() || kind(exec, target).await? == NodeKind::Dir)
        {
            format!("{name}.zip")
        } else {
            name
        };
        if let Some(stream) = &args.stream {
            name = format!(
                "{name}_{}",
                crate::core::filesystem::tree::fs_tree_export::sanitize_export_component(stream)
            );
        }
        if !names.insert(name.to_lowercase()) {
            return Err(CliError::new(
                "usage",
                format!("多个导出项同名：{name}；分别导出到不同目录"),
            ));
        }
        planned.push(name);
    }
    std::fs::create_dir_all(
        args.output
            .as_path()
            .parent()
            .unwrap_or(args.output.as_path()),
    )?;
    std::fs::create_dir(args.output.as_path()).map_err(|error| {
        CliError::new(
            "alreadyExists",
            format!("无法创建输出目录 {}：{error}", args.output.text()),
        )
    })?;
    let mut items = Vec::new();
    for (target, name) in targets.iter().zip(planned) {
        let destination = HostPath::new(args.output.as_path().join(name));
        items.push(export_one(exec, target, &destination, args.zip, args.stream.as_deref()).await?);
    }
    Ok(Output::json(
        json!({ "items": items, "destination": args.output.text() }),
    ))
}

pub async fn bodyfile(exec: &Exec, args: BodyfileArgs) -> Result<Output, CliError> {
    no_clobber(&args.output)?;
    let target = exec.target(&args.path, LastLink::Follow).await?;
    let summary = crate::commands::filesystem::filesystem_export::fs_timeline_export(
        exec.state(),
        target.image_id().clone(),
        target.node_id().into(),
        args.output.text(),
    )
    .await
    .map_err(operation("fs_timeline_export"))?;
    with_path(crate::cli::json_output(summary)?, &target.address())
}

pub fn render_strings(value: &Value) -> String {
    let mut out = String::new();
    for record in value["strings"].as_array().map_or(&[][..], Vec::as_slice) {
        out.push_str(&format!(
            "{:>10}  {}\n",
            record["offset"].as_u64().unwrap_or_default(),
            render::cell(&record["text"])
        ));
    }
    if value["truncated"] == Value::Bool(true) {
        out.push_str(&crate::cli::output::paint(
            crate::cli::output::Role::Muted,
            "… 达到上限或只扫描了部分文件：用 --offset/--length/--limit 继续\n",
        ));
    }
    out
}

/// Digests in the BSD tagged form `sha256sum -c` and friends understand.
pub fn render_hash(value: &Value) -> String {
    let mut out = String::new();
    for item in value["items"].as_array().map_or(&[][..], Vec::as_slice) {
        for (algo, name) in [
            ("md5", "MD5"),
            ("sha1", "SHA1"),
            ("sha256", "SHA256"),
            ("sm3", "SM3"),
        ] {
            if let Some(digest) = item[algo].as_str() {
                out.push_str(&format!(
                    "{name} ({}) = {digest}\n",
                    render::cell(&item["path"])
                ));
            }
        }
        if item["readFaultsDetected"] == Value::Bool(true) {
            out.push_str(&format!(
                "  ↑ 含 {} 字节合成补零，摘要不代表完整原文件\n",
                render::cell(&item["syntheticBytes"])
            ));
        }
    }
    out
}

pub fn render_export(value: &Value) -> String {
    let mut out = String::new();
    for item in value["items"].as_array().map_or(&[][..], Vec::as_slice) {
        out.push_str(&format!(
            "{} → {}\n",
            render::cell(&item["path"]),
            render::cell(&item["destination"])
        ));
    }
    out
}
