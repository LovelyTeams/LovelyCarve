//! `--json=fields`: keep only some fields of each list entry. Paging and
//! provenance fields at the top level survive projection.
use serde_json::{json, Map, Value};

use super::{CliError, Output};

pub(super) fn parse(fields: Option<&str>) -> Result<Vec<Vec<String>>, CliError> {
    let Some(fields) = fields.filter(|fields| !fields.trim().is_empty()) else {
        return Ok(Vec::new());
    };
    fields
        .split(',')
        .map(|field| {
            let path: Vec<String> = field.trim().split('.').map(str::to_owned).collect();
            if path.iter().any(String::is_empty) {
                return Err(CliError::new(
                    "usage",
                    "--json= 后面是逗号分隔的字段名；点路径不能有空段",
                ));
            }
            Ok(path)
        })
        .collect()
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
        return Err(CliError::new("usage", "--json=字段 只适用于 JSON 列表结果"));
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
                        return Err(CliError::new(
                            "usage",
                            "--json=字段 需要列表结果：items 或唯一的顶层数组",
                        ))
                    }
                }
            };
            object
                .get_mut(&key)
                .and_then(Value::as_array_mut)
                .ok_or_else(|| CliError::new("usage", "--json=字段 需要列表数组"))?
        }
        _ => return Err(CliError::new("usage", "--json=字段 需要列表结果")),
    };
    if items.iter().any(|item| !item.is_object()) {
        return Err(CliError::new(
            "usage",
            "--json=字段 需要对象条目，不能投影标量数组",
        ));
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
        let Output::Json(named) = apply(
            Output::json(json!({"functions":[{"name":"exam","pid":7}],"total":1})),
            &parse(Some("name")).unwrap(),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            named.value(),
            &json!({"functions":[{"name":"exam"}],"total":1})
        );
        let Output::Json(zoned) = apply(
            Output::json(
                json!([{"id":"x","timestampTimeZone":1,"timestampUnconvertedFields":["created"]}]),
            ),
            &parse(Some("id")).unwrap(),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(
            zoned.value(),
            &json!([{"id":"x","timestampTimeZone":1,"timestampUnconvertedFields":["created"]}])
        );
        assert!(apply(
            Output::json(json!({"rows":[],"columns":[]})),
            &parse(Some("id")).unwrap()
        )
        .is_err());
        assert!(apply(Output::json(json!(["x"])), &parse(Some("id")).unwrap()).is_err());
        assert!(parse(Some("name,,id")).is_err());
        assert!(parse(Some("")).unwrap().is_empty());
        assert!(apply(Output::json(json!({"id":"x"})), &parse(Some("id")).unwrap()).is_err());
    }
}
