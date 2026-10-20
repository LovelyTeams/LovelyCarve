//! Writes the generated dispatch and the command catalogue: each command's
//! usage, arguments and options, plus what the runtime needs to bind
//! parameters.
use std::collections::BTreeSet;

use quote::quote;
use serde_json::{json, Map, Value};

use super::manifest::{Kind, Manifest};
use super::operation::{Injection, Input, Output};
use super::resolve::{Resolved, ResolvedParam};

pub const BINARY: &str = "lovelycarve-cli";

/// One match arm per backend operation, calling the Tauri-free command.
pub fn dispatch(resolved: &[Resolved<'_>]) -> String {
    let mut arms = Vec::new();
    let mut emitted = BTreeSet::new();
    for command in resolved {
        let Some(operation) = command.operation else {
            continue;
        };
        if !emitted.insert(operation.name.clone()) {
            continue;
        }
        let name = &operation.name;
        let mut keys = Vec::new();
        let mut bindings = Vec::new();
        let mut arguments = Vec::new();
        for input in &operation.inputs {
            match input {
                Input::Injected(Injection::App) => arguments.push(quote!(host.clone())),
                Input::Injected(Injection::State) => {
                    arguments.push(quote!(crate::host::State::new(state)))
                }
                Input::Injected(Injection::LeaseOwner) => {
                    arguments.push(quote!(crate::host::LeaseOwner::process("cli")))
                }
                Input::Param(param) => {
                    let ident = &param.ident;
                    let key = &param.name;
                    let required = param.required;
                    keys.push(key.clone());
                    bindings
                        .push(quote!(let #ident = super::argument(&mut args, #key, #required)?;));
                    arguments.push(quote!(#ident));
                }
            }
        }
        let path = &operation.path;
        let mut call = quote!(crate::#path(#(#arguments),*));
        if operation.is_async {
            call = quote!(#call.await);
        }
        if operation.returns_result {
            call = quote!(#call.map_err(|error| super::command_error(error).in_operation(#name))?);
        }
        let result = match operation.output {
            Output::Json
                if matches!(
                    name.as_str(),
                    "image_hash_compute"
                        | "image_export_raw"
                        | "fs_export"
                        | "fs_export_dir"
                        | "fs_digest"
                        | "snapshot_list"
                ) =>
            {
                quote!(super::wire::flattened_output(&#call))
            }
            Output::Json => quote!(super::json_output(#call)),
            Output::Response => quote!(Ok(super::Output::Bytes(#call.into_bytes()))),
            Output::Bytes => quote!(Ok(super::Output::Bytes(#call))),
        };
        arms.push(quote!(#name => {
            super::validate_keys(&args, &[#(#keys),*])?;
            #(#bindings)*
            #result
        }));
    }
    quote! {
        /// Calls a backend command with JSON arguments keyed by API parameter name.
        pub(crate) async fn dispatch(
            host: &crate::host::AppHandle,
            state: &crate::state::AppState,
            operation: &str,
            mut args: serde_json::Map<String, serde_json::Value>,
        ) -> Result<super::Output, super::CliError> {
            match operation {
                #(#arms),*,
                _ => Err(super::CliError::invalid(format!("unknown backend command: {operation}"))),
            }
        }
    }
    .to_string()
}

fn scalar(kind: Kind) -> &'static str {
    match kind {
        Kind::Number => "number",
        Kind::Boolean => "boolean",
        Kind::String | Kind::Enum | Kind::Array | Kind::Json => "string",
    }
}

fn visible(param: &ResolvedParam) -> bool {
    !param.hidden && param.fixed.is_none()
}

fn argument_meta(param: &ResolvedParam) -> Value {
    let mut meta = json!({
        "name": param.option,
        "position": param.position,
        "type": scalar(param.kind),
        "required": param.required,
        "description": param.description,
    });
    if let Some(values) = &param.values {
        meta["enum"] = json!(values);
    }
    meta
}

fn option_meta(param: &ResolvedParam) -> Value {
    let mut meta = json!({
        "name": param.option,
        "type": scalar(param.kind),
        "required": param.required,
        "description": param.description,
    });
    if let Some(values) = &param.values {
        meta["enum"] = json!(values);
    }
    if let Some(default) = &param.default {
        meta["default"] = default.clone();
    }
    if let Some(short) = param.short {
        meta["alias"] = json!(short.to_string());
    }
    meta
}

fn flag(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "type": "boolean",
        "required": false,
        "description": description,
        "default": false,
    })
}

fn command_meta(command: &Resolved<'_>) -> Value {
    let spec = command.spec;
    let words = spec.words();
    let mut positional: Vec<&ResolvedParam> = command
        .params
        .iter()
        .filter(|param| param.position.is_some())
        .collect();
    positional.sort_by_key(|param| param.position);
    let arguments: Vec<Value> = positional
        .iter()
        .map(|param| argument_meta(param))
        .collect();
    let mut options: Vec<Value> = command
        .params
        .iter()
        .filter(|param| visible(param) && param.position.is_none() && !param.scope)
        .map(option_meta)
        .collect();
    let has_options = !options.is_empty();
    let raw_output = spec.raw_output
        || command
            .operation
            .is_some_and(|operation| operation.output != Output::Json);
    if raw_output {
        options.push(flag("text", "按 UTF-8 解码原始字节后输出"));
    }
    if command.destructive {
        let mut force = flag("force", "跳过确认（用于脚本和 CI）");
        force["alias"] = json!("f");
        options.push(force);
    }
    if command.params.iter().any(visible) {
        options.push(json!({
            "name": "body",
            "type": "string",
            "required": false,
            "description": "原始 JSON 参数对象（键为 API 参数名，可写 @文件路径）",
        }));
    }
    options.push(json!({"name":"fields", "type":"string", "required":false, "description":"列表字段投影，逗号分隔，支持点路径"}));
    options.push(flag("dry-run", "解析参数并显示将执行的请求，不实际执行"));
    let mut usage = vec![BINARY.to_owned(), spec.path.clone()];
    for param in &positional {
        let required = param.required;
        usage.push(if required {
            format!("<{}>", param.option)
        } else {
            format!("[{}]", param.option)
        });
    }
    if has_options {
        usage.push("[options]".into());
    }
    let mut meta = Map::new();
    meta.insert("command".into(), json!(format!("{BINARY} {}", spec.path)));
    meta.insert("name".into(), json!(words.last()));
    meta.insert("fullPath".into(), json!(words));
    meta.insert(
        "description".into(),
        json!(spec
            .description
            .clone()
            .unwrap_or_else(|| spec.summary.clone())),
    );
    meta.insert("summary".into(), json!(spec.summary));
    meta.insert("usage".into(), json!(usage.join(" ")));
    meta.insert("arguments".into(), json!(arguments));
    meta.insert("options".into(), json!(options));
    meta.insert("category".into(), json!(spec.category));
    meta.insert("operationId".into(), json!(words.join(".")));
    meta.insert("hideCommand".into(), json!(spec.hide_command));
    if !spec.examples.is_empty() {
        meta.insert("examples".into(), json!(spec.examples));
    }
    if let Some(operation) = command.operation {
        meta.insert("ipcCommand".into(), json!(operation.name));
        if !operation.docs.is_empty() {
            meta.insert("notes".into(), json!(operation.docs));
        }
    }
    if let Some(native) = &spec.native {
        meta.insert("native".into(), json!(native));
    }
    meta.insert("concurrent".into(), json!(spec.concurrent));
    meta.insert("destructive".into(), json!(command.destructive));
    if let Some(confirm) = &spec.confirm {
        meta.insert("confirm".into(), json!(confirm));
    }
    meta.insert("rawOutput".into(), json!(raw_output));
    if let Some(target) = &spec.target {
        meta.insert("target".into(), json!(target));
    }
    if !spec.keywords.is_empty() {
        meta.insert("keywords".into(), json!(spec.keywords));
    }
    if !spec.no_clobber.is_empty() {
        meta.insert("noClobber".into(), json!(spec.no_clobber));
    }
    let mut sensitive: Vec<Vec<String>> = command
        .params
        .iter()
        .filter(|param| param.secret)
        .map(|param| vec![param.name.clone()])
        .collect();
    sensitive.extend(spec.sensitive_paths.iter().cloned());
    if !sensitive.is_empty() {
        meta.insert("sensitivePaths".into(), json!(sensitive));
    }
    meta.insert("params".into(), json!(command.params));
    Value::Object(meta)
}

/// The catalogue the runtime embeds: every command, sorted by path, and the
/// description of every command group.
pub fn catalog(manifest: &Manifest, resolved: &[Resolved<'_>], version: &str) -> Value {
    let mut commands: Vec<&Resolved<'_>> = resolved.iter().collect();
    commands.sort_by(|left, right| left.spec.path.cmp(&right.spec.path));
    let superseded: Vec<Value> = manifest
        .superseded
        .iter()
        .map(|entry| json!({ "ipcCommand": entry.ipc, "by": format!("{BINARY} {}", entry.by) }))
        .collect();
    json!({
        "version": version,
        "binary": BINARY,
        "generatedAt": "build-time",
        "descriptions": manifest.groups,
        "commands": commands.iter().map(|command| command_meta(command)).collect::<Vec<_>>(),
        "superseded": superseded,
    })
}
