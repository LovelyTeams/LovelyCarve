//! Realm native-type preview for files inside a forensic image.
//!
//! The vendored `realm-db-reader` has been extended to read from immutable
//! in-memory bytes, so this module never materialises the database to a host
//! temp file. Commands keep a single-slot byte cache and pass the shared slice
//! here for table discovery and paged row reads.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;

use anyhow::Context;
use realm_db_reader::{Column, Group, Realm, RealmFileError, Table, Value};
use serde::Serialize;

const MAX_PAGE_ROWS: usize = 500;
const MAX_TEXT_CHARS: usize = 500;
const MAX_BINARY_PREVIEW_BYTES: usize = 64;
const MAX_LINKS: usize = 24;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealmDigest {
    pub tables: Vec<RealmTable>,
    pub bytes_read: u64,
    pub truncated_bytes: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealmTable {
    pub name: String,
    pub table_number: usize,
    pub columns: Vec<RealmColumn>,
    pub row_count: Option<usize>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealmColumn {
    pub index: usize,
    pub name: Option<String>,
    pub nullable: bool,
    pub indexed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealmRows {
    pub columns: Vec<RealmColumn>,
    pub rows: Vec<RealmRow>,
    pub total: usize,
    pub offset: usize,
    pub returned: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealmRow {
    pub index: usize,
    pub cells: Vec<RealmCell>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "camelCase")]
pub enum RealmCell {
    Null,
    Int(i64),
    Bool(bool),
    String {
        text: String,
        truncated: bool,
    },
    Binary(RealmBinary),
    Table {
        rows: usize,
    },
    Timestamp(String),
    Float(f32),
    Double(f64),
    Link(RealmLink),
    LinkList {
        links: Vec<RealmLink>,
        truncated: bool,
    },
    Backlink(RealmBacklink),
    Unsupported(String),
    Error(String),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealmBinary {
    pub size: usize,
    pub hex: String,
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealmLink {
    pub target_table_number: usize,
    pub row_number: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RealmBacklink {
    pub origin_table_number: usize,
    pub origin_column_number: usize,
    pub row_numbers: Vec<usize>,
    pub truncated: bool,
}

pub fn preview(
    bytes: Arc<[u8]>,
    _file_size: u64,
    truncated_bytes: bool,
) -> anyhow::Result<RealmDigest> {
    let bytes_read = bytes.len() as u64;
    let group = open_group(bytes)?;
    let names = group.get_table_names().to_vec();
    let mut tables = Vec::with_capacity(names.len());

    for (table_number, name) in names.into_iter().enumerate() {
        let table_result =
            catch_parser(|| group.get_table(table_number).map_err(anyhow::Error::from));
        let table = match table_result {
            Ok(table) => table,
            Err(err) => {
                tables.push(RealmTable {
                    name,
                    table_number,
                    columns: Vec::new(),
                    row_count: None,
                    error: Some(err.to_string()),
                });
                continue;
            }
        };

        let columns = columns_for(&table);
        let row_count = catch_parser(|| table.row_count().map_err(anyhow::Error::from));
        let (row_count, error) = match row_count {
            Ok(count) => (Some(count), None),
            Err(err) => (None, Some(err.to_string())),
        };
        tables.push(RealmTable {
            name,
            table_number,
            columns,
            row_count,
            error,
        });
    }

    let mut notes = Vec::new();
    if truncated_bytes {
        notes.push("文件超过 Realm 预览读取上限，当前解析的是前缀字节。".to_string());
    }

    Ok(RealmDigest {
        tables,
        bytes_read,
        truncated_bytes,
        notes,
    })
}

pub fn table_rows(
    bytes: Arc<[u8]>,
    table_number: usize,
    offset: usize,
    limit: usize,
) -> anyhow::Result<RealmRows> {
    let group = open_group(bytes)?;
    let table = catch_parser(|| group.get_table(table_number).map_err(anyhow::Error::from))
        .with_context(|| format!("打开 Realm 表 #{table_number} 失败"))?;
    let total = catch_parser(|| table.row_count().map_err(anyhow::Error::from))
        .context("读取 Realm 表行数失败")?;
    let columns = columns_for(&table);
    let offset = offset.min(total);
    let limit = limit.clamp(1, MAX_PAGE_ROWS);
    let end = offset.saturating_add(limit).min(total);
    let mut rows = Vec::with_capacity(end.saturating_sub(offset));

    for row_index in offset..end {
        let mut cells = Vec::with_capacity(columns.len());
        for column_index in 0..columns.len() {
            let cell = match catch_parser(|| {
                table
                    .get_value(row_index, column_index)
                    .map(value_to_cell)
                    .map_err(anyhow::Error::from)
            }) {
                Ok(cell) => cell,
                Err(err) => RealmCell::Error(err.to_string()),
            };
            cells.push(cell);
        }
        rows.push(RealmRow {
            index: row_index,
            cells,
        });
    }

    let returned = rows.len();
    Ok(RealmRows {
        columns,
        rows,
        total,
        offset,
        returned,
    })
}

fn open_group(bytes: Arc<[u8]>) -> anyhow::Result<Group> {
    catch_parser(|| {
        Realm::from_arc_bytes(bytes)
            .and_then(Realm::into_group)
            .map_err(anyhow::Error::from)
    })
    .context("打开 Realm 数据库失败")
}

fn columns_for(table: &Table) -> Vec<RealmColumn> {
    table
        .get_column_specs()
        .iter()
        .enumerate()
        .map(|(index, column)| column_meta(index, column.as_ref()))
        .collect()
}

fn column_meta(index: usize, column: &dyn Column) -> RealmColumn {
    RealmColumn {
        index,
        name: column.name().map(ToOwned::to_owned),
        nullable: column.nullable(),
        indexed: column.is_indexed(),
    }
}

fn value_to_cell(value: Value) -> RealmCell {
    match value {
        Value::Int(value) => RealmCell::Int(value),
        Value::Bool(value) => RealmCell::Bool(value),
        Value::String(value) => {
            let (text, truncated) = truncate_chars(&value, MAX_TEXT_CHARS);
            RealmCell::String { text, truncated }
        }
        Value::OldStringEnum(value) => RealmCell::Unsupported(format!("OldStringEnum({value})")),
        Value::Binary(bytes) => {
            let take = bytes.len().min(MAX_BINARY_PREVIEW_BYTES);
            RealmCell::Binary(RealmBinary {
                size: bytes.len(),
                hex: hex::encode(&bytes[..take]),
                truncated: take < bytes.len(),
            })
        }
        Value::Table(rows) => RealmCell::Table { rows: rows.len() },
        Value::OldMixed => RealmCell::Unsupported("OldMixed".to_string()),
        Value::OldDateTime => RealmCell::Unsupported("OldDateTime".to_string()),
        Value::Timestamp(value) => RealmCell::Timestamp(value.to_rfc3339()),
        Value::Float(value) => RealmCell::Float(value),
        Value::Double(value) => RealmCell::Double(value),
        Value::Reserved4 => RealmCell::Unsupported("Decimal/Reserved4".to_string()),
        Value::Link(link) => RealmCell::Link(RealmLink {
            target_table_number: link.target_table_number,
            row_number: link.row_number,
        }),
        Value::LinkList(links) => {
            let truncated = links.len() > MAX_LINKS;
            RealmCell::LinkList {
                links: links
                    .into_iter()
                    .take(MAX_LINKS)
                    .map(|link| RealmLink {
                        target_table_number: link.target_table_number,
                        row_number: link.row_number,
                    })
                    .collect(),
                truncated,
            }
        }
        Value::BackLink(backlink) => {
            let truncated = backlink.row_numbers.len() > MAX_LINKS;
            RealmCell::Backlink(RealmBacklink {
                origin_table_number: backlink.origin_table_number,
                origin_column_number: backlink.origin_column_number,
                row_numbers: backlink.row_numbers.into_iter().take(MAX_LINKS).collect(),
                truncated,
            })
        }
        Value::None => RealmCell::Null,
    }
}

fn truncate_chars(text: &str, max_chars: usize) -> (String, bool) {
    if text.chars().count() <= max_chars {
        return (text.to_string(), false);
    }
    (text.chars().take(max_chars).collect(), true)
}

fn catch_parser<T>(operation: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<T> {
    match catch_unwind(AssertUnwindSafe(operation)) {
        Ok(result) => result,
        Err(payload) => Err(anyhow::Error::from(RealmFileError::InvalidRealmFile {
            reason: format!("Realm parser panicked: {}", panic_message(payload)),
        })),
    }
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_string_cells_by_char_count() {
        let long = "数".repeat(MAX_TEXT_CHARS + 2);
        let (text, truncated) = truncate_chars(&long, MAX_TEXT_CHARS);
        assert!(truncated);
        assert_eq!(text.chars().count(), MAX_TEXT_CHARS);
    }

    #[test]
    fn renders_link_list_with_cap() {
        let links = (0..(MAX_LINKS + 3))
            .map(|row_number| realm_db_reader::Link::new(2, row_number))
            .collect();
        let cell = value_to_cell(Value::LinkList(links));
        let RealmCell::LinkList { links, truncated } = cell else {
            panic!("expected link list cell");
        };
        assert!(truncated);
        assert_eq!(links.len(), MAX_LINKS);
    }
}
