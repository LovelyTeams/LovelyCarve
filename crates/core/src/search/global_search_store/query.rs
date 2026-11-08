//! query implementation.
use super::*;

/// 关键词过滤的 LIKE 绑定值：小写 + 转义 `%`/`_`/`\`，双侧包 `%`。
pub(super) fn like_needle(keyword: &str) -> String {
    let escaped = keyword
        .to_lowercase()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

/// 公共 WHERE 构造（搜索 ID + 增量游标 + 关键词过滤）。返回 SQL 片段与
/// 按序绑定值；`?N` 编号占位符可重复出现（关键词的 4 列 LIKE 复用同一个）。
pub(super) fn build_where(
    query: &GlobalSearchPageQuery,
) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut clause = String::from("search_id = ?1");
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(query.search_id.clone())];
    if let Some(after_seq) = query.after_seq {
        let index = binds.len() + 1;
        clause.push_str(&format!(" AND hit_seq > ?{index}"));
        binds.push(Box::new(after_seq as i64));
    }
    if let Some(keyword) = query
        .keyword
        .as_deref()
        .map(str::trim)
        .filter(|keyword| !keyword.is_empty())
    {
        let index = binds.len() + 1;
        clause.push_str(&format!(
            " AND (context_low LIKE ?{index} ESCAPE '\\'
                 OR file_name_low LIKE ?{index} ESCAPE '\\'
                 OR file_path_low LIKE ?{index} ESCAPE '\\'
                 OR pattern_low LIKE ?{index} ESCAPE '\\')"
        ));
        binds.push(Box::new(like_needle(keyword)));
    }
    (clause, binds)
}

/// 去重分组的 PARTITION BY 表达式。
pub(super) fn distinct_partition(distinct: GlobalSearchDistinct) -> &'static str {
    match distinct {
        GlobalSearchDistinct::None => "node_id",
        GlobalSearchDistinct::File => "node_id",
        GlobalSearchDistinct::Sha256 => {
            "CASE WHEN sha256 = '' THEN node_id ELSE 'sha:' || sha256 END"
        }
    }
}

/// 分页读取命中（含同条件总数）。
pub fn page(
    case_dir: &Path,
    image_id: &str,
    query: &GlobalSearchPageQuery,
) -> anyhow::Result<GlobalSearchPage> {
    let empty = || GlobalSearchPage {
        search_id: query.search_id.clone(),
        items: Vec::new(),
        total: 0,
    };
    let Some(conn) = open_reader(case_dir, image_id)? else {
        return Ok(empty());
    };
    let (where_clause, binds) = build_where(query);
    let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|value| value.as_ref()).collect();

    let items = if query.distinct == GlobalSearchDistinct::None {
        let sql = format!(
            "SELECT {HIT_COLUMNS} FROM global_search_hits
             WHERE {where_clause} ORDER BY hit_seq LIMIT ?{} OFFSET ?{}",
            binds.len() + 1,
            binds.len() + 2
        );
        let mut stmt = conn.prepare(&sql)?;
        let limit = i64::try_from(query.limit).unwrap_or(i64::MAX);
        let offset = i64::try_from(query.skip).unwrap_or(0);
        let mut all_refs = refs.clone();
        all_refs.push(&limit);
        all_refs.push(&offset);
        let rows = stmt.query_map(all_refs.as_slice(), hit_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()?
    } else {
        let partition = distinct_partition(query.distinct);
        let sql = format!(
            "SELECT {HIT_COLUMNS} FROM (
                 SELECT *, ROW_NUMBER() OVER (PARTITION BY {partition} ORDER BY hit_seq) AS rn
                 FROM global_search_hits WHERE {where_clause}
             )
             WHERE rn = 1 ORDER BY hit_seq LIMIT ?{} OFFSET ?{}",
            binds.len() + 1,
            binds.len() + 2
        );
        let mut stmt = conn.prepare(&sql)?;
        let limit = i64::try_from(query.limit).unwrap_or(i64::MAX);
        let offset = i64::try_from(query.skip).unwrap_or(0);
        let mut all_refs = refs.clone();
        all_refs.push(&limit);
        all_refs.push(&offset);
        let rows = stmt.query_map(all_refs.as_slice(), hit_from_row)?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    // 总数：同条件（含去重/增量），不带分页。
    let total_sql = match query.distinct {
        GlobalSearchDistinct::None => {
            format!("SELECT COUNT(*) FROM global_search_hits WHERE {where_clause}")
        }
        GlobalSearchDistinct::File => format!(
            "SELECT COUNT(*) FROM (SELECT DISTINCT node_id FROM global_search_hits WHERE {where_clause})"
        ),
        GlobalSearchDistinct::Sha256 => format!(
            "SELECT COUNT(*) FROM (
                 SELECT DISTINCT CASE WHEN sha256 = '' THEN node_id ELSE 'sha:' || sha256 END
                 FROM global_search_hits WHERE {where_clause})"
        ),
    };
    let total: i64 = conn.query_row(&total_sql, refs.as_slice(), |row| row.get(0))?;

    Ok(GlobalSearchPage {
        search_id: query.search_id.clone(),
        items,
        total: total.max(0) as u64,
    })
}

/// 导出用全量读取（不分页；沿用过滤与去重，与表格所见一致）。
/// `sink` 逐行回调避免整表驻留内存。
pub fn export_rows(
    case_dir: &Path,
    image_id: &str,
    query: &GlobalSearchPageQuery,
    mut sink: impl FnMut(GlobalSearchHit) -> anyhow::Result<()>,
) -> anyhow::Result<u64> {
    let Some(conn) = open_reader(case_dir, image_id)? else {
        return Ok(0);
    };
    let (where_clause, binds) = build_where(query);
    let refs: Vec<&dyn rusqlite::ToSql> = binds.iter().map(|value| value.as_ref()).collect();
    let sql = if query.distinct == GlobalSearchDistinct::None {
        format!(
            "SELECT {HIT_COLUMNS} FROM global_search_hits
             WHERE {where_clause} ORDER BY hit_seq"
        )
    } else {
        let partition = distinct_partition(query.distinct);
        format!(
            "SELECT {HIT_COLUMNS} FROM (
                 SELECT *, ROW_NUMBER() OVER (PARTITION BY {partition} ORDER BY hit_seq) AS rn
                 FROM global_search_hits WHERE {where_clause}
             ) WHERE rn = 1 ORDER BY hit_seq"
        )
    };
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(refs.as_slice(), hit_from_row)?;
    let mut count = 0u64;
    for hit in rows {
        sink(hit?)?;
        count += 1;
    }
    Ok(count)
}
