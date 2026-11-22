//! `search`: the searches `grep` saved in the case — history, hits, CSV
//! export and removal.
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli::{
    exec::Exec,
    render,
    types::{parse_host_path, HostPath},
    workspace::Evidence,
    CliError, Output,
};

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchArgs {
    #[command(subcommand)]
    pub command: Option<SearchCommand>,
}

#[derive(Subcommand, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SearchCommand {
    /// 列出案件中保存的搜索（默认）
    Ls(SearchList),
    /// 分页读取一次搜索的命中
    Hits(SearchHits),
    /// 把命中导出为 CSV
    Export(SearchExport),
    /// 删除一次搜索及其命中
    Rm(SearchId),
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Distinct {
    File,
    Sha256,
}

impl Distinct {
    fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Sha256 => "sha256",
        }
    }
}

#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchList {
    /// 检材别名；默认全部检材
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
    /// 每份检材最多列出的条数
    #[arg(long, default_value_t = 100)]
    pub limit: u32,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHits {
    /// 搜索 ID（grep 输出的 searchId）
    #[arg(value_name = "搜索ID")]
    pub search_id: String,
    /// 检材别名；只有一份检材时可省略
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
    /// 跳过的命中数
    #[arg(long, default_value_t = 0)]
    pub offset: u64,
    /// 每页命中数
    #[arg(long, default_value_t = 200)]
    pub limit: u64,
    /// 只看路径或上下文包含此文本的命中
    #[arg(long, value_name = "文本")]
    pub keyword: Option<String>,
    /// 按文件或按 SHA-256 去重
    #[arg(long, value_enum)]
    pub distinct: Option<Distinct>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchExport {
    /// 搜索 ID
    #[arg(value_name = "搜索ID")]
    pub search_id: String,
    /// 输出的 CSV 文件（必须不存在）
    #[arg(short = 'o', long, value_parser = parse_host_path, value_name = "文件")]
    pub output: HostPath,
    /// 检材别名；只有一份检材时可省略
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
    /// 只导出路径或上下文包含此文本的命中
    #[arg(long, value_name = "文本")]
    pub keyword: Option<String>,
    /// 按文件或按 SHA-256 去重
    #[arg(long, value_enum)]
    pub distinct: Option<Distinct>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchId {
    /// 搜索 ID
    #[arg(value_name = "搜索ID")]
    pub search_id: String,
    /// 检材别名；只有一份检材时可省略
    #[arg(long, value_name = "别名")]
    pub evidence: Option<String>,
}

pub fn path(args: &SearchArgs) -> &'static str {
    match &args.command {
        None | Some(SearchCommand::Ls(_)) => "search ls",
        Some(SearchCommand::Hits(_)) => "search hits",
        Some(SearchCommand::Export(_)) => "search export",
        Some(SearchCommand::Rm(_)) => "search rm",
    }
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

async fn chosen(exec: &Exec, alias: Option<&str>) -> Result<Vec<Evidence>, CliError> {
    match alias {
        Some(alias) => Ok(vec![exec.resolve(alias.trim_start_matches('/')).await?]),
        None => {
            let mut items = Vec::new();
            for item in exec.evidence().await? {
                items.push(exec.resolve(&item.alias).await?);
            }
            Ok(items)
        }
    }
}

pub async fn run(exec: &Exec, args: SearchArgs) -> Result<Output, CliError> {
    match args
        .command
        .unwrap_or(SearchCommand::Ls(SearchList::default()))
    {
        SearchCommand::Ls(list) => {
            let mut items = Vec::new();
            for evidence in chosen(exec, list.evidence.as_deref()).await? {
                let history = crate::commands::search::global_search::global_search_history_list(
                    exec.state(),
                    evidence.image_id.clone(),
                    Some(list.limit.max(1)),
                )
                .await
                .map_err(operation("global_search_history_list"))?;
                for meta in history {
                    let mut value = serde_json::to_value(crate::cli::wire::Camel(meta))?;
                    value["evidence"] = json!(evidence.alias);
                    items.push(value);
                }
            }
            Ok(Output::json(json!({ "items": items })))
        }
        SearchCommand::Hits(hits) => {
            let evidence = exec.resolve_default(hits.evidence.as_deref()).await?;
            let page = crate::commands::search::global_search::global_search_hits_page(
                exec.state(),
                evidence.image_id.clone(),
                hits.search_id,
                Some(hits.offset),
                Some(hits.limit.clamp(1, 10_000)),
                hits.keyword,
                hits.distinct.map(|distinct| distinct.as_str().to_owned()),
                None,
            )
            .await
            .map_err(operation("global_search_hits_page"))?;
            let mut paths =
                super::browse::VolumePaths::new(exec, &evidence.alias, &evidence.image_id);
            let total = page.total;
            let mut items: Vec<Value> = Vec::with_capacity(page.items.len());
            for hit in page.items {
                items.push(json!({
                    "path": super::find::hit_address(&mut paths, &hit.volume_key, &hit.file_path, &hit.node_id).await?,
                    "offset": hit.file_offset,
                    "length": hit.match_len,
                    "encoding": hit.encoding,
                    "pattern": hit.pattern,
                    "context": hit.context_preview,
                    "matchCount": hit.match_count,
                    "sourceKind": hit.source_kind,
                    "md5": hit.md5,
                    "sha256": hit.sha256,
                    "id": hit.node_id,
                    "seq": hit.hit_seq,
                }));
            }
            let next = hits.offset + items.len() as u64;
            Ok(Output::json(json!({
                "items": items,
                "total": total,
                "nextOffset": (next < total).then_some(next),
            })))
        }
        SearchCommand::Export(export) => {
            if export.output.as_path().exists() {
                return Err(CliError::new(
                    "alreadyExists",
                    format!("{} 已存在：导出从不覆盖已有文件", export.output.text()),
                ));
            }
            let evidence = exec.resolve_default(export.evidence.as_deref()).await?;
            crate::cli::json_output(
                crate::commands::search::global_search::global_search_export(
                    exec.state(),
                    evidence.image_id,
                    export.search_id,
                    export.output.text(),
                    export.keyword,
                    export.distinct.map(|distinct| distinct.as_str().to_owned()),
                )
                .await
                .map_err(operation("global_search_export"))?,
            )
        }
        SearchCommand::Rm(id) => {
            let evidence = exec.resolve_default(id.evidence.as_deref()).await?;
            let removed = crate::commands::search::global_search::global_search_history_delete(
                exec.state(),
                evidence.image_id,
                id.search_id.clone(),
            )
            .await
            .map_err(operation("global_search_history_delete"))?;
            if !removed {
                return Err(CliError::new(
                    "notFound",
                    format!("找不到搜索：{}", id.search_id),
                ));
            }
            Ok(Output::json(
                json!({"searchId": id.search_id, "removed": true}),
            ))
        }
    }
}

pub fn render(args: &SearchArgs, value: &Value) -> Option<String> {
    let items = value["items"].as_array().map_or(&[][..], Vec::as_slice);
    match &args.command {
        None | Some(SearchCommand::Ls(_)) => Some(render::table(
            items,
            &[
                ("搜索ID", "searchId"),
                ("检材", "evidence"),
                ("状态", "status"),
                ("关键词", "keywords"),
                ("正则", "regexes"),
                ("命中", "totalMatches"),
            ],
        )),
        Some(SearchCommand::Hits(_)) => {
            let mut out = String::new();
            for item in items {
                out.push_str(&format!(
                    "{}:{}: {}\n",
                    render::cell(&item["path"]),
                    render::cell(&item["offset"]),
                    render::cell(&item["context"]).replace('\n', " ")
                ));
            }
            if let Some(next) = value["nextOffset"].as_u64() {
                out.push_str(&crate::cli::output::paint(
                    crate::cli::output::Role::Muted,
                    &format!(
                        "… 共 {} 条：用 --offset {next} 继续\n",
                        render::cell(&value["total"])
                    ),
                ));
            }
            Some(out)
        }
        _ => None,
    }
}
