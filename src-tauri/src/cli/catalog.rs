//! The command catalogue generated at build time from `cli/commands.toml` and
//! the backend signatures: the single source for parsing, help, search,
//! `schema`, completion and the JSONL session.
use std::{collections::BTreeMap, sync::OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::CliError;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    String,
    Number,
    Boolean,
    Enum,
    Array,
    Json,
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

impl Category {
    /// The spinner label while running, and the success marker afterwards.
    pub fn labels(self) -> (&'static str, &'static str) {
        match self {
            Self::Read => ("读取中", "已读取"),
            Self::Create => ("创建中", "已创建"),
            Self::Update => ("更新中", "已更新"),
            Self::Delete => ("删除中", "已删除"),
            Self::Action => ("处理中", "完成"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Param {
    /// The API name: a `--body`, `schema` and session key.
    pub name: String,
    /// The option (or positional) name on the command line.
    pub option: String,
    pub kind: Kind,
    #[serde(default)]
    pub values: Option<Vec<String>>,
    pub required: bool,
    pub api_required: bool,
    #[serde(default)]
    pub position: Option<usize>,
    pub secret: bool,
    pub generated: bool,
    pub hidden: bool,
    #[serde(default)]
    pub fixed: Option<Value>,
    #[serde(default)]
    pub default: Option<Value>,
    /// Supplied by the global `--image` option when absent.
    pub scope: bool,
    /// A path inside the image resolved to the command's target node.
    pub synthetic: bool,
    pub description: String,
    #[serde(default)]
    pub rust_type: Option<String>,
    /// A one-letter alias of the option.
    #[serde(default)]
    pub short: Option<char>,
}

impl Param {
    /// Whether the command line exposes this parameter.
    pub fn visible(&self) -> bool {
        !self.hidden && self.fixed.is_none()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Command {
    pub command: String,
    pub name: String,
    pub full_path: Vec<String>,
    pub description: String,
    pub summary: String,
    pub usage: String,
    pub arguments: Vec<Value>,
    pub options: Vec<Value>,
    pub category: Category,
    pub operation_id: String,
    #[serde(default)]
    pub ipc_command: Option<String>,
    #[serde(default)]
    pub native: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    pub concurrent: bool,
    pub destructive: bool,
    #[serde(default)]
    pub confirm: Option<String>,
    pub raw_output: bool,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub no_clobber: Vec<String>,
    #[serde(default)]
    pub sensitive_paths: Vec<Vec<String>>,
    /// Runnable and searchable, but not listed in help, completion or
    /// command suggestions.
    #[serde(default)]
    pub hide_command: bool,
    #[serde(default)]
    pub examples: Vec<Example>,
    pub params: Vec<Param>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Example {
    pub command: String,
    pub description: String,
}

impl Command {
    pub fn path(&self) -> String {
        self.full_path.join(" ")
    }

    pub fn param(&self, name: &str) -> Option<&Param> {
        self.params.iter().find(|param| param.name == name)
    }

    /// Whether `--body` can supply this command's parameters.
    pub fn takes_body(&self) -> bool {
        self.ipc_command.is_some() && self.params.iter().any(Param::visible)
    }

    pub fn positionals(&self) -> Vec<&Param> {
        let mut positionals: Vec<&Param> = self
            .params
            .iter()
            .filter(|param| param.position.is_some())
            .collect();
        positionals.sort_by_key(|param| param.position);
        positionals
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Superseded {
    pub ipc_command: String,
    pub by: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub version: String,
    pub binary: String,
    pub descriptions: BTreeMap<String, String>,
    pub commands: Vec<Command>,
    pub superseded: Vec<Superseded>,
}

/// Shell words of the previous CLI and the commands that replaced them.
const LEGACY_COMMANDS: &[(&str, &str)] = &[
    ("open", "images open"),
    ("info", "images get"),
    ("use", "context set"),
    ("select-partition", "context set"),
    ("list-partitions", "images get"),
    ("list-devices", "disks list"),
    ("cd", "context set"),
    ("pwd", "context get"),
    ("ls", "files list"),
    ("tree", "files list"),
    ("find", "files list"),
    ("stat", "files get"),
    ("cat", "files read"),
    ("hex", "files read"),
    ("strings", "files strings list"),
    ("file", "files identify"),
    ("hash", "files hash"),
    ("md5sum", "files hash"),
    ("sha1sum", "files hash"),
    ("sha256sum", "files hash"),
    ("export", "files export"),
    ("grep", "files search"),
    ("tasks", "tasks list"),
    ("wait", "tasks wait"),
    ("commands", "cli search"),
    ("capabilities", "system get"),
    ("call", "schema"),
    ("run", "shell"),
];

impl Catalog {
    /// The command that replaced an old command name: a backend IPC name
    /// (with `-` or `_`) or a shell word of the previous CLI.
    pub fn replacement(&self, name: &str) -> Option<String> {
        let ipc = name.replace('-', "_");
        let by_path = |path: &str| self.find_path(path).map(|command| command.command.clone());
        self.commands
            .iter()
            .find(|command| command.ipc_command.as_deref() == Some(ipc.as_str()))
            .map(|command| command.command.clone())
            .or_else(|| {
                self.superseded
                    .iter()
                    .find(|entry| entry.ipc_command == ipc)
                    .map(|entry| entry.by.clone())
            })
            .or_else(|| {
                LEGACY_COMMANDS
                    .iter()
                    .find(|(legacy, _)| *legacy == name)
                    .and_then(|(_, path)| by_path(path))
            })
    }

    pub fn find(&self, words: &[&str]) -> Option<&Command> {
        self.commands.iter().find(|command| {
            command
                .full_path
                .iter()
                .map(String::as_str)
                .eq(words.iter().copied())
        })
    }

    pub fn find_path(&self, path: &str) -> Option<&Command> {
        let words: Vec<&str> = path.split_whitespace().collect();
        self.find(&words)
    }

    pub fn description(&self, path: &str) -> Option<&str> {
        self.descriptions.get(path).map(String::as_str)
    }

    /// The listed commands and groups directly below `prefix`, sorted by name.
    pub fn children(&self, prefix: &[&str]) -> Vec<(String, String)> {
        let mut children = BTreeMap::new();
        for command in self.commands.iter().filter(|command| !command.hide_command) {
            let path = &command.full_path;
            if path.len() <= prefix.len() || !path.iter().zip(prefix).all(|(a, b)| a == b) {
                continue;
            }
            let name = path[prefix.len()].clone();
            if path.len() == prefix.len() + 1 {
                children.insert(name, command.summary.clone());
            } else {
                let group = path[..=prefix.len()].join(" ");
                let description = self.description(&group).unwrap_or_default().to_owned();
                children.entry(name).or_insert(description);
            }
        }
        children.into_iter().collect()
    }

    pub fn is_group(&self, words: &[&str]) -> bool {
        !words.is_empty() && self.descriptions.contains_key(&words.join(" "))
    }

    /// Whether `words` name a hidden command, or a group with only hidden commands.
    pub fn is_hidden(&self, words: &[&str]) -> bool {
        let below: Vec<&Command> = self
            .commands
            .iter()
            .filter(|command| {
                command.full_path.len() >= words.len()
                    && command
                        .full_path
                        .iter()
                        .zip(words)
                        .all(|(word, given)| word == given)
            })
            .collect();
        !below.is_empty() && below.iter().all(|command| command.hide_command)
    }
}

static CATALOG: OnceLock<Result<Catalog, String>> = OnceLock::new();

pub fn catalog() -> Result<&'static Catalog, CliError> {
    CATALOG
        .get_or_init(|| {
            serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/cli_catalog.json")))
                .map_err(|error| format!("命令目录损坏：{error}"))
        })
        .as_ref()
        .map_err(|error| CliError::new("operationFailed", error.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registered() -> Vec<String> {
        let source = include_str!("../lib.rs");
        let registry = source
            .split("tauri::generate_handler![")
            .nth(1)
            .unwrap()
            .split("])")
            .next()
            .unwrap();
        registry
            .lines()
            .filter_map(|line| {
                line.trim().strip_prefix("commands::").map(|path| {
                    path.trim_end_matches(',')
                        .rsplit("::")
                        .next()
                        .unwrap()
                        .to_owned()
                })
            })
            .collect()
    }

    #[test]
    fn every_backend_command_is_a_cli_command_or_superseded() {
        let catalog = catalog().unwrap();
        let mut covered: Vec<String> = catalog
            .commands
            .iter()
            .filter_map(|command| command.ipc_command.clone())
            .chain(
                catalog
                    .superseded
                    .iter()
                    .map(|entry| entry.ipc_command.clone()),
            )
            .collect();
        covered.sort();
        covered.dedup();
        let mut registered = registered();
        registered.sort();
        assert_eq!(covered, registered);
    }

    #[test]
    fn commands_follow_the_collection_verb_grammar() {
        let catalog = catalog().unwrap();
        for command in &catalog.commands {
            assert_eq!(
                command.command,
                format!("lovelycarve-cli {}", command.path())
            );
            assert_eq!(command.operation_id, command.full_path.join("."));
            if command.name == "list" && command.ipc_command.is_some() {
                assert!(command.arguments.is_empty(), "{}", command.command);
            }
            if command.name == "delete" {
                assert!(command.destructive, "{}", command.command);
            }
        }
        let open = catalog.find(&["images", "open"]).unwrap();
        assert_eq!(open.ipc_command.as_deref(), Some("image_load"));
        assert_eq!(open.usage, "lovelycarve-cli images open <path> [options]");
        let image = open.param("caseDir").unwrap();
        assert!(image.hidden);
        let get = catalog.find(&["files", "get"]).unwrap();
        assert_eq!(get.target.as_deref(), Some("nodeId"));
        assert_eq!(catalog.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn old_names_point_to_their_replacements() {
        let catalog = catalog().unwrap();
        for (old, new) in [
            ("image_load", "images open"),
            ("image-load", "images open"),
            ("fs_read_range", "files read"),
            ("ls", "files list"),
            ("wait", "tasks wait"),
        ] {
            assert_eq!(
                catalog.replacement(old),
                Some(format!("lovelycarve-cli {new}")),
                "{old}"
            );
        }
        for (_, path) in LEGACY_COMMANDS {
            assert!(catalog.find_path(path).is_some(), "{path}");
        }
        assert_eq!(catalog.replacement("images"), None);
        let path = catalog.find(&["files", "list"]).unwrap().path();
        assert_eq!(path, "files list");
    }
}
