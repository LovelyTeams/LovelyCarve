//! The reviewed CLI schema in `src/cli/commands.toml`: the annotations that
//! turn backend operations into commands.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// API parameter name → option name, for every command.
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    /// API parameter name → description, for every command.
    #[serde(default)]
    pub descriptions: BTreeMap<String, String>,
    /// Command group path → description.
    #[serde(default)]
    pub groups: BTreeMap<String, String>,
    #[serde(default, rename = "command")]
    pub commands: Vec<CommandSpec>,
    #[serde(default)]
    pub superseded: Vec<Superseded>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Read,
    Create,
    Update,
    Delete,
    Action,
}

/// The kind of value a parameter takes on the command line.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    String,
    Number,
    Boolean,
    /// A string from `values`.
    Enum,
    /// Repeated scalar values, optionally limited to `values`.
    Array,
    /// Any JSON value: objects, maps and arrays of objects, given inline or as `@file`.
    Json,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSpec {
    pub path: String,
    /// The backend operation, as registered in `generate_handler!`.
    pub ipc: Option<String>,
    /// A hand-written command implemented in `src/cli/native`.
    pub native: Option<String>,
    pub summary: String,
    pub description: Option<String>,
    pub category: Category,
    /// Positional parameters, in order.
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub secret: Vec<String>,
    /// Parameters the CLI never sets.
    #[serde(default)]
    pub hidden: Vec<String>,
    /// Parameters filled with a fresh ID when omitted.
    #[serde(default)]
    pub generated: Vec<String>,
    #[serde(default)]
    pub defaults: BTreeMap<String, Value>,
    /// Parameters with a value fixed by this command.
    #[serde(default)]
    pub fixed: BTreeMap<String, Value>,
    /// The node parameter a `path` inside the image resolves to.
    pub target: Option<String>,
    /// The question asked before the command runs, after the action it describes.
    pub confirm: Option<String>,
    #[serde(default)]
    pub concurrent: bool,
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Output paths that must not exist yet.
    #[serde(default)]
    pub no_clobber: Vec<String>,
    /// Nested argument paths redacted from `--dry-run`.
    #[serde(default)]
    pub sensitive_paths: Vec<Vec<String>>,
    /// The command writes raw bytes to stdout (native commands only).
    #[serde(default)]
    pub raw_output: bool,
    #[serde(default)]
    pub params: BTreeMap<String, ParamSpec>,
}

impl CommandSpec {
    pub fn words(&self) -> Vec<&str> {
        self.path.split(' ').collect()
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParamSpec {
    #[serde(rename = "type")]
    pub kind: Option<Kind>,
    pub values: Option<Vec<String>>,
    pub default: Option<Value>,
    pub required: Option<bool>,
    pub description: Option<String>,
    /// A one-letter alias of the option, such as `-j` for `--jobs`.
    pub short: Option<char>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Superseded {
    pub ipc: String,
    /// The command that covers this operation.
    pub by: String,
}

pub fn parse(text: &str) -> Result<Manifest, String> {
    toml::from_str(text).map_err(|error| format!("src/cli/commands.toml: {error}"))
}
