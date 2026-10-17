//! `schema`: the backend request a generated command sends, for agents.
//! Hand-written commands send no backend request and describe themselves in
//! `--help` instead.
use serde_json::{json, Value};

use super::{catalog::Catalog, CliError, Output};

fn schema(command: &super::catalog::Command) -> Value {
    let fields: Vec<Value> = command
        .params
        .iter()
        .filter(|param| param.visible() && !param.synthetic)
        .map(|param| {
            let mut field = json!({
                "name": param.name,
                "type": param.rust_type.clone().unwrap_or_else(|| "String".into()),
                "required": param.api_required,
                "description": param.description,
                "option": if param.position.is_some() {
                    format!("<{}>", param.option)
                } else if param.scope {
                    "--image".to_owned()
                } else {
                    format!("--{}", param.option)
                },
            });
            if let Some(values) = &param.values {
                field["enum"] = json!(values);
            }
            if let Some(default) = &param.default {
                field["default"] = default.clone();
            }
            if param.generated {
                field["generated"] = json!(true);
            }
            if param.secret {
                field["sensitive"] = json!(true);
            }
            field
        })
        .collect();
    let mut info = json!({
        "operationId": command.operation_id,
        "ipcCommand": command.ipc_command,
        "category": command.category,
        "hasRequestBody": !fields.is_empty(),
        "requestBodyFields": fields,
        "output": if command.raw_output { "bytes" } else { "json" },
    });
    if let Some(target) = &command.target {
        info["pathResolvesTo"] = json!(target);
    }
    if command.destructive {
        info["destructive"] = json!(true);
    }
    info
}

pub fn run(catalog: &Catalog, words: &[String], list: bool) -> Result<Output, CliError> {
    if list {
        let summary: Vec<Value> = catalog
            .commands
            .iter()
            .filter(|command| command.ipc_command.is_some())
            .map(|command| {
                json!({
                    "command": command.command,
                    "ipcCommand": command.ipc_command,
                    "operationId": command.operation_id,
                })
            })
            .collect();
        return Ok(Output::Json(json!(summary)));
    }
    if words.is_empty() {
        eprintln!(
            "用法：{0} schema <命令…>  或  {0} schema --list",
            catalog.binary
        );
        eprintln!("示例：{} schema images open", catalog.binary);
        return Err(CliError::exit(1));
    }
    let path = words.join(" ");
    match catalog.find_path(&path) {
        Some(command) if command.ipc_command.is_some() => Ok(Output::Json(schema(command))),
        found => {
            let matches: Vec<&str> = catalog
                .commands
                .iter()
                .filter(|command| command.ipc_command.is_some())
                .map(|command| command.full_path.join(" "))
                .filter(|candidate| candidate.contains(&path) || path.contains(candidate.as_str()))
                .map(|candidate| {
                    catalog
                        .find_path(&candidate)
                        .map_or("", |command| command.command.as_str())
                })
                .collect();
            if found.is_some() {
                eprintln!(
                    "\"{path}\" 是手写命令，没有后端请求结构；运行 {} {path} --help 查看参数。",
                    catalog.binary
                );
            } else if matches.is_empty() {
                eprintln!("找不到 \"{path}\" 的请求结构。使用 --list 查看全部。");
            } else {
                eprintln!("找不到 \"{path}\" 的请求结构。你是否要找：");
                for command in matches {
                    let rest = command.strip_prefix(&catalog.binary).unwrap_or(command);
                    eprintln!("  {} schema{rest}", catalog.binary);
                }
            }
            Err(CliError::exit(1))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::catalog::catalog;

    #[test]
    fn schema_describes_api_names_and_options() {
        let catalog = catalog().unwrap();
        let Output::Json(info) = run(catalog, &["images".into(), "open".into()], false).unwrap()
        else {
            panic!("schema is JSON");
        };
        assert_eq!(info["ipcCommand"], "image_load");
        assert_eq!(info["operationId"], "images.open");
        let fields = info["requestBodyFields"].as_array().unwrap();
        let path = fields.iter().find(|field| field["name"] == "path").unwrap();
        assert_eq!(
            (path["required"].clone(), path["option"].clone()),
            (json!(true), json!("<path>"))
        );
        let password = fields
            .iter()
            .find(|field| field["name"] == "qcowPassword")
            .unwrap();
        assert_eq!(password["sensitive"], true);
        assert!(fields.iter().all(|field| field["name"] != "caseDir"));
        assert!(run(catalog, &["files".into(), "list".into()], false).is_err());
    }
}
