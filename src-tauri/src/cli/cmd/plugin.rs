//! Plugin packages and the plugin market: list, inspect, install and remove
//! packages; manage index sources, list a source and install from it; write
//! a market directory's `index.json`.
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    app::error::AppError,
    cli::{
        exec::Exec,
        render,
        types::{parse_host_path, HostPath},
        CliError, Output,
    },
    commands::extensions::{plugin as plugins, plugin_market as market},
};

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginArgs {
    #[command(subcommand)]
    pub command: Option<PluginCommand>,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginCommand {
    /// 列出插件包（默认）
    Ls,
    /// 检视插件包（.lcpkg）：标识、版本、权限、摘要与已安装版本的关系，不安装
    Inspect(PackageArgs),
    /// 安装插件包（.lcpkg）：检视后安装同一份字节，替换已安装的同 ID 版本
    Install(PackageArgs),
    /// 卸载插件包
    Rm(PluginId),
    /// 插件包的说明（README.md）
    Readme(PluginId),
    /// 插件市场：索引源、清单与安装
    Market(MarketArgs),
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageArgs {
    /// 插件包文件
    #[arg(value_parser = parse_host_path, value_name = "插件包")]
    pub package: HostPath,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginId {
    /// 插件 ID（plugin ls 返回）
    #[arg(value_name = "插件ID")]
    pub plugin_id: String,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketArgs {
    #[command(subcommand)]
    pub command: Option<MarketCommand>,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MarketCommand {
    /// 列出索引源（默认）
    Sources,
    /// 添加索引源：https:// 地址，或本地市场目录、index.json 的绝对路径
    Add(SourceArg),
    /// 移除索引源
    Rm(SourceArg),
    /// 列出一个索引源的插件及其安装状态
    Ls(MarketListArgs),
    /// 从索引源下载插件包，按索引校验大小与 SHA-256，检视后安装
    Install(MarketInstallArgs),
    /// 为目录中的 .lcpkg 生成 index.json
    Index(MarketIndexArgs),
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceArg {
    /// 索引源
    #[arg(value_name = "索引源")]
    pub source: String,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketListArgs {
    /// 索引源；省略时取第一个
    #[arg(long, value_name = "索引源")]
    pub source: Option<String>,
    /// 重新读取索引，不用十分钟内读到的
    #[arg(long)]
    pub refresh: bool,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketInstallArgs {
    /// 插件 ID（plugin market ls 返回）
    #[arg(value_name = "插件ID")]
    pub plugin_id: String,
    /// 版本；省略时取当前平台可安装的最新版本
    #[arg(long, value_name = "版本")]
    pub version: Option<String>,
    /// 索引源；省略时取第一个
    #[arg(long, value_name = "索引源")]
    pub source: Option<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketIndexArgs {
    /// 放着 .lcpkg 的市场目录；index.json 写在这里
    #[arg(value_parser = parse_host_path, value_name = "目录")]
    pub directory: HostPath,
    /// 市场标题
    #[arg(long, value_name = "标题")]
    pub title: String,
    /// 市场说明
    #[arg(long, value_name = "说明")]
    pub description: Option<String>,
}

pub fn path(args: &PluginArgs) -> &'static str {
    match &args.command {
        None | Some(PluginCommand::Ls) => "plugin ls",
        Some(PluginCommand::Inspect(_)) => "plugin inspect",
        Some(PluginCommand::Install(_)) => "plugin install",
        Some(PluginCommand::Rm(_)) => "plugin rm",
        Some(PluginCommand::Readme(_)) => "plugin readme",
        Some(PluginCommand::Market(market)) => match &market.command {
            None | Some(MarketCommand::Sources) => "plugin market sources",
            Some(MarketCommand::Add(_)) => "plugin market add",
            Some(MarketCommand::Rm(_)) => "plugin market rm",
            Some(MarketCommand::Ls(_)) => "plugin market ls",
            Some(MarketCommand::Install(_)) => "plugin market install",
            Some(MarketCommand::Index(_)) => "plugin market index",
        },
    }
}

fn operation(name: &'static str) -> impl Fn(AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

pub async fn run(exec: &Exec, args: PluginArgs) -> Result<Output, CliError> {
    match args.command.unwrap_or(PluginCommand::Ls) {
        PluginCommand::Ls => crate::cli::json_output(
            plugins::plugin_list(exec.state())
                .await
                .map_err(operation("plugin_list"))?,
        ),
        PluginCommand::Inspect(package) => crate::cli::json_output(
            plugins::plugin_inspect(exec.state(), package.package.text())
                .await
                .map_err(operation("plugin_inspect"))?,
        ),
        PluginCommand::Install(package) => {
            let archive = package.package.text();
            let inspection = plugins::plugin_inspect(exec.state(), archive.clone())
                .await
                .map_err(operation("plugin_inspect"))?;
            install(exec, archive, &inspection).await
        }
        PluginCommand::Rm(id) => {
            plugins::plugin_uninstall(exec.state(), id.plugin_id.clone())
                .await
                .map_err(operation("plugin_uninstall"))?;
            Ok(Output::json(
                json!({"pluginId": id.plugin_id, "removed": true}),
            ))
        }
        PluginCommand::Readme(id) => {
            let readme = plugins::plugin_readme(exec.state(), id.plugin_id.clone())
                .await
                .map_err(operation("plugin_readme"))?;
            Ok(Output::json(
                json!({"pluginId": id.plugin_id, "readme": readme}),
            ))
        }
        PluginCommand::Market(args) => run_market(exec, args).await,
    }
}

/// Installs an inspected archive, unless the inspection found it blocked.
async fn install(
    exec: &Exec,
    archive: String,
    inspection: &crate::core::extensions::plugin::PackageInspection,
) -> Result<Output, CliError> {
    if let Some(blocked) = &inspection.blocked {
        return Err(CliError::new("invalidInput", blocked.message.clone()));
    }
    let installed = plugins::plugin_install(exec.state(), archive, inspection.sha256.clone())
        .await
        .map_err(operation("plugin_install"))?;
    Ok(Output::json(json!({
        "pluginId": installed.plugin_id,
        "version": installed.version,
        "replaced": installed.replaced,
        "change": inspection.change,
        "sha256": inspection.sha256,
    })))
}

async fn sources(exec: &Exec) -> Result<Vec<String>, CliError> {
    Ok(market::plugin_market_sources(exec.state())
        .await
        .map_err(operation("plugin_market_sources"))?
        .sources)
}

/// The named source, or the first configured one.
async fn source_or_first(exec: &Exec, source: Option<String>) -> Result<String, CliError> {
    match source {
        Some(source) => Ok(source),
        None => sources(exec).await?.into_iter().next().ok_or_else(|| {
            CliError::new(
                "notFound",
                "没有配置插件市场索引源（plugin market add <索引源>）",
            )
        }),
    }
}

async fn run_market(exec: &Exec, args: MarketArgs) -> Result<Output, CliError> {
    match args.command.unwrap_or(MarketCommand::Sources) {
        MarketCommand::Sources => crate::cli::json_output(
            market::plugin_market_sources(exec.state())
                .await
                .map_err(operation("plugin_market_sources"))?,
        ),
        MarketCommand::Add(source) => {
            let mut configured = sources(exec).await?;
            configured.push(source.source);
            crate::cli::json_output(
                market::plugin_market_set_sources(exec.state(), configured)
                    .await
                    .map_err(operation("plugin_market_set_sources"))?,
            )
        }
        MarketCommand::Rm(source) => {
            let configured = sources(exec).await?;
            let key = crate::core::extensions::plugin_market::SourceLocation::parse(&source.source)
                .map(|location| location.key())
                .unwrap_or_else(|_| source.source.trim().to_owned());
            if !configured.contains(&key) {
                return Err(CliError::new(
                    "notFound",
                    format!("{key} 不是已配置的索引源"),
                ));
            }
            crate::cli::json_output(
                market::plugin_market_set_sources(
                    exec.state(),
                    configured
                        .into_iter()
                        .filter(|stored| *stored != key)
                        .collect(),
                )
                .await
                .map_err(operation("plugin_market_set_sources"))?,
            )
        }
        MarketCommand::Ls(list) => {
            let source = source_or_first(exec, list.source).await?;
            crate::cli::json_output(
                market::plugin_market_list(exec.state(), source, Some(list.refresh))
                    .await
                    .map_err(operation("plugin_market_list"))?,
            )
        }
        MarketCommand::Install(request) => {
            let source = source_or_first(exec, request.source).await?;
            let version = match request.version {
                Some(version) => version,
                None => {
                    let catalog =
                        market::plugin_market_list(exec.state(), source.clone(), Some(false))
                            .await
                            .map_err(operation("plugin_market_list"))?;
                    catalog
                        .packages
                        .into_iter()
                        .find(|package| package.id == request.plugin_id)
                        .ok_or_else(|| {
                            CliError::new(
                                "notFound",
                                format!("{source} 没有列出 {}", request.plugin_id),
                            )
                        })?
                        .latest
                        .ok_or_else(|| {
                            CliError::new(
                                "notFound",
                                format!("{} 没有当前平台可安装的版本", request.plugin_id),
                            )
                        })?
                }
            };
            let download =
                market::plugin_market_download(exec.state(), source, request.plugin_id, version)
                    .await
                    .map_err(operation("plugin_market_download"))?;
            let installed = install(exec, download.archive.clone(), &download.inspection).await;
            market::plugin_market_release(exec.state(), download.sha256.clone())
                .await
                .map_err(operation("plugin_market_release"))?;
            installed
        }
        MarketCommand::Index(index) => crate::cli::json_output(
            market::plugin_market_index(index.directory.text(), index.title, index.description)
                .await
                .map_err(operation("plugin_market_index"))?,
        ),
    }
}

pub fn render(args: &PluginArgs, value: &Value) -> Option<String> {
    match &args.command {
        None | Some(PluginCommand::Ls) => Some(render::table(
            value["packages"].as_array().map_or(&[][..], Vec::as_slice),
            &[("插件", "id"), ("版本", "version"), ("名称", "name")],
        )),
        Some(PluginCommand::Inspect(_)) => Some(render_inspection(value)),
        Some(PluginCommand::Readme(_)) => Some(
            value["readme"]
                .as_str()
                .map_or_else(String::new, |readme| format!("{}\n", readme.trim_end())),
        ),
        Some(PluginCommand::Market(MarketArgs {
            command:
                None | Some(MarketCommand::Sources | MarketCommand::Add(_) | MarketCommand::Rm(_)),
        })) => Some(
            value["sources"]
                .as_array()
                .map_or_else(String::new, |sources| {
                    sources
                        .iter()
                        .map(|source| format!("{}\n", render::cell(source)))
                        .collect()
                }),
        ),
        Some(PluginCommand::Market(MarketArgs {
            command: Some(MarketCommand::Ls(_)),
        })) => Some(render::table(
            value["packages"].as_array().map_or(&[][..], Vec::as_slice),
            &[
                ("插件", "id"),
                ("最新", "latest"),
                ("已安装", "installed"),
                ("状态", "status"),
                ("名称", "name"),
            ],
        )),
        _ => None,
    }
}

fn render_inspection(value: &Value) -> String {
    let mut rows = vec![
        ("插件", render::cell(&value["pluginId"])),
        ("名称", render::cell(&value["name"])),
        ("版本", render::cell(&value["version"])),
        ("变化", render::cell(&value["change"])),
        ("已安装", render::cell(&value["installed"]["version"])),
        ("发布者", render::cell(&value["publisher"])),
        ("许可证", render::cell(&value["license"])),
        ("权限", render::cell(&value["permissions"])),
        (
            "分析器",
            value["analyzers"]
                .as_array()
                .map_or(0, Vec::len)
                .to_string(),
        ),
        ("文件系统", render::cell(&value["filesystems"])),
        ("SHA-256", render::cell(&value["sha256"])),
        ("库", render::cell(&value["library"])),
        ("库 SHA-256", render::cell(&value["librarySha256"])),
    ];
    if let Some(message) = value["blocked"]["message"].as_str() {
        rows.push(("不能安装", message.to_owned()));
    }
    render::table_rows(
        vec!["项".to_owned(), "值".to_owned()],
        rows.into_iter()
            .map(|(label, value)| vec![label.to_owned(), value])
            .collect(),
    )
}
