//! The standalone command-line interface. It never starts the desktop
//! runtime: it builds `AppState` itself and calls the command functions
//! through a dispatch generated from the backend registry and the reviewed
//! schema in `cli/commands.toml` (see build/cli_catalog.rs).
//!
//! Commands are `<collection>… <verb>`, with JSON results on stdout, messages
//! on stderr, `cli search` and `schema` for discovery, and `--dry-run`,
//! `--body` and `--force` on every command.
mod argv;
mod browsing;
mod catalog;
mod complete;
mod context;
mod dotenv;
mod grep;
mod interactive;
mod native;
mod output;
mod progress;
mod prompt;
mod runtime;
mod schema;
mod search;
mod session;
mod shell;
mod strings;

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};

mod generated {
    include!(concat!(env!("OUT_DIR"), "/cli_dispatch.rs"));
}

pub use runtime::run;

/// A failure reported to the user, with the process status it ends with.
#[derive(Debug, Serialize)]
pub struct CliError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(skip)]
    pub exit_code: i32,
    /// Already reported (for example `Aborted.`): nothing more to render.
    #[serde(skip)]
    pub reported: bool,
}

impl CliError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
            exit_code: 1,
            reported: false,
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalidInput", message)
    }

    /// End the process with `code` without rendering anything further.
    pub fn exit(code: i32) -> Self {
        Self {
            exit_code: code,
            reported: true,
            ..Self::new("exit", "")
        }
    }

    pub fn cancelled() -> Self {
        Self {
            exit_code: 130,
            ..Self::new("cancelled", "已取消")
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
        Self::new("io", error.to_string())
    }
}

impl From<serde_json::Error> for CliError {
    fn from(error: serde_json::Error) -> Self {
        Self::invalid(error.to_string())
    }
}

/// A backend error, keeping its stable code, message and details.
pub(crate) fn command_error(error: impl Serialize) -> CliError {
    match serde_json::to_value(error) {
        Ok(details) => CliError {
            details: (details.as_object().is_some_and(|object| object.len() > 2))
                .then(|| details.clone()),
            ..CliError::new(
                details
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or("commandFailed"),
                details
                    .get("message")
                    .and_then(Value::as_str)
                    .or_else(|| details.as_str())
                    .unwrap_or("命令执行失败"),
            )
        },
        Err(error) => CliError::invalid(error.to_string()),
    }
}

pub(crate) fn internal_error(error: impl std::fmt::Display) -> CliError {
    CliError::new("operationFailed", error.to_string())
}

/// A command result.
#[derive(Debug)]
pub(crate) enum Output {
    /// Structured data; `null` means the command returned nothing.
    Json(Value),
    /// Raw content for stdout.
    Bytes(Vec<u8>),
    /// Raw content the command already wrote to stdout.
    Written,
}

impl Output {
    /// The JSONL session form: raw bytes are hex encoded.
    fn into_json(self) -> Value {
        match self {
            Self::Json(value) => value,
            Self::Bytes(bytes) => {
                serde_json::json!({"encoding": "hex", "data": hex::encode(bytes)})
            }
            Self::Written => Value::Null,
        }
    }
}

pub(crate) fn json_output(value: impl Serialize) -> Result<Output, CliError> {
    Ok(Output::Json(serde_json::to_value(value)?))
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
