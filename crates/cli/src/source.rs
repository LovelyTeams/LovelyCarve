//! Opt-in source envelopes captured from the resolved request, before execution.
use serde_json::{json, Map, Value};

use super::{
    browsing,
    catalog::Command,
    context::{Exec, Target},
    CliError, Output,
};

pub(super) async fn capture(
    exec: &Exec,
    command: &Command,
    values: &Map<String, Value>,
    target: Option<&Target>,
) -> Result<Value, CliError> {
    let mut source = json!({"operationId": command.operation_id});
    if let Some(case) = exec.state.active_case.lock().await.as_ref() {
        source["caseId"] = json!(case.profile().case_uid);
    }
    // These native commands resolve a possibly unopened reference themselves.
    // Their returned context is the authoritative selected identity.
    if matches!(
        command.native.as_deref(),
        Some("context-use" | "context-set" | "context-get")
    ) {
        return Ok(source);
    }
    let Some(image_id) = values.get("imageId").and_then(Value::as_str) else {
        return Ok(source);
    };
    let image = exec
        .state
        .images
        .lock()
        .await
        .get(image_id)
        .cloned()
        .ok_or_else(|| CliError::new("noImage", "来源检材已关闭"))?;
    source["imageId"] = json!(image_id);
    source["evidencePath"] = json!(image.info().path);
    source["indexOnly"] = json!(image.is_index_only());
    source["addressSpace"] = json!("image");
    if let Some(volume) = values.get("volumeKey") {
        source["indexVolumeKey"] = volume.clone();
    }
    for key in ["offset", "length", "maxBytes", "table"] {
        if let Some(value) = values.get(key) {
            source[key] = value.clone();
        }
    }
    if let Some(target) = target {
        source["addressSpace"] = json!("filesystem");
        source["nodeId"] = json!(target.node_id);
        let location = match &target.location {
            Some(location) => location.clone(),
            None if target.node_id.is_empty() => Vec::new(),
            None => match super::navigation::locate_node(exec, image_id, &target.node_id).await {
                Ok(location) => location,
                Err(error) => {
                    source["path"] = Value::Null;
                    source["pathResolutionError"] = json!(error.message);
                    return Ok(source);
                }
            },
        };
        source["path"] = json!(browsing::display_path(&location));
        // The nearest volume ancestor is an identity, not a database primary key.
        if let Some((_, volume)) = location
            .iter()
            .rev()
            .find(|(_, id)| id.starts_with("part:") || id.starts_with("lvm:") || id == "root")
        {
            source["volumeKey"] = json!(volume);
        } else if image.info().partitions.is_empty() {
            source["volumeKey"] = json!("root");
        }
    }
    Ok(source)
}

pub(super) fn completed(source: &mut Option<Value>, command: &Command, output: &Output) {
    let (Some(source), Output::Json(output)) = (source, output) else {
        return;
    };
    if command.ipc_command.as_deref() == Some("image_load") {
        for (from, to) in [
            ("id", "imageId"),
            ("path", "evidencePath"),
            ("indexOnly", "indexOnly"),
        ] {
            if let Some(value) = output.value().get(from) {
                source[to] = value.clone();
            }
        }
    }
    if matches!(
        command.native.as_deref(),
        Some("context-use" | "context-set" | "context-get")
    ) {
        if let Some(image) = output
            .value()
            .get("image")
            .filter(|image| image.is_object())
        {
            source["imageId"] = image["id"].clone();
            source["evidencePath"] = image["path"].clone();
            source["nodeId"] = output.value()["nodeId"].clone();
            source["path"] = output.value()["dir"].clone();
        }
    }
    if matches!(
        command.ipc_command.as_deref(),
        Some("case_open" | "case_create")
    ) {
        if let Some(id) = output.value().get("id") {
            source["caseId"] = id.clone();
        }
    }
    if command.ipc_command.as_deref() == Some("fs_sqlite_table_rows") {
        let columns = output.value()["columns"].as_array();
        let rows = output.value()["rows"].as_array();
        if let (Some(columns), Some(rows)) = (columns, rows) {
            let keys: Vec<_> = columns
                .iter()
                .enumerate()
                .filter(|(_, column)| column["pk"].as_u64().is_some_and(|position| position > 0))
                .collect();
            source["recordKeys"] = Value::Array(rows.iter().map(|row| {
                let primary_key: Map<String, Value> = keys.iter().filter_map(|(index, column)| {
                    let cell = row.get(*index)?;
                    matches!(cell["kind"].as_str(), Some("integer" | "real" | "text"))
                        .then(|| (column["name"].as_str().unwrap_or_default().to_owned(), cell["value"].clone()))
                }).collect();
                json!({"addressSpace":"sqlite", "primaryKey": if !keys.is_empty() && primary_key.len() == keys.len() { Some(primary_key) } else { None }})
            }).collect());
        }
    }
}

pub(super) fn wrap(output: Output, source: Option<Value>) -> Output {
    match source {
        Some(source) => {
            Output::json(json!({"source": source, "result": output.into_json().value()}))
        }
        None => output,
    }
}

pub(super) fn split(output: Output, enabled: bool) -> Result<(Output, Option<Value>), CliError> {
    if !enabled {
        return Ok((output, None));
    }
    let Output::Json(json) = output else {
        return Err(CliError::invalid("来源封装需要 JSON 结果"));
    };
    let value = json.value();
    let source = value
        .get("source")
        .cloned()
        .ok_or_else(|| CliError::invalid("常驻会话未返回来源；请使用当前版本重新打开会话"))?;
    let result = value
        .get("result")
        .cloned()
        .ok_or_else(|| CliError::invalid("来源封装缺少 result"))?;
    Ok((Output::json(result), Some(source)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn database_primary_keys_remain_in_their_own_address_space() {
        let command = super::super::catalog::catalog()
            .unwrap()
            .find_path("files sqlite rows list")
            .unwrap();
        let mut source = Some(json!({"imageId":"image", "nodeId":"ext4:2:123", "table":"tree"}));
        let output = Output::json(
            json!({"columns":[{"name":"inode","pk":1},{"name":"name","pk":0}],
            "rows":[[{"kind":"integer","value":1423},{"kind":"text","value":"pve-ssl.pem"}]]}),
        );
        completed(&mut source, command, &output);
        assert_eq!(
            source.unwrap(),
            json!({"imageId":"image", "nodeId":"ext4:2:123", "table":"tree",
            "recordKeys":[{"addressSpace":"sqlite", "primaryKey":{"inode":1423}}]})
        );
    }

    #[test]
    fn source_envelope_round_trips_without_changing_result_shape() {
        let value = json!({"items":[{"id":"node"}],"nextCursor":"cursor","truncated":true});
        let source = json!({"imageId":"evidence"});
        let (output, restored) = split(
            wrap(Output::json(value.clone()), Some(source.clone())),
            true,
        )
        .unwrap();
        assert_eq!(restored, Some(source));
        let Output::Json(output) = output else {
            panic!("JSON");
        };
        assert_eq!(output.value(), &value);
    }
}
