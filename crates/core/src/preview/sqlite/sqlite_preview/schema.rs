//! schema implementation.
use super::*;

pub(super) fn read_digest(session: &SqliteSession, key: &SqliteKey) -> anyhow::Result<InnerDigest> {
    let (conn, encrypted) = open_session_conn(session, key)?;
    read_digest_from_conn(conn, encrypted)
}

#[cfg(test)]
pub(super) fn read_digest_path(path: &Path, key: &SqliteKey) -> anyhow::Result<InnerDigest> {
    let (conn, encrypted) = open_path_conn(path, key)?;
    read_digest_from_conn(conn, encrypted)
}

pub(super) fn read_digest_from_conn(
    conn: Connection,
    encrypted: bool,
) -> anyhow::Result<InnerDigest> {
    crate::preview::sqlite::sqlite_preview_budget::run(&conn, || {
        read_digest_bounded(&conn, encrypted)
    })
}

pub(super) fn read_digest_bounded(
    conn: &Connection,
    encrypted: bool,
) -> anyhow::Result<InnerDigest> {
    let header = read_header(conn)?;

    let mut entries: Vec<(String, String, Option<String>)> = conn
        .prepare(
            "SELECT type, name, sql FROM sqlite_master \
                 WHERE type IN ('table','view') \
                 AND name NOT LIKE 'sqlite_%' \
                 ORDER BY type, name COLLATE NOCASE",
        )?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });

    let mut tables = Vec::with_capacity(entries.len());
    for (kind, name, sql) in entries {
        match digest_one(conn, &kind, &name, sql.clone()) {
            Ok(table) => tables.push(table),
            Err(err)
                if err.downcast_ref::<rusqlite::Error>().is_some_and(|error| {
                    error.sqlite_error_code() == Some(rusqlite::ErrorCode::OperationInterrupted)
                }) =>
            {
                return Err(err)
            }
            Err(err) => tables.push(SqliteTable {
                name,
                kind,
                columns: Vec::new(),
                row_count: None,
                sample_rows: Vec::new(),
                error: Some(err.to_string()),
                create_sql: sql,
            }),
        }
    }

    let indexes = read_indexes(conn).unwrap_or_default();

    Ok(InnerDigest {
        header,
        tables,
        indexes,
        encrypted,
    })
}

pub(super) fn read_header(conn: &Connection) -> anyhow::Result<SqliteHeader> {
    let page_size: u32 = pragma_i64(conn, "page_size") as u32;
    let user_version: i64 = pragma_i64(conn, "user_version");
    let application_id: i64 = pragma_i64(conn, "application_id");
    let schema_version: i64 = pragma_i64(conn, "schema_version");
    let encoding: String = conn
        .pragma_query_value(None, "encoding", |row| row.get(0))
        .unwrap_or_else(|_| "unknown".to_string());
    let journal_mode: Option<String> = conn
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .ok();
    Ok(SqliteHeader {
        page_size,
        user_version,
        application_id,
        schema_version,
        encoding,
        journal_mode,
    })
}

/// Read an integer-valued PRAGMA, tolerating SQLCipher's habit of
/// returning some header pragmas (e.g. `page_size`) as TEXT on an
/// encrypted connection. Falls back to parsing the text, then to 0,
/// rather than failing the whole digest over a cosmetic header field.
pub(super) fn pragma_i64(conn: &Connection, name: &str) -> i64 {
    if let Ok(value) = conn.pragma_query_value(None, name, |row| row.get::<_, i64>(0)) {
        return value;
    }
    conn.pragma_query_value(None, name, |row| {
        let text: String = row.get(0)?;
        Ok(text.trim().parse::<i64>().unwrap_or(0))
    })
    .unwrap_or(0)
}

pub(super) fn digest_one(
    conn: &Connection,
    kind: &str,
    name: &str,
    create_sql: Option<String>,
) -> anyhow::Result<SqliteTable> {
    let columns = read_columns(conn, name)?;
    let row_count = if kind == "table" {
        count_rows(conn, name).ok()
    } else {
        None
    };
    let sample_rows = if columns.is_empty() {
        Vec::new()
    } else {
        sample_rows(conn, name, &columns, DEFAULT_ROW_SAMPLE)?
    };
    Ok(SqliteTable {
        name: name.to_string(),
        kind: kind.to_string(),
        columns,
        row_count,
        sample_rows,
        error: None,
        create_sql,
    })
}

pub(super) fn read_columns(conn: &Connection, table: &str) -> anyhow::Result<Vec<SqliteColumn>> {
    let pragma = format!("PRAGMA table_info({})", quote_ident(table));
    let mut stmt = conn.prepare(&pragma)?;
    let rows = stmt.query_map([], |row| {
        Ok(SqliteColumn {
            name: row.get::<_, String>(1)?,
            type_name: row.get::<_, String>(2).unwrap_or_default(),
            notnull: row.get::<_, i64>(3).unwrap_or(0) != 0,
            default_value: row.get::<_, Option<String>>(4).unwrap_or(None),
            pk: row.get::<_, i64>(5).unwrap_or(0),
        })
    })?;
    let mut out = Vec::new();
    for column in rows {
        out.push(column?);
    }
    Ok(out)
}

pub(super) fn count_rows(conn: &Connection, table: &str) -> anyhow::Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {}", quote_ident(table));
    let count: i64 = conn.query_row(&sql, [], |row| row.get(0))?;
    Ok(count)
}

pub(super) fn sample_rows(
    conn: &Connection,
    table: &str,
    columns: &[SqliteColumn],
    limit: usize,
) -> anyhow::Result<Vec<Vec<SqliteCell>>> {
    let sql = format!("SELECT * FROM {} LIMIT {}", quote_ident(table), limit);
    let mut stmt = conn.prepare(&sql)?;
    let mut rows = stmt.query([])?;
    let mut out: Vec<Vec<SqliteCell>> = Vec::new();
    while let Some(row) = rows.next()? {
        let mut cells = Vec::with_capacity(columns.len());
        for index in 0..columns.len() {
            let cell = match row.get_ref(index)? {
                ValueRef::Null => SqliteCell::Null,
                ValueRef::Integer(value) => SqliteCell::Integer(value),
                ValueRef::Real(value) => SqliteCell::Real(value),
                ValueRef::Text(bytes) => {
                    let text = String::from_utf8_lossy(bytes);
                    SqliteCell::Text(truncate_text(&text))
                }
                ValueRef::Blob(bytes) => SqliteCell::Blob { size: bytes.len() },
            };
            cells.push(cell);
        }
        out.push(cells);
    }
    Ok(out)
}

pub(super) fn read_indexes(conn: &Connection) -> anyhow::Result<Vec<SqliteIndex>> {
    let mut stmt = conn.prepare(
        "SELECT name, tbl_name FROM sqlite_master \
             WHERE type='index' AND name NOT LIKE 'sqlite_%' \
             ORDER BY tbl_name COLLATE NOCASE, name COLLATE NOCASE",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut out = Vec::new();
    for entry in rows {
        let (name, table) = entry?;
        let unique = match conn.query_row(
            "SELECT \"unique\" FROM pragma_index_list(?1) WHERE name = ?2",
            rusqlite::params![&table, &name],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(value) => value != 0,
            Err(_) => false,
        };
        out.push(SqliteIndex {
            name,
            table,
            unique,
        });
    }
    Ok(out)
}

pub(super) fn quote_ident(name: &str) -> String {
    let escaped = name.replace('"', "\"\"");
    format!("\"{escaped}\"")
}

pub(crate) fn truncate_text(text: &str) -> String {
    if text.chars().count() <= MAX_CELL_TEXT {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(MAX_CELL_TEXT).collect();
        out.push('…');
        out
    }
}
