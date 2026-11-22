//! Results that finished but are not complete: a digest or export over
//! zero-filled source bytes, a search that could not read every file, an
//! export or search that stopped early. Each command names the fields that say
//! so (`Meta::incomplete`). The result is still returned, but the command says
//! what is missing and ends with status 2, so it never passes for a complete
//! result.
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::Output;

/// The process status of a command whose result is incomplete.
pub const EXIT_INCOMPLETE: i32 = 2;

/// Why a finished result cannot be taken as complete.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Incomplete {
    pub code: &'static str,
    pub message: String,
    /// `{"fields": {<path>: <value>}}`: the fields that are set.
    pub details: Value,
}

/// `true`, a nonzero number, a nonempty string, array or object.
fn set(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|number| number != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

/// The values a dot path names. An array along the way stands for each of
/// its elements, so `items.readFaultsDetected` checks every item.
fn lookup<'a>(value: &'a Value, path: &[&str], found: &mut Vec<&'a Value>) {
    match (value, path.split_first()) {
        (Value::Array(items), Some(_)) => {
            for item in items {
                lookup(item, path, found);
            }
        }
        (_, None) => found.push(value),
        (Value::Object(fields), Some((key, rest))) => {
            if let Some(child) = fields.get(*key) {
                lookup(child, rest, found);
            }
        }
        (_, Some(_)) => {}
    }
}

/// The verdict on a result before any field projection.
pub fn check(fields: &[&str], output: &Output) -> Option<Incomplete> {
    let Output::Json(json) = output else {
        return None;
    };
    if fields.is_empty() {
        return None;
    }
    let result = json.value();
    let set_fields: Map<String, Value> = fields
        .iter()
        .filter_map(|path| {
            let segments: Vec<&str> = path.split('.').collect();
            let mut found = Vec::new();
            lookup(result, &segments, &mut found);
            let set_values: Vec<&Value> = found.into_iter().filter(|value| set(value)).collect();
            match set_values.as_slice() {
                [] => None,
                [one] => Some(((*path).to_owned(), (*one).clone())),
                many => Some((
                    (*path).to_owned(),
                    Value::Array(many.iter().map(|value| (*value).clone()).collect()),
                )),
            }
        })
        .collect();
    if set_fields.is_empty() {
        return None;
    }
    let shown: Vec<String> = set_fields
        .iter()
        .map(|(path, value)| format!("{path} = {value}"))
        .collect();
    Some(Incomplete {
        code: "incompleteResult",
        message: format!(
            "结果不完整：{}。结果仍会返回，但不能当作完整结果使用。",
            shown.join("、")
        ),
        details: json!({ "fields": set_fields }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &[&str] = &["items.readFaultsDetected", "items.syntheticBytes"];

    #[test]
    fn set_fields_mark_a_result_incomplete_and_unset_ones_do_not() {
        let clean = Output::json(
            json!({"items": [{"md5": "x", "readFaultsDetected": false, "syntheticBytes": 0}]}),
        );
        assert_eq!(check(HASH, &clean), None);
        let faulty = Output::json(json!({"items": [
            {"md5": "x", "readFaultsDetected": false, "syntheticBytes": 0},
            {"md5": "y", "readFaultsDetected": true, "syntheticBytes": 4096}
        ]}));
        assert_eq!(
            check(HASH, &faulty).unwrap(),
            Incomplete {
                code: "incompleteResult",
                message: "结果不完整：items.readFaultsDetected = true、items.syntheticBytes = 4096。结果仍会返回，但不能当作完整结果使用。".into(),
                details: json!({"fields": {"items.readFaultsDetected": true, "items.syntheticBytes": 4096}}),
            }
        );
        // Nested fields, strings and counts.
        let partial = Output::json(json!({"failed": 0, "reportError": "disk full",
            "files": {"failed": 2, "readFaults": {"readFaultsDetected": false}}}));
        assert_eq!(
            check(
                &[
                    "failed",
                    "reportError",
                    "files.failed",
                    "files.readFaults.readFaultsDetected"
                ],
                &partial
            )
            .unwrap()
            .details,
            json!({"fields": {"reportError": "disk full", "files.failed": 2}})
        );
        // Commands without fields and raw output are never judged.
        assert_eq!(check(&[], &Output::json(json!({"truncated": true}))), None);
        assert_eq!(check(HASH, &Output::Bytes(b"x".to_vec())), None);
    }
}
