//! query implementation.
use super::*;

// ── 查询：types / count / datas / range ───────────────────────────────

/// 共享过滤条件构造。返回 `(where 子句, 绑定值)`;空条件返回 `"1=1"`。
pub(super) fn build_filters(query: &TimelineQuery) -> (String, Vec<Value>) {
    let mut conditions: Vec<String> = Vec::new();
    let mut values: Vec<Value> = Vec::new();
    if let Some(start) = query.start {
        values.push(Value::Integer(start));
        conditions.push(format!("tl_time >= ?{}", values.len()));
    }
    if let Some(end) = query.end {
        values.push(Value::Integer(end));
        conditions.push(format!("tl_time <= ?{}", values.len()));
    }
    if !query.from_apps.is_empty() && query.pairs.is_empty() {
        let slots: Vec<String> = query
            .from_apps
            .iter()
            .map(|app| {
                values.push(Value::Text(app.clone()));
                format!("?{}", values.len())
            })
            .collect();
        conditions.push(format!("from_app IN ({})", slots.join(",")));
    }
    if !query.data_types.is_empty() && query.pairs.is_empty() {
        let slots: Vec<String> = query
            .data_types
            .iter()
            .map(|data_type| {
                values.push(Value::Integer(*data_type as i64));
                format!("?{}", values.len())
            })
            .collect();
        conditions.push(format!("type IN ({})", slots.join(",")));
    }
    if !query.pairs.is_empty() {
        // 行值 IN：两级勾选的 (from_app, type) 对精确匹配。
        let slots: Vec<String> = query
            .pairs
            .iter()
            .map(|pair| {
                values.push(Value::Text(pair.from_app.clone()));
                let app_slot = values.len();
                values.push(Value::Integer(pair.data_type as i64));
                format!("(?{app_slot}, ?{})", values.len())
            })
            .collect();
        conditions.push(format!("(from_app, type) IN (VALUES {})", slots.join(",")));
    }
    if let Some(text) = query
        .text
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        values.push(Value::Text(format!("%{}%", like_needle(text))));
        conditions.push(format!("content LIKE ?{} ESCAPE '\\'", values.len()));
    }
    if !query.show_delete {
        conditions.push("\"delete\" = 0".to_string());
    }
    if conditions.is_empty() {
        ("1=1".to_string(), values)
    } else {
        (conditions.join(" AND "), values)
    }
}

/// LIKE 转义（与 global_search_store 同法）。
pub(super) fn like_needle(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

pub(super) fn value_params(values: &[Value]) -> Vec<&dyn ToSql> {
    values.iter().map(|value| value as &dyn ToSql).collect()
}

/// 两级(应用 → 数据类型)维度清单,`group by from_app, type` + 计数。
pub fn types(conn: &Connection) -> anyhow::Result<Vec<TimelineTypeGroup>> {
    let (where_clause, values) = build_filters(&TimelineQuery {
        show_delete: true,
        ..Default::default()
    });
    let sql = format!(
        "SELECT from_app, type, COUNT(*) FROM timeline WHERE {where_clause}
         GROUP BY from_app, type ORDER BY from_app, type"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(value_params(&values).as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
        ))
    })?;
    let mut groups: Vec<TimelineTypeGroup> = Vec::new();
    for row in rows {
        let (from_app, data_type, count) = row?;
        let info = TimelineTypeInfo {
            data_type: data_type as i32,
            data_type_name: data_type_name(data_type as i32),
            count: count as u64,
        };
        match groups.last_mut() {
            // GROUP BY from_app 保序，相邻同名组合并。
            Some(group) if group.from_app == from_app => group.types.push(info),
            _ => groups.push(TimelineTypeGroup {
                from_app,
                types: vec![info],
            }),
        }
    }
    Ok(groups)
}

/// strftime 动态时间桶聚合（时区参数化）。`limit/offset` 供超大桶数分页。
pub fn count_buckets(
    conn: &Connection,
    query: &TimelineQuery,
    limit: Option<u64>,
    offset: u64,
) -> anyhow::Result<Vec<TimelineBucket>> {
    let (where_clause, mut values) = build_filters(query);
    let format_slot = values.len() + 1;
    let tz_slot = values.len() + 2;
    values.push(Value::Text(query.time_unit.strftime_format().to_string()));
    values.push(Value::Text(format!("{:+} hours", query.time_zone_hours)));
    let limit_clause = match limit {
        Some(limit) => {
            values.push(Value::Integer(limit as i64));
            format!("LIMIT ?{}", values.len())
        }
        None => "LIMIT -1".to_string(),
    };
    values.push(Value::Integer(offset as i64));
    let offset_slot = values.len();
    let sql = format!(
        "SELECT strftime(?{format_slot}, tl_time, 'unixepoch', ?{tz_slot}) AS bucket,
                COUNT(*) AS cnt
         FROM timeline WHERE {where_clause}
         GROUP BY bucket ORDER BY bucket {limit_clause} OFFSET ?{offset_slot}"
    );
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(value_params(&values).as_slice(), |row| {
        Ok(TimelineBucket {
            date_str: row.get(0)?,
            count: row.get::<_, i64>(1)? as u64,
        })
    })?;
    Ok(rows.flatten().collect())
}

pub(super) fn map_row(row: &rusqlite::Row<'_>, eid: &str) -> rusqlite::Result<TimelineRow> {
    Ok(TimelineRow {
        id: row.get(0)?,
        oid: row.get(1)?,
        tid: row.get(2)?,
        eid: eid.to_string(),
        deleted: row.get::<_, i64>(3)? != 0,
        tl_time: row.get(4)?,
        from_app: row.get(5)?,
        data_type: row.get::<_, i64>(6)? as i32,
        tl_flag: row.get(7)?,
        content: row.get(8)?,
        extra: row.get(9)?,
    })
}

/// 明细分页。`total` 同条件 COUNT 另查(与文件全显两段式一致,不用
/// `COUNT(*) OVER()`)。`eid` 是库所属的检材,填进每一行。
pub fn page(
    conn: &Connection,
    eid: &str,
    page_query: &TimelinePageQuery,
) -> anyhow::Result<TimelineDataPage> {
    let (where_clause, mut values) = build_filters(&page_query.query);
    let total: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM timeline WHERE {where_clause}"),
        value_params(&values).as_slice(),
        |row| row.get(0),
    )?;
    values.push(Value::Integer(page_query.limit as i64));
    let limit_slot = values.len();
    values.push(Value::Integer(page_query.skip as i64));
    let offset_slot = values.len();
    let order = if page_query.ascending { "ASC" } else { "DESC" };
    let sql = format!(
        "SELECT {ROW_COLUMNS} FROM timeline WHERE {where_clause}
         ORDER BY tl_time {order}, id {order}
         LIMIT ?{limit_slot} OFFSET ?{offset_slot}"
    );
    let mut statement = conn.prepare(&sql)?;
    let items = statement
        .query_map(value_params(&values).as_slice(), |row| map_row(row, eid))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(TimelineDataPage {
        items,
        total: total as u64,
    })
}

/// 时间范围 `min/max(tl_time)`。无行返回 `None`。
pub fn time_range(conn: &Connection) -> anyhow::Result<Option<(i64, i64)>> {
    let (where_clause, values) = build_filters(&TimelineQuery {
        show_delete: true,
        ..Default::default()
    });
    conn.query_row(
        &format!("SELECT MIN(tl_time), MAX(tl_time) FROM timeline WHERE {where_clause}"),
        value_params(&values).as_slice(),
        |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
    )
    .optional()
    // 聚合恒返回一行；MIN/MAX 为 NULL（无行）时视为无范围。
    .map(|range| range.and_then(|(min, max)| min.zip(max)))
    .map_err(Into::into)
}

/// 导出:与明细表同条件的逐行流式读取(`tl_time asc`,不整表驻留内存)。
pub fn export_rows(
    conn: &Connection,
    eid: &str,
    query: &TimelineQuery,
    mut on_row: impl FnMut(&TimelineRow) -> anyhow::Result<()>,
) -> anyhow::Result<u64> {
    let (where_clause, values) = build_filters(query);
    let sql = format!(
        "SELECT {ROW_COLUMNS} FROM timeline WHERE {where_clause} ORDER BY tl_time ASC, id ASC"
    );
    let mut statement = conn.prepare(&sql)?;
    let mut rows = statement.query(value_params(&values).as_slice())?;
    let mut yielded = 0u64;
    while let Some(row) = rows.next()? {
        on_row(&map_row(row, eid)?)?;
        yielded += 1;
    }
    Ok(yielded)
}
