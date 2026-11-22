use anyhow::{ensure, Context};
use rusqlite::config::DbConfig;
use rusqlite::fallible_iterator::FallibleIterator;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::limits::Limit;
use rusqlite::types::Value;
use rusqlite::{params_from_iter, Batch};
use serde_json::Value as Json;

use super::*;
use crate::core::common::error::Failure;
use crate::core::preview::sqlite::sqlite_preview_budget;

const MAX_SQL_BYTES: usize = 64 * 1024;
const MAX_QUERY_ROWS: u32 = 100;
const MAX_QUERY_COLUMNS: usize = 128;
const MAX_PARAMETERS: usize = 256;

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
enum QueryCell {
    Null,
    // JSON clients commonly decode numbers through IEEE 754.
    Integer(String),
    Real(f64),
    Text(String),
    Blob { size: usize },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SqliteQueryPage {
    columns: Vec<String>,
    rows: Vec<Vec<QueryCell>>,
    offset: u32,
    next_offset: Option<u32>,
    shortened_cells: bool,
    sidecars: Vec<String>,
}

/// Run exactly one read-only statement against the imported database view.
pub fn query_session(
    session: &SqliteSession,
    key: &SqliteKey,
    sql: &str,
    parameters: &[Json],
    offset: u32,
    limit: u32,
) -> anyhow::Result<SqliteQueryPage> {
    ensure!(
        sql.len() <= MAX_SQL_BYTES && !sql.contains('\0'),
        "SQL exceeds 64 KiB or contains a NUL byte"
    );
    ensure!(
        parameters.len() <= MAX_PARAMETERS,
        Failure::invalid_input("Too many SQL parameters")
    );
    ensure!(
        (1..=MAX_QUERY_ROWS).contains(&limit),
        Failure::invalid_input("SQL row limit must be 1–100")
    );
    ensure!(
        !session.truncated,
        Failure::resource_limit("SQLite database exceeds the preview import limit")
    );
    if let Some(issue) = &session.wal_issue {
        anyhow::bail!("SQLite WAL coverage could not be verified: {issue}");
    }
    let values = parameters
        .iter()
        .map(|parameter| {
            Ok(match parameter {
                Json::Null => Value::Null,
                Json::Bool(value) => Value::Integer(i64::from(*value)),
                Json::Number(number) if number.is_i64() => Value::Integer(
                    number
                        .as_i64()
                        .context(Failure::invalid_input("Invalid SQLite integer"))?,
                ),
                Json::Number(number) if number.is_f64() => Value::Real(
                    number
                        .as_f64()
                        .context(Failure::invalid_input("Invalid SQLite real"))?,
                ),
                Json::String(value) => Value::Text(value.clone()),
                _ => {
                    anyhow::bail!("SQL parameters must be JSON scalars with signed 64-bit integers")
                }
            })
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    let (connection, _) = open_session_conn(session, key)?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    connection.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    connection.pragma_update(None, "query_only", true)?;
    connection.set_limit(Limit::SQLITE_LIMIT_LENGTH, 16 * 1024 * 1024)?;
    connection.set_limit(Limit::SQLITE_LIMIT_VARIABLE_NUMBER, MAX_PARAMETERS as i32)?;
    connection.authorizer(Some(authorize_select))?;

    sqlite_preview_budget::run(&connection, || {
        let result = select_page(&connection, sql, values, offset, limit, &session.sidecars);
        // SQLite includes the rejected SQL in some errors. Keep its diagnostic
        // and drop the SQL text, which can contain sensitive evidence values.
        if let Some(rusqlite::Error::SqlInputError { msg, .. }) = result
            .as_ref()
            .err()
            .and_then(|error| error.downcast_ref::<rusqlite::Error>())
        {
            anyhow::bail!("{msg}");
        }
        result
    })
}

fn authorize_select(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Select | AuthAction::Recursive => Authorization::Allow,
        AuthAction::Read { .. } if matches!(context.database_name, None | Some("main")) => {
            Authorization::Allow
        }
        AuthAction::Function { function_name }
            if !matches!(
                function_name,
                "load_extension" | "fts3_tokenizer" | "sqlite_log"
            ) && !function_name.starts_with("sqlcipher_") =>
        {
            Authorization::Allow
        }
        _ => Authorization::Deny,
    }
}

fn select_page(
    connection: &Connection,
    sql: &str,
    parameters: Vec<Value>,
    offset: u32,
    limit: u32,
    sidecars: &[String],
) -> anyhow::Result<SqliteQueryPage> {
    let mut batch = Batch::new(connection, sql);
    let mut statement = batch.next()?.context("Enter a SELECT statement")?;
    ensure!(batch.next()?.is_none(), "Only one SQL statement is allowed");
    ensure!(
        statement.readonly() && statement.is_explain() == 0 && statement.column_count() > 0,
        "Only read-only queries that return columns are allowed"
    );
    let count = statement.column_count();
    ensure!(
        count <= MAX_QUERY_COLUMNS,
        "SQL result has more than 128 columns"
    );
    let columns = statement
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect();
    let mut cursor = statement.query(params_from_iter(parameters))?;
    let mut page = SqliteQueryPage {
        columns,
        rows: Vec::new(),
        offset,
        next_offset: None,
        shortened_cells: false,
        sidecars: sidecars.to_vec(),
    };
    for _ in 0..offset {
        if cursor.next()?.is_none() {
            return Ok(page);
        }
    }
    for _ in 0..limit {
        let Some(row) = cursor.next()? else {
            return Ok(page);
        };
        let mut cells = Vec::with_capacity(count);
        for index in 0..count {
            let cell = match row.get_ref(index)? {
                ValueRef::Null => QueryCell::Null,
                ValueRef::Integer(value) => QueryCell::Integer(value.to_string()),
                ValueRef::Real(value) if value.is_finite() => QueryCell::Real(value),
                ValueRef::Real(value) => QueryCell::Text(value.to_string()),
                ValueRef::Text(bytes) => {
                    let text = String::from_utf8_lossy(bytes);
                    page.shortened_cells |= matches!(text, std::borrow::Cow::Owned(_))
                        || text.chars().nth(MAX_CELL_TEXT).is_some();
                    QueryCell::Text(truncate_text(&text))
                }
                ValueRef::Blob(bytes) => QueryCell::Blob { size: bytes.len() },
            };
            cells.push(cell);
        }
        page.rows.push(cells);
    }
    if cursor.next()?.is_some() {
        page.next_offset = Some(
            offset
                .checked_add(limit)
                .context(Failure::invalid_input("SQL offset overflow"))?,
        );
    }
    Ok(page)
}
