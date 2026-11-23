//! `volumes`, `ls`, `tree` and `stat`: browsing over the index-first
//! directory pages, with every entry carrying the address it can be read by.
use std::collections::HashMap;

use clap::{Args, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{
    cli::{
        browsing::{self, LastLink},
        exec::{Exec, RequestOptions},
        render,
        timezone::{self, Shape},
        types::{parse_address, Address, Parts},
        workspace::{cli_path, evidence_rows, Target},
        CliError, Output,
    },
    core::{
        catalog::{
            directory_query::DirectoryQuery,
            flat_query::{FlatFileFilter, FlatSortKey},
        },
        common::ids::ImageId,
        filesystem::fs_all::ROOT_VOLUME_KEY,
    },
};

const MAX_PAGE: usize = 5000;

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumesArgs {
    /// 检材别名；省略时列出全部检材的卷
    #[arg(value_name = "别名")]
    pub alias: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Created,
    Accessed,
    Ext,
    Kind,
    Inode,
}

impl From<SortKey> for FlatSortKey {
    fn from(key: SortKey) -> Self {
        match key {
            SortKey::Name => Self::Name,
            SortKey::Size => Self::Size,
            SortKey::Modified => Self::Modified,
            SortKey::Created => Self::Created,
            SortKey::Accessed => Self::Accessed,
            SortKey::Ext => Self::Ext,
            SortKey::Kind => Self::Kind,
            SortKey::Inode => Self::Inode,
        }
    }
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LsArgs {
    /// 目录；/ 列出检材，/<别名> 列出分区与卷
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Option<Address>,
    /// 同时显示节点 ID、inode 与全部时间
    #[arg(short = 'l', long)]
    pub long: bool,
    /// 每页条数（1–5000）
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
    /// 上一页给出的 nextCursor
    #[arg(long, value_name = "游标")]
    pub cursor: Option<String>,
    /// 读取所有页
    #[arg(long, conflicts_with = "cursor")]
    pub all: bool,
    /// 排序键（目录总在文件之前）
    #[arg(long, value_enum)]
    pub sort: Option<SortKey>,
    /// 降序
    #[arg(long)]
    pub desc: bool,
    /// 只列已删除的条目
    #[arg(long, conflicts_with = "no_deleted")]
    pub deleted: bool,
    /// 不列已删除的条目
    #[arg(long)]
    pub no_deleted: bool,
    /// 只列这些扩展名的文件，逗号分隔
    #[arg(long, value_delimiter = ',', value_name = "扩展名")]
    pub ext: Vec<String>,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeArgs {
    /// 起点目录
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// 递归深度（1–128）
    #[arg(long, default_value_t = 2)]
    pub depth: u32,
    /// 只列名称包含此文本的条目（忽略大小写）
    #[arg(long, value_name = "文本")]
    pub name: Option<String>,
    /// 最多检查的条目数（1–100000）
    #[arg(long, default_value_t = 10_000)]
    pub limit: usize,
}

#[derive(Args, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatArgs {
    /// 文件或目录；末尾的符号链接描述链接本身
    #[arg(value_parser = parse_address, value_name = "路径")]
    pub path: Address,
    /// 同时读取内容属性：MIME、魔数、熵与数据运行段
    #[arg(long)]
    pub content: bool,
}

fn operation(name: &'static str) -> impl Fn(crate::app::error::AppError) -> CliError {
    move |error| crate::cli::command_error(error).in_operation(name)
}

/// A node with the address it is read by.
pub fn node_value(node: &impl Serialize, address: String) -> Result<Value, CliError> {
    let mut value = serde_json::to_value(crate::cli::wire::Camel(node))?;
    if let Some(fields) = value.as_object_mut() {
        fields.insert("path".into(), json!(address));
    }
    Ok(value)
}

/// The address of a child of `parent`.
fn child_address(parent: &Target, name: &str, id: &str) -> String {
    let mut location = parent.location.clone();
    location.push((name.to_owned(), id.to_owned()));
    cli_path(&parent.evidence.alias, &location)
}

pub async fn ls(exec: &Exec, args: LsArgs, options: &RequestOptions) -> Result<Output, CliError> {
    let address = args.path.clone().unwrap_or_else(Address::root);
    if address.parts() == Parts::Root {
        return Ok(Output::json(evidence_rows(&exec.evidence().await?)));
    }
    if !(1..=MAX_PAGE).contains(&args.limit) {
        return Err(CliError::new("usage", "--limit 的范围是 1–5000"));
    }
    let target = exec.target(&address, LastLink::Follow).await?;
    let query = (args.sort.is_some()
        || args.desc
        || args.deleted
        || args.no_deleted
        || !args.ext.is_empty())
    .then(|| DirectoryQuery {
        filter: FlatFileFilter {
            deleted: if args.deleted {
                Some(true)
            } else if args.no_deleted {
                Some(false)
            } else {
                None
            },
            extensions: args
                .ext
                .iter()
                .map(|ext| ext.trim_start_matches('.').to_lowercase())
                .collect(),
            ..FlatFileFilter::default()
        },
        sort: args.sort.map(FlatSortKey::from),
        descending: args.desc,
        hide_empty_dirs: false,
    });
    let mut items = Vec::new();
    let mut cursor = args.cursor.clone();
    let (next_cursor, total, truncated) = loop {
        let page = crate::commands::filesystem::filesystem::fs_children_page(
            exec.state(),
            target.image_id().clone(),
            target.node_id().into(),
            cursor.clone(),
            Some(args.limit),
            query.clone(),
        )
        .await
        .map_err(operation("fs_children_page"))?;
        for node in &page.items {
            items.push(node_value(
                node,
                child_address(&target, &node.name, &node.id),
            )?);
        }
        if !args.all || page.next_cursor.is_none() || page.truncated {
            break (page.next_cursor, page.total, page.truncated);
        }
        cursor = page.next_cursor;
    };
    // `ls` on a file shows the file, as ls(1) does.
    if items.is_empty() && cursor.is_none() && !target.location.is_empty() {
        if let Some(node) = node_of(exec, &target).await? {
            if node["kind"] != "dir" {
                let mut value = json!({"path": target.address(), "items": [node], "nextCursor": null, "total": 1, "truncated": false});
                timezone::apply(
                    exec,
                    target.image_id(),
                    options.tz.as_deref(),
                    &mut value,
                    Shape::Page,
                )
                .await?;
                return Ok(Output::json(value));
            }
        }
    }
    let mut value = json!({
        "path": target.address(),
        "items": items,
        "nextCursor": next_cursor,
        "total": total,
        "truncated": truncated,
    });
    timezone::apply(
        exec,
        target.image_id(),
        options.tz.as_deref(),
        &mut value,
        Shape::Page,
    )
    .await?;
    Ok(Output::json(value))
}

/// The directory entry of the target node; `None` at the evidence root.
async fn node_of(exec: &Exec, target: &Target) -> Result<Option<Value>, CliError> {
    let mut page = |parent_id: String, cursor, page_size| {
        exec.page(target.image_id(), parent_id, cursor, page_size)
    };
    match browsing::node(&target.location, &mut page).await? {
        Some(node) => Ok(Some(node_value(&node, target.address())?)),
        None => Ok(None),
    }
}

pub async fn tree(
    exec: &Exec,
    args: TreeArgs,
    options: &RequestOptions,
) -> Result<Output, CliError> {
    if args.path.parts() == Parts::Root {
        return Err(CliError::new(
            "usage",
            "tree 需要检材内的起点，例如 /<别名> 或 /<别名>/part:0",
        ));
    }
    let target = exec.target(&args.path, LastLink::Follow).await?;
    let task = exec.state.tasks.register(
        format!("cli-tree-{}", uuid::Uuid::new_v4()),
        crate::task_manager::TaskKind::FlatList,
        Some(target.image_id().clone()),
    );
    let _permit = exec
        .state
        .tasks
        .acquire_resources(&task)
        .await
        .map_err(crate::app::error::AppError::from)
        .map_err(crate::cli::command_error)?;
    let cancellation = task.token();
    let depth = args.depth.clamp(1, 128);
    let limit = args.limit.clamp(1, 100_000);
    let filter = args.name.as_ref().map(|name| name.to_lowercase());
    let mut page = |parent_id: String, cursor, page_size| {
        exec.page(target.image_id(), parent_id, cursor, page_size)
    };
    let (entries, truncated) = browsing::walk(
        &target.location,
        depth,
        limit,
        filter.as_deref(),
        &cancellation,
        &mut page,
    )
    .await?;
    task.finish(crate::task_manager::TaskOutcome::Succeeded);
    // The walk names paths from the evidence root; the first level reads
    // better, and stays stable, by its ID.
    let prefix = if target.location.is_empty() {
        String::new()
    } else {
        browsing::display_path(&target.location)
    };
    let root_ids = if target.location.is_empty() {
        root_ids(exec, target.image_id()).await?
    } else {
        HashMap::new()
    };
    let mut items = Vec::new();
    for entry in entries {
        let relative = entry
            .path
            .strip_prefix(&prefix)
            .unwrap_or(&entry.path)
            .to_owned();
        let address = if target.location.is_empty() {
            address_from_root_path(&target.evidence.alias, &root_ids, &relative)
        } else {
            format!("{}{relative}", target.address())
        };
        let depth = relative.matches('/').count();
        let mut node = serde_json::to_value(crate::cli::wire::Camel(&entry.node))?;
        if let Some(fields) = node.as_object_mut() {
            fields.insert("path".into(), json!(address));
            fields.insert("depth".into(), json!(depth));
        }
        items.push(node);
    }
    let mut value = json!({"path": target.address(), "items": items, "truncated": truncated});
    timezone::apply(
        exec,
        target.image_id(),
        options.tz.as_deref(),
        &mut value,
        Shape::Page,
    )
    .await?;
    Ok(Output::json(value))
}

/// The address of a path an index or search reports from the evidence root
/// (`/<root name>/…`), with the first level by its ID where `root_ids` has it.
pub fn address_from_root_path(
    alias: &str,
    root_ids: &HashMap<String, String>,
    path: &str,
) -> String {
    let mut parts = path.trim_start_matches('/').splitn(2, '/');
    let first = parts.next().unwrap_or_default();
    let first = root_ids.get(first).map_or(first, String::as_str);
    match parts.next() {
        Some(rest) => format!("/{alias}/{first}/{rest}"),
        None if first.is_empty() => format!("/{alias}"),
        None => format!("/{alias}/{first}"),
    }
}

/// Addresses for the paths indexes and searches report inside volumes, each
/// volume's root looked up once.
pub struct VolumePaths<'a> {
    exec: &'a Exec,
    alias: String,
    image_id: ImageId,
    roots: HashMap<String, String>,
    root_ids: Option<HashMap<String, String>>,
}

impl<'a> VolumePaths<'a> {
    pub fn new(exec: &'a Exec, alias: &str, image_id: &ImageId) -> Self {
        Self {
            exec,
            alias: alias.to_owned(),
            image_id: image_id.clone(),
            roots: HashMap::new(),
            root_ids: None,
        }
    }

    pub fn alias(&self) -> &str {
        &self.alias
    }

    /// The address of `path` (`/…` from the root of `volume_key`).
    pub async fn address(&mut self, volume_key: &str, path: &str) -> Result<String, CliError> {
        // A logical source's volume is the evidence itself: its paths start at
        // a root entry's name, which addresses write as the entry's ID.
        if volume_key == ROOT_VOLUME_KEY {
            if self.root_ids.is_none() {
                self.root_ids = Some(root_ids(self.exec, &self.image_id).await?);
            }
            let ids = self.root_ids.get_or_insert_with(HashMap::new);
            return Ok(address_from_root_path(&self.alias, ids, path));
        }
        if !self.roots.contains_key(volume_key) {
            let root = volume_address(self.exec, &self.alias, &self.image_id, volume_key).await;
            self.roots.insert(volume_key.to_owned(), root);
        }
        let root = self.roots.get(volume_key).map_or("", String::as_str);
        Ok(format!("{root}{path}"))
    }
}

/// The address of a volume's root directory, which index and search paths
/// are relative to.
pub async fn volume_address(
    exec: &Exec,
    alias: &str,
    image_id: &ImageId,
    volume_key: &str,
) -> String {
    let at_root = exec
        .page(image_id, String::new(), None, 500)
        .await
        .is_ok_and(|page| page.items.iter().any(|node| node.id == volume_key));
    if at_root && !volume_key.contains('/') {
        return format!("/{alias}/{volume_key}");
    }
    match crate::cli::navigation::locate_node(exec, image_id, volume_key).await {
        Ok(location) => cli_path(alias, &location),
        Err(_) => format!("/{alias}/@{volume_key}"),
    }
}

/// The IDs of the evidence root's entries, by name, where names are unique.
pub async fn root_ids(
    exec: &Exec,
    image_id: &ImageId,
) -> Result<HashMap<String, String>, CliError> {
    let page = exec.page(image_id, String::new(), None, 500).await?;
    let mut ids: HashMap<String, Option<String>> = HashMap::new();
    for node in page.items {
        if node.id.contains('/') {
            continue;
        }
        ids.entry(node.name)
            .and_modify(|id| *id = None)
            .or_insert(Some(node.id));
    }
    Ok(ids
        .into_iter()
        .filter_map(|(name, id)| id.map(|id| (name, id)))
        .collect())
}

pub async fn stat(
    exec: &Exec,
    args: StatArgs,
    options: &RequestOptions,
) -> Result<Output, CliError> {
    if args.path.parts() == Parts::Root {
        return Ok(Output::json(evidence_rows(&exec.evidence().await?)));
    }
    let target = exec.target(&args.path, LastLink::Keep).await?;
    let image = exec.image(target.image_id()).await?;
    let mut value = match node_of(exec, &target).await? {
        Some(node) => node,
        None => json!({
            "id": "",
            "name": target.evidence.alias,
            "kind": "dir",
            "size": image.len(),
            "deleted": false,
            "path": target.address(),
        }),
    };
    if let Some(fields) = value.as_object_mut() {
        fields.insert("nodeId".into(), json!(target.node_id()));
        fields.insert("evidence".into(), json!(target.evidence.alias));
        fields.insert("indexOnly".into(), json!(image.is_index_only()));
        if !target.location.is_empty() {
            let parent = &target.location[..target.location.len() - 1];
            fields.insert(
                "parent".into(),
                json!(cli_path(&target.evidence.alias, parent)),
            );
        }
    }
    if args.content && !target.location.is_empty() {
        let content = crate::commands::preview::filesystem_preview::fs_stat(
            exec.state(),
            target.image_id().clone(),
            target.node_id().into(),
        )
        .await
        .map_err(operation("fs_stat"))?;
        value["content"] = serde_json::to_value(crate::cli::wire::Camel(content))?;
    }
    timezone::apply(
        exec,
        target.image_id(),
        options.tz.as_deref(),
        &mut value,
        Shape::Node,
    )
    .await?;
    Ok(Output::json(value))
}

/// Partitions and volumes of each evidence item, with file system,
/// encryption and index state.
pub async fn volumes(exec: &Exec, args: VolumesArgs) -> Result<Output, CliError> {
    let evidence = match &args.alias {
        Some(alias) => vec![exec.resolve(alias.trim_start_matches('/')).await?],
        None => {
            let mut loaded = Vec::new();
            for item in exec.evidence().await? {
                loaded.push(exec.resolve(&item.alias).await?);
            }
            loaded
        }
    };
    let snapshots = if exec.state.active_case.lock().await.is_some() {
        let list = crate::commands::casework::case_snapshot::snapshot_list(exec.state())
            .await
            .map_err(operation("snapshot_list"))?;
        match crate::cli::wire::flattened_output(&list)? {
            Output::Json(json) => json.value().as_array().cloned().unwrap_or_default(),
            _ => Vec::new(),
        }
    } else {
        Vec::new()
    };
    let mut rows = Vec::new();
    for item in &evidence {
        let info =
            crate::commands::evidence::image::image_info(exec.state(), item.image_id.clone())
                .await
                .map_err(operation("image_info"))?;
        let info = serde_json::to_value(crate::cli::wire::Camel(info))?;
        // Each unlocked partition with the scheme it was opened with.
        let unlocked: Vec<(u64, Value)> = info["unlockedPartitions"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| Some((item["id"].as_u64()?, item["scheme"].clone())))
                    .collect()
            })
            .unwrap_or_default();
        let filesystems = crate::commands::filesystem::filedb::filedb_volumes(
            exec.state(),
            item.image_id.clone(),
        )
        .await
        .map(|volumes| serde_json::to_value(crate::cli::wire::Camel(volumes)).unwrap_or_default())
        .unwrap_or_default();
        let filesystems = filesystems.as_array().cloned().unwrap_or_default();
        let has_volume = |key: &str| filesystems.iter().any(|volume| volume["volumeKey"] == key);
        let root = exec.page(&item.image_id, String::new(), None, 500).await?;
        let single_root = root.items.len() == 1;
        for node in root.items {
            // A logical source (a folder or an archive) is one volume, keyed
            // `root`, below its single root entry.
            let logical = single_root && !has_volume(&node.id) && has_volume(ROOT_VOLUME_KEY);
            let volume_key = if logical {
                ROOT_VOLUME_KEY
            } else {
                node.id.as_str()
            };
            let partition = super::crypto::partition_number(&node.id).and_then(|number| {
                info["partitions"]
                    .as_array()
                    .and_then(|partitions| {
                        partitions
                            .iter()
                            .find(|partition| partition["id"] == number)
                    })
                    .cloned()
            });
            let kind = partition
                .as_ref()
                .and_then(|partition| partition["kind"].as_str())
                .map(str::to_owned);
            let number = super::crypto::partition_number(&node.id);
            // An unlocked partition reads as its plaintext kind; a locked one
            // still has its encryption header's.
            let opened = number.and_then(|number| {
                unlocked
                    .iter()
                    .find(|(id, _)| *id == u64::from(number))
                    .map(|(_, scheme)| scheme.clone())
            });
            let encryption = match (opened, kind.as_deref(), number) {
                (Some(scheme), _, _) => json!({"scheme": scheme, "state": "unlocked"}),
                (None, Some(scheme @ ("bitlocker" | "luks" | "filevault")), Some(_)) => {
                    json!({"scheme": scheme, "state": "locked"})
                }
                (None, _, _) => Value::Null,
            };
            let filesystem = filesystems
                .iter()
                .find(|volume| volume["volumeKey"] == volume_key)
                .cloned();
            let index = snapshots
                .iter()
                .find(|snapshot| {
                    snapshot["imageId"] == item.image_id.as_str()
                        && snapshot["volumeKey"] == volume_key
                })
                .cloned();
            let kind = if logical {
                info["sourceFormat"].as_str().map(str::to_owned)
            } else {
                filesystem
                    .as_ref()
                    .and_then(|volume| volume["kind"].as_str().map(str::to_owned))
                    .or(kind)
            };
            rows.push(json!({
                "path": format!("/{}/{}", item.alias, node.id),
                "evidence": item.alias,
                "id": node.id,
                "volumeKey": volume_key,
                "name": node.name,
                "kind": kind,
                "size": node.size,
                "label": partition.as_ref().map_or(Value::Null, |partition| partition["label"].clone()),
                "encryption": encryption,
                "index": index.as_ref().map(index_state),
                "indexFiles": index.as_ref().map(|index| index["entryCount"].clone()),
            }));
        }
    }
    Ok(Output::json(json!({ "items": rows })))
}

/// `ready`, `stale`, `truncated` or `building` for a published index row.
pub fn index_state(snapshot: &Value) -> Value {
    let state = if snapshot["stale"] == Value::Bool(true) {
        "stale"
    } else if snapshot["truncated"] == Value::Bool(true) {
        "truncated"
    } else if snapshot["indexed"] == Value::Bool(true) {
        "ready"
    } else {
        "building"
    };
    json!(state)
}

pub fn render_volumes(value: &Value) -> String {
    let items: Vec<Value> = value["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    let mut row = item.clone();
                    row["size"] = json!(item["size"].as_u64().map(render::human_size));
                    row["encryption"] = match item["encryption"].as_object() {
                        Some(encryption) => json!(format!(
                            "{} {}",
                            render::cell(&encryption["scheme"]),
                            if encryption["state"] == "locked" {
                                "已锁定"
                            } else {
                                "已解锁"
                            }
                        )),
                        None => Value::Null,
                    };
                    row
                })
                .collect()
        })
        .unwrap_or_default();
    if items.is_empty() {
        return "（没有卷）\n".to_owned();
    }
    render::table(
        &items,
        &[
            ("卷", "path"),
            ("类型", "kind"),
            ("大小", "size"),
            ("加密", "encryption"),
            ("索引", "index"),
            ("名称", "name"),
        ],
    )
}

fn time(item: &Value, role: &str) -> String {
    item["timestamps"][role]
        .as_str()
        .map_or_else(|| "-".to_owned(), crate::cli::output::sanitize)
}

pub fn render_ls(args: &LsArgs, value: &Value) -> String {
    let items = value["items"].as_array().map_or(&[][..], Vec::as_slice);
    if value["path"] == "/" {
        if items.is_empty() {
            return "（没有打开任何检材；用 evidence add <检材路径> 打开）\n".to_owned();
        }
        return render::table(
            items,
            &[
                ("检材", "path"),
                ("格式", "sourceFormat"),
                ("大小", "size"),
                ("检材路径", "evidencePath"),
            ],
        );
    }
    let mut header = vec!["类型".to_owned(), "大小".to_owned(), "修改时间".to_owned()];
    if args.long {
        header.extend([
            "创建时间".to_owned(),
            "访问时间".to_owned(),
            "INODE".to_owned(),
            "ID".to_owned(),
        ]);
    }
    header.push("名称".to_owned());
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            let deleted = item["deleted"] == Value::Bool(true);
            let kind = match item["kind"].as_str() {
                Some("dir") => "目录",
                Some("file") => "文件",
                Some("symlink") => "链接",
                Some("unallocated") => "未分配",
                _ => "-",
            };
            let mut row = vec![
                if deleted {
                    format!("{kind}✗")
                } else {
                    kind.to_owned()
                },
                render::cell(&item["size"]),
                time(item, "modified"),
            ];
            if args.long {
                row.extend([
                    time(item, "created"),
                    time(item, "accessed"),
                    render::cell(&item["inode"]),
                    render::cell(&item["id"]),
                ]);
            }
            let mut name = render::cell(&item["name"]);
            if let Some(target) = item["linkTarget"].as_str() {
                name.push_str(&format!(" -> {}", crate::cli::output::sanitize(target)));
            }
            row.push(name);
            row
        })
        .collect();
    if rows.is_empty() {
        return format!("（{} 为空）\n", render::cell(&value["path"]));
    }
    let mut out = render::table_rows(header, rows);
    out.push_str(&render::page_footer(
        value,
        &format!("ls {}", render::cell(&value["path"])),
    ));
    out
}

/// One node: identity, size and times first, then where its times are shown
/// in, and its content properties when asked for.
pub fn render_stat(value: &Value) -> String {
    if value["items"].is_array() {
        return render::generic(value);
    }
    let kind = match value["kind"].as_str() {
        Some("dir") => "目录",
        Some("file") => "文件",
        Some("symlink") => "符号链接",
        Some("unallocated") => "未分配",
        _ => "-",
    };
    let mut rows: Vec<(&str, String)> = vec![
        ("路径", render::cell(&value["path"])),
        (
            "类型",
            if value["deleted"] == Value::Bool(true) {
                format!("{kind}（已删除）")
            } else {
                kind.to_owned()
            },
        ),
        ("大小", render::cell(&value["size"])),
    ];
    for (label, key) in [
        ("inode", "inode"),
        ("节点 ID", "nodeId"),
        ("链接目标", "linkTarget"),
    ] {
        if !value[key].is_null() && value[key] != "" {
            rows.push((label, render::cell(&value[key])));
        }
    }
    for (label, role) in [
        ("修改", "modified"),
        ("访问", "accessed"),
        ("创建", "created"),
        ("出生", "birth"),
        ("MFT 变更", "mftChanged"),
    ] {
        if let Some(time) = value["timestamps"][role].as_str() {
            rows.push((label, crate::cli::output::sanitize(time)));
        }
    }
    let zone = &value["timestampDisplay"]["timeZone"];
    if let Some(name) = zone["zone"].as_str() {
        let mut shown = name.to_owned();
        if let Some(warning) = zone["warnings"]
            .as_array()
            .and_then(|warnings| warnings.first())
        {
            shown.push_str(&format!("（{}）", render::cell(warning)));
        }
        rows.push(("时区", shown));
    }
    if let Some(fields) = value["timestampDisplay"]["unconvertedFields"].as_array() {
        if !fields.is_empty() {
            rows.push((
                "未换算",
                format!("{}（没有已知偏移，保留原值）", render::cell(&json!(fields))),
            ));
        }
    }
    let width = rows
        .iter()
        .map(|(label, _)| crate::cli::text::display_width(label))
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (label, shown) in rows {
        let padding = " ".repeat(width - crate::cli::text::display_width(label) + 2);
        out.push_str(&format!(
            "{}{padding}{shown}\n",
            crate::cli::output::bold(label)
        ));
    }
    if value["content"].is_object() {
        out.push('\n');
        out.push_str(&render::outline(&json!({"content": value["content"]})));
    }
    out
}

pub fn render_tree(value: &Value) -> String {
    let mut lines = vec![render::cell(&value["path"])];
    for item in value["items"].as_array().map_or(&[][..], Vec::as_slice) {
        let depth = item["depth"].as_u64().unwrap_or(1) as usize;
        let mut name = render::cell(&item["name"]);
        if item["kind"] == "dir" {
            name.push('/');
        }
        if item["deleted"] == Value::Bool(true) {
            name.push_str(" ✗");
        }
        lines.push(format!("{}{name}", "  ".repeat(depth.max(1))));
    }
    let mut out = lines.join("\n") + "\n";
    out.push_str(&render::page_footer(value, "tree"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_rows_say_whether_they_can_be_used() {
        assert_eq!(
            index_state(&json!({"indexed": true, "stale": false, "truncated": false})),
            "ready"
        );
        assert_eq!(
            index_state(&json!({"indexed": true, "stale": true})),
            "stale"
        );
        assert_eq!(
            index_state(&json!({"indexed": true, "truncated": true})),
            "truncated"
        );
        assert_eq!(index_state(&json!({"indexed": false})), "building");
    }
}
