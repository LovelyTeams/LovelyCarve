//! Command facade. Each command function the frontends call lives in a
//! per-domain submodule under `commands/`; this file owns only the
//! transport helpers the command files share (bounded node queries and CSV
//! cell escaping). Runtime access and cross-domain workflows live in `app`.
//!
//! The submodules below each correspond to a coherent slice of the command
//! surface. Adding a new command means adding it to the matching
//! submodule and registering it in the `command_registry!` list in `lib.rs`.

use std::collections::HashSet;

pub mod analysis;
pub mod app_error;
pub mod carving;
pub mod casework;
pub mod evidence;
pub mod extensions;
pub mod filesystem;
pub mod preview;
pub mod search;
pub mod system;
pub mod timeline;

/// Hard limit for point-lookups of persisted per-node derived data. These
/// commands exist to avoid whole-image maps, but an unbounded node-id list
/// would still create a huge IPC payload and SQL `IN (...)` clause.
pub(crate) const MAX_DERIVED_NODE_QUERY: usize = 5_000;

pub(crate) fn bounded_unique_node_ids(
    node_ids: Vec<String>,
) -> Result<Vec<String>, app_error::AppError> {
    if node_ids.len() > MAX_DERIVED_NODE_QUERY {
        return Err(app_error::AppError::resource_limit(format!(
            "单次最多查询 {MAX_DERIVED_NODE_QUERY} 个节点派生数据"
        ))
        .with_context_value(serde_json::json!({
            "limit": MAX_DERIVED_NODE_QUERY,
            "requested": node_ids.len(),
        })));
    }

    let mut seen = HashSet::with_capacity(node_ids.len());
    let mut unique = Vec::with_capacity(node_ids.len());
    for node_id in node_ids {
        if seen.insert(node_id.clone()) {
            unique.push(node_id);
        }
    }
    Ok(unique)
}

/// Escapes one cell of a CSV export. Evidence values (file names, paths,
/// registry and plugin values) are attacker-controlled, and Excel / WPS /
/// LibreOffice evaluate a cell starting with `=`, `+`, `-`, `@`, TAB or CR as
/// a formula (`=HYPERLINK(...)`, DDE `=cmd|' /C calc'!A0`), so such cells get
/// a leading `'` and open as text (OWASP CSV injection guidance). Plain
/// negative numbers such as `-12` or `-3.5` stay as they are: spreadsheets
/// read them as numbers, never as formulas, and the prefix would turn numeric
/// columns into text. Cells containing `,`, `"`, CR or LF are then quoted
/// with inner quotes doubled.
pub(crate) fn quote_csv_field(field: &str) -> String {
    let formula_like =
        field.starts_with(['=', '+', '-', '@', '\t', '\r']) && !is_plain_negative_number(field);
    let prefix = if formula_like { "'" } else { "" };
    if field.contains([',', '"', '\r', '\n']) {
        format!("\"{prefix}{}\"", field.replace('"', "\"\""))
    } else {
        format!("{prefix}{field}")
    }
}

/// `-` followed by ASCII digits with an optional `.digits` fraction.
fn is_plain_negative_number(field: &str) -> bool {
    let is_digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    field
        .strip_prefix('-')
        .is_some_and(|unsigned| match unsigned.split_once('.') {
            Some((integer, fraction)) => is_digits(integer) && is_digits(fraction),
            None => is_digits(unsigned),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_csv_field_quotes_only_when_needed() {
        assert_eq!(quote_csv_field("plain"), "plain");
        assert_eq!(quote_csv_field("a,b"), "\"a,b\"");
        assert_eq!(quote_csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(quote_csv_field("line\nbreak"), "\"line\nbreak\"");
    }

    #[test]
    fn quote_csv_field_neutralizes_formula_prefixes() {
        assert_eq!(quote_csv_field("=1+1"), "'=1+1");
        assert_eq!(quote_csv_field("+x"), "'+x");
        assert_eq!(quote_csv_field("-x"), "'-x");
        assert_eq!(quote_csv_field("@SUM(A1)"), "'@SUM(A1)");
        assert_eq!(quote_csv_field("\t=1+1"), "'\t=1+1");
        assert_eq!(quote_csv_field("\r=1+1"), "\"'\r=1+1\"");
        assert_eq!(quote_csv_field("=cmd|' /C calc'!A0"), "'=cmd|' /C calc'!A0");
        assert_eq!(quote_csv_field("=SUM(1,2)"), "\"'=SUM(1,2)\"");
        assert_eq!(
            quote_csv_field(r#"=HYPERLINK("http://x","y")"#),
            r#""'=HYPERLINK(""http://x"",""y"")""#
        );
    }

    #[test]
    fn quote_csv_field_keeps_plain_negative_numbers_numeric() {
        assert_eq!(quote_csv_field("-12"), "-12");
        assert_eq!(quote_csv_field("-3.5"), "-3.5");
        assert_eq!(quote_csv_field("-1+2"), "'-1+2");
        assert_eq!(quote_csv_field("-3."), "'-3.");
        assert_eq!(quote_csv_field("-.5"), "'-.5");
        assert_eq!(quote_csv_field("-"), "'-");
    }
}
