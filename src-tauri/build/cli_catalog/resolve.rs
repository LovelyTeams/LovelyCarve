//! Turns the manifest and the backend signatures into commands, enforcing the
//! naming and argument rules every command follows.
use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use super::emit::BINARY;
use super::manifest::{Category, CommandSpec, Kind, Manifest};
use super::operation::Operation;

/// The verbs a command may end with. CRUD verbs first, then the domain
/// actions; adding one is a design decision, not a convenience.
pub const VERBS: &[&str] = &[
    "get", "list", "create", "update", "edit", "delete", "count", "open", "close", "read",
    "export", "hash", "identify", "search", "scan", "detect", "lookup", "unlock", "lock",
    "assemble", "build", "audit", "start", "pause", "resume", "restart", "cancel", "wait",
    "release", "share", "unshare", "reveal", "set", "use", "attach", "serve",
];

/// Synonyms that commands must not use, with the verb that replaces each.
pub const BANNED_VERBS: &[(&str, &str)] = &[
    ("info", "get"),
    ("status", "get"),
    ("stat", "get"),
    ("show", "get"),
    ("describe", "get"),
    ("view", "get"),
    ("inspect", "get"),
    ("fetch", "get"),
    ("remove", "delete"),
    ("rm", "delete"),
    ("drop", "delete"),
    ("unload", "close"),
    ("load", "open"),
    ("ls", "list"),
    ("page", "list"),
    ("query", "list"),
    ("browse", "list"),
    ("find", "list"),
    ("save", "update"),
    ("persist", "edit"),
    ("apply", "edit"),
    ("recover", "export"),
    ("compute", "hash"),
];

/// Options the CLI owns. A parameter may only take one of these names when it
/// is bound to the matching global option (`imageId` → `--image`).
pub const RESERVED_OPTIONS: &[&str] = &[
    "image", "case", "session", "events", "timezone", "fields", "quiet", "help", "version",
    "dry-run", "body", "force", "text", "json", "ndjson", "pretty", "output", "remote", "cwd",
    "config", "yes", "local", "file",
];

/// Global options that supply a parameter when a command does not.
pub const SCOPE_OPTIONS: &[(&str, &str)] = &[("imageId", "image")];

/// Option names reserved as a namespace, such as opt-in feature gates.
pub const RESERVED_PREFIXES: &[&str] = &["experimental-"];

/// One-letter aliases the CLI owns: `-i`, `-q`, `-h`, `-v`, `-f` (`--force`)
/// and `-c`, kept for a configuration file option.
pub const RESERVED_SHORTS: &[char] = &['i', 'q', 'h', 'v', 'f', 'c'];

/// Hand-written commands that stand alone at the root instead of under a collection.
pub const ROOT_COMMANDS: &[&str] = &["schema", "complete", "session", "shell", "tools"];

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedParam {
    pub name: String,
    pub option: String,
    pub kind: Kind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<String>>,
    /// Required on the command line: no default, generated value, scope or path.
    pub required: bool,
    /// Required by the backend operation.
    pub api_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<usize>,
    pub secret: bool,
    pub generated: bool,
    pub hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fixed: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    /// Supplied by a global option when absent.
    pub scope: bool,
    /// A path inside the image that resolves to the command's `target`.
    pub synthetic: bool,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rust_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub short: Option<char>,
}

pub struct Resolved<'a> {
    pub spec: &'a CommandSpec,
    pub operation: Option<&'a Operation>,
    pub params: Vec<ResolvedParam>,
    pub destructive: bool,
}

fn kebab(name: &str) -> String {
    let mut output = String::new();
    for character in name.chars() {
        if character.is_ascii_uppercase() {
            output.push('-');
            output.push(character.to_ascii_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

fn is_word(word: &str) -> bool {
    let mut parts = word.split('-');
    let first = parts.next().unwrap_or_default();
    first.starts_with(|c: char| c.is_ascii_lowercase())
        && first
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && parts.all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

fn fallback_description(name: &str) -> String {
    kebab(name).replace('-', " ")
}

pub fn resolve<'a>(
    manifest: &'a Manifest,
    operations: &'a BTreeMap<String, Operation>,
) -> Result<Vec<Resolved<'a>>, Vec<String>> {
    let mut errors = Vec::new();
    let mut resolved = Vec::new();
    for spec in &manifest.commands {
        match command(manifest, operations, spec) {
            Ok(command) => resolved.push(command),
            Err(mut problems) => errors.append(&mut problems),
        }
    }
    errors.extend(global_rules(manifest, operations, &resolved));
    if errors.is_empty() {
        Ok(resolved)
    } else {
        Err(errors)
    }
}

fn command<'a>(
    manifest: &'a Manifest,
    operations: &'a BTreeMap<String, Operation>,
    spec: &'a CommandSpec,
) -> Result<Resolved<'a>, Vec<String>> {
    let at = &spec.path;
    let mut errors = Vec::new();
    let words = spec.words();
    let root = words.len() == 1 && ROOT_COMMANDS.contains(&words[0]);
    if root && spec.native.is_none() {
        errors.push(format!("{at}: root commands are hand-written"));
    }
    if !root && (words.len() < 2 || words.iter().any(|word| !is_word(word))) {
        errors.push(format!(
            "{at}: a command is `<collection>… <verb>` in lowercase kebab-case words"
        ));
    }
    let verb = words.last().copied().unwrap_or_default();
    let banned = BANNED_VERBS.iter().find(|(banned, _)| *banned == verb);
    match banned {
        _ if root => {}
        Some((_, replacement)) => {
            errors.push(format!("{at}: use `{replacement}`, never `{verb}`"));
        }
        None if !VERBS.contains(&verb) => errors.push(format!(
            "{at}: `{verb}` is not in the verb vocabulary (build/cli_catalog/resolve.rs)"
        )),
        None => {}
    }
    for word in &words[..words.len().saturating_sub(1)] {
        if BANNED_VERBS.iter().any(|(banned, _)| banned == word) {
            errors.push(format!("{at}: `{word}` is not a collection name"));
        }
    }
    if !root && (verb == "delete") != (spec.category == Category::Delete) {
        errors.push(format!(
            "{at}: the `delete` verb and the `delete` category go together"
        ));
    }
    let operation = match (&spec.ipc, &spec.native) {
        (Some(ipc), None) => match operations.get(ipc) {
            Some(operation) => Some(operation),
            None => {
                errors.push(format!("{at}: `{ipc}` is not a registered backend command"));
                None
            }
        },
        (None, Some(_)) => None,
        _ => {
            errors.push(format!("{at}: set exactly one of `ipc` and `native`"));
            None
        }
    };
    if spec.ipc.is_some() && spec.raw_output {
        errors.push(format!(
            "{at}: `raw_output` is derived from the operation's return type"
        ));
    }
    if spec.ipc.is_some() && matches!(verb, "list" | "count") && !spec.args.is_empty() {
        errors.push(format!(
            "{at}: `{verb}` commands take no positional arguments; pass the parent as an option"
        ));
    }
    for example in &spec.examples {
        if !example.command.starts_with(&format!("{BINARY} ")) {
            errors.push(format!(
                "{at}: example `{}` must start with `{BINARY} `",
                example.command
            ));
        }
    }

    let mut params = Vec::new();
    if let Some(operation) = operation {
        for param in operation.params() {
            let overrides = spec.params.get(&param.name);
            params.push(ResolvedParam {
                name: param.name.clone(),
                option: String::new(),
                kind: overrides.and_then(|o| o.kind).unwrap_or(param.kind),
                values: overrides.and_then(|o| o.values.clone()),
                required: false,
                api_required: param.required,
                position: None,
                secret: false,
                generated: false,
                hidden: false,
                fixed: None,
                default: overrides.and_then(|o| o.default.clone()),
                scope: false,
                synthetic: false,
                description: describe(manifest, spec, &param.name),
                rust_type: Some(param.rust_type.clone()),
                short: None,
            });
            if overrides.and_then(|o| o.required).is_some() {
                errors.push(format!(
                    "{at}: `{}` requiredness comes from the Rust signature",
                    param.name
                ));
            }
        }
        if let Some(target) = &spec.target {
            match params.iter().find(|param| &param.name == target) {
                None => errors.push(format!("{at}: target `{target}` is not a parameter")),
                Some(_) if params.iter().any(|param| param.name == "path") => errors.push(format!(
                    "{at}: `path` already names a parameter; it cannot also be a target"
                )),
                Some(_) => {
                    let overrides = spec.params.get("path");
                    params.push(ResolvedParam {
                        name: "path".into(),
                        option: String::new(),
                        kind: Kind::String,
                        values: None,
                        required: false,
                        api_required: false,
                        position: None,
                        secret: false,
                        generated: false,
                        hidden: false,
                        fixed: None,
                        default: overrides.and_then(|o| o.default.clone()),
                        scope: false,
                        synthetic: true,
                        description: describe(manifest, spec, "path"),
                        rust_type: None,
                        short: None,
                    });
                }
            }
        }
        for name in spec.params.keys() {
            if !params.iter().any(|param| &param.name == name) {
                errors.push(format!(
                    "{at}: `{name}` is not a parameter of `{}`",
                    operation.name
                ));
            }
        }
    } else if spec.native.is_some() {
        for (name, overrides) in &spec.params {
            let Some(kind) = overrides.kind else {
                errors.push(format!("{at}: native parameter `{name}` needs a `type`"));
                continue;
            };
            let synthetic = spec.target.as_deref().is_some() && name == "path";
            params.push(ResolvedParam {
                name: name.clone(),
                option: String::new(),
                kind,
                values: overrides.values.clone(),
                required: false,
                api_required: overrides.required.unwrap_or(false),
                position: None,
                secret: false,
                generated: false,
                hidden: false,
                fixed: None,
                default: overrides.default.clone(),
                scope: false,
                synthetic,
                description: describe(manifest, spec, name),
                rust_type: None,
                short: None,
            });
        }
        if let Some(target) = &spec.target {
            if !spec.params.contains_key("path") {
                errors.push(format!(
                    "{at}: native target `{target}` needs a `path` parameter"
                ));
            }
            // A path names a node of some image: the scope option picks it.
            if !spec.params.contains_key("imageId") {
                params.push(ResolvedParam {
                    name: "imageId".into(),
                    option: String::new(),
                    kind: Kind::String,
                    values: None,
                    required: false,
                    api_required: false,
                    position: None,
                    secret: false,
                    generated: false,
                    hidden: false,
                    fixed: None,
                    default: None,
                    scope: false,
                    synthetic: false,
                    description: describe(manifest, spec, "imageId"),
                    rust_type: None,
                    short: None,
                });
            }
            params.push(ResolvedParam {
                name: target.clone(),
                option: String::new(),
                kind: Kind::String,
                values: None,
                required: false,
                api_required: false,
                position: None,
                secret: false,
                generated: false,
                hidden: false,
                fixed: None,
                default: None,
                scope: false,
                synthetic: false,
                description: describe(manifest, spec, target),
                rust_type: None,
                short: None,
            });
        }
    }

    let mut check = |list: &[String], label: &str| {
        for name in list {
            if !params.iter().any(|param| &param.name == name) {
                errors.push(format!("{at}: {label} `{name}` is not a parameter"));
            }
        }
    };
    check(&spec.args, "argument");
    check(&spec.secret, "secret");
    check(&spec.hidden, "hidden");
    check(&spec.generated, "generated");
    check(&spec.no_clobber, "no_clobber");
    check(
        &spec.defaults.keys().cloned().collect::<Vec<_>>(),
        "default",
    );
    check(&spec.fixed.keys().cloned().collect::<Vec<_>>(), "fixed");

    for param in &mut params {
        param.secret = spec.secret.contains(&param.name);
        param.hidden = spec.hidden.contains(&param.name);
        param.generated = spec.generated.contains(&param.name);
        param.fixed = spec.fixed.get(&param.name).cloned();
        if let Some(default) = spec.defaults.get(&param.name) {
            param.default = Some(default.clone());
        }
        param.position = spec.args.iter().position(|name| name == &param.name);
        param.short = spec
            .params
            .get(&param.name)
            .and_then(|overrides| overrides.short);
        let scope = SCOPE_OPTIONS
            .iter()
            .find(|(name, _)| *name == param.name.as_str());
        param.scope = scope.is_some();
        param.option = manifest
            .options
            .get(&param.name)
            .cloned()
            .unwrap_or_else(|| kebab(&param.name));
        let is_target = spec.target.as_deref() == Some(param.name.as_str());
        // A hand-written command always needs the node its path names.
        let target_required = match (spec.target.as_deref(), operation) {
            (Some(target), Some(operation)) => operation
                .param(target)
                .is_some_and(|target| target.required),
            (Some(_), None) => true,
            (None, _) => false,
        };
        param.required = if param.synthetic {
            target_required && param.default.is_none()
        } else {
            param.api_required
                && !param.generated
                && !param.hidden
                && !param.scope
                && !is_target
                && param.default.is_none()
                && param.fixed.is_none()
        };
        if param.secret && param.kind != Kind::String {
            errors.push(format!("{at}: secret `{}` must be a string", param.name));
        }
        if matches!(param.kind, Kind::Enum) && param.values.is_none() {
            errors.push(format!("{at}: enum `{}` needs `values`", param.name));
        }
        if param.hidden && param.api_required {
            errors.push(format!(
                "{at}: hidden `{}` is required by the backend",
                param.name
            ));
        }
        let reserved = RESERVED_OPTIONS.contains(&param.option.as_str())
            || RESERVED_PREFIXES
                .iter()
                .any(|prefix| param.option.starts_with(prefix));
        if reserved
            && !(param.scope
                && SCOPE_OPTIONS
                    .iter()
                    .any(|(name, option)| *name == param.name && *option == param.option))
        {
            errors.push(format!(
                "{at}: option `--{}` is reserved for the CLI",
                param.option
            ));
        }
    }
    let mut seen = BTreeSet::new();
    for param in params
        .iter()
        .filter(|param| !param.hidden && param.fixed.is_none())
    {
        if !seen.insert(param.option.clone()) {
            errors.push(format!(
                "{at}: option `--{}` is defined twice",
                param.option
            ));
        }
    }
    let mut shorts = BTreeSet::new();
    for param in &params {
        let Some(short) = param.short else {
            continue;
        };
        if !short.is_ascii_alphabetic() {
            errors.push(format!(
                "{at}: short option `-{short}` must be one ASCII letter"
            ));
        }
        if param.position.is_some() || param.hidden || param.fixed.is_some() || param.scope {
            errors.push(format!(
                "{at}: `{}` is not an option, so it has no short form",
                param.name
            ));
        }
        if RESERVED_SHORTS.contains(&short) || !shorts.insert(short) {
            errors.push(format!("{at}: short option `-{short}` is already taken"));
        }
    }
    let mut optional_seen = false;
    for name in &spec.args {
        if let Some(param) = params.iter().find(|param| &param.name == name) {
            let required = param.required || (param.synthetic && param.default.is_none());
            if required && optional_seen {
                errors.push(format!(
                    "{at}: required argument `{name}` follows an optional one"
                ));
            }
            optional_seen |= !required;
            if param.hidden || param.fixed.is_some() {
                errors.push(format!(
                    "{at}: `{name}` cannot be both positional and fixed"
                ));
            }
        }
    }
    let destructive = spec.category == Category::Delete || spec.confirm.is_some();
    if errors.is_empty() {
        Ok(Resolved {
            spec,
            operation,
            params,
            destructive,
        })
    } else {
        Err(errors)
    }
}

fn describe(manifest: &Manifest, spec: &CommandSpec, name: &str) -> String {
    spec.params
        .get(name)
        .and_then(|param| param.description.clone())
        .or_else(|| manifest.descriptions.get(name).cloned())
        .unwrap_or_else(|| fallback_description(name))
}

fn global_rules(
    manifest: &Manifest,
    operations: &BTreeMap<String, Operation>,
    resolved: &[Resolved<'_>],
) -> Vec<String> {
    let mut errors = Vec::new();
    let mut paths = BTreeSet::new();
    for spec in &manifest.commands {
        if !paths.insert(spec.path.as_str()) {
            errors.push(format!("{}: duplicate command path", spec.path));
        }
    }
    let mut prefixes = BTreeSet::new();
    for spec in &manifest.commands {
        let words = spec.words();
        for length in 1..words.len() {
            prefixes.insert(words[..length].join(" "));
        }
    }
    for path in &paths {
        if prefixes.contains(*path) {
            errors.push(format!("{path}: a command cannot also be a command group"));
        }
    }
    for prefix in &prefixes {
        if !manifest.groups.contains_key(prefix) {
            errors.push(format!("[groups] needs a description for `{prefix}`"));
        }
    }
    for group in manifest.groups.keys() {
        if !prefixes.contains(group) {
            errors.push(format!("[groups] `{group}` is not a command group"));
        }
    }
    let mut natives = BTreeSet::new();
    for spec in &manifest.commands {
        if let Some(native) = &spec.native {
            if !natives.insert(native.as_str()) {
                errors.push(format!("{}: native `{native}` is used twice", spec.path));
            }
        }
    }
    let mapped: BTreeSet<&str> = resolved
        .iter()
        .filter_map(|command| command.operation.map(|operation| operation.name.as_str()))
        .collect();
    let mut superseded = BTreeSet::new();
    for entry in &manifest.superseded {
        if !operations.contains_key(&entry.ipc) {
            errors.push(format!(
                "[[superseded]] `{}` is not a registered backend command",
                entry.ipc
            ));
        }
        if mapped.contains(entry.ipc.as_str()) {
            errors.push(format!(
                "[[superseded]] `{}` is also mapped to a command",
                entry.ipc
            ));
        }
        if !paths.contains(entry.by.as_str()) {
            errors.push(format!(
                "[[superseded]] `{}`: `{}` is not a command",
                entry.ipc, entry.by
            ));
        }
        if !superseded.insert(entry.ipc.as_str()) {
            errors.push(format!("[[superseded]] `{}` is listed twice", entry.ipc));
        }
    }
    for name in operations.keys() {
        if !mapped.contains(name.as_str()) && !superseded.contains(name.as_str()) {
            errors.push(format!(
                "backend command `{name}` has no CLI command: map it in src/cli/commands.toml"
            ));
        }
    }
    errors
}
