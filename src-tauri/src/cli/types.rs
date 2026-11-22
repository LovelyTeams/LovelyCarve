//! Argument types the commands share: evidence addresses, host paths and
//! secrets. Each is parsed where the command line is read, so a command sent
//! to a case backend carries absolute host paths and resolved credentials.
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::CliError;

thread_local! {
    /// The interactive shell's directory, which relative addresses typed
    /// there start from. Unset everywhere else: addresses are absolute.
    static SHELL_DIRECTORY: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Run `parse` with relative addresses resolved against `directory`.
pub fn with_shell_directory<T>(directory: Option<&str>, parse: impl FnOnce() -> T) -> T {
    let previous = SHELL_DIRECTORY.with(|cell| cell.replace(directory.map(str::to_owned)));
    let result = parse();
    SHELL_DIRECTORY.with(|cell| *cell.borrow_mut() = previous);
    result
}

/// A location in the workspace: `/` (every evidence item), `/<alias>` (the
/// evidence root, which lists its partitions and volumes) or
/// `/<alias>/<path>`. The segment after the alias may be a root node ID
/// (`part:0`); `/<alias>/@<node id>` names any node by its ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Address(String);

/// An address split at the evidence alias.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parts<'a> {
    /// `/`: every evidence item.
    Root,
    /// `/<alias>…`: a path inside one evidence item, as given after the alias
    /// (`""` for the evidence root).
    Path { alias: &'a str, path: &'a str },
    /// `/<alias>/@<node id>`.
    Node { alias: &'a str, node_id: &'a str },
}

impl Address {
    pub fn root() -> Self {
        Self("/".to_owned())
    }

    /// An address the CLI itself composed, such as one it prints.
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn parts(&self) -> Parts<'_> {
        let mut rest = self.0.as_str();
        loop {
            rest = rest.trim_start_matches('/');
            let (segment, tail) = match rest.find('/') {
                Some(index) => (&rest[..index], &rest[index..]),
                None => (rest, ""),
            };
            match segment {
                "" => return Parts::Root,
                // Nothing lies above the workspace root.
                "." | ".." => rest = tail,
                alias => {
                    let after = tail.trim_start_matches('/');
                    if let Some(node_id) = after.strip_prefix('@') {
                        if !node_id.is_empty() {
                            return Parts::Node { alias, node_id };
                        }
                    }
                    return Parts::Path { alias, path: tail };
                }
            }
        }
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

fn join(directory: &str, relative: &str) -> String {
    if directory.ends_with('/') {
        format!("{directory}{relative}")
    } else {
        format!("{directory}/{relative}")
    }
}

/// The clap parser of an address: absolute, or relative to the shell's
/// directory. Git Bash / MSYS2 rewriting of `/…` arguments is undone first.
pub fn parse_address(text: &str) -> Result<Address, String> {
    let text = super::msys::evidence_path(text).into_owned();
    if text.starts_with('/') {
        return Ok(Address(text));
    }
    if let Some(reason) = super::msys::host_path(&text) {
        return Err(reason);
    }
    SHELL_DIRECTORY
        .with(|cell| {
            cell.borrow()
                .as_deref()
                .map(|directory| join(directory, &text))
        })
        .map(Address)
        .ok_or_else(|| {
            format!(
                "检材内路径须以 / 开头：/<检材别名>/<路径>，例如 /{text}（evidence ls 列出别名；\
                 相对路径只能在 shell 中使用）"
            )
        })
}

/// A path on the examiner's machine, made absolute where it was typed.
/// References to derived evidence (`assembled://md0`, also `assembled:\md0`
/// as Windows shells may hand it over) are kept as given.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct HostPath(PathBuf);

impl HostPath {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn text(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }
}

pub fn parse_host_path(text: &str) -> Result<HostPath, String> {
    if text.is_empty() {
        return Err("路径不能为空".to_owned());
    }
    // A scheme is a word before `:`; a drive letter is a single letter.
    let reference = text.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme
                .chars()
                .all(|character| character.is_ascii_alphanumeric())
    });
    if reference || text.contains("://") {
        return Ok(HostPath(PathBuf::from(text)));
    }
    std::path::absolute(text)
        .map(HostPath)
        .map_err(|error| format!("无法解析路径 {text}：{error}"))
}

/// A password or key: `@file`, `-` for standard input, or asked for at the
/// terminal when the option is given without a value. Values never appear on
/// the command line, where shell history and process lists would keep them.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Secret {
    Ask,
    Stdin,
    File(PathBuf),
    Value(String),
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ask => formatter.write_str("Ask"),
            Self::Stdin => formatter.write_str("Stdin"),
            Self::File(path) => write!(formatter, "File({})", path.display()),
            Self::Value(_) => formatter.write_str("Value(<redacted>)"),
        }
    }
}

pub fn parse_secret(text: &str) -> Result<Secret, String> {
    match text {
        "" => Ok(Secret::Ask),
        "-" => Ok(Secret::Stdin),
        _ => match text.strip_prefix('@') {
            Some("") => Err("@ 后面需要文件路径".to_owned()),
            Some(path) => std::path::absolute(path)
                .map(Secret::File)
                .map_err(|error| format!("无法解析路径 {path}：{error}")),
            None => Err(
                "密码和密钥不写在命令行上：用 @文件、-（读标准输入），或只写选项名在终端中输入"
                    .to_owned(),
            ),
        },
    }
}

impl Secret {
    /// Read the value now. `label` names the option in prompts and errors;
    /// a prompt needs `interactive`.
    pub fn resolve(&mut self, label: &str, interactive: bool) -> Result<(), CliError> {
        let value = match self {
            Self::Value(_) => return Ok(()),
            Self::File(path) => std::fs::read_to_string(&*path)
                .map(|text| text.trim_end_matches(['\r', '\n']).to_owned())
                .map_err(|error| {
                    CliError::invalid(format!("--{label}：无法读取 {}：{error}", path.display()))
                })?,
            Self::Stdin => super::prompt::read_piped_stdin()?,
            Self::Ask if interactive => super::prompt::required_field(label, "", true)?,
            Self::Ask => {
                return Err(CliError::new(
                    "usage",
                    format!("--{label} 需要值：写成 --{label} @文件 或 --{label} -（读标准输入）"),
                ))
            }
        };
        *self = Self::Value(value);
        Ok(())
    }

    /// The value; [`Secret::resolve`] must have run.
    pub fn value(&self) -> Result<&str, CliError> {
        match self {
            Self::Value(value) => Ok(value),
            Self::Ask | Self::Stdin | Self::File(_) => Err(super::internal_error("凭据尚未读取")),
        }
    }
}

/// A JSON value given inline or as `@file`.
pub fn parse_json_text(text: &str) -> Result<Value, CliError> {
    let source = match text.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)
            .map(|text| text.trim_start_matches('\u{feff}').to_owned())
            .map_err(|error| CliError::invalid(format!("无法读取 {path}：{error}")))?,
        None => text.to_owned(),
    };
    serde_json::from_str(&source)
        .map_err(|error| CliError::invalid(format!("不是有效的 JSON：{error}")))
}

/// `key=value` pairs as a JSON object; a value that parses as JSON keeps its
/// type, anything else is a string.
pub fn options_object(pairs: &[String]) -> Result<serde_json::Map<String, Value>, CliError> {
    let mut object = serde_json::Map::new();
    for pair in pairs {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| CliError::new("usage", format!("选项须写成 键=值：{pair}")))?;
        let value = serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned()));
        object.insert(key.to_owned(), value);
    }
    Ok(object)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_split_at_the_alias_and_resolve_against_the_shell() {
        assert_eq!(Address::root().parts(), Parts::Root);
        assert_eq!(Address::new("//./../").parts(), Parts::Root);
        assert_eq!(
            Address::new("/nas").parts(),
            Parts::Path {
                alias: "nas",
                path: ""
            }
        );
        assert_eq!(
            Address::new("/nas/part:0/etc/../etc/passwd").parts(),
            Parts::Path {
                alias: "nas",
                path: "/part:0/etc/../etc/passwd"
            }
        );
        assert_eq!(
            Address::new("/nas/@ntfs:part:0:5").parts(),
            Parts::Node {
                alias: "nas",
                node_id: "ntfs:part:0:5"
            }
        );
        // A real name that starts with @ below the first level is a name.
        assert_eq!(
            Address::new("/nas/part:0/@eaDir").parts(),
            Parts::Path {
                alias: "nas",
                path: "/part:0/@eaDir"
            }
        );
        assert!(parse_address("etc/passwd").is_err());
        let relative = with_shell_directory(Some("/nas/part:0"), || parse_address("etc"));
        assert_eq!(relative.unwrap().to_string(), "/nas/part:0/etc");
        assert_eq!(
            with_shell_directory(Some("/"), || parse_address("nas"))
                .unwrap()
                .to_string(),
            "/nas"
        );
    }

    #[test]
    fn secrets_are_never_literal_and_paths_become_absolute() {
        assert!(matches!(parse_secret(""), Ok(Secret::Ask)));
        assert!(matches!(parse_secret("-"), Ok(Secret::Stdin)));
        assert!(matches!(parse_secret("@key.txt"), Ok(Secret::File(path)) if path.is_absolute()));
        assert!(parse_secret("hunter2").is_err());
        assert!(parse_host_path("out.raw").unwrap().as_path().is_absolute());
        for reference in ["assembled://md0", "assembled:\\md0", "decrypted://x"] {
            assert_eq!(parse_host_path(reference).unwrap().text(), reference);
        }
        #[cfg(windows)]
        assert_eq!(
            parse_host_path("D:\\ev\\disk.E01").unwrap().text(),
            "D:\\ev\\disk.E01"
        );
        assert_eq!(
            format!("{:?}", Secret::Value("hunter2".into())),
            "Value(<redacted>)"
        );
        assert_eq!(
            Value::Object(
                options_object(&[
                    "items=[\"users\"]".into(),
                    "depth=2".into(),
                    "name=x y".into()
                ])
                .unwrap()
            ),
            serde_json::json!({"items": ["users"], "depth": 2, "name": "x y"})
        );
    }
}
