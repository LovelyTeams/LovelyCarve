//! `plugin.toml` of an LPP v2 package: identity, the library, resource
//! limits, permissions, analyzers and the frozen filesystem capability.
//!
//! The manifest says what the host may schedule: scope (`system`,
//! `evidence` or `files`), trigger, platforms, declared inputs, options and
//! the facts an analyzer provides or requires. What the library implements
//! (analyzer ids, record schemas) comes from its own `plugin.hello` and is
//! checked against the manifest at every handshake.

use std::{
    collections::HashSet,
    env::consts::{DLL_PREFIX, DLL_SUFFIX},
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
};

use lovelycarve_plugin::{glob::Glob, schema::validate_identifier, wire::PROTOCOL_VERSION};
use serde::{Deserialize, Serialize};

use super::{PluginError, Result};

/// OS families a system-scope analyzer may name (`SystemInfo::family`).
pub const SYSTEM_FAMILIES: [&str; 7] = [
    "linux", "windows", "macos", "esxi", "mikrotik", "openwrt", "dingtalk",
];

/// Smallest and largest memory limit a package may declare.
const MIN_MEMORY: u64 = 64 * 1024 * 1024;
const MAX_MEMORY: u64 = 64 * 1024 * 1024 * 1024;
/// Memory limit of a package that declares none.
pub const DEFAULT_MEMORY: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    /// LPP protocol version; must be 2.
    pub protocol: u16,
    /// Library path relative to the package directory. A name without a
    /// library extension (`linuxinfo`) resolves to the platform file name:
    /// `linuxinfo.dll`, `liblinuxinfo.so` or `liblinuxinfo.dylib`.
    pub library: String,
    /// Memory limit of the plugin process (`512MiB`, `1GiB`).
    #[serde(default)]
    pub memory: Option<String>,
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,
    #[serde(default, rename = "analyzer")]
    pub analyzers: Vec<AnalyzerManifest>,
    #[serde(default, rename = "filesystem")]
    pub filesystems: Vec<FilesystemManifest>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginPermission {
    /// `file.materialize`: host-owned temporary copies of input files.
    FileMaterialize,
    /// The plugin process may use the network (`LOVELYCARVE_NETWORK_PERMISSION`).
    Network,
}

/// How tasks of an analyzer are created: `auto` when evidence comes online
/// or the facts it requires are published, `manual` only by the user.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTrigger {
    Auto,
    #[default]
    #[serde(alias = "manual")]
    Manually,
}

/// What one run of an analyzer looks at.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnalyzerScope {
    /// One detected operating system, its volumes mounted in one tree.
    System,
    /// Every volume of an evidence item.
    Evidence,
    /// The files the user selected.
    Files,
}

impl AnalyzerScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Evidence => "evidence",
            Self::Files => "files",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalyzerManifest {
    pub id: String,
    /// Default (Chinese) display name.
    pub name: String,
    pub scope: AnalyzerScope,
    /// System scope: OS families the analyzer applies to; empty = all.
    #[serde(default)]
    pub platforms: Vec<String>,
    #[serde(default)]
    pub trigger: TaskTrigger,
    /// Result tree category (`baseinfo`, `vehicle`, …).
    #[serde(default = "AnalyzerManifest::default_category")]
    pub category: String,
    /// Profile facts a successful run publishes.
    #[serde(default)]
    pub provides: Vec<String>,
    /// Profile facts an automatic run waits for.
    #[serde(default)]
    pub requires: Vec<String>,
    /// System / evidence scope: view path patterns the host resolves into
    /// the run's inputs (evidence scope: under every volume).
    #[serde(default)]
    pub inputs: Vec<String>,
    /// Files scope: file name extensions the analyzer accepts.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Files scope: MIME types the analyzer accepts.
    #[serde(default)]
    pub mime_types: Vec<String>,
    /// Hidden from the default task list.
    #[serde(default)]
    pub internal: bool,
    /// At most one unfinished task per target.
    #[serde(default = "AnalyzerManifest::default_single_instance")]
    pub single_instance: bool,
    #[serde(default, rename = "option")]
    pub options: Vec<OptionManifest>,
}

impl AnalyzerManifest {
    fn default_category() -> String {
        "analysis".to_owned()
    }

    fn default_single_instance() -> bool {
        true
    }

    /// Whether this analyzer applies to an OS family.
    pub fn applies_to_family(&self, family: &str) -> bool {
        self.platforms.is_empty()
            || self
                .platforms
                .iter()
                .any(|platform| platform.eq_ignore_ascii_case(family))
    }

    /// Whether a files-scope analyzer accepts a file by name or MIME type.
    pub fn accepts_file(&self, name: &str, mime: Option<&str>) -> bool {
        if self.extensions.is_empty() && self.mime_types.is_empty() {
            return true;
        }
        let extension = Path::new(name)
            .extension()
            .and_then(OsStr::to_str)
            .map(str::to_ascii_lowercase);
        extension.is_some_and(|extension| self.extensions.contains(&extension))
            || mime.is_some_and(|mime| {
                self.mime_types
                    .iter()
                    .any(|accepted| accepted.eq_ignore_ascii_case(mime))
            })
    }
}

/// An option shown before a run and passed in `run.start`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OptionManifest {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: OptionKind,
    pub label: String,
    #[serde(default)]
    pub choices: Vec<OptionChoice>,
    #[serde(default)]
    pub default: Option<toml::Value>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OptionKind {
    /// Any subset of the choices (empty = all).
    Multi,
    /// One of the choices.
    Choice,
    Flag,
    Text,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OptionChoice {
    pub id: String,
    pub label: String,
}

/// A filesystem the library parses (frozen `fs.*` methods).
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FilesystemManifest {
    pub id: String,
    #[serde(default)]
    pub priority: i32,
}

impl PluginManifest {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        Self::parse(&text).map_err(|error| match error {
            PluginError::Manifest(message) => {
                PluginError::Manifest(format!("{}: {message}", path.display()))
            }
            error => error,
        })
    }

    pub fn parse(text: &str) -> Result<Self> {
        let manifest: Self = toml::from_str(text).map_err(|error| {
            let legacy = toml::from_str::<toml::Table>(text).is_ok_and(|table| {
                ["protocol_min", "capabilities", "task", "process"]
                    .iter()
                    .any(|key| table.contains_key(*key))
                    || table.get("library").is_some_and(toml::Value::is_table)
            });
            PluginError::Manifest(if legacy {
                "this is an LPP v1 manifest; LovelyCarve loads LPP v2 packages only \
                 (rebuild the plugin with SDK 0.2, see docs/PLUGIN_PROTOCOL.md)"
                    .to_owned()
            } else {
                error.to_string()
            })
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        let invalid = |message: String| Err(PluginError::Manifest(message));
        if self.id.is_empty()
            || self.id.len() > 128
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return invalid("id must be 1..=128 ASCII letters, digits, '.', '-' or '_'".to_owned());
        }
        if self.name.trim().is_empty() || self.version.trim().is_empty() {
            return invalid("name and version must not be empty".to_owned());
        }
        if self.protocol != PROTOCOL_VERSION {
            return invalid(format!(
                "plugin protocol {} is not supported; LovelyCarve speaks LPP {PROTOCOL_VERSION}",
                self.protocol
            ));
        }
        let library = Path::new(&self.library);
        if self.library.trim().is_empty()
            || library
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return invalid(
                "library must be a relative path inside the package directory".to_owned(),
            );
        }
        self.memory_limit()?;
        let mut permissions = HashSet::new();
        if let Some(permission) = self
            .permissions
            .iter()
            .find(|permission| !permissions.insert(**permission))
        {
            return invalid(format!(
                "permission {permission:?} is declared more than once"
            ));
        }
        let mut analyzers = HashSet::new();
        for analyzer in &self.analyzers {
            if !analyzers.insert(analyzer.id.as_str()) {
                return invalid(format!(
                    "analyzer {} is declared more than once",
                    analyzer.id
                ));
            }
            analyzer.validate().map_err(|message| {
                PluginError::Manifest(format!("analyzer {}: {message}", analyzer.id))
            })?;
        }
        let mut filesystems = HashSet::new();
        for filesystem in &self.filesystems {
            if filesystem.id.is_empty()
                || !filesystem.id.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'_' | b'.')
                })
            {
                return invalid(
                    "filesystem ids must be non-empty lowercase ASCII identifiers".to_owned(),
                );
            }
            if !filesystems.insert(filesystem.id.as_str()) {
                return invalid(format!(
                    "filesystem {} is declared more than once",
                    filesystem.id
                ));
            }
        }
        Ok(())
    }

    /// The plugin process memory limit in bytes.
    pub fn memory_limit(&self) -> Result<u64> {
        let Some(text) = &self.memory else {
            return Ok(DEFAULT_MEMORY);
        };
        let bytes = parse_size(text).ok_or_else(|| {
            PluginError::Manifest(format!(
                "memory {text:?} must be a size such as 512MiB or 2GiB"
            ))
        })?;
        if !(MIN_MEMORY..=MAX_MEMORY).contains(&bytes) {
            return Err(PluginError::Manifest(format!(
                "memory {text:?} must be between 64MiB and 64GiB"
            )));
        }
        Ok(bytes)
    }

    /// The plugin library inside `plugin_directory`.
    pub fn library_path(&self, plugin_directory: &Path) -> PathBuf {
        let relative = Path::new(&self.library);
        let has_library_extension =
            relative
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| {
                    ["dll", "so", "dylib"]
                        .iter()
                        .any(|known| extension.eq_ignore_ascii_case(known))
                });
        if has_library_extension {
            return plugin_directory.join(relative);
        }
        let file_name = relative
            .file_name()
            .map(|name| format!("{DLL_PREFIX}{}{DLL_SUFFIX}", name.to_string_lossy()))
            .unwrap_or_default();
        plugin_directory.join(relative.with_file_name(file_name))
    }

    pub fn analyzer(&self, id: &str) -> Option<&AnalyzerManifest> {
        self.analyzers.iter().find(|analyzer| analyzer.id == id)
    }

    pub fn has_permission(&self, permission: PluginPermission) -> bool {
        self.permissions.contains(&permission)
    }
}

impl AnalyzerManifest {
    fn validate(&self) -> std::result::Result<(), String> {
        validate_identifier("analyzer id", &self.id)?;
        if self.name.trim().is_empty() || self.name.len() > 256 {
            return Err("name must be 1..=256 bytes".to_owned());
        }
        validate_identifier("category", &self.category)?;
        for fact in self.provides.iter().chain(&self.requires) {
            validate_identifier("fact", fact)?;
        }
        match self.scope {
            AnalyzerScope::System => {
                for platform in &self.platforms {
                    if !SYSTEM_FAMILIES.contains(&platform.as_str()) {
                        return Err(format!(
                            "platform {platform:?} is not one of {}",
                            SYSTEM_FAMILIES.join(", ")
                        ));
                    }
                }
            }
            AnalyzerScope::Evidence | AnalyzerScope::Files if !self.platforms.is_empty() => {
                return Err("platforms apply to system-scope analyzers only".to_owned());
            }
            AnalyzerScope::Evidence | AnalyzerScope::Files => {}
        }
        if self.scope == AnalyzerScope::Files {
            if !self.inputs.is_empty() {
                return Err("files-scope analyzers take the selected files, not inputs".to_owned());
            }
            if !self.requires.is_empty() {
                return Err("files-scope analyzers cannot require facts".to_owned());
            }
        } else if !self.extensions.is_empty() || !self.mime_types.is_empty() {
            return Err("extensions and mime_types apply to files-scope analyzers only".to_owned());
        }
        if self.scope == AnalyzerScope::Files && self.trigger == TaskTrigger::Auto {
            return Err("files-scope analyzers run only when the user selects files".to_owned());
        }
        if self.inputs.len() > 64 {
            return Err("an analyzer declares at most 64 input patterns".to_owned());
        }
        for pattern in &self.inputs {
            Glob::new(pattern).map_err(|error| format!("input {pattern:?}: {error}"))?;
        }
        for extension in &self.extensions {
            if extension.is_empty()
                || !extension.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'_')
                })
            {
                return Err("extensions must be lowercase without a leading dot".to_owned());
            }
        }
        for mime in &self.mime_types {
            if !mime.contains('/') || !mime.is_ascii() || mime.contains(char::is_whitespace) {
                return Err("MIME types must be ASCII type/subtype values".to_owned());
            }
        }
        let mut options = HashSet::new();
        for option in &self.options {
            if !options.insert(option.id.as_str()) {
                return Err(format!("option {} is declared more than once", option.id));
            }
            option.validate()?;
        }
        Ok(())
    }
}

impl OptionManifest {
    fn validate(&self) -> std::result::Result<(), String> {
        validate_identifier("option id", &self.id)?;
        if self.label.trim().is_empty() {
            return Err(format!("option {} needs a label", self.id));
        }
        let mut choices = HashSet::new();
        for choice in &self.choices {
            validate_identifier("choice id", &choice.id)?;
            if !choices.insert(choice.id.as_str()) {
                return Err(format!("choice {} is declared more than once", choice.id));
            }
        }
        match self.kind {
            OptionKind::Multi | OptionKind::Choice if self.choices.is_empty() => {
                return Err(format!("option {} needs choices", self.id));
            }
            OptionKind::Flag | OptionKind::Text if !self.choices.is_empty() => {
                return Err(format!("option {} takes no choices", self.id));
            }
            OptionKind::Multi | OptionKind::Choice | OptionKind::Flag | OptionKind::Text => {}
        }
        if let Some(default) = &self.default {
            let valid = match (self.kind, default) {
                (OptionKind::Flag, toml::Value::Boolean(_)) => true,
                (OptionKind::Text, toml::Value::String(_)) => true,
                (OptionKind::Choice, toml::Value::String(choice)) => {
                    choices.contains(choice.as_str())
                }
                (OptionKind::Multi, toml::Value::Array(items)) => items
                    .iter()
                    .all(|item| item.as_str().is_some_and(|choice| choices.contains(choice))),
                _ => false,
            };
            if !valid {
                return Err(format!(
                    "option {} has a default of the wrong type",
                    self.id
                ));
            }
        }
        Ok(())
    }

    /// The default as JSON (empty selection, `false`, empty text when absent).
    pub fn default_json(&self) -> serde_json::Value {
        match (&self.default, self.kind) {
            (Some(value), _) => toml_to_json(value),
            (None, OptionKind::Multi) => serde_json::Value::Array(Vec::new()),
            (None, OptionKind::Flag) => serde_json::Value::Bool(false),
            (None, OptionKind::Choice) => self
                .choices
                .first()
                .map_or(serde_json::Value::Null, |choice| {
                    serde_json::Value::String(choice.id.clone())
                }),
            (None, OptionKind::Text) => serde_json::Value::String(String::new()),
        }
    }

    /// Checks a value the user picked.
    pub fn check(&self, value: &serde_json::Value) -> std::result::Result<(), String> {
        let known = |choice: &str| self.choices.iter().any(|item| item.id == choice);
        let valid = match (self.kind, value) {
            (OptionKind::Flag, serde_json::Value::Bool(_)) => true,
            (OptionKind::Text, serde_json::Value::String(text)) => text.len() <= 4096,
            (OptionKind::Choice, serde_json::Value::String(choice)) => known(choice),
            (OptionKind::Multi, serde_json::Value::Array(items)) => {
                items.iter().all(|item| item.as_str().is_some_and(known))
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(format!("option {} has an invalid value", self.id))
        }
    }
}

fn toml_to_json(value: &toml::Value) -> serde_json::Value {
    match value {
        toml::Value::String(text) => serde_json::Value::String(text.clone()),
        toml::Value::Integer(number) => serde_json::Value::from(*number),
        toml::Value::Float(number) => serde_json::Number::from_f64(*number)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        toml::Value::Boolean(flag) => serde_json::Value::Bool(*flag),
        toml::Value::Datetime(datetime) => serde_json::Value::String(datetime.to_string()),
        toml::Value::Array(items) => {
            serde_json::Value::Array(items.iter().map(toml_to_json).collect())
        }
        toml::Value::Table(table) => serde_json::Value::Object(
            table
                .iter()
                .map(|(key, item)| (key.clone(), toml_to_json(item)))
                .collect(),
        ),
    }
}

/// `512MiB`, `1GiB`, `2048MB`, `1073741824`.
fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().ok()?;
    let multiplier: u64 = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 1,
        "kib" | "k" => 1 << 10,
        "mib" | "m" => 1 << 20,
        "gib" | "g" => 1 << 30,
        "kb" => 1_000,
        "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        _ => return None,
    };
    number.checked_mul(multiplier)
}

#[cfg(test)]
mod tests;
