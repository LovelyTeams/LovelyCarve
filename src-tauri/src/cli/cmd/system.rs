//! Time zones, the host, and the commands about the CLI itself: `schema`,
//! `completion`, `batch` and `rpc`.
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::cli::{
    browsing::LastLink,
    exec::{Exec, RequestOptions},
    types::{parse_address, Address, Parts},
    CliError, Output,
};

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[command(args_conflicts_with_subcommands = true)]
pub struct TzArgs {
    #[command(subcommand)]
    pub command: Option<TzCommand>,
    /// 检材或其中的路径：看这个位置的有效时区与识别结果
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Option<Address>,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TzCommand {
    /// 保存显示时区：给检材路径时只对该检材，否则对整个案件；auto 清除设置
    Set(TzSet),
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TzSet {
    /// IANA 名称（Asia/Shanghai）、UTC、±HH:MM 或 auto
    #[arg(value_name = "时区", allow_hyphen_values = true)]
    pub zone: String,
    /// 只对这份检材保存
    #[arg(value_parser = parse_address, value_name = "检材")]
    pub path: Option<Address>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaArgs {
    /// 命令，如 ls 或 sqlite query；省略时列出全部命令
    #[arg(value_name = "命令")]
    pub words: Vec<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shell {
    Bash,
    Zsh,
    Fish,
    Powershell,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionArgs {
    /// shell 类型
    #[arg(value_enum)]
    pub shell: Shell,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompleteArgs {
    /// 已输入的单词
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub words: Vec<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BatchArgs {
    /// 允许并行的只读请求数（1–64）
    #[arg(short = 'j', long, default_value_t = 1)]
    pub jobs: usize,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RpcArgs {
    /// 后端命令名，如 fs_size_tree（rpc --list 列出）
    #[arg(value_name = "命令", required_unless_present = "list")]
    pub operation: Option<String>,
    /// 参数 JSON 对象，键为 IPC 参数名（文本或 @文件）；imageId 也可以写检材别名
    #[arg(long, value_parser = parse_body, value_name = "JSON")]
    pub body: Option<Value>,
    /// 列出全部后端命令及其参数
    #[arg(long)]
    pub list: bool,
}

fn parse_body(text: &str) -> Result<Value, String> {
    crate::cli::types::parse_json_text(text).map_err(|error| error.message)
}

pub fn tz_path(args: &TzArgs) -> &'static str {
    match args.command {
        None => "tz",
        Some(TzCommand::Set(_)) => "tz set",
    }
}

/// The evidence an optional address names.
async fn image_of(
    exec: &Exec,
    path: Option<&Address>,
) -> Result<Option<crate::core::common::ids::ImageId>, CliError> {
    let Some(path) = path.filter(|path| path.parts() != Parts::Root) else {
        return Ok(None);
    };
    Ok(Some(
        exec.target(path, LastLink::Follow)
            .await?
            .image_id()
            .clone(),
    ))
}

pub async fn tz(exec: &Exec, args: TzArgs, options: &RequestOptions) -> Result<Output, CliError> {
    match args.command {
        None => {
            let image = match image_of(exec, args.path.as_ref()).await? {
                Some(image) => Some(image),
                None => exec
                    .resolve_default(None)
                    .await
                    .ok()
                    .map(|evidence| evidence.image_id),
            };
            Ok(Output::json(
                crate::cli::timezone::get(exec, image.as_ref(), options.tz.as_deref()).await?,
            ))
        }
        Some(TzCommand::Set(set)) => {
            let image = image_of(exec, set.path.as_ref()).await?;
            Ok(Output::json(
                crate::cli::timezone::set(exec, image.as_ref(), &set.zone).await?,
            ))
        }
    }
}

pub async fn disks() -> Result<Output, CliError> {
    crate::cli::json_output(
        crate::commands::evidence::image::physical_disks_list()
            .await
            .map_err(|error| {
                crate::cli::command_error(error).in_operation("physical_disks_list")
            })?,
    )
}

/// Every backend operation `rpc` can call, with its parameters.
pub fn operations() -> Result<Value, CliError> {
    serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/cli_rpc_operations.json"
    )))
    .map_err(crate::cli::internal_error)
}

pub async fn rpc(exec: &Exec, args: RpcArgs) -> Result<Output, CliError> {
    if args.list {
        return Ok(Output::json(json!({ "items": operations()? })));
    }
    let operation = args
        .operation
        .ok_or_else(|| CliError::new("usage", "需要后端命令名；rpc --list 列出全部"))?;
    let mut body = match args.body {
        Some(Value::Object(body)) => body,
        Some(_) => return Err(CliError::new("usage", "--body 必须是 JSON 对象")),
        None => Map::new(),
    };
    // Evidence may be named by its alias, as everywhere else.
    if let Some(Value::String(reference)) = body.get("imageId").cloned() {
        if let Ok(evidence) = exec.resolve(reference.trim_start_matches('/')).await {
            body.insert("imageId".into(), json!(evidence.image_id));
        }
    }
    let foreground = exec.background.foreground();
    let result = crate::cli::generated::dispatch(&exec.host, &exec.state, &operation, body).await;
    foreground.finish(&result);
    result
}

/// What `system` reports: the build, its optional features and components,
/// and the protocols other programs speak to it.
pub fn capabilities() -> Value {
    use lovelycarve_component_abi::inference::OcrExecution;
    let inference = crate::core::platform::components::inference::load();
    let ocr = inference
        .as_ref()
        .ok()
        .and_then(|inference| inference.capabilities().ocr);
    let component = match &inference {
        Ok(inference) => {
            let capabilities = inference.capabilities();
            json!({
                "available": true,
                "path": inference.path().display().to_string(),
                "version": inference.version(),
                "fileTypes": capabilities.file_types,
                "ocr": capabilities.ocr,
            })
        }
        Err(error) => json!({"available": false, "error": format!("{error:#}")}),
    };
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "platform": std::env::consts::OS,
        "standalone": !cfg!(feature = "desktop"),
        "qrcode": cfg!(feature = "qrcode-wechat"),
        "ocr": ocr.is_some(),
        "ocrCuda": ocr == Some(OcrExecution::Cuda),
        "dokanMount": cfg!(all(windows, not(target_arch = "aarch64"))),
        "components": {"inference": component},
        "output": {"default": "text", "json": "--json[=fields]", "errors": "framed text on stderr; one JSON line with --json"},
        "exitCodes": {"success": 0, "failure": 1, "incompleteResult": crate::cli::incomplete::EXIT_INCOMPLETE, "interrupted": 130},
        "backend": {"protocolVersion": crate::cli::backend::PROTOCOL_VERSION, "open": "case open <dir>", "close": "case close <dir>"},
        "batch": {"request": {"id": "string", "argv": "[string]", "output": "optional new file for the result"},
            "references": "{\"$ref\":\"<id>#/<json pointer>\"} as an argv element", "maxJobs": 64},
        "rpcOperations": operations().ok().and_then(|items| items.as_array().map(Vec::len)),
    })
}
