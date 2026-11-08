//! Request contracts shared by one-shot commands, --body and JSONL sessions.
use serde::Serialize;
use serde_json::{json, Map, Value};

use super::{
    catalog::{Catalog, Category, Command, Kind},
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
    #[serde(skip_serializing_if = "Option::is_none")]
    schema: Option<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Schema<'a> {
    operation_id: &'a str,
    ipc_command: Option<&'a str>,
    command: String,
    summary: &'a str,
    description: &'a str,
    usage: &'a str,
    examples: &'a [super::catalog::Example],
    request_fields: Vec<&'static str>,
    request_example: Value,
    tool_request_fields: Vec<&'static str>,
    tool_request_example: Value,
    response_envelope: Value,
    category: Category,
    has_request_body: bool,
    request_body_fields: Vec<Field<'a>>,
    output: &'static str,
    session_output: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    target_alternatives: Vec<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path_resolves_to: Option<&'a str>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    destructive: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Summary<'a> {
    command: &'a str,
    ipc_command: Option<&'a str>,
    operation_id: &'a str,
}

fn schema(command: &Command) -> Schema<'_> {
    let fields: Vec<Field<'_>> = command
        .params
        .iter()
        .filter(|param| param.visible())
        .map(|param| Field {
            name: &param.name,
            rust_type: param.rust_type.as_deref().unwrap_or(match param.kind {
                Kind::String | Kind::Enum => "String",
                Kind::Number => "u64",
                Kind::Boolean => "bool",
                Kind::Array => "Vec<Value>",
                Kind::Json => "Value",
            }),
            required: param.required && !param.synthetic,
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
            schema: super::schema_types::for_type(param.rust_type.as_deref()),
        })
        .collect();
    Schema {
        command: command.path(),
        summary: &command.summary,
        description: &command.description,
        usage: &command.usage,
        examples: &command.examples,
        request_fields: vec![
            "id",
            "command",
            "args",
            "fields",
            "timezone",
            "force",
            "dryRun",
            "withSource",
            "output",
        ],
        request_example: {
            let mut request = example(command);
            if let Some(request) = request.as_object_mut() {
                request.remove("session");
            }
            request["id"] = json!("<请求ID>");
            request
        },
        tool_request_fields: vec![
            "id",
            "command",
            "args",
            "session",
            "fields",
            "timezone",
            "force",
            "dryRun",
            "timeoutMs",
            "help",
        ],
        tool_request_example: example(command),
        response_envelope: json!({"withSource": {"source":"实际解析的检材、卷与节点来源", "result":"原命令结果"},
            "fields":"顶层字段，仅对 list/search 条目投影；不属于 args", "session":"DSH 工具的顶层会话字段；sessions 生命周期使用 args.name"}),
        operation_id: &command.operation_id,
        ipc_command: command.ipc_command.as_deref(),
        category: command.category,
        has_request_body: !fields.is_empty(),
        request_body_fields: fields,
        output: if command.raw_output { "bytes" } else { "json" },
        session_output: if command.raw_output {
            "{encoding: hex, data: string}"
        } else {
            "json"
        },
        target_alternatives: command
            .target
            .as_deref()
            .map(|target| vec!["path", target])
            .unwrap_or_default(),
        path_resolves_to: command.target.as_deref(),
        destructive: command.destructive,
    }
}

fn example(command: &Command) -> Value {
    let mut args = Map::new();
    for param in command.params.iter().filter(|param| {
        param.visible() && param.required && !param.scope && !param.synthetic && !param.generated
    }) {
        let value = match param.kind {
            Kind::String | Kind::Enum => json!(param
                .values
                .as_ref()
                .and_then(|values| values.first())
                .cloned()
                .unwrap_or_else(|| format!("<{}>", param.name))),
            Kind::Number => json!(1),
            Kind::Boolean => json!(false),
            Kind::Array => json!([]),
            Kind::Json => json!({}),
        };
        args.insert(param.name.clone(), value);
    }
    if command.target.is_some() {
        args.insert("path".into(), json!("/"));
    }
    if command.path() == "files list" {
        args.insert("pageSize".into(), json!(20));
    }
    if command.path() == "indexes files list" {
        args.insert(
            "filter".into(),
            json!({"nameGlob":"*.conf", "limit":20, "offset":0}),
        );
    }
    let mut request = json!({"command":command.path(), "args":args});
    if !command.path().starts_with("sessions ")
        && command.native.as_deref() != Some("schema")
        && command.path() != "system get"
    {
        request["session"] = json!("<会话名>");
    }
    if command.path() == "files list" {
        request["fields"] = json!("id,name,kind,size");
    }
    request
}

pub(super) fn usage(command: &Command, mut error: CliError) -> CliError {
    error.details = Some(json!({
        "allowedFields": {"args":command.params.iter().filter(|param| param.visible()).map(|param| &param.name).collect::<Vec<_>>(),
            "request":["id","command","args","fields","timezone","force","dryRun","withSource","output"]},
        "schemaRequest":{"command":format!("schema {}", command.path())},
        "exampleRequest":example(command),
    }));
    error
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
            .map(|command| Summary {
                command: &command.command,
                ipc_command: command.ipc_command.as_deref(),
                operation_id: &command.operation_id,
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
        return json_output(schema(command));
    }
    let matches: Vec<&str> = catalog
        .commands
        .iter()
        .filter(|command| {
            let candidate = command.full_path.join(" ");
            candidate.contains(&path) || path.contains(candidate.as_str())
        })
        .map(|command| command.command.as_str())
        .collect();
    let lines = if matches.is_empty() {
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
    use crate::catalog::catalog;

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
        let info = info.value();
        assert_eq!(info["command"], "images open");
        assert!(info["requestFields"]
            .as_array()
            .unwrap()
            .contains(&json!("withSource")));
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
        let Output::Json(list) = run(
            catalog,
            &["files".into(), "list".into()],
            false,
            Mode::Session,
        )
        .unwrap() else {
            panic!("schema is JSON");
        };
        assert_eq!(list.value()["ipcCommand"], "fs_children_page");
        for path in ["files get", "files read", "files tree list", "files hash"] {
            let words = path
                .split_whitespace()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let Output::Json(info) = run(catalog, &words, false, Mode::Session).unwrap() else {
                panic!("schema is JSON");
            };
            let fields = info.value()["requestBodyFields"].as_array().unwrap();
            assert!(fields.iter().any(|field| field["name"] == "path"));
            assert!(fields.iter().any(|field| field["name"] == "nodeId"));
            assert_eq!(
                info.value()["targetAlternatives"],
                serde_json::json!(["path", "nodeId"])
            );
        }
        assert!(run(catalog, &[], false, Mode::Session).is_err());
    }
}
