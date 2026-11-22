//! Text output: tables for lists, `key: value` lines for objects, an
//! indented outline for nested data. Tables are for reading; `--json` is the
//! form scripts depend on.
use serde_json::Value;

use super::{output, text};

/// A scalar as a table cell or value: strings as they are, `null` as `-`.
pub fn cell(value: &Value) -> String {
    match value {
        Value::Null => "-".to_owned(),
        Value::Bool(true) => "是".to_owned(),
        Value::Bool(false) => "否".to_owned(),
        Value::String(text) => output::sanitize(text),
        Value::Number(number) => number.to_string(),
        Value::Array(items) if items.iter().all(scalar) => {
            items.iter().map(cell).collect::<Vec<_>>().join(", ")
        }
        Value::Array(_) | Value::Object(_) => output::sanitize(&value.to_string()),
    }
}

fn scalar(value: &Value) -> bool {
    !matches!(value, Value::Array(_) | Value::Object(_))
}

/// The value a dot path names.
pub fn field<'a>(value: &'a Value, path: &str) -> &'a Value {
    path.split('.')
        .try_fold(value, |value, key| value.get(key))
        .unwrap_or(&Value::Null)
}

/// Rows under a header, each column as wide as its widest cell; the last
/// column is never padded.
pub fn table(items: &[Value], columns: &[(&str, &str)]) -> String {
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            columns
                .iter()
                .map(|(_, path)| cell(field(item, path)))
                .collect()
        })
        .collect();
    table_rows(
        columns
            .iter()
            .map(|(title, _)| (*title).to_owned())
            .collect(),
        rows,
    )
}

/// A table from cells already formatted.
pub fn table_rows(header: Vec<String>, rows: Vec<Vec<String>>) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut widths: Vec<usize> = header
        .iter()
        .map(|title| text::display_width(title))
        .collect();
    for row in &rows {
        for (index, value) in row.iter().enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(text::display_width(value));
            }
        }
    }
    let line = |cells: &[String], bold: bool| {
        let mut out = String::new();
        for (index, value) in cells.iter().enumerate() {
            let last = index + 1 == cells.len();
            let shown = if bold {
                output::bold(value)
            } else {
                value.clone()
            };
            out.push_str(&shown);
            if !last {
                let padding = widths[index].saturating_sub(text::display_width(value)) + 2;
                out.push_str(&" ".repeat(padding));
            }
        }
        out.trim_end().to_owned()
    };
    let mut lines = vec![line(&header, true)];
    lines.extend(rows.iter().map(|row| line(row, false)));
    lines.join("\n") + "\n"
}

/// Bytes as people read them, with the exact count for small values.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit + 1 < UNITS.len() {
        size /= 1024.0;
        unit += 1;
    }
    format!("{size:.1} {}", UNITS[unit])
}

/// Nested data as an indented outline: `key: value`, nested objects and
/// lists below their key, lists of flat objects as tables.
pub fn outline(value: &Value) -> String {
    let mut lines = Vec::new();
    outline_into(value, 0, &mut lines);
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

fn outline_into(value: &Value, depth: usize, lines: &mut Vec<String>) {
    let indent = "  ".repeat(depth);
    match value {
        Value::Object(fields) => {
            for (key, child) in fields {
                match child {
                    Value::Object(inner) if inner.is_empty() => {}
                    Value::Array(items) if items.is_empty() => {}
                    Value::Null => {}
                    Value::Object(_) => {
                        lines.push(format!("{indent}{}:", output::bold(key)));
                        outline_into(child, depth + 1, lines);
                    }
                    Value::Array(items) if items.iter().all(scalar) => {
                        lines.push(format!("{indent}{}: {}", output::bold(key), cell(child)));
                    }
                    Value::Array(items) if flat_objects(items) => {
                        lines.push(format!("{indent}{}:", output::bold(key)));
                        let columns = columns_of(items);
                        let columns: Vec<(&str, &str)> = columns
                            .iter()
                            .map(|name| (name.as_str(), name.as_str()))
                            .collect();
                        for line in table(items, &columns).lines() {
                            lines.push(format!("{indent}  {line}"));
                        }
                    }
                    Value::Array(items) => {
                        lines.push(format!("{indent}{}:", output::bold(key)));
                        for item in items {
                            lines.push(format!("{indent}  -"));
                            outline_into(item, depth + 2, lines);
                        }
                    }
                    _ => lines.push(format!("{indent}{}: {}", output::bold(key), cell(child))),
                }
            }
        }
        Value::Array(items) if flat_objects(items) => {
            let columns = columns_of(items);
            let columns: Vec<(&str, &str)> = columns
                .iter()
                .map(|name| (name.as_str(), name.as_str()))
                .collect();
            for line in table(items, &columns).lines() {
                lines.push(format!("{indent}{line}"));
            }
        }
        Value::Array(items) => {
            for item in items {
                if scalar(item) {
                    lines.push(format!("{indent}- {}", cell(item)));
                } else {
                    lines.push(format!("{indent}-"));
                    outline_into(item, depth + 1, lines);
                }
            }
        }
        _ => lines.push(format!("{indent}{}", cell(value))),
    }
}

/// Objects whose fields are all scalars: they read best as table rows.
fn flat_objects(items: &[Value]) -> bool {
    !items.is_empty()
        && items.iter().all(|item| {
            item.as_object().is_some_and(|fields| {
                fields.values().all(|value| {
                    scalar(value)
                        || value
                            .as_array()
                            .is_some_and(|values| values.iter().all(scalar))
                })
            })
        })
}

/// The keys of `items` in first-seen order.
fn columns_of(items: &[Value]) -> Vec<String> {
    let mut columns: Vec<String> = Vec::new();
    for item in items {
        if let Some(fields) = item.as_object() {
            for key in fields.keys() {
                if !columns.contains(key) {
                    columns.push(key.clone());
                }
            }
        }
    }
    columns
}

/// The generic text form: a page of `items` as a table with its paging
/// footer, anything else as an outline.
pub fn generic(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Object(fields) => match fields.get("items").and_then(Value::as_array) {
            Some(items) if flat_objects(items) || items.is_empty() => {
                let columns = columns_of(items);
                let columns: Vec<(&str, &str)> = columns
                    .iter()
                    .map(|name| (name.as_str(), name.as_str()))
                    .collect();
                let mut out = table(items, &columns);
                let mut rest = value.clone();
                if let Some(rest) = rest.as_object_mut() {
                    rest.remove("items");
                }
                let rest = outline(&rest);
                if !rest.trim().is_empty() {
                    out.push_str(&rest);
                }
                out
            }
            _ => outline(value),
        },
        _ => outline(value),
    }
}

/// The footer of a page: how to read the rest, when there is more.
pub fn page_footer(value: &Value, command: &str) -> String {
    let mut lines = Vec::new();
    if let Some(cursor) = value["nextCursor"].as_str() {
        lines.push(format!("… 还有更多：{command} --cursor {cursor}"));
    }
    if value["truncated"] == Value::Bool(true) {
        lines.push("… 结果被截断：不是完整列表".to_owned());
    }
    if lines.is_empty() {
        String::new()
    } else {
        lines
            .iter()
            .map(|line| output::paint(output::Role::Muted, line))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tables_align_wide_characters_and_outlines_nest() {
        let items = vec![
            json!({"name": "分区0", "size": 512, "deleted": false}),
            json!({"name": "a", "size": null, "deleted": true}),
        ];
        assert_eq!(
            table(
                &items,
                &[("名称", "name"), ("大小", "size"), ("已删除", "deleted")]
            ),
            "名称   大小  已删除\n分区0  512   否\na      -     是\n"
        );
        assert_eq!(table(&[], &[("名称", "name")]), "");
        assert_eq!(
            outline(
                &json!({"id": "x", "partitions": [{"id": 0, "kind": "ntfs"}], "ewf": {"md5": "ab"}, "none": null})
            ),
            "ewf:\n  md5: ab\nid: x\npartitions:\n  id  kind\n  0   ntfs\n"
        );
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(10 * 1024 * 1024), "10.0 MiB");
    }
}
