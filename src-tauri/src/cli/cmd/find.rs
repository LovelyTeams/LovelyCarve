//! `find` (file names in the volume indexes), `grep` (file contents through
//! the global search) and `journal` (file system change journals).
use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::browse::VolumePaths;
use crate::{
    cli::{
        browsing::{display_path, volume_root, LastLink},
        exec::Exec,
        render,
        stream::require_source,
        types::{parse_address, Address, Parts},
        workspace::{Evidence, Target},
        CliError, Output,
    },
    commands::search::global_search::{global_search_hits_page, global_search_start},
    core::{
        catalog::filedb::filedb_query::FileFilter,
        filesystem::meta_addr::{IS_DELETE_DELETED, IS_DELETE_LIVE},
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_hits_keep_the_path_inside_their_volume() {
        assert_eq!(
            hit_path("part:0", "part:0:/document1.txt"),
            Some("/document1.txt")
        );
        assert_eq!(hit_path("part:0", "/a/b"), Some("/a/b"));
        assert_eq!(
            hit_path("root", "root:/evidence/a.txt"),
            Some("/evidence/a.txt")
        );
        assert_eq!(hit_path("part:0", "$未分配簇"), None);
    }
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FindKind {
    File,
    Dir,
    Symlink,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindArgs {
    /// 文件名通配符：匹配完整文件名，忽略大小写，支持 * ? [abc] [a-z] [^abc]
    #[arg(value_name = "通配符")]
    pub pattern: String,
    /// 只在这个检材、卷或目录下查找；默认全部检材
    #[arg(long, value_parser = parse_address, value_name = "路径")]
    pub under: Option<Address>,
    /// 条目类型
    #[arg(long = "type", value_enum)]
    pub kind: Option<FindKind>,
    /// 只找已删除的条目
    #[arg(long, conflicts_with = "no_deleted")]
    pub deleted: bool,
    /// 不找已删除的条目
    #[arg(long)]
    pub no_deleted: bool,
    /// 最小文件大小（如 4096、64K、10M）
    #[arg(long, value_parser = super::parse_size, value_name = "大小")]
    pub min_size: Option<u64>,
    /// 最大文件大小
    #[arg(long, value_parser = super::parse_size, value_name = "大小")]
    pub max_size: Option<u64>,
    /// 每个卷最多返回的条数（1–10000）
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
    /// 每个卷跳过的条数；翻页时保持其余条件不变
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepArgs {
    /// 关键词；加 -E 时为正则表达式
    #[arg(value_name = "模式", allow_hyphen_values = true)]
    pub pattern: String,
    /// 搜索范围：检材、卷、目录或文件；默认全部检材
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Option<Address>,
    /// 忽略大小写
    #[arg(short = 'i', long)]
    pub ignore_case: bool,
    /// 按正则表达式匹配
    #[arg(short = 'E', long)]
    pub regex: bool,
    /// 搜索的编码，逗号分隔（如 UTF-8,GBK,UTF-16LE）；默认全部支持的编码
    #[arg(long, value_delimiter = ',', value_name = "编码")]
    pub encoding: Vec<String>,
    /// 不搜索已删除文件
    #[arg(long)]
    pub no_deleted: bool,
    /// 最多返回的命中条数（1–100000）；全部命中保存在案件中，可用 search hits 继续读取
    #[arg(long, default_value_t = 1000)]
    pub limit: u64,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JournalArgs {
    /// NTFS 或 ext4 卷，或卷中的任一文件
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// 每页条数
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
    /// 上一页给出的 nextCursor
    #[arg(long, value_name = "游标")]
    pub cursor: Option<String>,
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

/// The evidence and, below its root, the subtree an optional scope names.
async fn scopes(
    exec: &Exec,
    scope: Option<&Address>,
) -> Result<Vec<(Evidence, Option<Target>)>, CliError> {
    if let Some(scope) = scope.filter(|scope| scope.parts() != Parts::Root) {
        let target = exec.target(scope, LastLink::Follow).await?;
        let evidence = target.evidence.clone();
        return Ok(vec![(
            evidence,
            (!target.location.is_empty()).then_some(target),
        )]);
    }
    let mut scopes = Vec::new();
    for item in exec.evidence().await? {
        scopes.push((exec.resolve(&item.alias).await?, None));
    }
    if scopes.is_empty() {
        return Err(CliError::new(
            "noImage",
            "没有打开任何检材：用 evidence add <检材路径> 或 --image <检材路径> 打开",
        ));
    }
    Ok(scopes)
}

/// The volume a target lies in and its path inside that volume, which is how
/// the volume index names it.
fn volume_and_path(target: &Target) -> (Option<String>, String) {
    match target.location.iter().rposition(|(_, id)| volume_root(id)) {
        Some(index) => {
            let names: Vec<&str> = target.location[index + 1..]
                .iter()
                .map(|(name, _)| name.as_str())
                .collect();
            (
                Some(target.location[index].1.clone()),
                format!("/{}", names.join("/")),
            )
        }
        None => (None, display_path(&target.location)),
    }
}

pub async fn find(exec: &Exec, args: FindArgs) -> Result<Output, CliError> {
    let mut items = Vec::new();
    let mut truncated = false;
    // Volumes in scope that have no index to search: the result does not cover them.
    let mut unindexed = Vec::new();
    let mut searched = 0usize;
    let limit = args.limit.clamp(1, 10_000);
    for (evidence, target) in scopes(exec, args.under.as_ref()).await? {
        let (only, path_prefix) = match &target {
            Some(target) => {
                let (volume, path) = volume_and_path(target);
                let mut prefix = path;
                if !prefix.ends_with('/') {
                    prefix.push('/');
                }
                (volume, (prefix != "/").then_some(prefix))
            }
            None => (None, None),
        };
        let volumes = crate::commands::filesystem::filedb::filedb_volumes(
            exec.state(),
            evidence.image_id.clone(),
        )
        .await
        .map_err(operation("filedb_volumes"))?;
        let mut paths = VolumePaths::new(exec, &evidence.alias, &evidence.image_id);
        for volume in volumes {
            if only.as_ref().is_some_and(|only| *only != volume.volume_key) {
                continue;
            }
            if volume.indexes.is_empty() {
                unindexed.push(paths.address(&volume.volume_key, "").await?);
                continue;
            }
            searched += 1;
            let filter = FileFilter {
                name_glob: Some(args.pattern.clone()),
                path_prefix: path_prefix.clone(),
                kinds: args
                    .kind
                    .map(|kind| {
                        vec![match kind {
                            FindKind::File => "file",
                            FindKind::Dir => "dir",
                            FindKind::Symlink => "symlink",
                        }
                        .to_owned()]
                    })
                    .unwrap_or_default(),
                min_size: args.min_size.map(|size| size as i64),
                max_size: args.max_size.map(|size| size as i64),
                is_delete: if args.deleted {
                    Some(IS_DELETE_DELETED)
                } else if args.no_deleted {
                    Some(IS_DELETE_LIVE)
                } else {
                    None
                },
                limit,
                offset: args.offset,
                ..FileFilter::default()
            };
            let hits = crate::commands::filesystem::filedb::filedb_query_files(
                exec.state(),
                evidence.image_id.clone(),
                Some(volume.volume_key.clone().into()),
                filter,
            )
            .await
            .map_err(operation("filedb_query_files"))?;
            truncated |= hits.len() >= limit;
            // Index paths are relative to the root of the index that holds them.
            let index_key = volume
                .indexes
                .first()
                .map_or(volume.volume_key.as_str(), |index| {
                    index.volume_key.as_str()
                });
            for hit in hits {
                items.push(json!({
                    "name": hit.name,
                    "path": paths.address(index_key, &hit.full_path).await?,
                    "kind": hit.kind,
                    "size": hit.size,
                    "deleted": hit.is_delete == IS_DELETE_DELETED,
                    "id": hit.nid,
                    "evidence": evidence.alias,
                    "volumeKey": volume.volume_key,
                }));
            }
        }
    }
    if searched == 0 {
        return Err(CliError::new(
            "indexUnavailable",
            "没有可查的卷索引：find 在卷索引中查找文件名。卷索引建立在案件中：evidence add --wait 等待建立，\
             或用 index build <卷> 构建；index ls 查看状态",
        ));
    }
    Ok(Output::json(json!({
        "items": items,
        "total": items.len(),
        "truncated": truncated,
        "unindexed": unindexed,
    })))
}

pub async fn grep(exec: &Exec, args: GrepArgs) -> Result<Output, CliError> {
    let limit = args.limit.clamp(1, 100_000);
    let mut items = Vec::new();
    let mut searches = Vec::new();
    // `stopped`: the search itself did not finish or hit its own cap, so the
    // saved hits are not all there are; `more`: only --limit held back hits.
    let (mut read_errors, mut stopped, mut more, mut total) = (0u64, false, false, 0u64);
    for (evidence, target) in scopes(exec, args.path.as_ref()).await? {
        let image = exec.image(&evidence.image_id).await?;
        require_source(&image)?;
        // A volume is searched as a volume; a directory or file as a subtree.
        let node = target.as_ref().map_or("", Target::node_id).to_owned();
        let (partitions, tree_nodes) = if node.is_empty() {
            (Vec::new(), Vec::new())
        } else if volume_root(&node) {
            (vec![node], Vec::new())
        } else {
            (Vec::new(), vec![node])
        };
        let options = serde_json::from_value(json!({
            "keys": if args.regex { Vec::<String>::new() } else { vec![args.pattern.clone()] },
            "regexes": if args.regex { vec![args.pattern.clone()] } else { Vec::<String>::new() },
            "codepages": args.encoding,
            "match_case": !args.ignore_case,
            "search_deleted": !args.no_deleted,
            "scope": { "partitions": partitions, "tree_nodes": tree_nodes },
        }))?;
        let search_id = super::task_id("grep");
        let summary = global_search_start(
            exec.host.clone(),
            exec.state(),
            evidence.image_id.clone(),
            search_id.clone(),
            options,
        )
        .await
        .map_err(operation("global_search_start"))?;
        read_errors += summary.read_errors;
        let mut paths = VolumePaths::new(exec, &evidence.alias, &evidence.image_id);
        let mut collected = 0u64;
        let found = loop {
            let remaining = limit.saturating_sub(items.len() as u64);
            if remaining == 0 {
                break summary.total_matches.max(collected);
            }
            let page = global_search_hits_page(
                exec.state(),
                evidence.image_id.clone(),
                search_id.clone(),
                Some(collected),
                Some(remaining.min(2000)),
                None,
                None,
                None,
            )
            .await
            .map_err(operation("global_search_hits_page"))?;
            let empty = page.items.is_empty();
            for hit in page.items {
                collected += 1;
                items.push(json!({
                    "path": hit_address(&mut paths, &hit.volume_key, &hit.file_path, &hit.node_id).await?,
                    "offset": hit.file_offset,
                    "length": hit.match_len,
                    "encoding": hit.encoding,
                    "pattern": hit.pattern,
                    "context": hit.context_preview,
                    "matchCount": hit.match_count,
                    "sourceKind": hit.source_kind,
                    "id": hit.node_id,
                    "evidence": evidence.alias,
                    "searchId": search_id,
                }));
            }
            if empty || collected >= page.total {
                break page.total;
            }
        };
        total += found;
        stopped |= summary.truncated || summary.status != "success";
        more |= collected < found;
        let mut summary = serde_json::to_value(crate::cli::wire::Camel(summary))?;
        summary["evidence"] = json!(evidence.alias);
        searches.push(summary);
    }
    Ok(Output::json(json!({
        "items": items,
        "total": total,
        "truncated": more || stopped,
        "summary": {"readErrors": read_errors, "truncated": stopped},
        "searches": searches,
    })))
}

pub async fn journal(exec: &Exec, args: JournalArgs) -> Result<Output, CliError> {
    let target = exec.target(&args.path, LastLink::Follow).await?;
    if target.location.is_empty() {
        return Err(CliError::new(
            "usage",
            "journal 需要卷或卷中的文件，例如 /<别名>/part:0",
        ));
    }
    let info =
        crate::commands::evidence::image::image_info(exec.state(), target.image_id().clone())
            .await
            .map_err(operation("image_info"))?;
    let kind = super::crypto::partition_number(&target.location[0].1)
        .and_then(|number| {
            info.partitions
                .iter()
                .find(|partition| partition.id == number)
        })
        .and_then(|partition| serde_json::to_value(partition.kind).ok())
        .and_then(|kind| kind.as_str().map(str::to_owned))
        .unwrap_or_default();
    let node_id = target.node_id();
    let ext = kind == "ext4" || node_id.starts_with("ext");
    let page_size = Some(args.limit.clamp(1, 5000));
    let mut value = if ext {
        let page = crate::commands::filesystem::filesystem_journal::fs_ext4_journal_blocks_page(
            exec.state(),
            target.image_id().clone(),
            node_id.into(),
            args.cursor,
            page_size,
        )
        .await
        .map_err(operation("fs_ext4_journal_blocks_page"))?;
        let mut value = serde_json::to_value(crate::cli::wire::Camel(page))?;
        value["journal"] = json!("ext4");
        value
    } else {
        let page = crate::commands::filesystem::filesystem_journal::fs_usn_journal_page(
            exec.state(),
            target.image_id().clone(),
            node_id.into(),
            args.cursor,
            page_size,
        )
        .await
        .map_err(operation("fs_usn_journal_page"))?;
        let mut value = serde_json::to_value(crate::cli::wire::Camel(page))?;
        value["journal"] = json!("usn");
        value
    };
    value["path"] = json!(target.address());
    Ok(Output::json(value))
}

/// The path inside its volume of a hit in a file: hits name the volume and
/// the path (`part:0:/etc/passwd`). Hits in unallocated space and partition
/// gaps have none.
pub fn hit_path<'a>(volume_key: &str, file_path: &'a str) -> Option<&'a str> {
    let relative = file_path
        .strip_prefix(volume_key)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or(file_path);
    relative.starts_with('/').then_some(relative)
}

/// The address of a search hit: its file, or its node when it has no path.
pub async fn hit_address(
    paths: &mut VolumePaths<'_>,
    volume_key: &str,
    file_path: &str,
    node_id: &str,
) -> Result<String, CliError> {
    match hit_path(volume_key, file_path) {
        Some(path) => paths.address(volume_key, path).await,
        None => Ok(format!("/{}/@{node_id}", paths.alias())),
    }
}

pub fn render_find(value: &Value) -> String {
    let items = value["items"].as_array().map_or(&[][..], Vec::as_slice);
    if items.is_empty() {
        return "（没有匹配的文件）\n".to_owned();
    }
    let rows: Vec<Value> = items
        .iter()
        .map(|item| {
            let mut row = item.clone();
            if item["deleted"] == Value::Bool(true) {
                row["path"] = json!(format!("{} ✗", render::cell(&item["path"])));
            }
            row
        })
        .collect();
    let mut out = render::table(
        &rows,
        &[("类型", "kind"), ("大小", "size"), ("路径", "path")],
    );
    if value["truncated"] == Value::Bool(true) {
        out.push_str(&crate::cli::output::paint(
            crate::cli::output::Role::Muted,
            "… 有卷达到 --limit：用 --offset 翻页\n",
        ));
    }
    out
}

pub fn render_grep(value: &Value) -> String {
    let mut out = String::new();
    for item in value["items"].as_array().map_or(&[][..], Vec::as_slice) {
        out.push_str(&format!(
            "{}:{}: {}\n",
            render::cell(&item["path"]),
            render::cell(&item["offset"]),
            render::cell(&item["context"]).replace('\n', " ")
        ));
    }
    if value["truncated"] == Value::Bool(true) {
        out.push_str(&crate::cli::output::paint(
            crate::cli::output::Role::Muted,
            &format!(
                "… 共 {} 条命中，只显示了一部分；用 search hits <搜索ID> 继续读取\n",
                render::cell(&value["total"])
            ),
        ));
    }
    out
}
