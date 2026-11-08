//! Results that finished but are not complete: a digest or export over
//! zero-filled source bytes, a search that could not read every file, an
//! export or search that stopped early. The catalogue names the fields that
//! say so (`incomplete` in `cli/commands.toml`). The result is still returned,
//! but the command says what is missing and ends with status 2, so it never
//! passes for a complete result.
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::{catalog::Command, Output};

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

/// The catalogue's verdict on a result before any field projection.
pub fn check(command: &Command, output: &Output) -> Option<Incomplete> {
    let Output::Json(json) = output else {
        return None;
    };
    if command.incomplete.is_empty() {
        return None;
    }
    let result = json.value();
    let fields: Map<String, Value> = command
        .incomplete
        .iter()
        .filter_map(|path| {
            let value = path
                .split('.')
                .try_fold(result, |value, key| value.get(key))?;
            set(value).then(|| (path.clone(), value.clone()))
        })
        .collect();
    if fields.is_empty() {
        return None;
    }
    let shown: Vec<String> = fields
        .iter()
        .map(|(path, value)| format!("{path} = {value}"))
        .collect();
    Some(Incomplete {
        code: "incompleteResult",
        message: format!(
            "结果不完整：{}。结果仍会返回，但不能当作完整结果使用。",
            shown.join("、")
        ),
        details: json!({ "fields": fields }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::catalog;

    #[test]
    fn set_fields_mark_a_result_incomplete_and_unset_ones_do_not() {
        let catalog = catalog().unwrap();
        let hash = catalog.find(&["images", "hash"]).unwrap();
        let clean = Output::json(
            json!({"md5": "x", "readFaultsDetected": false, "syntheticBytes": 0, "badRegions": []}),
        );
        assert_eq!(check(hash, &clean), None);
        let faulty = Output::json(
            json!({"md5": "x", "readFaultsDetected": true, "syntheticBytes": 4096, "badRegions": [{}]}),
        );
        let incomplete = check(hash, &faulty).unwrap();
        assert_eq!(
            incomplete,
            Incomplete {
                code: "incompleteResult",
                message: "结果不完整：readFaultsDetected = true、syntheticBytes = 4096。结果仍会返回，但不能当作完整结果使用。".into(),
                details: json!({"fields": {"readFaultsDetected": true, "syntheticBytes": 4096}}),
            }
        );
        // Nested fields, strings and counts.
        let export = catalog.find(&["carve", "scans", "export"]).unwrap();
        let partial = Output::json(json!({"failed": 0, "reportError": "disk full",
            "files": {"failed": 2, "readFaults": {"readFaultsDetected": false}}}));
        assert_eq!(
            check(export, &partial).unwrap().details,
            json!({"fields": {"reportError": "disk full", "files.failed": 2}})
        );
        let search = catalog.find(&["files", "search"]).unwrap();
        let unread = Output::json(
            json!({"summary": {"readErrors": 3, "truncated": false}, "hits": [], "total": 0, "truncated": false}),
        );
        assert!(check(search, &unread).is_some());
        // Commands without the annotation and raw output are never judged.
        let list = catalog.find(&["files", "list"]).unwrap();
        assert_eq!(check(list, &Output::json(json!({"truncated": true}))), None);
        assert_eq!(check(hash, &Output::Bytes(b"x".to_vec())), None);
    }
}
