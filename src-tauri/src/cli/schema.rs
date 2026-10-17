//! `schema`: the backend request a generated command sends, for agents.
//! Hand-written commands send no backend request and describe themselves in
//! `--help` instead.
use serde::Serialize;
use serde_json::Value;

use super::{
    catalog::{Catalog, Category, Command},
    context::Mode,
    json_output, CliError, Output,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Field<'a> {
    name: &'a str,
    #[serde(rename = "type")]
    rust_type: &'a str,
    required: bool,
    description: &'a str,
    option: String,
    #[serde(rename = "enum", skip_serializing_if = "Option::is_none")]
    values: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<&'a Value>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    generated: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    sensitive: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Schema<'a> {
    operation_id: &'a str,
    ipc_command: &'a str,
    category: Category,
    has_request_body: bool,
    request_body_fields: Vec<Field<'a>>,
    output: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    path_resolves_to: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    destructive: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Summary<'a> {
    command: &'a str,
    ipc_command: &'a str,
    operation_id: &'a str,
}

fn schema<'a>(command: &'a Command, ipc_command: &'a str) -> Schema<'a> {
    let fields: Vec<Field<'a>> = command
        .params
        .iter()
        .filter(|param| param.visible() && !param.synthetic)
        .map(|param| Field {
            name: &param.name,
            rust_type: param.rust_type.as_deref().unwrap_or("String"),
            required: param.api_required,
            description: &param.description,
            option: if param.position.is_some() {
                format!("<{}>", param.option)
            } else if param.scope {
                "--image".to_owned()
            } else {
                format!("--{}", param.option)
            },
            values: param.values.as_deref(),
            default: param.default.as_ref(),
            generated: param.generated,
            sensitive: param.secret,
        })
        .collect();
    Schema {
        operation_id: &command.operation_id,
        ipc_command,
        category: command.category,
        has_request_body: !fields.is_empty(),
        request_body_fields: fields,
        output: if command.raw_output { "bytes" } else { "json" },
        path_resolves_to: command.target.as_deref(),
        destructive: command.destructive,
    }
}

/// A failure that one-shot and shell users read as cf's plain stderr lines,
/// and sessions receive as an error response.
fn fail(mode: Mode, lines: &[String]) -> CliError {
    match mode {
        Mode::Session => CliError::invalid(lines.join("\n")),
        Mode::OneShot | Mode::Shell => {
            for line in lines {
                eprintln!("{line}");
            }
            CliError::exit(1)
        }
    }
}

pub fn run(
    catalog: &Catalog,
    words: &[String],
    list: bool,
    mode: Mode,
) -> Result<Output, CliError> {
    if list {
        let summary: Vec<Summary<'_>> = catalog
            .commands
            .iter()
            .filter_map(|command| {
                Some(Summary {
                    command: &command.command,
                    ipc_command: command.ipc_command.as_deref()?,
                    operation_id: &command.operation_id,
                })
            })
            .collect();
        return json_output(summary);
    }
    if words.is_empty() {
        return Err(fail(
            mode,
            &[
                format!(
                    "用法：{0} schema <命令…>  或  {0} schema --list",
                    catalog.binary
                ),
                format!("示例：{} schema images open", catalog.binary),
            ],
        ));
    }
    let path = words.join(" ");
    let found = catalog.find_path(&path);
    if let Some(command) = found {
        if let Some(ipc_command) = command.ipc_command.as_deref() {
            return json_output(schema(command, ipc_command));
        }
    }
    let matches: Vec<&str> = catalog
        .commands
        .iter()
        .filter(|command| command.ipc_command.is_some())
        .filter(|command| {
            let candidate = command.full_path.join(" ");
            candidate.contains(&path) || path.contains(candidate.as_str())
        })
        .map(|command| command.command.as_str())
        .collect();
    let lines = if found.is_some() {
        vec![format!(
            "\"{path}\" 是手写命令，没有后端请求结构；运行 {} {path} --help 查看参数。",
            catalog.binary
        )]
    } else if matches.is_empty() {
        vec![format!(
            "找不到 \"{path}\" 的请求结构。使用 --list 查看全部。"
        )]
    } else {
        let mut lines = vec![format!("找不到 \"{path}\" 的请求结构。你是否要找：")];
        for command in matches {
            let rest = command.strip_prefix(&catalog.binary).unwrap_or(command);
            lines.push(format!("  {} schema{rest}", catalog.binary));
        }
        lines
    };
    Err(fail(mode, &lines))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::catalog::catalog;

    #[test]
    fn schema_describes_api_names_and_options() {
        let catalog = catalog().unwrap();
        let Output::Json(info) = run(
            catalog,
            &["images".into(), "open".into()],
            false,
            Mode::Session,
        )
        .unwrap() else {
            panic!("schema is JSON");
        };
        assert!(info
            .text()
            .starts_with(r#"{"operationId":"images.open","ipcCommand":"image_load""#));
        let info = info.value();
        assert_eq!(info["ipcCommand"], "image_load");
        assert_eq!(info["operationId"], "images.open");
        let fields = info["requestBodyFields"].as_array().unwrap();
        let path = fields.iter().find(|field| field["name"] == "path").unwrap();
        assert_eq!(
            (path["required"].clone(), path["option"].clone()),
            (serde_json::json!(true), serde_json::json!("<path>"))
        );
        let password = fields
            .iter()
            .find(|field| field["name"] == "qcowPassword")
            .unwrap();
        assert_eq!(password["sensitive"], true);
        assert!(fields.iter().all(|field| field["name"] != "caseDir"));
        let error = run(
            catalog,
            &["files".into(), "list".into()],
            false,
            Mode::Session,
        )
        .unwrap_err();
        assert!(error.message.contains("手写命令"));
        assert!(run(catalog, &[], false, Mode::Session).is_err());
    }
}
