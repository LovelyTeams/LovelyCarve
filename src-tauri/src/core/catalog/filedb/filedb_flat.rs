//! `filedb` 上的文件全显(allowAllFiles)查询:放弃 `pid = ?` 约束,
//! 递归平铺子树,使用 `LIMIT @limit OFFSET @offset` 分页;
//! 总数由独立 COUNT 给出(见
//! [`page_sql`])。
//!
//! 起点(`parent_id`)不再是 `pid` 约束,而是子树范围:整卷起点不加范围,
//! 目录起点先经 `nid` 索引取到它的 `full_path`,再用 `full_path_low` 的前缀
//! 区间圈定整棵子树(索引可服务)。过滤条件是 `flat_query::FlatFileFilter`
//! 的逐条 SQL 镜像,排序表达式取自 `FlatSortKey::order_expression`,两边
//! 语义由本模块测试与 Rust 谓词/比较器直接对拍。
//!
//! 索引从不下探已删除目录(构建器不展开它们),所以本模块只报出范围内的
//! 已删目录,由命令层实时拼接其子树:此时索引返回的是全局序最靠前的
//! `skip + limit` 行窗口,交给 `FlatPageAssembler` 合并切页。
//!
//! 每个查询只读一个卷的索引文件(`core::catalog::filedb::filedb_file`)。索引文件不跑
//! ANALYZE,计划由下面锁定的写法与 `EXPLAIN QUERY PLAN` 测试固定。

mod sql;
use sql::*;
pub(crate) use sql::{metadata_filter_sql, page_join_sql};

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Context;
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection};

use crate::core::catalog::filedb::filedb_file;
use crate::core::catalog::filedb::filedb_query::{
    deserialize_node, like_contains_escaped, parent_key, prefix_bounds, walked_dir_exists,
};
use crate::core::catalog::flat_query::{
    name_query_like_pattern, time_range_sql_bounds, FlatFileFilter, FlatSort, FlatSortKey,
    FlatTimeRange,
};
use crate::core::filesystem::meta_addr::{IS_DELETE_DELETED, IS_DELETE_LIVE};
use crate::core::filesystem::tree::fs_tree::FsWalkEntry;

/// 一次索引侧全显分页查询。
pub struct FlatPageQuery<'a> {
    /// 卷索引文件。
    pub index: &'a Path,
    pub volume_key: &'a str,
    pub parent_id: &'a str,
    pub filter: &'a FlatFileFilter,
    pub sort: FlatSort,
    pub skip: usize,
    pub limit: usize,
    /// 调用方要把本卷与其它来源合并(多卷起点)时置位:返回窗口而非最终页。
    pub force_window: bool,
    /// 窗口模式的起点下界。与拼接行合并时,参与合并区间 `[skip, skip+limit)`
    /// 的索引行只可能落在 `[skip - 拼接行数, skip + limit)` 内(SQL 行在合并序
    /// 中的位置不会低于自身下标),调用方用缓存的拼接行数收缩窗口,深翻页时
    /// 不必从 0 行开始取。无缓存(首次调用)传 0。
    pub window_floor: usize,
}

/// 索引侧结果。`windowed` 为 true 时 `items` 是按全局序最靠前的
/// `skip + limit` 行(需 `FlatPageAssembler` 合并切页),否则已是最终一页。
#[derive(Debug, Clone)]
pub struct FlatIndexPage {
    pub items: Vec<FsWalkEntry>,
    pub total: u64,
    /// 范围内的已删除目录 `(nid, 相对起点路径)`,命令层实时拼接其子树。
    pub deleted_dirs: Vec<(String, String)>,
    pub windowed: bool,
}

/// 起点范围内按扩展名聚合的文件数(仅 kind = file,与前端类别条口径一致)。
/// `deleted_dirs` 同 [`FlatIndexPage::deleted_dirs`]:索引未下探的已删目录,
/// 命令层实时拼接其子树后再计入。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FlatExtCounts {
    /// 小写扩展名 → 数量;无扩展名的键为空串。
    pub counts: BTreeMap<String, u64>,
    pub total: u64,
    pub deleted_dirs: Vec<(String, String)>,
}

/// 起点的子树范围。
enum FlatScope {
    /// 整卷:不加 full_path 约束。
    Volume,
    /// 目录子树:`full_path_low` 落在 `prefix_low + "/"` 前缀区间内。
    Subtree { prefix: String, prefix_low: String },
}

impl FlatScope {
    /// 子树的 `full_path_low` 半开区间;整卷为 `None`。
    fn range_bounds(&self) -> Option<(String, String)> {
        match self {
            Self::Volume => None,
            Self::Subtree { prefix_low, .. } => prefix_bounds(&format!("{prefix_low}/")),
        }
    }

    /// 锚定路径过滤的绝对形态:把「相对起点」的查询前缀拼回卷内绝对路径。
    fn absolute_low_prefix(&self, relative_low: &str) -> String {
        match self {
            Self::Volume => relative_low.to_string(),
            Self::Subtree { prefix_low, .. } => format!("{prefix_low}{relative_low}"),
        }
    }

    /// 卷内绝对路径 → 相对起点路径(带前导 `/`)。
    fn relative_path(&self, full_path: &str) -> String {
        match self {
            Self::Volume => full_path.to_string(),
            Self::Subtree { prefix, .. } => full_path
                .strip_prefix(prefix.as_str())
                .filter(|rest| rest.starts_with('/'))
                .map_or_else(|| full_path.to_string(), str::to_string),
        }
    }
}

/// WHERE 子句与其绑定参数,按追加顺序拼接。
struct WhereClause {
    sql: String,
    binds: Vec<Value>,
    /// 带 `full_path_low` 前缀区间(子树起点或锚定路径过滤)。
    path_range: bool,
}

impl WhereClause {
    fn new() -> Self {
        Self {
            sql: String::new(),
            binds: Vec::new(),
            path_range: false,
        }
    }

    fn push(&mut self, fragment: &str, values: impl IntoIterator<Item = Value>) {
        if !self.sql.is_empty() {
            self.sql.push_str(" AND ");
        }
        self.sql.push_str(fragment);
        self.binds.extend(values);
    }

    /// WHERE 条件文本;没有任何条件时为恒真式。
    fn condition(&self) -> &str {
        if self.sql.is_empty() {
            "1 = 1"
        } else {
            &self.sql
        }
    }

    fn push_path_range(&mut self, (lower, upper): (String, String)) {
        self.push(
            "full_path_low >= ? AND full_path_low < ?",
            [Value::Text(lower), Value::Text(upper)],
        );
        self.path_range = true;
    }

    fn scope(&mut self, scope: &FlatScope) {
        if let Some(bounds) = scope.range_bounds() {
            self.push_path_range(bounds);
        }
    }

    /// 取数的 FROM 目标。带路径区间时锁定 `full_path_low` 区间计划:
    /// `flat_default` 带着 full_path_low,没有统计的规划器会改为沿它的索引
    /// 序整段扫描、区间当残差——大子树更快,小子树却要扫满整个索引(100 万
    /// 行约 0.3 s,原本毫秒级)。锁定后耗时与子树大小成正比。
    fn source(&self) -> &'static str {
        if self.path_range {
            "filedb INDEXED BY full_path_low"
        } else {
            "filedb"
        }
    }
}

const EXT_EXPR: &str = "LOWER(COALESCE(extension, ''))";

/// 类型列漏斗匹配的标签表达式,与 `flat_query` 侧 `kind_label` 逐项对齐
/// (含未分配空间虚拟节点按 id 前缀的特判)。
const KIND_LABEL_EXPR: &str = "(CASE
    WHEN nid LIKE 'diskgap:%' OR nid LIKE 'unalloc:%' THEN '未分配空间'
    WHEN kind = 'dir' THEN '目录'
    WHEN kind = 'file' THEN '文件'
    WHEN kind = 'symlink' THEN '符号链接'
    WHEN kind = 'unallocated' THEN '未分配'
    ELSE kind END)";

const KIND_ENGLISH_LABEL_EXPR: &str = "(CASE
    WHEN nid LIKE 'diskgap:%' OR nid LIKE 'unalloc:%' THEN 'unallocated space'
    WHEN kind = 'dir' THEN 'folder'
    WHEN kind = 'file' THEN 'file'
    WHEN kind = 'symlink' THEN 'symbolic link'
    WHEN kind = 'unallocated' THEN 'unallocated'
    ELSE kind END)";

/// 本卷里子项不在索引中的已删除目录 `(nid, full_path)`,子树起点另追加
/// `full_path_low` 区间。
///
/// 锁定只收已删除目录的部分索引 `deleted_dir_path`:整卷起点扫它,目录起点
/// 按前缀区间定位,每取一页的开销只与范围内的已删目录数有关。已删除标记
/// 必须写成常量 1(`IS_DELETE_DELETED`),绑定参数匹配不上部分索引的条件。
/// 不锁定时没有统计的规划器走 `is_delete` 索引,逐个回表整卷所有已删除行
/// (含已删文件,每行约 5 µs):已删行占 3.6% 的 130 万行卷上,哪怕只有
/// 500 个文件的子树也要约 230 ms。
///
/// 不带 ORDER BY,由调用方排序。
const DELETED_DIRS_SQL: &str = "SELECT f.nid, f.full_path FROM filedb f INDEXED BY deleted_dir_path
     WHERE f.kind = 'dir' AND f.is_delete = 1
       AND NOT EXISTS (SELECT 1 FROM filedb c WHERE c.pid = f.nid)";

fn count_rows(conn: &Connection, sql: &str, binds: &[Value]) -> anyhow::Result<u64> {
    let count: i64 = conn.query_row(sql, params_from_iter(binds.iter()), |row| row.get(0))?;
    u64::try_from(count).context("filedb flat count is negative")
}

/// 索引侧的一页全显:[`page_sql`] 取行;总数来自同一 WHERE 的 COUNT,页没
/// 取满时直接由行数推出。
pub fn flat_page(query: FlatPageQuery<'_>) -> anyhow::Result<Option<FlatIndexPage>> {
    let conn = filedb_file::open_reader(query.index)?;
    let Some(scope) = resolve_scope(&conn, query.volume_key, query.parent_id)? else {
        return Ok(None);
    };
    let deleted_dirs = deleted_dirs_under(&conn, &scope)?;
    let windowed = query.force_window || !deleted_dirs.is_empty();
    let (limit, offset) = if windowed {
        let floor = query
            .window_floor
            .min(query.skip)
            .min(query.skip.saturating_add(query.limit));
        (
            query.skip.saturating_add(query.limit).saturating_sub(floor),
            floor,
        )
    } else {
        (query.limit, query.skip)
    };

    let clause = flat_clause(&scope, query.filter);
    let mut binds = clause.binds.clone();
    binds.push(Value::Integer(
        i64::try_from(limit).context("flat page limit exceeds i64")?,
    ));
    binds.push(Value::Integer(
        i64::try_from(offset).context("flat page offset exceeds i64")?,
    ));
    let mut stmt = conn.prepare(&page_sql(&clause, query.sort))?;
    let mut rows = stmt.query(params_from_iter(binds.iter()))?;
    let mut items = Vec::new();
    while let Some(row) = rows.next()? {
        let node = deserialize_node(row, 0)?;
        let full_path: String = row.get(1)?;
        items.push(FsWalkEntry {
            node,
            path: scope.relative_path(&full_path),
        });
    }
    drop(rows);
    drop(stmt);
    // 没取满说明结果集已到头,总数就是 offset + 行数,不必再 COUNT——过滤
    // 越稀疏,COUNT 越接近整卷扫描。空页且 offset > 0 时分不清「恰在末尾」
    // 与「越过末尾」,仍要 COUNT。
    let total = if items.len() < limit && (offset == 0 || !items.is_empty()) {
        u64::try_from(offset.saturating_add(items.len())).context("flat page total exceeds u64")?
    } else {
        count_rows(&conn, &count_sql(query.filter, &clause), &clause.binds)?
    };
    Ok(Some(FlatIndexPage {
        items,
        total,
        deleted_dirs,
        windowed,
    }))
}

/// 起点范围内的扩展名直方图(文件类别条的计数来源;不受列表过滤影响,
/// 与逐级浏览时按整目录计数的口径一致)。
pub fn flat_ext_counts(
    index: &Path,
    volume_key: &str,
    parent_id: &str,
) -> anyhow::Result<Option<FlatExtCounts>> {
    let conn = filedb_file::open_reader(index)?;
    let Some(scope) = resolve_scope(&conn, volume_key, parent_id)? else {
        return Ok(None);
    };
    let deleted_dirs = deleted_dirs_under(&conn, &scope)?;
    let mut clause = WhereClause::new();
    clause.push("kind = 'file'", []);
    clause.scope(&scope);
    let sql = format!(
        "SELECT {EXT_EXPR} AS ext, COUNT(*) FROM {} WHERE {} GROUP BY ext",
        clause.source(),
        clause.condition()
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(clause.binds.iter()), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut summary = FlatExtCounts {
        deleted_dirs,
        ..FlatExtCounts::default()
    };
    for row in rows {
        let (extension, count) = row?;
        let count = u64::try_from(count).context("filedb extension count is negative")?;
        summary.total += count;
        summary.counts.insert(extension, count);
    }
    Ok(Some(summary))
}

#[cfg(test)]
mod tests;
