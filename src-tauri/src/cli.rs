//! The standalone command-line interface. It never starts the desktop
//! runtime: it builds `AppState` itself and calls the command functions
//! through a dispatch generated from the backend registry and the reviewed
//! schema in `cli/commands.toml` (see build/cli_catalog.rs).
//!
//! Commands are `<collection>… <verb>`, with JSON results on stdout, messages
//! on stderr, `cli search`, `docs search` and `schema` for discovery, and
//! `--dry-run`, `--body` and `--force` on every command.
mod argv;
mod browsing;
mod catalog;
mod complete;
mod context;
mod docs;
mod dotenv;
mod grep;
mod help;
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
mod tools;
mod wire;

use std::{borrow::Cow, sync::OnceLock};

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};

mod generated {
    include!(concat!(env!("OUT_DIR"), "/cli_dispatch.rs"));
}

pub use runtime::run;

/// Where a failure came from.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Origin {
    /// The command line, its values or the CLI itself.
    #[default]
    Cli,
    /// A backend command, named by its operation when known.
    Backend(Option<String>),
}

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
    #[serde(skip)]
    pub origin: Origin,
}

impl CliError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
            exit_code: 1,
            reported: false,
            origin: Origin::Cli,
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

    /// A prompt the user cancelled; the prompt already shows it.
    pub fn cancelled() -> Self {
        Self {
            exit_code: 130,
            reported: true,
            ..Self::new("cancelled", "已取消")
        }
    }

    /// The backend operation this failure came from.
    pub fn in_operation(self, operation: &str) -> Self {
        Self {
            origin: Origin::Backend(Some(operation.to_owned())),
            ..self
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
            origin: Origin::Backend(None),
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

/// Structured data. Its JSON text keeps object keys in the order the command
/// wrote them, as cf prints an API response; the value is parsed on demand.
#[derive(Debug)]
pub(crate) struct Json {
    text: Option<String>,
    value: OnceLock<Value>,
}

impl Json {
    pub fn serialize(data: &impl Serialize) -> Result<Self, CliError> {
        Ok(Self {
            text: Some(serde_json::to_string(&wire::Camel(data))?),
            value: OnceLock::new(),
        })
    }

    pub fn value(&self) -> &Value {
        self.value.get_or_init(|| match &self.text {
            Some(text) => serde_json::from_str(text).unwrap_or_else(|error| {
                tracing::warn!("command output is not JSON: {error}");
                Value::Null
            }),
            None => Value::Null,
        })
    }

    /// Compact JSON text.
    pub fn text(&self) -> Cow<'_, str> {
        match &self.text {
            Some(text) => Cow::Borrowed(text),
            None => Cow::Owned(self.value().to_string()),
        }
    }

    pub fn is_null(&self) -> bool {
        match &self.text {
            Some(text) => text == "null",
            None => self.value().is_null(),
        }
    }
}

impl From<Value> for Json {
    fn from(value: Value) -> Self {
        Self {
            text: None,
            value: OnceLock::from(value),
        }
    }
}

/// A command result.
#[derive(Debug)]
pub(crate) enum Output {
    /// Structured data; `null` means the command returned nothing.
    Json(Json),
    /// Raw content for stdout.
    Bytes(Vec<u8>),
    /// Raw content the command already wrote to stdout.
    Written,
}

impl Output {
    pub fn json(value: Value) -> Self {
        Self::Json(value.into())
    }

    /// The JSONL session form: raw bytes are hex encoded.
    fn into_json(self) -> Json {
        match self {
            Self::Json(json) => json,
            Self::Bytes(bytes) => {
                serde_json::json!({"encoding": "hex", "data": hex::encode(bytes)}).into()
            }
            Self::Written => Value::Null.into(),
        }
    }
}

pub(crate) fn json_output(value: impl Serialize) -> Result<Output, CliError> {
    Ok(Output::Json(Json::serialize(&value)?))
}

fn validate_keys(args: &Map<String, Value>, allowed: &[&str]) -> Result<(), CliError> {
    if let Some(key) = args.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(CliError::invalid(format!("未知参数：{key}")));
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
        None if required => return Err(CliError::invalid(format!("缺少参数：{key}"))),
        None => Value::Null,
    };
    serde_json::from_value(value).map_err(|error| CliError::invalid(format!("参数 {key}：{error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_output_keeps_field_order_and_parses_on_demand() {
        #[derive(Serialize)]
        struct Image {
            id: &'static str,
            path: &'static str,
            bytes: u64,
        }
        let json = Json::serialize(&Image {
            id: "x",
            path: "D:/disk.E01",
            bytes: 7,
        })
        .unwrap();
        assert_eq!(json.text(), r#"{"id":"x","path":"D:/disk.E01","bytes":7}"#);
        assert_eq!(json.value()["bytes"], 7);
        assert!(Json::serialize(&()).unwrap().is_null());
        assert!(Json::from(Value::Null).is_null());
        assert_eq!(
            Json::from(serde_json::json!({"b": 1, "a": 2})).text(),
            r#"{"a":2,"b":1}"#
        );
    }
}
