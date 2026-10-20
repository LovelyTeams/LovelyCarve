//! Command-line parsing: a clap tree built from the catalogue, and argument
//! values keyed by API parameter name. Help screens come from `help`.
use std::collections::BTreeMap;

use clap::{
    builder::PossibleValuesParser,
    error::{ContextKind, ContextValue, ErrorKind},
    Arg, ArgAction, ArgMatches,
};
use serde_json::{Map, Value};

use super::{
    catalog::{Catalog, Command, Kind, Param},
    complete::GLOBAL_OPTIONS,
    help, CliError,
};

/// The value of a secret option passed without one: ask, or read stdin.
pub const ASK: &str = "-";

#[derive(Debug, Default, Clone)]
pub struct Globals {
    pub timezone: Option<String>,
    pub quiet: bool,
    pub events: bool,
    pub session: Option<String>,
    /// The evidence scope: an image ID or an evidence path.
    pub image: Option<String>,
    pub case: Option<String>,
}

#[derive(Debug)]
pub struct Invocation {
    pub command: &'static Command,
    /// Values given on the command line, keyed by API parameter name.
    pub values: Map<String, Value>,
    /// Secret parameters passed without a value.
    pub ask: Vec<String>,
    pub dry_run: bool,
    pub force: bool,
    pub text: bool,
}

#[derive(Debug)]
pub enum Parsed {
    Splash,
    Version {
        quiet: bool,
    },
    Help(String),
    UsageError {
        help: Option<String>,
        error: CliError,
    },
    Complete(Vec<String>),
    Run {
        globals: Globals,
        invocation: Invocation,
    },
}

fn global_args(command: clap::Command) -> clap::Command {
    let mut command = command
        .disable_help_flag(true)
        .disable_version_flag(true)
        .disable_help_subcommand(true);
    for (long, short, _, takes_value) in GLOBAL_OPTIONS {
        let mut arg = Arg::new(*long).long(*long).global(true);
        if *long == "timezone" {
            arg = arg.allow_hyphen_values(true);
        }
        if let Some(short) = short {
            arg = arg.short(*short);
        }
        arg = match *long {
            "help" => arg.action(ArgAction::Help),
            _ if *takes_value => arg,
            _ => arg.action(ArgAction::SetTrue),
        };
        command = command.arg(arg);
    }
    command
}

fn param_arg(command: &Command, param: &Param) -> Option<Arg> {
    if !param.visible() || (param.scope && param.position.is_none()) {
        return None;
    }
    let mut arg = Arg::new(param.name.clone());
    if command.native.as_deref() == Some("timezones-set") && param.name == "zone" {
        arg = arg.allow_hyphen_values(true);
    }
    if let Some(position) = param.position {
        arg = arg.index(position + 1).value_name(param.option.clone());
        // `--body` may supply a required subject instead; the command then
        // checks it before running.
        let mut alternatives = Vec::new();
        if param.synthetic {
            alternatives.extend(command.target.clone());
        }
        if command.takes_body() {
            alternatives.push("body".to_owned());
        }
        if param.required {
            arg = if alternatives.is_empty() {
                arg.required(true)
            } else {
                arg.required_unless_present_any(alternatives)
            };
        }
    } else {
        arg = arg.long(param.option.clone());
        if let Some(short) = param.short {
            arg = arg.short(short);
        }
    }
    arg = match param.kind {
        // `--flag` alone means true; `--flag=false` turns an API default off.
        Kind::Boolean => arg
            .num_args(0..=1)
            .require_equals(true)
            .default_missing_value("true")
            .value_parser(clap::value_parser!(bool)),
        Kind::Enum => arg.value_parser(PossibleValuesParser::new(
            param.values.clone().unwrap_or_default(),
        )),
        Kind::Array if param.position.is_some() => {
            arg.num_args(if param.required { 1.. } else { 0.. })
        }
        // `--algos md5 sha256`, `--algos md5 --algos sha256` or a JSON array.
        Kind::Array => arg.num_args(1..).action(ArgAction::Append),
        Kind::String if param.secret && param.position.is_none() => {
            arg.num_args(0..=1).default_missing_value(ASK)
        }
        Kind::String | Kind::Number | Kind::Json => arg,
    };
    Some(arg)
}

fn switch(id: &str) -> Arg {
    Arg::new(id.to_owned())
        .long(id.to_owned())
        .action(ArgAction::SetTrue)
}

fn leaf(name: &str, command: &Command) -> clap::Command {
    let mut leaf = clap::Command::new(name.to_owned())
        .hide(command.hide_command)
        .disable_help_flag(true);
    for param in &command.params {
        if let Some(arg) = param_arg(command, param) {
            leaf = leaf.arg(arg);
        }
    }
    if command.raw_output {
        leaf = leaf.arg(switch("text"));
    }
    if command.destructive {
        leaf = leaf.arg(switch("force").short('f'));
    }
    if command.takes_body() {
        leaf = leaf.arg(Arg::new("body").long("body"));
    }
    leaf.arg(switch("dry-run"))
}

#[derive(Default)]
struct Node<'a> {
    command: Option<&'a Command>,
    children: BTreeMap<String, Node<'a>>,
}

fn to_clap(catalog: &Catalog, name: &str, path: &[String], node: &Node<'_>) -> clap::Command {
    if let Some(command) = node.command {
        return leaf(name, command);
    }
    let words: Vec<&str> = path.iter().map(String::as_str).collect();
    let mut group = clap::Command::new(name.to_owned())
        .hide(catalog.is_hidden(&words))
        .disable_help_flag(true)
        .disable_help_subcommand(true);
    for (child, node) in &node.children {
        let mut child_path = path.to_vec();
        child_path.push(child.clone());
        group = group.subcommand(to_clap(catalog, child, &child_path, node));
    }
    group
}

pub fn build(catalog: &'static Catalog) -> clap::Command {
    let mut tree = Node::default();
    for command in &catalog.commands {
        let mut node = &mut tree;
        for word in &command.full_path {
            node = node.children.entry(word.clone()).or_default();
        }
        node.command = Some(command);
    }
    let mut root = clap::Command::new(catalog.binary.clone()).bin_name(catalog.binary.clone());
    root = global_args(root);
    for (name, node) in &tree.children {
        root = root.subcommand(to_clap(catalog, name, std::slice::from_ref(name), node));
    }
    root
}

/// The command path in `args`, skipping options and their values.
pub fn scan_path(catalog: &Catalog, args: &[String]) -> Vec<String> {
    let mut path: Vec<String> = Vec::new();
    let mut skip = false;
    for arg in args {
        if skip {
            skip = false;
            continue;
        }
        if arg == "--" {
            break;
        }
        if arg.starts_with('-') {
            skip = matches!(
                arg.as_str(),
                "--image" | "-i" | "--case" | "--session" | "--timezone"
            );
            continue;
        }
        let words: Vec<&str> = path.iter().map(String::as_str).collect();
        if catalog.find(&words).is_some() {
            break;
        }
        let mut next = words.clone();
        next.push(arg);
        if catalog.is_group(&next) || catalog.find(&next).is_some() {
            path.push(arg.clone());
        } else {
            break;
        }
    }
    path
}

fn context_text(error: &clap::Error, kind: ContextKind) -> Option<String> {
    match error.get(kind)? {
        ContextValue::String(text) => Some(text.clone()),
        ContextValue::Strings(texts) => Some(texts.join(", ")),
        ContextValue::StyledStr(text) => Some(text.to_string()),
        ContextValue::StyledStrs(texts) => Some(
            texts
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        _ => None,
    }
}

/// Command names clap suggests, without hidden commands.
fn suggested_commands(catalog: &Catalog, path: &[String], error: &clap::Error) -> Vec<String> {
    let suggestions = match error.get(ContextKind::SuggestedSubcommand) {
        Some(ContextValue::String(name)) => vec![name.clone()],
        Some(ContextValue::Strings(names)) => names.clone(),
        _ => Vec::new(),
    };
    suggestions
        .into_iter()
        .filter(|name| {
            let mut words: Vec<&str> = path.iter().map(String::as_str).collect();
            words.push(name);
            !catalog.is_hidden(&words)
        })
        .collect()
}

fn usage_error(catalog: &'static Catalog, path: &[String], error: &clap::Error) -> Parsed {
    let invalid = context_text(error, ContextKind::InvalidArg);
    let value = context_text(error, ContextKind::InvalidValue);
    let mut message = match error.kind() {
        ErrorKind::InvalidSubcommand => {
            let word = context_text(error, ContextKind::InvalidSubcommand).unwrap_or_default();
            match catalog.replacement(&word).filter(|_| path.is_empty()) {
                Some(replacement) => format!("未知命令：{word}；现在的命令是 {replacement}"),
                None => format!("未知命令：{word}"),
            }
        }
        ErrorKind::UnknownArgument => {
            format!("未知选项或多余参数：{}", invalid.unwrap_or_default())
        }
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => format!(
            "{} 的值无效：{}",
            invalid.unwrap_or_default(),
            value.unwrap_or_default()
        ),
        ErrorKind::MissingRequiredArgument => {
            format!("缺少参数：{}", invalid.unwrap_or_default())
        }
        ErrorKind::NoEquals => format!("{} 需要写成 --选项=值", invalid.unwrap_or_default()),
        _ => error
            .to_string()
            .lines()
            .next()
            .unwrap_or_default()
            .trim_start_matches("error: ")
            .to_owned(),
    };
    if let Some(valid) = context_text(error, ContextKind::ValidValue) {
        message.push_str(&format!("（可选：{valid}）"));
    }
    let prefix = if path.is_empty() {
        String::new()
    } else {
        format!("{} ", path.join(" "))
    };
    for suggestion in suggested_commands(catalog, path, error) {
        message.push_str(&format!(
            "\n你是否要找：{} {prefix}{suggestion}？",
            catalog.binary
        ));
    }
    if let Some(suggestion) = context_text(error, ContextKind::SuggestedArg) {
        message.push_str(&format!(
            "\n你是否要找：{} {prefix}{suggestion}？",
            catalog.binary
        ));
    }
    if error.kind() == ErrorKind::InvalidSubcommand {
        message.push_str(&format!(
            "\n用 {} cli search \"<要完成的任务>\" 查找命令",
            catalog.binary
        ));
    }
    Parsed::UsageError {
        help: Some(help::render(catalog, path)),
        error: CliError::new("usage", message),
    }
}

fn read_file(path: &str, option: &str) -> Result<String, CliError> {
    if path.is_empty() {
        return Err(CliError::invalid(format!(
            "--{option} 的 @ 后面需要文件路径"
        )));
    }
    std::fs::read_to_string(path)
        .map(|text| text.trim_start_matches('\u{feff}').to_owned())
        .map_err(|error| CliError::invalid(format!("--{option}：无法读取文件 {path}：{error}")))
}

/// Inline JSON, or `@path` to a JSON file (curl's `-d @file`).
pub fn parse_json(raw: &str, option: &str) -> Result<Value, CliError> {
    let text = match raw.strip_prefix('@') {
        Some(path) => read_file(path, option)?,
        None => raw.to_owned(),
    };
    serde_json::from_str(&text)
        .map_err(|error| CliError::invalid(format!("--{option} 不是有效的 JSON：{error}")))
}

fn scalar(param: &Param, raw: &str) -> Result<Value, CliError> {
    match param.kind {
        Kind::Number => raw
            .parse::<serde_json::Number>()
            .map(Value::Number)
            .map_err(|_| CliError::invalid(format!("--{} 需要数字：{raw}", param.option))),
        Kind::Json => match parse_json(raw, &param.option) {
            Ok(value) => Ok(value),
            // A bare word is a string, so enum-typed values need no quotes.
            Err(_) if !raw.starts_with(['@', '{', '[', '"']) => Ok(Value::String(raw.to_owned())),
            Err(error) => Err(error),
        },
        Kind::String if param.secret => match raw.strip_prefix('@') {
            Some(path) => {
                let text = read_file(path, &param.option)?;
                Ok(Value::String(
                    text.strip_suffix('\n')
                        .map(|text| text.strip_suffix('\r').unwrap_or(text))
                        .unwrap_or(&text)
                        .to_owned(),
                ))
            }
            None => Ok(Value::String(raw.to_owned())),
        },
        Kind::String | Kind::Enum | Kind::Boolean | Kind::Array => {
            Ok(Value::String(raw.to_owned()))
        }
    }
}

fn array(param: &Param, items: Vec<String>) -> Result<Value, CliError> {
    let values = match items.as_slice() {
        [single] if single.trim_start().starts_with('[') => {
            match parse_json(single, &param.option)? {
                Value::Array(values) => values,
                _ => {
                    return Err(CliError::invalid(format!(
                        "--{} 需要 JSON 数组",
                        param.option
                    )))
                }
            }
        }
        _ => items.into_iter().map(Value::String).collect(),
    };
    if let Some(allowed) = &param.values {
        for value in &values {
            if !value
                .as_str()
                .is_some_and(|value| allowed.iter().any(|item| item == value))
            {
                return Err(CliError::invalid(format!(
                    "--{} 的值无效：{value}（可选：{}）",
                    param.option,
                    allowed.join(", ")
                )));
            }
        }
    }
    Ok(Value::Array(values))
}

fn flag(matches: &ArgMatches, id: &str) -> bool {
    matches
        .try_get_one::<bool>(id)
        .ok()
        .flatten()
        .copied()
        .unwrap_or(false)
}

fn invocation(command: &'static Command, matches: &ArgMatches) -> Result<Invocation, CliError> {
    let mut values = Map::new();
    let mut ask = Vec::new();
    if let Some(raw) = matches.try_get_one::<String>("body").ok().flatten() {
        match parse_json(raw, "body")? {
            Value::Object(body) => values = body,
            _ => return Err(CliError::invalid("--body 需要 JSON 对象")),
        }
    }
    for param in &command.params {
        if param_arg(command, param).is_none() {
            continue;
        }
        let id = param.name.as_str();
        let value = match param.kind {
            Kind::Boolean => matches
                .try_get_one::<bool>(id)
                .ok()
                .flatten()
                .map(|value| Value::Bool(*value)),
            Kind::Array => match matches.try_get_many::<String>(id).ok().flatten() {
                Some(items) => Some(array(param, items.cloned().collect())?),
                None => None,
            },
            _ => match matches.try_get_one::<String>(id).ok().flatten() {
                Some(raw) if param.secret && raw == ASK => {
                    ask.push(param.name.clone());
                    None
                }
                Some(raw) => Some(scalar(param, raw)?),
                None => None,
            },
        };
        if let Some(value) = value {
            values.insert(param.name.clone(), value);
        }
    }
    command.validate(&values)?;
    Ok(Invocation {
        command,
        values,
        ask,
        dry_run: flag(matches, "dry-run"),
        force: flag(matches, "force"),
        text: flag(matches, "text"),
    })
}

fn globals(matches: &ArgMatches) -> Globals {
    Globals {
        timezone: matches
            .try_get_one::<String>("timezone")
            .ok()
            .flatten()
            .cloned(),
        session: matches
            .try_get_one::<String>("session")
            .ok()
            .flatten()
            .cloned(),
        quiet: flag(matches, "quiet"),
        events: flag(matches, "events"),
        image: matches
            .try_get_one::<String>("image")
            .ok()
            .flatten()
            .cloned(),
        case: matches
            .try_get_one::<String>("case")
            .ok()
            .flatten()
            .cloned(),
    }
}

/// Parse the arguments after the program name.
pub fn parse(catalog: &'static Catalog, args: &[String]) -> Parsed {
    if args.is_empty() {
        return Parsed::Splash;
    }
    let separator = args.iter().position(|arg| arg == "--");
    let before = &args[..separator.unwrap_or(args.len())];
    if let Some(separator) = separator {
        if scan_path(catalog, before).first().map(String::as_str) == Some("complete") {
            return Parsed::Complete(args[separator + 1..].to_vec());
        }
    }
    if before.iter().any(|arg| arg == "--version" || arg == "-v") {
        return Parsed::Version {
            quiet: before.iter().any(|arg| arg == "--quiet" || arg == "-q"),
        };
    }
    let path = scan_path(catalog, args);
    let matches = match build(catalog)
        .try_get_matches_from(std::iter::once(catalog.binary.clone()).chain(args.iter().cloned()))
    {
        Ok(matches) => matches,
        Err(error) if error.kind() == ErrorKind::DisplayHelp => {
            return Parsed::Help(help::render(catalog, &path))
        }
        Err(error) => return usage_error(catalog, &path, &error),
    };
    let globals = globals(&matches);
    let mut words = Vec::new();
    let mut current = &matches;
    while let Some((name, sub)) = current.subcommand() {
        words.push(name.to_owned());
        current = sub;
    }
    let found: Vec<&str> = words.iter().map(String::as_str).collect();
    let Some(command) = catalog.find(&found) else {
        // A group without its command: show what it contains.
        return Parsed::Help(help::render(catalog, &words));
    };
    match invocation(command, current) {
        Ok(invocation) => Parsed::Run {
            globals,
            invocation,
        },
        Err(error) => Parsed::UsageError {
            help: None,
            error: CliError {
                code: "usage".into(),
                ..error
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::catalog::catalog;

    fn run(args: &[&str]) -> (Globals, Invocation) {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
        match parse(catalog().unwrap(), &args) {
            Parsed::Run {
                globals,
                invocation,
            } => (globals, invocation),
            other => panic!("not a command: {other:?}"),
        }
    }

    #[test]
    fn the_generated_tree_is_consistent() {
        build(catalog().unwrap()).debug_assert();
    }

    #[test]
    fn values_are_keyed_by_api_name() {
        let (globals, invocation) = run(&[
            "--image",
            r"D:\Evidence\disk.E01",
            "images",
            "export",
            "--destination",
            r"D:\out.raw",
            "--mode",
            "unlockedPartitions",
            "--read-fault-policy",
            "strict",
        ]);
        assert_eq!(globals.image.as_deref(), Some(r"D:\Evidence\disk.E01"));
        assert_eq!(invocation.command.path(), "images export");
        assert_eq!(
            Value::Object(invocation.values),
            serde_json::json!({
                "destination": r"D:\out.raw", "mode": "unlockedPartitions", "readFaultPolicy": "strict"
            })
        );

        let (_, invocation) = run(&[
            "files",
            "hash",
            "/C/Windows/notepad.exe",
            "--algos",
            "md5",
            "--algos",
            "sha256",
        ]);
        assert_eq!(
            Value::Object(invocation.values),
            serde_json::json!({"path": "/C/Windows/notepad.exe", "algos": ["md5", "sha256"]})
        );

        let (_, invocation) = run(&[
            "files",
            "sqlite",
            "rows",
            "list",
            "--path",
            "a.db",
            "--table",
            "t",
            "--descending",
            "--limit",
            "5",
        ]);
        assert_eq!(
            Value::Object(invocation.values),
            serde_json::json!({"path": "a.db", "table": "t", "descending": true, "limit": 5})
        );

        let (_, invocation) = run(&["ios-backups", "unlock", "--password"]);
        assert_eq!(invocation.ask, ["password"]);

        let (_, invocation) = run(&["cases", "delete", r"D:\cases\a", "-f", "--dry-run"]);
        assert!(invocation.force && invocation.dry_run);
    }

    #[test]
    fn arrays_take_several_values_after_one_option() {
        let (_, invocation) = run(&[
            "files",
            "hash",
            "/C/Windows/notepad.exe",
            "--algos",
            "md5",
            "sha256",
        ]);
        assert_eq!(
            invocation.values["algos"],
            serde_json::json!(["md5", "sha256"])
        );
        let (_, invocation) = run(&["files", "hash", "/a", "--algos", r#"["sm3"]"#]);
        assert_eq!(invocation.values["algos"], serde_json::json!(["sm3"]));
    }

    #[test]
    fn body_supplies_arguments_and_options_override_it() {
        let (_, invocation) = run(&[
            "carve",
            "scans",
            "create",
            "--body",
            r#"{"options":{"scope":"whole"},"imageId":"x"}"#,
            "--options",
            r#"{"scope":"unallocated"}"#,
        ]);
        assert_eq!(invocation.values["options"]["scope"], "unallocated");
        assert_eq!(invocation.values["imageId"], "x");
        // A required subject may come from --body instead of the command line.
        let (_, invocation) = run(&["images", "open", "--body", r#"{"path":"D:/disk.E01"}"#]);
        assert_eq!(invocation.values["path"], "D:/disk.E01");
        let (_, invocation) = run(&["files", "export", "--body", r#"{"nodeId":"n1"}"#]);
        assert_eq!(invocation.values["nodeId"], "n1");
    }

    #[test]
    fn groups_show_help_and_mistakes_are_usage_errors() {
        let catalog = catalog().unwrap();
        assert!(matches!(
            parse(catalog, &["images".into()]),
            Parsed::Help(_)
        ));
        assert!(matches!(parse(catalog, &[]), Parsed::Splash));
        assert!(matches!(
            parse(catalog, &["-v".into()]),
            Parsed::Version { .. }
        ));
        for args in [
            vec!["imagez"],
            vec!["images", "open"],
            vec!["images", "export", "--mode", "everything"],
            vec!["cli", "search", "open", "image"],
        ] {
            let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
            assert!(
                matches!(parse(catalog, &args), Parsed::UsageError { .. }),
                "{args:?}"
            );
        }
        match parse(catalog, &["complete".into(), "--".into(), "ima".into()]) {
            Parsed::Complete(words) => assert_eq!(words, ["ima"]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn hidden_commands_run_but_are_never_suggested() {
        let catalog = catalog().unwrap();
        assert_eq!(run(&["tools"]).1.command.path(), "tools");
        assert!(matches!(
            parse(catalog, &["tools".into(), "--help".into()]),
            Parsed::Help(_)
        ));
        match parse(catalog, &["toolz".into()]) {
            Parsed::UsageError { error, .. } => {
                assert!(!error.message.contains("tools"), "{}", error.message)
            }
            other => panic!("{other:?}"),
        }
        match parse(catalog, &["imagez".into()]) {
            Parsed::UsageError { error, .. } => assert!(
                error.message.contains("lovelycarve-cli images？"),
                "{}",
                error.message
            ),
            other => panic!("{other:?}"),
        }
    }
}
