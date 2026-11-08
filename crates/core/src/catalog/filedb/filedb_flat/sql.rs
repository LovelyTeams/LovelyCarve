//! sql implementation.
use super::*;

pub(super) fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(",")
}

pub(super) fn text_values<'a>(values: impl IntoIterator<Item = &'a String>) -> Vec<Value> {
    values.into_iter().map(|v| Value::Text(v.clone())).collect()
}

/// `FlatFileFilter` 的 SQL 镜像。每一条与 `flat_query::flat_filter_keeps`
/// 的对应分支一一对应;改一边必须同步另一边(测试 `sql_matches_rust_predicate`
/// 对拍)。
pub(super) fn push_filter(clause: &mut WhereClause, filter: &FlatFileFilter, scope: &FlatScope) {
    if let Some(keyword) = &filter.keyword {
        let like = like_contains_escaped(keyword);
        clause.push(
            "(name_low LIKE ? ESCAPE '\\' OR full_path_low LIKE ? ESCAPE '\\')",
            [Value::Text(like.clone()), Value::Text(like)],
        );
    }
    if let Some(query) = &filter.name_query {
        clause.push(
            "name_low LIKE ? ESCAPE '\\'",
            [Value::Text(if query.contains(['*', '?']) {
                name_query_like_pattern(query)
            } else {
                format!("%{}%", name_query_like_pattern(query))
            })],
        );
    }
    if let Some(query) = &filter.path_query {
        if query.starts_with('/') {
            // 锚定前缀:相对起点的前缀拼回绝对路径后走 full_path_low 区间。
            if let Some(bounds) = prefix_bounds(&scope.absolute_low_prefix(query)) {
                clause.push_path_range(bounds);
            }
        } else {
            clause.push(
                "full_path_low LIKE ? ESCAPE '\\'",
                [Value::Text(like_contains_escaped(query))],
            );
        }
    }
    if let Some(query) = &filter.ext_query {
        clause.push(
            &format!("{EXT_EXPR} LIKE ? ESCAPE '\\'"),
            [Value::Text(like_contains_escaped(query))],
        );
    }
    if let Some(query) = &filter.kind_query {
        clause.push(
            &format!("({KIND_LABEL_EXPR} LIKE ? ESCAPE '\\' OR {KIND_ENGLISH_LABEL_EXPR} LIKE ? ESCAPE '\\')"),
            [
                Value::Text(like_contains_escaped(query)),
                Value::Text(like_contains_escaped(query)),
            ],
        );
    }
    if let Some(query) = &filter.link_query {
        clause.push(
            "LOWER(COALESCE(json_extract(node_json, '$.link_target'), '')) LIKE ? ESCAPE '\\'",
            [Value::Text(like_contains_escaped(query))],
        );
    }
    if let Some(query) = &filter.inode_query {
        clause.push(
            "CAST(COALESCE(json_extract(node_json, '$.inode'), '') AS TEXT) LIKE ? ESCAPE '\\'",
            [Value::Text(like_contains_escaped(query))],
        );
    }
    if let Some(deleted) = filter.deleted {
        let is_delete = if deleted {
            IS_DELETE_DELETED
        } else {
            IS_DELETE_LIVE
        };
        clause.push("is_delete = ?", [Value::Integer(is_delete)]);
    }
    if let Some(bound) = filter.size_ge {
        clause.push("size >= ?", [Value::Integer(clamp_i64(bound))]);
    }
    if let Some(bound) = filter.size_le {
        clause.push("size <= ?", [Value::Integer(clamp_i64(bound))]);
    }
    if let Some(size_mod) = &filter.size_mod {
        clause.push(
            "(size % ?) = ?",
            [
                Value::Integer(clamp_i64(size_mod.divisor)),
                Value::Integer(clamp_i64(size_mod.remainder)),
            ],
        );
    }
    push_time_range(
        clause,
        FlatSortKey::Created.order_expression(),
        filter.time_created.as_ref(),
    );
    push_time_range(
        clause,
        FlatSortKey::Accessed.order_expression(),
        filter.time_accessed.as_ref(),
    );
    push_time_range(
        clause,
        FlatSortKey::Modified.order_expression(),
        filter.time_modified.as_ref(),
    );
    match (
        filter.extensions.is_empty(),
        filter.other_category_exts.is_empty(),
    ) {
        (true, true) => {}
        (false, true) => clause.push(
            &format!("{EXT_EXPR} IN ({})", placeholders(filter.extensions.len())),
            text_values(&filter.extensions),
        ),
        (true, false) => clause.push(
            &format!(
                "({EXT_EXPR} = '' OR {EXT_EXPR} NOT IN ({}))",
                placeholders(filter.other_category_exts.len())
            ),
            text_values(&filter.other_category_exts),
        ),
        (false, false) => {
            let mut values = text_values(&filter.extensions);
            values.extend(text_values(&filter.other_category_exts));
            clause.push(
                &format!(
                    "({EXT_EXPR} IN ({}) OR {EXT_EXPR} = '' OR {EXT_EXPR} NOT IN ({}))",
                    placeholders(filter.extensions.len()),
                    placeholders(filter.other_category_exts.len())
                ),
                values,
            );
        }
    }
}

/// 时间区间:列值为空(NULL → '')的行在区间激活时一律排除,与谓词一致。
pub(super) fn push_time_range(clause: &mut WhereClause, expr: &str, range: Option<&FlatTimeRange>) {
    let Some(range) = range else {
        return;
    };
    let (from, to) = time_range_sql_bounds(range);
    if from.is_none() && to.is_none() {
        return;
    }
    clause.push(&format!("{expr} != ''"), []);
    if let Some(from) = from {
        clause.push(&format!("{expr} >= ?"), [Value::Text(from)]);
    }
    if let Some(to) = to {
        clause.push(&format!("{expr} < ?"), [Value::Text(to)]);
    }
}

/// Metadata predicate shared by directory SQL paging.
pub(crate) fn metadata_filter_sql(filter: &FlatFileFilter) -> (String, Vec<Value>) {
    let mut clause = WhereClause::new();
    push_filter(&mut clause, filter, &FlatScope::Volume);
    (clause.condition().to_string(), clause.binds)
}

pub(super) fn clamp_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

pub(super) fn order_clause(sort: FlatSort) -> String {
    match sort.key {
        None => String::from("kind ASC, name COLLATE natsort ASC, nid ASC"),
        Some(key) => format!(
            "{} {}, nid ASC",
            key.order_expression(),
            if sort.desc { "DESC" } else { "ASC" }
        ),
    }
}

/// 解析起点范围。`None` = 本卷索引无法回答(起点不在本卷、不是目录、或
/// 索引从未展开过它——典型是已删除目录),调用方退回快照 / 实时遍历。
pub(super) fn resolve_scope(
    conn: &Connection,
    volume_key: &str,
    parent_id: &str,
) -> anyhow::Result<Option<FlatScope>> {
    let key = parent_key(parent_id);
    if key == volume_key || key == "root" {
        return Ok(Some(FlatScope::Volume));
    }
    let mut stmt = conn.prepare_cached(
        "SELECT full_path, kind FROM filedb
         WHERE nid = ?1
         LIMIT 1",
    )?;
    let mut rows = stmt.query(params![key])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let full_path: String = row.get(0)?;
    let kind: String = row.get(1)?;
    if kind != "dir" || !walked_dir_exists(conn, key)? {
        return Ok(None);
    }
    Ok(Some(FlatScope::Subtree {
        prefix_low: full_path.to_lowercase(),
        prefix: full_path,
    }))
}

/// [`DELETED_DIRS_SQL`] 加上起点的 `full_path_low` 区间与它的绑定参数。
pub(super) fn deleted_dirs_sql(scope: &FlatScope) -> (String, Vec<Value>) {
    let mut sql = String::from(DELETED_DIRS_SQL);
    let mut binds = Vec::new();
    if let Some((lower, upper)) = scope.range_bounds() {
        sql.push_str(" AND f.full_path_low >= ? AND f.full_path_low < ?");
        binds.push(Value::Text(lower));
        binds.push(Value::Text(upper));
    }
    (sql, binds)
}

/// 起点范围内「索引确实没有子项」的已删除目录 `(nid, 相对起点路径)`,按
/// 绝对路径排序。
///
/// 已删目录不进 `filedb_walked_dirs`,但它的子项可能仍在索引里(取决于构建
/// 时该目录是否可列)——子项在索引里的目录不需要实时拼接,也不能因此把
/// 查询切到窗口模式;只有子项缺失的才既是拼接目标、又是窗口模式的触发器。
/// 子项存在性用 `pid` 索引上的 EXISTS 探测,每个已删目录 O(log n)。
pub(super) fn deleted_dirs_under(
    conn: &Connection,
    scope: &FlatScope,
) -> anyhow::Result<Vec<(String, String)>> {
    let (sql, binds) = deleted_dirs_sql(scope);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(binds.iter()), |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut deleted = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    // 拼接缓存按整个列表判等,顺序必须确定:同路径的已删目录再按 nid 排。
    deleted.sort_by(|(left_nid, left_path), (right_nid, right_path)| {
        left_path
            .cmp(right_path)
            .then_with(|| left_nid.cmp(right_nid))
    });
    Ok(deleted
        .into_iter()
        .map(|(nid, full_path)| (nid, scope.relative_path(&full_path)))
        .collect())
}

/// 全显的 WHERE:文件行(含符号链接)、起点子树范围与过滤条件。
pub(super) fn flat_clause(scope: &FlatScope, filter: &FlatFileFilter) -> WhereClause {
    let mut clause = WhereClause::new();
    clause.push("kind IN ('file', 'symlink')", []);
    clause.scope(scope);
    push_filter(&mut clause, filter, scope);
    clause
}

/// 两段式取页的外层:把内层 `page_sql` 切出的一页 `ordinal` 按 rowid 回表取
/// 整行,再按同一 `order` 输出。绑定参数就是 `page_sql` 的参数。
///
/// 必须是 CROSS JOIN。索引文件不跑 ANALYZE,没有统计时普通 JOIN 下规划器会
/// 让 filedb 做外层、沿能满足外层 ORDER BY 的索引扫整卷来省掉最后那次排序,
/// 把页子查询放进内层,而内层的子查询对外层每一行都要重跑一遍——100 万行
/// 的卷上无过滤每页数秒,带关键词时合计数十分钟。SQLite 从不重排 CROSS
/// JOIN 两侧:页子查询恒为外层、只跑一次,filedb 按 rowid 逐行回表,至多
/// LIMIT 行。
pub(crate) fn page_join_sql(columns: &str, page_sql: &str, order: &str) -> String {
    format!(
        "SELECT {columns}
         FROM ({page_sql}) page
         CROSS JOIN filedb f ON f.ordinal = page.ordinal
         ORDER BY {order}"
    )
}

/// 全显取页:内层只把「排序键 + 主键」送进排序器(默认序直接沿
/// `flat_default` 的索引序)切出 `LIMIT ? OFFSET ?` 页,外层按主键回表取
/// `node_json`。一步到位的窗口函数写法(`SELECT …, COUNT(*) OVER () …
/// ORDER BY … LIMIT ? OFFSET ?`)会让排序器搬运全部命中行的 node_json(每行
/// KiB 级),深 offset 下是数量级差距,所以总数由独立的 COUNT 承担。
pub(super) fn page_sql(clause: &WhereClause, sort: FlatSort) -> String {
    let order = order_clause(sort);
    page_join_sql(
        "f.node_json, f.full_path",
        &format!(
            "SELECT ordinal FROM {} WHERE {} ORDER BY {order} LIMIT ? OFFSET ?",
            clause.source(),
            clause.condition()
        ),
        &order,
    )
}

/// 全显总数。整卷起点且没有过滤时只按 `kind` 计数,锁定最窄的 `datatype`
/// 覆盖索引:没有统计时规划器在同样以 `kind` 打头的索引间会挑最宽的
/// `flat_default`,多读数倍的页。有过滤时由 `flat_default` 覆盖,带路径区间
/// 时见 [`WhereClause::source`]。
pub(super) fn count_sql(filter: &FlatFileFilter, clause: &WhereClause) -> String {
    let source = if !clause.path_range && filter.is_empty() {
        "filedb INDEXED BY datatype"
    } else {
        clause.source()
    };
    format!("SELECT COUNT(*) FROM {source} WHERE {}", clause.condition())
}
