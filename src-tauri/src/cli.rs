//! Headless command surface. Dispatch and help are generated from the desktop
//! registry, while terminal navigation is provided by a stateful shell.
mod browsing;
pub mod catalog;
mod input;
mod runtime;
mod search;
mod session;
mod shell;
mod strings;
mod workflow;

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};
use tauri::ipc::{InvokeResponseBody, IpcResponse};

mod generated {
    include!(concat!(env!("OUT_DIR"), "/cli_dispatch.rs"));
}

pub use runtime::run;

#[derive(Debug, Serialize)]
pub struct CliError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl CliError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "invalidInput".into(),
            message: message.into(),
            details: None,
        }
    }
}
impl std::fmt::Display for CliError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}
impl std::error::Error for CliError {}
impl From<std::io::Error> for CliError {
    fn from(error: std::io::Error) -> Self {
        Self {
            code: "io".into(),
            message: error.to_string(),
            details: None,
        }
    }
}
impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::invalid(error.to_string())
    }
}

pub(crate) fn command_error(error: impl Serialize) -> CliError {
    match serde_json::to_value(error) {
        Ok(details) => CliError {
            code: details
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or("commandFailed")
                .into(),
            message: details
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| details.as_str())
                .unwrap_or("命令执行失败")
                .into(),
            details: Some(details),
        },
        Err(error) => CliError::invalid(error.to_string()),
    }
}

pub(crate) fn internal_error(error: impl std::fmt::Display) -> CliError {
    CliError {
        code: "operationFailed".into(),
        message: error.to_string(),
        details: None,
    }
}

#[derive(Debug)]
pub(crate) enum Output {
    Json(Value),
    Binary(Vec<u8>),
}
impl Output {
    fn json(self) -> Value {
        match self {
            Self::Json(value) => value,
            Self::Binary(bytes) => serde_json::json!({"encoding":"hex","data":hex::encode(bytes)}),
        }
    }
}

fn response(value: impl IpcResponse) -> Result<Output, CliError> {
    match value
        .body()
        .map_err(|error| CliError::invalid(error.to_string()))?
    {
        InvokeResponseBody::Json(value) => Ok(Output::Json(serde_json::from_str(&value)?)),
        InvokeResponseBody::Raw(bytes) => Ok(Output::Binary(bytes)),
    }
}

fn validate_keys(args: &Map<String, Value>, allowed: &[&str]) -> Result<(), CliError> {
    if let Some(key) = args.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(CliError::invalid(format!("未知参数: {key}")));
    }
    Ok(())
}
fn argument<T: DeserializeOwned>(
    args: &mut Map<String, Value>,
    key: &str,
    required: bool,
) -> Result<T, CliError> {
    let value = match args.remove(key) {
        Some(value) => value,
        None if required => return Err(CliError::invalid(format!("缺少参数: {key}"))),
        None => Value::Null,
    };
    serde_json::from_value(value).map_err(|error| CliError::invalid(format!("参数 {key}: {error}")))
}
