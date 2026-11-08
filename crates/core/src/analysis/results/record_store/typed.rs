//! One table per record type: column layout, typed binding and reading.
//!
//! Field `i` of a schema is column `c<i>`; a timestamp field also has the
//! sort column `s<i>` (microseconds since the epoch). SQLite types follow
//! the field kind, so sorting and range filters compare numbers as numbers.

use std::sync::Arc;

use lovelycarve_plugin::{
    schema::{self, FieldKind, RecordSchema},
    Value,
};
use rusqlite::{types::Value as SqlValue, Connection, OptionalExtension};

/// A registered record type and its table.
pub(crate) struct TypeTable {
    pub type_id: i64,
    pub schema: Arc<RecordSchema>,
    insert_sql: String,
}

/// A timestamp with a timeline role, as `record_time` stores it.
pub(super) struct TimePoint<'a> {
    pub field: &'a str,
    pub role: &'a str,
    pub epoch_us: i64,
    pub zoned: bool,
}

impl TypeTable {
    /// Registers `schema` (once per type name and field structure) and makes
    /// sure its table exists.
    pub(super) fn register(connection: &Connection, schema: &RecordSchema) -> anyhow::Result<Self> {
        schema.validate().map_err(|message| {
            anyhow::anyhow!("record schema {}: {message}", schema.record_type)
        })?;
        let hash = schema::record_schema_hash(schema);
        connection.execute(
            "INSERT INTO record_type (record_type, schema_hash, schema) VALUES (?1, ?2, ?3)
             ON CONFLICT (record_type, schema_hash) DO NOTHING",
            rusqlite::params![schema.record_type, hash, serde_json::to_string(schema)?],
        )?;
        let type_id: i64 = connection.query_row(
            "SELECT type_id FROM record_type WHERE record_type = ?1 AND schema_hash = ?2",
            rusqlite::params![schema.record_type, hash],
            |row| row.get(0),
        )?;
        let mut columns = vec![
            "record_id INTEGER PRIMARY KEY REFERENCES record (record_id) ON DELETE CASCADE"
                .to_owned(),
        ];
        let mut names = vec!["record_id".to_owned()];
        for (index, field) in schema.fields.iter().enumerate() {
            columns.push(format!("c{index} {}", sql_type(field.kind)));
            names.push(format!("c{index}"));
            if field.kind == FieldKind::Timestamp {
                columns.push(format!("s{index} INTEGER"));
                names.push(format!("s{index}"));
            }
        }
        let table = table_name(type_id);
        connection.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS {table} ({});",
            columns.join(", ")
        ))?;
        let placeholders = (1..=names.len())
            .map(|position| format!("?{position}"))
            .collect::<Vec<_>>()
            .join(", ");
        Ok(Self {
            type_id,
            schema: Arc::new(schema.clone()),
            insert_sql: format!(
                "INSERT INTO {table} ({}) VALUES ({placeholders})",
                names.join(", ")
            ),
        })
    }

    pub(super) fn insert(
        &self,
        connection: &Connection,
        record_id: i64,
        values: &[Value],
    ) -> rusqlite::Result<()> {
        let mut parameters = vec![SqlValue::Integer(record_id)];
        for (field, value) in self.schema.fields.iter().zip(values) {
            parameters.push(sql_value(field.kind, value));
            if field.kind == FieldKind::Timestamp {
                parameters.push(
                    timestamp_text(value)
                        .and_then(schema::parse_timestamp)
                        .map_or(SqlValue::Null, |parsed| {
                            SqlValue::Integer(parsed.epoch_micros())
                        }),
                );
            }
        }
        connection
            .prepare_cached(&self.insert_sql)?
            .execute(rusqlite::params_from_iter(parameters))?;
        Ok(())
    }

    /// The timeline points of one record's values.
    pub(super) fn time_points<'a>(&'a self, values: &'a [Value]) -> Vec<TimePoint<'a>> {
        self.schema
            .fields
            .iter()
            .zip(values)
            .filter_map(|(field, value)| {
                let role = field.timeline.as_deref()?;
                if field.kind != FieldKind::Timestamp {
                    return None;
                }
                let parsed = schema::parse_timestamp(timestamp_text(value)?)?;
                Some(TimePoint {
                    field: &field.id,
                    role,
                    epoch_us: parsed.epoch_micros(),
                    zoned: parsed.offset_minutes.is_some(),
                })
            })
            .collect()
    }
}

/// The table of a registered type.
pub(crate) fn table_name(type_id: i64) -> String {
    format!("rec_{type_id}")
}

/// The value column of field `index`.
pub(crate) fn value_column(index: usize) -> String {
    format!("c{index}")
}

/// The column that orders field `index` of `schema`.
pub(crate) fn sort_column(schema: &RecordSchema, index: usize) -> Option<String> {
    let field = schema.fields.get(index)?;
    Some(match field.kind {
        FieldKind::Timestamp => format!("s{index}"),
        _ => value_column(index),
    })
}

/// Looks up a registered type by table id (for reading).
pub(crate) fn registered_schema(
    connection: &Connection,
    type_id: i64,
) -> anyhow::Result<Option<RecordSchema>> {
    let text: Option<String> = connection
        .query_row(
            "SELECT schema FROM record_type WHERE type_id = ?1",
            [type_id],
            |row| row.get(0),
        )
        .optional()?;
    text.map(|text| serde_json::from_str(&text).map_err(Into::into))
        .transpose()
}

fn sql_type(kind: FieldKind) -> &'static str {
    match kind {
        FieldKind::Int | FieldKind::Size | FieldKind::Bool => "INTEGER",
        FieldKind::Float | FieldKind::Duration => "REAL",
        FieldKind::Text
        | FieldKind::Timestamp
        | FieldKind::Path
        | FieldKind::Hash
        | FieldKind::Ip
        | FieldKind::Enum
        | FieldKind::List
        | FieldKind::Json => "TEXT",
    }
}

fn timestamp_text(value: &Value) -> Option<&str> {
    value.as_str()
}

/// A validated value as its column stores it.
fn sql_value(kind: FieldKind, value: &Value) -> SqlValue {
    if value.is_nil() {
        return SqlValue::Null;
    }
    match kind {
        FieldKind::Text
        | FieldKind::Timestamp
        | FieldKind::Path
        | FieldKind::Hash
        | FieldKind::Ip
        | FieldKind::Enum => value
            .as_str()
            .map_or(SqlValue::Null, |text| SqlValue::Text(text.to_owned())),
        FieldKind::Int | FieldKind::Size => match value {
            Value::Integer(number) => match number.as_i64() {
                Some(signed) => SqlValue::Integer(signed),
                // Above i64::MAX: keep the order, lose the last digits.
                None => SqlValue::Real(number.as_f64().unwrap_or(f64::MAX)),
            },
            _ => SqlValue::Null,
        },
        FieldKind::Float | FieldKind::Duration => match value {
            Value::Integer(number) => SqlValue::Real(number.as_f64().unwrap_or_default()),
            Value::F32(number) => SqlValue::Real(f64::from(*number)),
            Value::F64(number) => SqlValue::Real(*number),
            _ => SqlValue::Null,
        },
        FieldKind::Bool => match value {
            Value::Boolean(flag) => SqlValue::Integer(i64::from(*flag)),
            _ => SqlValue::Null,
        },
        FieldKind::List | FieldKind::Json => SqlValue::Text(json_of(value).to_string()),
    }
}

/// Reads column `column` of a page row as the JSON the UI shows.
pub(crate) fn column_json(
    kind: FieldKind,
    row: &rusqlite::Row<'_>,
    column: usize,
) -> rusqlite::Result<serde_json::Value> {
    use rusqlite::types::ValueRef;
    let value = row.get_ref(column)?;
    Ok(match (kind, value) {
        (_, ValueRef::Null) => serde_json::Value::Null,
        (FieldKind::Bool, ValueRef::Integer(number)) => serde_json::Value::Bool(number != 0),
        (FieldKind::List | FieldKind::Json, ValueRef::Text(text)) => {
            serde_json::from_slice(text).unwrap_or(serde_json::Value::Null)
        }
        (_, ValueRef::Integer(number)) => serde_json::Value::from(number),
        (_, ValueRef::Real(number)) => serde_json::Number::from_f64(number)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        (_, ValueRef::Text(text)) => {
            serde_json::Value::String(String::from_utf8_lossy(text).into_owned())
        }
        (_, ValueRef::Blob(_)) => serde_json::Value::Null,
    })
}

/// A MessagePack value as JSON (validated values only: no binary, finite
/// numbers, text keys).
pub(crate) fn json_of(value: &Value) -> serde_json::Value {
    match value {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(flag) => serde_json::Value::Bool(*flag),
        Value::Integer(number) => number
            .as_i64()
            .map(serde_json::Value::from)
            .or_else(|| number.as_u64().map(serde_json::Value::from))
            .unwrap_or(serde_json::Value::Null),
        Value::F32(number) => serde_json::Number::from_f64(f64::from(*number))
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        Value::F64(number) => serde_json::Number::from_f64(*number)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        Value::String(text) => {
            serde_json::Value::String(text.as_str().unwrap_or_default().to_owned())
        }
        Value::Array(items) => serde_json::Value::Array(items.iter().map(json_of).collect()),
        Value::Map(pairs) => serde_json::Value::Object(
            pairs
                .iter()
                .filter_map(|(key, item)| Some((key.as_str()?.to_owned(), json_of(item))))
                .collect(),
        ),
        Value::Binary(_) | Value::Ext(..) => serde_json::Value::Null,
    }
}
