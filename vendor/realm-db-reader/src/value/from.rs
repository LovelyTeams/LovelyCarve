use chrono::{DateTime, Utc};

use crate::table::Row;
use crate::value::{Backlink, Link, Value};

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Value::Int(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_string())
    }
}

impl From<chrono::DateTime<Utc>> for Value {
    fn from(value: DateTime<Utc>) -> Self {
        Value::Timestamp(value)
    }
}

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Value::Float(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Float(value as f32)
    }
}

impl From<Link> for Value {
    fn from(value: Link) -> Self {
        Value::Link(value)
    }
}

impl From<Vec<Link>> for Value {
    fn from(value: Vec<Link>) -> Self {
        Value::LinkList(value)
    }
}

impl From<Backlink> for Value {
    fn from(value: Backlink) -> Self {
        Value::BackLink(value)
    }
}

impl From<Vec<Row<'static>>> for Value {
    fn from(value: Vec<Row<'static>>) -> Self {
        Value::Table(value)
    }
}

impl<T> From<Option<T>> for Value
where
    T: Into<Value>,
{
    fn from(value: Option<T>) -> Self {
        match value {
            Some(v) => v.into(),
            None => Value::None,
        }
    }
}
