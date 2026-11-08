//! query implementation.
use super::*;

pub(super) fn read_rows(
    session: &SqliteSession,
    key: &SqliteKey,
    table: &str,
    offset: i64,
    limit: i64,
    order_by: Option<&str>,
    descending: bool,
) -> anyhow::Result<SqliteRows> {
    let (conn, _encrypted) = open_session_conn(session, key)?;

    read_rows_from_conn(&conn, table, offset, limit, order_by, descending)
}

pub(super) fn read_rows_from_conn(
    conn: &Connection,
    table: &str,
    offset: i64,
    limit: i64,
    order_by: Option<&str>,
    descending: bool,
) -> anyhow::Result<SqliteRows> {
    crate::preview::sqlite::sqlite_preview_budget::run(conn, || {
        let columns = read_columns(conn, table)?;
        let total = count_rows(conn, table)?;
        let offset = offset.max(0);
        let limit = limit.clamp(1, MAX_PAGE_ROWS);

        let order_clause = match order_by {
            Some(col) if columns.iter().any(|c| c.name == col) => {
                let dir = if descending { "DESC" } else { "ASC" };
                format!(" ORDER BY {} {dir}", quote_ident(col))
            }
            _ => String::new(),
        };
        let sql = format!(
            "SELECT * FROM {}{order_clause} LIMIT {limit} OFFSET {offset}",
            quote_ident(table)
        );

        let mut stmt = conn.prepare(&sql)?;
        let mut query = stmt.query([])?;
        let mut rows: Vec<Vec<SqliteCell>> = Vec::new();
        while let Some(row) = query.next()? {
            let mut cells = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                let cell = match row.get_ref(index)? {
                    ValueRef::Null => SqliteCell::Null,
                    ValueRef::Integer(value) => SqliteCell::Integer(value),
                    ValueRef::Real(value) => SqliteCell::Real(value),
                    ValueRef::Text(bytes) => {
                        SqliteCell::Text(truncate_text(&String::from_utf8_lossy(bytes)))
                    }
                    ValueRef::Blob(bytes) => SqliteCell::Blob { size: bytes.len() },
                };
                cells.push(cell);
            }
            rows.push(cells);
        }
        let returned = rows.len();
        Ok(SqliteRows {
            columns,
            rows,
            total,
            offset,
            returned,
        })
    })
}

#[cfg(test)]
pub(super) fn read_rows_path(
    path: &Path,
    key: &SqliteKey,
    table: &str,
    offset: i64,
    limit: i64,
    order_by: Option<&str>,
    descending: bool,
) -> anyhow::Result<SqliteRows> {
    let (conn, _encrypted) = open_path_conn(path, key)?;
    read_rows_from_conn(&conn, table, offset, limit, order_by, descending)
}
