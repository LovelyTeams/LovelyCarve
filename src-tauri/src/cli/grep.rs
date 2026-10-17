//! `files search` delegates matching, cancellation and persistence to the
//! global search, scoped to one subtree, and collects the hits it stored.
use serde_json::{json, Map, Value};

use super::{context::Exec, CliError, Output};
use crate::commands::search::global_search::{global_search_hits_page, global_search_start};

pub async fn run(
    exec: &Exec,
    image_id: &str,
    node_id: &str,
    values: &Map<String, Value>,
) -> Result<Output, CliError> {
    let pattern = values
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::invalid("files search 需要关键词或正则"))?;
    let flag = |name: &str| values.get(name).and_then(Value::as_bool).unwrap_or(false);
    let regex = flag("regex");
    let codepages = values
        .get("codepage")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let options = serde_json::from_value(json!({
        "keys": if regex { Vec::<String>::new() } else { vec![pattern.to_owned()] },
        "regexes": if regex { vec![pattern.to_owned()] } else { Vec::<String>::new() },
        "codepages": codepages,
        "match_case": !flag("ignoreCase"),
        "search_deleted": !flag("excludeDeleted"),
        "scope": { "tree_nodes": if node_id.is_empty() { Vec::<String>::new() } else { vec![node_id.to_owned()] } },
    }))?;
    let search_id = values
        .get("searchId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("cli-files-search-{}", uuid::Uuid::new_v4()));
    let summary = global_search_start(
        exec.host.clone(),
        crate::host::State::new(&exec.state),
        image_id.to_owned(),
        search_id.clone(),
        options,
    )
    .await
    .map_err(super::command_error)?;
    let limit = values
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(10_000)
        .clamp(1, 100_000);
    let mut hits = Vec::new();
    let total = loop {
        let page = global_search_hits_page(
            crate::host::State::new(&exec.state),
            image_id.to_owned(),
            search_id.clone(),
            Some(hits.len() as u64),
            Some((limit - hits.len() as u64).min(2000)),
            None,
            None,
            None,
        )
        .await
        .map_err(super::command_error)?;
        let empty = page.items.is_empty();
        hits.extend(page.items);
        if empty || hits.len() as u64 >= page.total || hits.len() as u64 >= limit {
            break page.total;
        }
    };
    let truncated = summary.truncated || summary.status != "success" || (hits.len() as u64) < total;
    Ok(Output::Json(
        json!({"summary": summary, "hits": hits, "total": total, "truncated": truncated}),
    ))
}
