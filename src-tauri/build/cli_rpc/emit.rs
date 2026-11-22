//! Writes the generated `rpc` dispatch and the operation list `rpc --list`
//! and `schema rpc` print.
use std::collections::BTreeMap;

use quote::quote;
use serde_json::{json, Value};

use super::operation::{Injection, Input, Kind, Operation, Output};

/// Commands whose results use `serde(flatten)`; only their outer records are
/// converted to camelCase.
const FLATTENED: [&str; 6] = [
    "image_hash_compute",
    "image_export_raw",
    "fs_export",
    "fs_export_dir",
    "fs_digest",
    "snapshot_list",
];

/// One match arm per backend operation, calling the Tauri-free command.
pub fn dispatch(operations: &BTreeMap<String, Operation>) -> String {
    let mut arms = Vec::new();
    for (name, operation) in operations {
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
            Output::Json if FLATTENED.contains(&name.as_str()) => {
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
        /// Calls a backend command with JSON arguments keyed by IPC parameter name.
        pub(crate) async fn dispatch(
            host: &crate::host::AppHandle,
            state: &crate::state::AppState,
            operation: &str,
            mut args: serde_json::Map<String, serde_json::Value>,
        ) -> Result<super::Output, super::CliError> {
            match operation {
                #(#arms),*,
                _ => Err(super::CliError::new(
                    "notFound",
                    format!("没有名为 {operation} 的后端命令；用 rpc --list 查看"),
                )),
            }
        }
    }
    .to_string()
}

fn kind(kind: Kind) -> &'static str {
    match kind {
        Kind::String => "string",
        Kind::Number => "number",
        Kind::Boolean => "boolean",
        Kind::Array => "string[]",
        Kind::Json => "json",
    }
}

/// Every operation with its parameters, in name order.
pub fn operations(operations: &BTreeMap<String, Operation>) -> Value {
    Value::Array(
        operations
            .values()
            .map(|operation| {
                json!({
                    "name": operation.name,
                    "params": operation.params().map(|param| json!({
                        "name": param.name,
                        "type": kind(param.kind),
                        "rustType": param.rust_type,
                        "required": param.required,
                    })).collect::<Vec<_>>(),
                    "output": match operation.output {
                        Output::Json => "json",
                        Output::Response | Output::Bytes => "bytes",
                    },
                    "returnsOption": operation.returns_option,
                    "docs": operation.docs,
                })
            })
            .collect(),
    )
}
