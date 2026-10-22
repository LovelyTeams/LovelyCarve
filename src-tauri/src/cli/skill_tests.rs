//! The packaged agent skill (`skills/lovelycarve-cli`) against the command
//! catalogue. Its command reference is rendered from the catalogue, so this
//! test fails while the file is stale and `just update-cli-skill` rewrites it;
//! the command lines and JSONL requests in the hand-written pages must parse.
use std::{collections::BTreeMap, path::PathBuf};

use serde_json::Value;

use super::{
    argv::{parse, Parsed},
    catalog::{catalog, Catalog, Command, Kind, Param},
    complete::GLOBAL_OPTIONS,
    help::{BODY_HELP, DRY_RUN_HELP, FIELDS_HELP, FORCE_HELP, TEXT_HELP},
    shell,
};

/// The reference rendered from the catalogue.
const REFERENCE: &str = "references/commands.md";
/// The hand-written pages whose examples are checked.
const PAGES: &[&str] = &["SKILL.md", "references/workflows.md"];

fn skill_file(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../skills/lovelycarve-cli")
        .join(name)
}

fn read(name: &str) -> String {
    let path = skill_file(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
        .replace("\r\n", "\n")
}

/// A parameter as written in a synopsis: `<path>`, `[--depth <number>]`.
fn synopsis(param: &Param) -> String {
    let repeat = if param.kind == Kind::Array { "…" } else { "" };
    if param.position.is_some() {
        let name = match &param.values {
            Some(values) => values.join("|"),
            None => param.option.clone(),
        };
        return if param.required {
            format!("<{name}>{repeat}")
        } else {
            format!("[{name}{repeat}]")
        };
    }
    let value = match (&param.values, param.kind) {
        (_, Kind::Boolean) => return format!("[--{}]", param.option),
        (Some(values), _) => values.join("|"),
        (None, Kind::Number) => "number".to_owned(),
        (None, Kind::Json) => "json".to_owned(),
        (None, Kind::String | Kind::Enum | Kind::Array) if param.secret => "secret".to_owned(),
        (None, Kind::String | Kind::Enum | Kind::Array) => "string".to_owned(),
    };
    let option = format!("--{} <{value}>{repeat}", param.option);
    if param.required {
        option
    } else {
        format!("[{option}]")
    }
}

/// The command with its positional arguments, then its options.
fn command_line(command: &Command) -> String {
    let mut words = vec![command.command.clone()];
    words.extend(
        command
            .positionals()
            .into_iter()
            .filter(|param| param.visible())
            .map(synopsis),
    );
    words.extend(
        command
            .params
            .iter()
            .filter(|param| param.visible() && param.position.is_none() && !param.scope)
            .map(synopsis),
    );
    if command.raw_output {
        words.push("[--text]".to_owned());
    }
    if command.destructive {
        words.push("[--force]".to_owned());
    }
    words.join(" ")
}

fn entry(command: &Command) -> String {
    let mut text = format!("- `{}`\n  {}", command_line(command), command.summary);
    if !command.description.is_empty() && command.description != command.summary {
        text.push('。');
        text.push_str(&command.description);
    }
    if !command.keywords.is_empty() {
        text.push_str(&format!("\n  Keywords: {}.", command.keywords.join(", ")));
    }
    for example in &command.examples {
        text.push_str(&format!(
            "\n  - `{}`: {}",
            example.command, example.description
        ));
    }
    text.push('\n');
    text
}

fn render(catalog: &Catalog) -> String {
    let mut collections: BTreeMap<&str, Vec<&Command>> = BTreeMap::new();
    let mut roots = Vec::new();
    for command in catalog
        .commands
        .iter()
        .filter(|command| !command.hide_command)
    {
        match command.full_path.as_slice() {
            [_] => roots.push(command),
            [collection, ..] => collections
                .entry(collection.as_str())
                .or_default()
                .push(command),
            [] => {}
        }
    }
    let mut text = String::from(
        "# lovelycarve-cli command reference\n\n\
         Generated from the CLI command catalogue by `just update-cli-skill`; do not edit by hand.\n\n\
         Each entry is a command's synopsis and its description. Positional arguments come first, \
         then options; `<…>` is required, `[…]` optional, `a|b` lists the accepted values and \
         `…` marks a repeatable value. A command whose synopsis has both a path and a node ID \
         option (`--node-id`, `--parent-id`, `--root-id`) takes one of them. Run \
         `lovelycarve-cli <command> --help` for option descriptions and defaults, and \
         `lovelycarve-cli schema <command>` for the JSON request.\n\n\
         ## Options on every command\n\n\
         | Option | Description |\n\
         | --- | --- |\n",
    );
    for (long, short, description, takes_value) in GLOBAL_OPTIONS {
        let value = if *takes_value { " <value>" } else { "" };
        let name = match short {
            Some(short) => format!("-{short}, --{long}{value}"),
            None => format!("--{long}{value}"),
        };
        text.push_str(&format!("| `{name}` | {description} |\n"));
    }
    for (name, description) in [
        ("--dry-run", DRY_RUN_HELP),
        ("--body <json>", BODY_HELP),
        ("--fields <paths>", FIELDS_HELP),
        ("--text", TEXT_HELP),
        ("--force", FORCE_HELP),
    ] {
        text.push_str(&format!("| `{name}` | {description} |\n"));
    }
    text.push_str(
        "\n`--body` applies to commands with parameters and `--fields` to list output; \
         `--text` and `--force` appear in the synopses of the commands that take them.\n\n\
         ## Contents\n\n",
    );
    for collection in collections.keys() {
        text.push_str(&format!(
            "- [{collection}](#{collection}): {}\n",
            catalog.description(collection).unwrap_or_default()
        ));
    }
    text.push_str("- [Root commands](#root-commands)\n");
    for (collection, commands) in &collections {
        text.push_str(&format!("\n## {collection}\n\n"));
        if let Some(description) = catalog.description(collection) {
            text.push_str(&format!("{description}\n\n"));
        }
        for command in commands {
            text.push_str(&entry(command));
        }
    }
    text.push_str("\n## Root commands\n\n");
    for command in roots {
        text.push_str(&entry(command));
    }
    text
}

#[test]
fn command_reference_matches_the_catalogue() {
    let expected = render(catalog().unwrap());
    let actual = read(REFERENCE);
    if actual != expected {
        let line = actual
            .lines()
            .zip(expected.lines())
            .position(|(actual, expected)| actual != expected)
            .unwrap_or_else(|| actual.lines().count().min(expected.lines().count()))
            + 1;
        panic!(
            "skills/lovelycarve-cli/{REFERENCE} differs from the catalogue from line {line}; \
             run `just update-cli-skill`"
        );
    }
}

#[test]
#[ignore = "rewrites the skill's command reference; run `just update-cli-skill`"]
fn update_command_reference() {
    let path = skill_file(REFERENCE);
    std::fs::write(&path, render(catalog().unwrap()))
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}

#[test]
fn reference_lists_every_listed_command_once() {
    let catalog = catalog().unwrap();
    let reference = render(catalog);
    for command in &catalog.commands {
        let entry = format!("- `{}", command.command);
        let found = reference
            .lines()
            .filter(|line| {
                line.strip_prefix(&entry)
                    .is_some_and(|rest| rest.starts_with([' ', '`']))
            })
            .count();
        assert_eq!(
            found,
            usize::from(!command.hide_command),
            "{}",
            command.command
        );
    }
}

#[test]
fn skill_examples_parse_against_the_catalogue() {
    let catalog = catalog().unwrap();
    let mut examples = 0;
    for page in PAGES {
        let text = read(page);
        for line in text
            .lines()
            .filter(|line| line.starts_with("lovelycarve-cli "))
        {
            let mut words = shell::tokenize(line).unwrap();
            // A redirection or a pipe ends the command.
            if let Some(end) = words
                .iter()
                .position(|word| matches!(word.as_str(), ">" | ">>" | "2>" | "|"))
            {
                words.truncate(end);
            }
            assert!(
                matches!(parse(catalog, &words[1..]), Parsed::Run { .. }),
                "{page}: {line}"
            );
            examples += 1;
        }
        for line in text.lines().filter(|line| line.starts_with("{\"id\":")) {
            let request: Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("{page}: {line}: {error}"));
            let command = request["command"]
                .as_str()
                .and_then(|path| catalog.find_path(path))
                .unwrap_or_else(|| panic!("{page}: unknown command in {line}"));
            let mut args = request["args"].as_object().cloned().unwrap_or_default();
            // The session resolves `$ref` values before validating them.
            args.retain(|_, value| value.get("$ref").is_none());
            command
                .validate(&args)
                .unwrap_or_else(|error| panic!("{page}: {line}: {error}"));
            examples += 1;
        }
    }
    assert!(
        examples > 0,
        "the skill must contain executable command examples"
    );
}
