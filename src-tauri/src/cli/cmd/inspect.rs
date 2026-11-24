//! Structured files: one `inspect` that recognizes the format, SQLite with
//! its own verbs, and text recognition in images.
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    cli::{
        browsing::LastLink,
        exec::Exec,
        stream::{self, ReadPlan},
        types::{parse_address, parse_host_path, parse_secret, Address, HostPath, Secret},
        workspace::Target,
        CliError, Output,
    },
    core::{
        common::ids::NodeId,
        preview::{
            kdbx_preview::{KdbxCredentials, KdbxKeyfile},
            sqlite::sqlite_preview::SqliteKey,
        },
    },
};

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    Auto,
    Sqlite,
    Realm,
    Zip,
    Csv,
    Binary,
    Gguf,
    VimSwap,
    Kdbx,
}

#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KdbxKeyArgs {
    /// KeePass 主密码（@文件、- 或在终端中输入）
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_parser = parse_secret, value_name = "@文件")]
    pub password: Option<Secret>,
    /// KeePass 密钥文件（主机上的文件）
    #[arg(long, value_parser = parse_host_path, value_name = "文件")]
    pub keyfile: Option<HostPath>,
    /// KeePass 转换密钥（64 个十六进制字符）
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_parser = parse_secret, value_name = "@文件")]
    pub transformed_key: Option<Secret>,
}

impl KdbxKeyArgs {
    fn credentials(&self) -> Result<Option<KdbxCredentials>, CliError> {
        if self.password.is_none() && self.keyfile.is_none() && self.transformed_key.is_none() {
            return Ok(None);
        }
        Ok(Some(KdbxCredentials {
            password: self
                .password
                .as_ref()
                .map(Secret::value)
                .transpose()?
                .map(str::to_owned),
            keyfile: self
                .keyfile
                .as_ref()
                .map(|path| KdbxKeyfile::Host { path: path.text() }),
            transformed_key: self
                .transformed_key
                .as_ref()
                .map(Secret::value)
                .transpose()?
                .map(str::to_owned),
        }))
    }

    fn secrets_mut(&mut self) -> Vec<(&'static str, &mut Secret)> {
        let mut secrets = Vec::new();
        if let Some(secret) = self.password.as_mut() {
            secrets.push(("password", secret));
        }
        if let Some(secret) = self.transformed_key.as_mut() {
            secrets.push(("transformed-key", secret));
        }
        secrets
    }
}

#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteKeyArgs {
    /// SQLCipher 口令（@文件、- 或在终端中输入）
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_parser = parse_secret, value_name = "@文件")]
    pub passphrase: Option<Secret>,
    /// SQLCipher 原始密钥（十六进制）
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_parser = parse_secret, value_name = "@文件")]
    pub raw_key: Option<Secret>,
    /// SQLCipher 兼容版本（1–4）
    #[arg(long, value_name = "版本")]
    pub cipher_compat: Option<u8>,
    /// SQLCipher 页大小
    #[arg(long, value_name = "字节")]
    pub cipher_page_size: Option<u32>,
    /// SQLCipher KDF 迭代次数
    #[arg(long, value_name = "次数")]
    pub kdf_iter: Option<u32>,
}

impl SqliteKeyArgs {
    fn key(&self) -> Result<Option<SqliteKey>, CliError> {
        if self.passphrase.is_none() && self.raw_key.is_none() {
            return Ok(None);
        }
        Ok(Some(SqliteKey {
            passphrase: self
                .passphrase
                .as_ref()
                .map(Secret::value)
                .transpose()?
                .map(str::to_owned),
            raw_hex_key: self
                .raw_key
                .as_ref()
                .map(Secret::value)
                .transpose()?
                .map(str::to_owned),
            compatibility: self.cipher_compat,
            page_size: self.cipher_page_size,
            kdf_iter: self.kdf_iter,
        }))
    }

    fn secrets_mut(&mut self) -> Vec<(&'static str, &mut Secret)> {
        let mut secrets = Vec::new();
        if let Some(secret) = self.passphrase.as_mut() {
            secrets.push(("passphrase", secret));
        }
        if let Some(secret) = self.raw_key.as_mut() {
            secrets.push(("raw-key", secret));
        }
        secrets
    }
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InspectArgs {
    /// 文件
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// 按此格式解析；默认按文件头与扩展名识别
    #[arg(long = "as", value_enum, default_value_t = Format::Auto)]
    pub format: Format,
    /// CSV 分隔符；默认自动识别
    #[arg(long, value_name = "字符")]
    pub delimiter: Option<String>,
    #[command(flatten)]
    pub key: SqliteKeyArgs,
    #[command(flatten)]
    #[serde(default)]
    pub kdbx: KdbxKeyArgs,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteArgs {
    #[command(subcommand)]
    pub command: SqliteCommand,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SqliteCommand {
    /// 表、视图与索引的结构及行数
    Tables(SqliteTarget),
    /// 分页读取一张表的记录
    Rows(SqliteRows),
    /// 执行一条只读 SELECT（或 WITH … SELECT）；自动读取同目录的 WAL
    Query(SqliteQuery),
    /// 从空闲页、日志与 WAL 中恢复删除的记录
    Deleted(SqliteTarget),
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteTarget {
    /// 数据库文件
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    #[command(flatten)]
    pub key: SqliteKeyArgs,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteRows {
    /// 数据库文件
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// 表名
    #[arg(value_name = "表")]
    pub table: String,
    /// 跳过的行数
    #[arg(long, default_value_t = 0)]
    pub offset: i64,
    /// 每页行数
    #[arg(long, default_value_t = 100)]
    pub limit: i64,
    /// 排序列
    #[arg(long, value_name = "列")]
    pub order_by: Option<String>,
    /// 降序
    #[arg(long)]
    pub desc: bool,
    #[command(flatten)]
    pub key: SqliteKeyArgs,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteQuery {
    /// 数据库文件
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// 单条只读 SELECT 或 WITH … SELECT；? 或 ?1 为占位符
    #[arg(value_name = "SQL")]
    pub sql: String,
    /// 绑定参数，按顺序对应占位符（可重复）；JSON 标量保持类型，其余按字符串
    #[arg(long = "param", value_name = "值", allow_hyphen_values = true)]
    pub params: Vec<String>,
    /// 跳过的行数；下一页传上一页的 nextOffset
    #[arg(long, default_value_t = 0)]
    pub offset: u32,
    /// 每页行数（1–100）
    #[arg(long, default_value_t = 50)]
    pub limit: u32,
    #[command(flatten)]
    pub key: SqliteKeyArgs,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageArgs {
    /// 图片文件
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QrArgs {
    /// 图片文件
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// 找到这么多个二维码后提前停止；0 表示找全
    #[arg(long, default_value_t = 0)]
    pub expect: u32,
}

pub fn sqlite_path(args: &SqliteArgs) -> &'static str {
    match &args.command {
        SqliteCommand::Tables(_) => "sqlite tables",
        SqliteCommand::Rows(_) => "sqlite rows",
        SqliteCommand::Query(_) => "sqlite query",
        SqliteCommand::Deleted(_) => "sqlite deleted",
    }
}

pub fn sqlite_secrets_mut(args: &mut SqliteArgs) -> Vec<(&'static str, &mut Secret)> {
    match &mut args.command {
        SqliteCommand::Tables(target) | SqliteCommand::Deleted(target) => target.key.secrets_mut(),
        SqliteCommand::Rows(rows) => rows.key.secrets_mut(),
        SqliteCommand::Query(query) => query.key.secrets_mut(),
    }
}

pub fn inspect_secrets_mut(args: &mut InspectArgs) -> Vec<(&'static str, &mut Secret)> {
    let mut secrets = args.key.secrets_mut();
    secrets.extend(args.kdbx.secrets_mut());
    secrets
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

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

fn parent(target: &Target) -> Option<NodeId> {
    target.parent_id().map(NodeId::from)
}

fn with_path(
    value: impl Serialize,
    target: &Target,
    format: Option<&str>,
) -> Result<Output, CliError> {
    let mut value = serde_json::to_value(crate::cli::wire::Camel(value))?;
    if let Some(fields) = value.as_object_mut() {
        fields.insert("path".into(), json!(target.address()));
        if let Some(format) = format {
            fields.insert("format".into(), json!(format));
        }
    }
    Ok(Output::json(value))
}

/// The format a file's first bytes, then its name, say it has.
fn recognize(head: &[u8], name: &str) -> Option<Format> {
    let starts = |magic: &[u8]| head.starts_with(magic);
    if starts(b"SQLite format 3\0") {
        return Some(Format::Sqlite);
    }
    if starts(b"PK\x03\x04") || starts(b"PK\x05\x06") {
        return Some(Format::Zip);
    }
    if starts(b"GGUF") {
        return Some(Format::Gguf);
    }
    if starts(b"b0VIM") {
        return Some(Format::VimSwap);
    }
    // KeePass 2.x / KDBX, and the KeePass 2 pre-release signature.
    if starts(b"\x03\xd9\xa2\x9a\x67\xfb\x4b\xb5") || starts(b"\x03\xd9\xa2\x9a\x66\xfb\x4b\xb5") {
        return Some(Format::Kdbx);
    }
    if head.get(16..20) == Some(b"T-DB") {
        return Some(Format::Realm);
    }
    if starts(b"MZ")
        || starts(b"\x7fELF")
        || starts(b"!<arch>\n")
        || [
            [0xFE, 0xED, 0xFA, 0xCE],
            [0xFE, 0xED, 0xFA, 0xCF],
            [0xCE, 0xFA, 0xED, 0xFE],
            [0xCF, 0xFA, 0xED, 0xFE],
            [0xCA, 0xFE, 0xBA, 0xBE],
        ]
        .iter()
        .any(|magic| starts(magic))
    {
        return Some(Format::Binary);
    }
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_lowercase())?;
    match extension.as_str() {
        "db" | "sqlite" | "sqlite3" | "db3" | "sqlitedb" => Some(Format::Sqlite),
        "realm" => Some(Format::Realm),
        "zip" | "apk" | "jar" | "ipa" | "docx" | "xlsx" | "pptx" => Some(Format::Zip),
        "csv" | "tsv" => Some(Format::Csv),
        "gguf" => Some(Format::Gguf),
        "swp" | "swo" => Some(Format::VimSwap),
        "kdbx" => Some(Format::Kdbx),
        "exe" | "dll" | "sys" | "so" | "dylib" | "elf" | "o" | "a" | "ko" => Some(Format::Binary),
        _ => None,
    }
}

pub async fn inspect(exec: &Exec, args: InspectArgs) -> Result<Output, CliError> {
    let target = file(exec, &args.path).await?;
    let format = match args.format {
        Format::Auto => {
            let head = stream::buffer(
                exec,
                ReadPlan {
                    image_id: target.image_id().clone(),
                    node_id: target.node_id().to_owned(),
                    offset: 0,
                    length: Some(64),
                },
            )
            .await?;
            recognize(&head, target.name()).ok_or_else(|| {
                CliError::new(
                    "unsupportedFormat",
                    format!(
                        "无法识别 {} 的格式；用 --as sqlite|realm|zip|csv|binary|gguf|vim-swap|kdbx 指定，或用 identify 识别内容类型",
                        target.address()
                    ),
                )
            })?
        }
        format => format,
    };
    let (image_id, node_id) = (target.image_id().clone(), NodeId::from(target.node_id()));
    let state = exec.state();
    use crate::commands::preview::filesystem_native_preview as preview;
    match format {
        Format::Sqlite => with_path(
            preview::fs_sqlite_preview(state, image_id, node_id, parent(&target), args.key.key()?)
                .await
                .map_err(operation("fs_sqlite_preview"))?,
            &target,
            Some("sqlite"),
        ),
        Format::Realm => with_path(
            preview::fs_realm_preview(state, image_id, node_id)
                .await
                .map_err(operation("fs_realm_preview"))?,
            &target,
            Some("realm"),
        ),
        Format::Zip => with_path(
            preview::fs_zip_preview(state, image_id, node_id)
                .await
                .map_err(operation("fs_zip_preview"))?,
            &target,
            Some("zip"),
        ),
        Format::Csv => with_path(
            preview::fs_csv_preview(state, image_id, node_id, args.delimiter)
                .await
                .map_err(operation("fs_csv_preview"))?,
            &target,
            Some("csv"),
        ),
        Format::Binary => with_path(
            preview::fs_binary_preview(state, image_id, node_id)
                .await
                .map_err(operation("fs_binary_preview"))?,
            &target,
            Some("binary"),
        ),
        Format::Gguf => with_path(
            preview::fs_gguf_preview(state, image_id, node_id)
                .await
                .map_err(operation("fs_gguf_preview"))?,
            &target,
            Some("gguf"),
        ),
        Format::VimSwap => with_path(
            preview::fs_vim_swap_preview(state, image_id, node_id)
                .await
                .map_err(operation("fs_vim_swap_preview"))?,
            &target,
            Some("vim-swap"),
        ),
        Format::Kdbx => {
            use crate::commands::preview::kdbx_preview as kdbx;
            match args.kdbx.credentials()? {
                None => with_path(
                    kdbx::fs_kdbx_header(state, image_id, node_id)
                        .await
                        .map_err(operation("fs_kdbx_header"))?,
                    &target,
                    Some("kdbx"),
                ),
                Some(credentials) => with_path(
                    kdbx::fs_kdbx_open(state, image_id, node_id, credentials)
                        .await
                        .map_err(operation("fs_kdbx_open"))?,
                    &target,
                    Some("kdbx"),
                ),
            }
        }
        Format::Auto => Err(crate::cli::internal_error("格式未确定")),
    }
}

/// A bound parameter: a JSON scalar keeps its type, anything else is text.
fn parameter(text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(value @ (Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_))) => value,
        _ => Value::String(text.to_owned()),
    }
}

pub async fn sqlite(exec: &Exec, args: SqliteArgs) -> Result<Output, CliError> {
    use crate::commands::preview::filesystem_native_preview as preview;
    match args.command {
        SqliteCommand::Tables(target_args) => {
            let target = file(exec, &target_args.path).await?;
            with_path(
                preview::fs_sqlite_preview(
                    exec.state(),
                    target.image_id().clone(),
                    target.node_id().into(),
                    parent(&target),
                    target_args.key.key()?,
                )
                .await
                .map_err(operation("fs_sqlite_preview"))?,
                &target,
                None,
            )
        }
        SqliteCommand::Rows(rows) => {
            let target = file(exec, &rows.path).await?;
            with_path(
                preview::fs_sqlite_table_rows(
                    exec.state(),
                    target.image_id().clone(),
                    target.node_id().into(),
                    parent(&target),
                    rows.table,
                    rows.offset.max(0),
                    rows.limit.clamp(1, 10_000),
                    rows.order_by,
                    rows.desc,
                    rows.key.key()?,
                )
                .await
                .map_err(operation("fs_sqlite_table_rows"))?,
                &target,
                None,
            )
        }
        SqliteCommand::Query(query) => {
            let target = file(exec, &query.path).await?;
            let parameters = (!query.params.is_empty())
                .then(|| query.params.iter().map(|text| parameter(text)).collect());
            with_path(
                preview::fs_sqlite_query(
                    exec.state(),
                    target.image_id().clone(),
                    target.node_id().into(),
                    parent(&target),
                    query.sql,
                    parameters,
                    query.offset,
                    query.limit.clamp(1, 100),
                    query.key.key()?,
                )
                .await
                .map_err(operation("fs_sqlite_query"))?,
                &target,
                None,
            )
        }
        SqliteCommand::Deleted(target_args) => {
            let target = file(exec, &target_args.path).await?;
            with_path(
                preview::fs_sqlite_deleted_records(
                    exec.state(),
                    target.image_id().clone(),
                    target.node_id().into(),
                    parent(&target),
                    target_args.key.key()?,
                )
                .await
                .map_err(operation("fs_sqlite_deleted_records"))?,
                &target,
                None,
            )
        }
    }
}

pub async fn ocr(exec: &Exec, args: ImageArgs) -> Result<Output, CliError> {
    let target = file(exec, &args.path).await?;
    let outcome = crate::commands::preview::ocr::ocr_scan(
        exec.state(),
        target.image_id().clone(),
        target.node_id().into(),
        Some(super::task_id("ocr")),
    )
    .await
    .map_err(operation("ocr_scan"))?;
    with_path(outcome, &target, None)
}

pub async fn qr(exec: &Exec, args: QrArgs) -> Result<Output, CliError> {
    let target = file(exec, &args.path).await?;
    let outcome = crate::commands::preview::qrcode::qr_scan(
        exec.host.clone(),
        exec.state(),
        target.image_id().clone(),
        target.node_id().into(),
        args.expect,
        super::task_id("qr"),
    )
    .await
    .map_err(operation("qr_scan"))?;
    with_path(outcome, &target, None)
}

/// One SQLite cell as text: its value, `NULL`, or the size of a blob.
fn sqlite_cell(cell: &Value) -> String {
    match cell["kind"].as_str() {
        Some("null") => "NULL".to_owned(),
        Some("blob") => match cell["size"].as_u64().or_else(|| cell["length"].as_u64()) {
            Some(size) => format!("<blob {size} B>"),
            None => "<blob>".to_owned(),
        },
        _ => crate::cli::render::cell(&cell["value"]),
    }
}

fn sqlite_rows(header: Vec<String>, rows: &[Value]) -> String {
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            row.as_array()
                .or_else(|| row["values"].as_array())
                .map(|cells| cells.iter().map(sqlite_cell).collect())
                .unwrap_or_default()
        })
        .collect();
    if rows.is_empty() {
        return "（没有记录）\n".to_owned();
    }
    crate::cli::render::table_rows(header, rows)
}

/// Tables of rows for the SQLite commands that return rows.
pub fn render_sqlite(args: &SqliteArgs, value: &Value) -> Option<String> {
    match &args.command {
        SqliteCommand::Rows(_) => {
            let header = value["columns"]
                .as_array()
                .map(|columns| {
                    columns
                        .iter()
                        .map(|column| crate::cli::render::cell(&column["name"]))
                        .collect()
                })
                .unwrap_or_default();
            let mut out = sqlite_rows(
                header,
                value["rows"].as_array().map_or(&[][..], Vec::as_slice),
            );
            if let (Some(offset), Some(returned), Some(total)) = (
                value["offset"].as_u64(),
                value["returned"].as_u64(),
                value["total"].as_u64(),
            ) {
                if offset + returned < total {
                    out.push_str(&crate::cli::output::paint(
                        crate::cli::output::Role::Muted,
                        &format!("… 共 {total} 行：用 --offset {} 继续\n", offset + returned),
                    ));
                }
            }
            Some(out)
        }
        SqliteCommand::Query(_) => {
            let header = value["columns"]
                .as_array()
                .map(|columns| columns.iter().map(crate::cli::render::cell).collect())
                .unwrap_or_default();
            let mut out = sqlite_rows(
                header,
                value["rows"].as_array().map_or(&[][..], Vec::as_slice),
            );
            if let Some(next) = value["nextOffset"].as_u64() {
                out.push_str(&crate::cli::output::paint(
                    crate::cli::output::Role::Muted,
                    &format!("… 还有更多：用同样的 SQL 和 --offset {next} 继续\n"),
                ));
            }
            Some(out)
        }
        SqliteCommand::Deleted(_) => {
            let records = value["records"].as_array().map_or(&[][..], Vec::as_slice);
            if records.is_empty() {
                return Some("（没有恢复出删除记录）\n".to_owned());
            }
            let rows: Vec<Vec<String>> = records
                .iter()
                .map(|record| {
                    let values: Vec<String> = record["values"]
                        .as_array()
                        .map(|cells| cells.iter().map(sqlite_cell).collect())
                        .unwrap_or_default();
                    vec![
                        crate::cli::render::cell(&record["table"]),
                        crate::cli::render::cell(&record["page"]),
                        crate::cli::render::cell(&record["offset"]),
                        crate::cli::render::cell(&record["origin"]),
                        values.join(" | "),
                    ]
                })
                .collect();
            Some(crate::cli::render::table_rows(
                ["表", "页", "偏移", "来源", "值"]
                    .map(str::to_owned)
                    .to_vec(),
                rows,
            ))
        }
        SqliteCommand::Tables(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_rows_read_as_tables() {
        let args = SqliteArgs {
            command: SqliteCommand::Query(SqliteQuery {
                path: Address::new("/x/db"),
                sql: "SELECT 1".into(),
                params: vec![],
                offset: 0,
                limit: 50,
                key: SqliteKeyArgs::default(),
            }),
        };
        let value = json!({"columns": ["chat_id", "body"], "rows": [
            [{"kind": "integer", "value": "10"}, {"kind": "text", "value": "secret"}],
            [{"kind": "integer", "value": "11"}, {"kind": "null", "value": null}]
        ], "nextOffset": 2});
        let text = render_sqlite(&args, &value).unwrap();
        assert!(
            text.starts_with("chat_id  body\n10       secret\n11       NULL\n"),
            "{text}"
        );
        assert!(text.contains("--offset 2"));
    }

    #[test]
    fn formats_are_recognized_by_header_then_name() {
        assert_eq!(
            recognize(b"SQLite format 3\0rest", "x.bin"),
            Some(Format::Sqlite)
        );
        assert_eq!(recognize(b"PK\x03\x04", "a"), Some(Format::Zip));
        assert_eq!(recognize(b"\x7fELF\x02", "a"), Some(Format::Binary));
        let mut realm = vec![0u8; 24];
        realm[16..20].copy_from_slice(b"T-DB");
        assert_eq!(recognize(&realm, "default"), Some(Format::Realm));
        assert_eq!(recognize(b"a,b,c\n", "data.CSV"), Some(Format::Csv));
        assert_eq!(
            recognize(
                b"\x03\xd9\xa2\x9a\x67\xfb\x4b\xb5\x01\x00\x04\x00",
                "vault.bin"
            ),
            Some(Format::Kdbx)
        );
        assert_eq!(recognize(b"", "Passwords.KDBX"), Some(Format::Kdbx));
        // KeePass 1.x shares the first signature but is not KDBX.
        assert_eq!(
            recognize(b"\x03\xd9\xa2\x9a\x65\xfb\x4b\xb5", "old.kdb"),
            None
        );
        assert_eq!(recognize(b"", "notes.txt"), None);
        assert_eq!(parameter("5"), json!(5));
        assert_eq!(parameter("2026-08-01"), json!("2026-08-01"));
        assert_eq!(parameter("\"5\""), json!("5"));
        assert_eq!(parameter("[1]"), json!("[1]"));
    }
}
