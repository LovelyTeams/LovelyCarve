//! CLI-only response shaping. Pagination and forensic provenance survive projection.
use serde_json::{json, Map, Value};

use super::{catalog::Command, CliError, Output};

pub(super) fn agent_page_size(command: &Command, values: &mut Map<String, Value>, agentic: bool) {
    if agentic && command.path() == "files list" {
        values.entry("pageSize").or_insert(json!(50));
    }
}

pub(super) fn parse(fields: Option<&str>) -> Result<Vec<Vec<String>>, CliError> {
    let Some(fields) = fields else {
        return Ok(Vec::new());
    };
    fields
        .split(',')
        .map(|field| {
            let path: Vec<String> = field.trim().split('.').map(str::to_owned).collect();
            if path.iter().any(String::is_empty) {
                return Err(CliError::invalid(
                    "--fields 需要逗号分隔的字段名；点路径不能有空段",
                ));
            }
            Ok(path)
        })
        .collect()
}

pub(super) fn validate(command: &Command, fields: &[Vec<String>]) -> Result<(), CliError> {
    if !fields.is_empty()
        && !matches!(
            command.full_path.last().map(String::as_str),
            Some("list" | "search")
        )
    {
        return Err(CliError::invalid(
            "--fields 仅适用于 list / search 的 JSON 列表结果",
        ));
    }
    Ok(())
}

fn select(source: &Value, target: &mut Map<String, Value>, path: &[String]) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    let Some(value) = source.get(first) else {
        return;
    };
    if rest.is_empty() {
        target.insert(first.clone(), value.clone());
    } else {
        let mut nested = target
            .get(first)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        select(value, &mut nested, rest);
        if !nested.is_empty() {
            target.insert(first.clone(), Value::Object(nested));
        }
    }
}

pub(super) fn apply(output: Output, fields: &[Vec<String>]) -> Result<Output, CliError> {
    if fields.is_empty() {
        return Ok(output);
    }
    let Output::Json(output) = output else {
        return Err(CliError::invalid("--fields 仅适用于 JSON 列表结果"));
    };
    let mut value = output.value().clone();
    let items = match &mut value {
        Value::Array(items) => items,
        Value::Object(object) => {
            let key = if object.get("items").is_some_and(Value::is_array) {
                "items".to_owned()
            } else {
                let arrays: Vec<_> = object
                    .iter()
                    .filter(|(_, value)| value.is_array())
                    .map(|(key, _)| key.clone())
                    .collect();
                match arrays.as_slice() {
                    [key] => key.clone(),
                    _ => {
                        return Err(CliError::invalid(
                            "--fields 无法确定列表：需要 items 或唯一的顶层数组",
                        ))
                    }
                }
            };
            object
                .get_mut(&key)
                .and_then(Value::as_array_mut)
                .ok_or_else(|| CliError::invalid("--fields 需要列表数组"))?
        }
        _ => return Err(CliError::invalid("--fields 需要列表结果")),
    };
    if items.iter().any(|item| !item.is_object()) {
        return Err(CliError::invalid("--fields 需要对象条目，不能投影标量数组"));
    }
    for item in items {
        let mut projected = Map::new();
        for path in fields {
            select(item, &mut projected, path);
        }
        for key in ["timestampTimeZone", "timestampUnconvertedFields"] {
            if let Some(value) = item.get(key) {
                projected.insert(key.into(), value.clone());
            }
            if let Some(value) = item.get("node").and_then(|node| node.get(key)) {
                let node = projected.entry("node").or_insert_with(|| json!({}));
                if let Some(node) = node.as_object_mut() {
                    node.insert(key.into(), value.clone());
                }
            }
        }
        *item = Value::Object(projected);
    }
    Ok(Output::json(value))
}

/// Keep cursors visible even when the consumer truncates a large `items` array.
/// Other JSON keeps its original serialization order.
pub(super) fn page_text(value: &Value) -> Option<String> {
    let object = value
        .as_object()
        .filter(|object| object.contains_key("items"))?;
    let mut text = String::from("{");
    for key in ["nextCursor", "total", "truncated"]
        .into_iter()
        .filter(|key| object.contains_key(*key))
        .chain(
            object
                .keys()
                .map(String::as_str)
                .filter(|key| !matches!(*key, "nextCursor" | "total" | "truncated" | "items")),
        )
        .chain(std::iter::once("items"))
    {
        if text.len() > 1 {
            text.push(',');
        }
        text.push_str(&json!(key).to_string());
        text.push(':');
        text.push_str(&object[key].to_string());
    }
    text.push('}');
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_keeps_pagination_and_nested_types() {
        let source = json!({"items":[{"id":"x","name":"same","size":7,"timestamps":{"modified":null,"created":"raw"}}],"total":2,"nextCursor":"x","truncated":true,"timestampDisplay":{"timeZone":"UTC"}});
        let Output::Json(projected) = apply(
            Output::json(source),
            &parse(Some("id,size,timestamps.modified,missing")).unwrap(),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            projected.value(),
            &json!({"items":[{"id":"x","size":7,"timestamps":{"modified":null}}],"total":2,"nextCursor":"x","truncated":true,"timestampDisplay":{"timeZone":"UTC"}})
        );
        let text = page_text(projected.value()).unwrap();
        assert!(text.starts_with("{\"nextCursor\":\"x\",\"total\":2,\"truncated\":true,"));
        assert!(text.find("timestampDisplay").unwrap() < text.find("items").unwrap());

        let Output::Json(deep) = apply(
            Output::json(json!([{"node":{"timestamps":{"modified":"m","created":"c"}}}])),
            &parse(Some(
                "node.timestamps.modified,node.timestamps.created,node.timestamps.missing",
            ))
            .unwrap(),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            deep.value(),
            &json!([{"node":{"timestamps":{"modified":"m","created":"c"}}}])
        );
        let Output::Json(named) = apply(
            Output::json(json!({"sessions":[{"name":"exam","pid":7}],"total":1})),
            &parse(Some("name")).unwrap(),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            named.value(),
            &json!({"sessions":[{"name":"exam"}],"total":1})
        );
        let Output::Json(qualified) = apply(Output::json(json!([{"node":{"id":"x","timestampTimeZone":1,"timestampUnconvertedFields":["created"]}}])), &parse(Some("node.id")).unwrap()).unwrap() else { panic!() };
        assert_eq!(
            qualified.value(),
            &json!([{"node":{"id":"x","timestampTimeZone":1,"timestampUnconvertedFields":["created"]}}])
        );
        assert!(apply(
            Output::json(json!({"rows":[],"columns":[]})),
            &parse(Some("id")).unwrap()
        )
        .is_err());
        assert!(apply(Output::json(json!(["x"])), &parse(Some("id")).unwrap()).is_err());
        assert!(parse(Some("name,,id")).is_err());
        assert!(parse(Some("timestamps..modified")).is_err());
        assert!(apply(Output::json(json!({"id":"x"})), &parse(Some("id")).unwrap()).is_err());
    }

    #[test]
    fn agent_default_preserves_explicit_page_sizes() {
        let command = super::super::catalog::catalog()
            .unwrap()
            .find_path("files list")
            .unwrap();
        let mut values = Map::new();
        agent_page_size(command, &mut values, false);
        assert!(values.is_empty());
        agent_page_size(command, &mut values, true);
        assert_eq!(values, Map::from_iter([("pageSize".into(), json!(50))]));
        values.insert("pageSize".into(), json!(500));
        agent_page_size(command, &mut values, true);
        assert_eq!(values["pageSize"], 500);
    }
}
