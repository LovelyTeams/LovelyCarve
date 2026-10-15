//! 文件全显(前端 `deepsearch` / 后端 `allowAllFiles`)的查询域模型。
//!
//! 全显查询在列表请求上增加一个
//! 布尔,置位后放弃 `pid = ?` 的目录约束,把当前节点子树按 datatype=file
//! 平铺成分页查询,keyword/表达式过滤、排序与总数统计由数据源侧完成,
//! 通过 `skip` / `limit` 控制分页。
//!
//! LovelyCarve 的全显有三条数据源:filedb 索引(SQLite)、内存卷快照、实时
//! 遍历。本模块承载三者共用的过滤谓词与排序比较器,并给出把多个「已按同一
//! 比较器排好序的来源窗口」装配成一页的装配器——保证三条来源(以及多卷
//! 合并、已删目录实时拼接)对同一查询给出完全一致的结果。SQL 侧的镜像
//! 表达式见 `filedb_query::flat_page`,两边语义由测试对齐。

mod filter;
#[cfg(test)]
use filter::*;
pub use filter::{flat_filter_keeps, node_filter_keeps};
pub(crate) use filter::{name_query_like_pattern, time_range_sql_bounds};

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use crate::core::common::natsort::natural_cmp;
use crate::core::filesystem::tree::fs_tree::{FsNode, FsWalkEntry, NodeKind};

/// 全显分页默认页大小,供滚动预取。
pub const FLAT_PAGE_DEFAULT_SIZE: usize = 100;

/// 单次全显分页上限。与 `filedb_query::MAX_PAGE_SIZE` 对齐;前端深跳转补页
/// 用大页(2000)拉齐,后端仍以此封顶。
pub const FLAT_PAGE_MAX_SIZE: usize = 5_000;

/// 一次实时遍历的安全阀,沿用流式全显时代的 5,000,000 条上限:阻止坏文件
/// 系统的环路遍历无限增长,同时与卷快照构建上限一致(快照能建出来的卷,
/// 实时全显也能枚举完)。
pub const FLAT_WALK_SAFETY_CAP: usize = 5_000_000;

/// 全显视图只铺文件(datatype=file):目录导航交给左侧树,平铺结果不含
/// 目录行;符号链接按文件对待(带指向目标的文件记录)。
fn is_flat_kind(node: &FsNode) -> bool {
    matches!(node.kind, NodeKind::File | NodeKind::Symlink)
}

/// 列排序键。与前端表格列一一对应;默认(无排序键)按「类型,名称」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlatSortKey {
    Kind,
    Name,
    Ext,
    Size,
    Created,
    Accessed,
    Modified,
    Link,
    Inode,
    Path,
    Deleted,
}

impl FlatSortKey {
    /// 前端列 key → 排序键。派生列(MD5/标签/识别类型等)的数据住在独立
    /// 存档里,三条数据源无法统一排序,不接受。
    pub fn from_column(column: &str) -> Option<Self> {
        match column {
            "kind" => Some(Self::Kind),
            "name" => Some(Self::Name),
            "ext" => Some(Self::Ext),
            "size" => Some(Self::Size),
            "created" => Some(Self::Created),
            "accessed" => Some(Self::Accessed),
            "modified" => Some(Self::Modified),
            "link" => Some(Self::Link),
            "inode" => Some(Self::Inode),
            "path" => Some(Self::Path),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }

    /// SQL 排序表达式,与 [`FlatSort::compare`] 的 Rust 取值逐字对齐(NULL
    /// 与空串都排最前、`name` 用 natsort、`is_delete` 列按 1=已删/2=正常
    /// 映射回布尔序)。`filedb_query::flat_page` 负责附加方向与 nid 兜底。
    pub fn order_expression(self) -> &'static str {
        match self {
            Self::Kind => "kind",
            Self::Name => "name COLLATE natsort",
            Self::Ext => "LOWER(COALESCE(extension, ''))",
            Self::Size => "size",
            Self::Created => "COALESCE(crtime, json_extract(node_json, '$.timestamps.birth'), '')",
            Self::Accessed => "COALESCE(atime, '')",
            Self::Modified => "COALESCE(mtime, '')",
            Self::Link => "COALESCE(json_extract(node_json, '$.link_target'), '')",
            Self::Inode => "COALESCE(json_extract(node_json, '$.inode'), -1)",
            Self::Path => "full_path",
            Self::Deleted => "(is_delete = 1)",
        }
    }
}

/// 一次全显查询的排序说明。`desc` 只作用于主键;nid 恒升序兜底,保证分页
/// 翻页时同一行的位置确定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlatSort {
    pub key: Option<FlatSortKey>,
    pub desc: bool,
}

impl FlatSort {
    /// 比较两条全显行。要求双方路径同源(同为「相对当前节点」的路径)。
    pub fn compare(&self, left: &FsWalkEntry, right: &FsWalkEntry) -> Ordering {
        let key = match self.key {
            // 无排序键:默认「类型, 名称」(Type, Name),不反转。
            None => kind_str(&left.node)
                .cmp(kind_str(&right.node))
                .then_with(|| natural_cmp(&left.node.name, &right.node.name)),
            Some(key) => compare_by_key(key, left, right),
        };
        let key = if self.desc && self.key.is_some() {
            key.reverse()
        } else {
            key
        };
        key.then_with(|| left.node.id.cmp(&right.node.id))
    }
}

fn kind_str(node: &FsNode) -> &'static str {
    crate::core::catalog::filedb::filedb_store::node_kind_as_str(node.kind)
}

fn compare_by_key(key: FlatSortKey, left: &FsWalkEntry, right: &FsWalkEntry) -> Ordering {
    match key {
        FlatSortKey::Kind => kind_str(&left.node).cmp(kind_str(&right.node)),
        FlatSortKey::Name => natural_cmp(&left.node.name, &right.node.name),
        FlatSortKey::Ext => flat_ext_of(&left.node).cmp(&flat_ext_of(&right.node)),
        FlatSortKey::Size => left.node.size.cmp(&right.node.size),
        FlatSortKey::Created => {
            raw_timestamp(created_of(&left.node)).cmp(&raw_timestamp(created_of(&right.node)))
        }
        FlatSortKey::Accessed => raw_timestamp(left.node.timestamps.accessed.as_deref())
            .cmp(&raw_timestamp(right.node.timestamps.accessed.as_deref())),
        FlatSortKey::Modified => raw_timestamp(left.node.timestamps.modified.as_deref())
            .cmp(&raw_timestamp(right.node.timestamps.modified.as_deref())),
        FlatSortKey::Link => left
            .node
            .link_target
            .as_deref()
            .unwrap_or_default()
            .cmp(right.node.link_target.as_deref().unwrap_or_default()),
        FlatSortKey::Inode => left.node.inode.cmp(&right.node.inode),
        FlatSortKey::Path => left.path.cmp(&right.path),
        FlatSortKey::Deleted => left.node.deleted.cmp(&right.node.deleted),
    }
}

fn created_of(node: &FsNode) -> Option<&str> {
    node.timestamps
        .created
        .as_deref()
        .or(node.timestamps.birth.as_deref())
}

/// 排序用的原始时间戳:与 SQL `COALESCE(col, '')` 对齐——缺失与空串同列
/// 最前;有值时按 RFC3339 原串字典序(前端 `sortValueOf` 亦按原串排)。
fn raw_timestamp(value: Option<&str>) -> Option<&str> {
    value.filter(|v| !v.is_empty())
}

/// 大小过滤的结构化形态:字节数 `mod divisor = remainder`(找扇区对齐文件
/// 的取证用法)。`divisor = 0` 视为未设置。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatSizeMod {
    pub divisor: u64,
    pub remainder: u64,
}

/// 单个时间列的区间过滤。边界是前端显示口径的 `YYYY-MM-DD HH:MM:SS`
/// (UTC——后端时间戳字符串本就是 UTC,显示层不做时区换算);两端均含,
/// 单端可缺。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FlatTimeRange {
    pub from: Option<String>,
    pub to: Option<String>,
}

impl FlatTimeRange {
    fn is_effective(&self) -> bool {
        self.from.as_deref().is_some_and(|v| !v.is_empty())
            || self.to.as_deref().is_some_and(|v| !v.is_empty())
    }
}

/// 全显的服务端表达式过滤。字段与前端漏斗一一对应;由前端把各列
/// 漏斗的结构化状态(而非 UI 字符串)序列化进来,后端不解析展示格式。
///
/// 名称/路径语义与逐级浏览的客户端过滤一致:名称支持 `*`/`?` 通配(无
/// 通配符即子串);路径以 `/` 开头为锚定前缀,否则子串——但锚定的基准是
/// **当前节点的相对路径**(全显行本就携带相对路径)。`extensions` 与
/// `other_category_exts` 表达文件类别条:前者是所选类别扩展名的并集,
/// 后者在「其他」类别被选中时给出全部已映射扩展名(补集 + 无扩展名归
/// 「其他」)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct FlatFileFilter {
    /// 关键词命中名称或路径(小写副本 `name_low` /
    /// `full_path_low` 的 contains 语义)。
    pub keyword: Option<String>,
    pub name_query: Option<String>,
    pub path_query: Option<String>,
    pub ext_query: Option<String>,
    pub kind_query: Option<String>,
    pub link_query: Option<String>,
    pub inode_query: Option<String>,
    pub deleted: Option<bool>,
    pub size_ge: Option<u64>,
    pub size_le: Option<u64>,
    pub size_mod: Option<FlatSizeMod>,
    pub time_created: Option<FlatTimeRange>,
    pub time_accessed: Option<FlatTimeRange>,
    pub time_modified: Option<FlatTimeRange>,
    pub extensions: Vec<String>,
    pub other_category_exts: Vec<String>,
}

impl FlatFileFilter {
    /// 归一化:小写化匹配串、去空值、剔除非法整除参数,让谓词与 SQL 两
    /// 侧都只面对干净输入。
    pub fn normalized(self) -> Self {
        let lowercase_needle = |value: Option<String>| {
            value
                .map(|v| v.trim().to_lowercase())
                .filter(|v| !v.is_empty())
        };
        Self {
            keyword: lowercase_needle(self.keyword),
            name_query: self.name_query.and_then(|value| {
                let value = if value.contains(['*', '?']) {
                    value.to_lowercase()
                } else {
                    value.trim().to_lowercase()
                };
                (!value.is_empty()).then_some(value)
            }),
            path_query: lowercase_needle(self.path_query),
            ext_query: lowercase_needle(self.ext_query),
            kind_query: lowercase_needle(self.kind_query),
            link_query: lowercase_needle(self.link_query),
            inode_query: lowercase_needle(self.inode_query),
            deleted: self.deleted,
            size_ge: self.size_ge,
            size_le: self.size_le,
            size_mod: self
                .size_mod
                .filter(|m| m.divisor > 0 && m.remainder < m.divisor),
            time_created: self.time_created.filter(FlatTimeRange::is_effective),
            time_accessed: self.time_accessed.filter(FlatTimeRange::is_effective),
            time_modified: self.time_modified.filter(FlatTimeRange::is_effective),
            extensions: self
                .extensions
                .into_iter()
                .map(|e| e.trim().trim_start_matches('.').to_lowercase())
                .filter(|e| !e.is_empty())
                .collect(),
            other_category_exts: self
                .other_category_exts
                .into_iter()
                .map(|e| e.trim().trim_start_matches('.').to_lowercase())
                .filter(|e| !e.is_empty())
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// 全显一页的返回:`items` 是该页行(路径相对查询起点),`total` 是过滤后
/// 的总命中数,`truncated`
/// 仅在实时遍历撞上安全阀时置位。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsFlatPage {
    pub items: Vec<FsWalkEntry>,
    pub total: u64,
    pub truncated: bool,
}

/// 把多个「各自按 `sort` 排好序的来源窗口」装配成一页。每个来源给出它按
/// 全局序最靠前的 `skip + limit` 行(纯 SQL 单来源快路径可退化为直接
/// `LIMIT limit OFFSET skip`,无需装配器);装配器把窗口合并排序后统一
/// 跳 `skip` 取 `limit`,总数取各来源总数之和。
///
/// 正确性:某行若在全局合并序中位于前 `skip + limit` 位,则它在自身来源
/// 内的序号不会更大(其它来源的行只会把它往后顶),因此每个来源取前
/// `skip + limit` 行足以覆盖合并后的前 `skip + limit` 行。
#[derive(Debug)]
pub struct FlatPageAssembler {
    sort: FlatSort,
    skip: usize,
    limit: usize,
    rows: Vec<FsWalkEntry>,
    total: u64,
    truncated: bool,
}

impl FlatPageAssembler {
    pub fn new(sort: FlatSort, skip: usize, limit: usize) -> Self {
        Self {
            sort,
            skip,
            limit,
            rows: Vec::new(),
            total: 0,
            truncated: false,
        }
    }

    /// 追加一个来源的有序窗口。`total` 是该来源过滤后的总数(不是窗口行数)。
    pub fn add_sorted_source(
        &mut self,
        rows: impl IntoIterator<Item = FsWalkEntry>,
        total: u64,
        truncated: bool,
    ) {
        self.rows.extend(rows);
        self.total += total;
        self.truncated |= truncated;
    }

    pub fn finish(mut self) -> FsFlatPage {
        let sort = self.sort;
        // 稳定排序:完全同键(跨卷 nid 撞名)时保持来源追加序,翻页确定。
        self.rows.sort_by(|left, right| sort.compare(left, right));
        let start = self.skip.min(self.rows.len());
        let end = start.saturating_add(self.limit).min(self.rows.len());
        let items: Vec<FsWalkEntry> = self.rows.drain(start..end).collect();
        FsFlatPage {
            items,
            total: self.total,
            truncated: self.truncated,
        }
    }
}

/// 节点扩展名,与前端 `extOf` / 索引 `catalog_extension` 同口径:目录、
/// 点开头的隐藏文件、以点结尾的名字都视为无扩展名(空串);其余取最后一
/// 段点后内容并小写。
pub fn flat_ext_of(node: &FsNode) -> String {
    if node.kind == NodeKind::Dir {
        return String::new();
    }
    let Some((_, extension)) = node.name.rsplit_once('.') else {
        return String::new();
    };
    if extension.is_empty() {
        return String::new();
    }
    if node.name.starts_with('.') && !node.name[1..].contains('.') {
        return String::new();
    }
    extension.to_lowercase()
}

/// Chinese and English type-filter labels, including virtual unallocated spans.
fn kind_filter_labels(node: &FsNode) -> (&'static str, &'static str) {
    if node.id.starts_with("diskgap:") || node.id.starts_with("unalloc:") {
        return ("未分配空间", "unallocated space");
    }
    match node.kind {
        NodeKind::Dir => ("目录", "folder"),
        NodeKind::File => ("文件", "file"),
        NodeKind::Symlink => ("符号链接", "symbolic link"),
        NodeKind::Unallocated => ("未分配", "unallocated"),
    }
}

#[cfg(test)]
mod tests;
