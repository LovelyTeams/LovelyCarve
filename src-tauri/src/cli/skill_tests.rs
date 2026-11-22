//! The packaged agent skill (`skills/lovelycarve-cli`) against the command
//! tree. Its command reference is rendered from the tree, so this test fails
//! while the file is stale and `just update-cli-skill` rewrites it; the
//! command lines and batch requests in the hand-written pages must parse.
use std::path::PathBuf;

use clap::{Arg, CommandFactory};
use serde_json::Value;

use super::{cmd::Cli, help::SECTIONS, shell};

/// The reference rendered from the command tree.
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

fn help(arg: &Arg) -> String {
    arg.get_help().map(ToString::to_string).unwrap_or_default()
}

fn value_name(arg: &Arg) -> String {
    let values: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| value.get_name().to_owned())
        .collect();
    if !values.is_empty() {
        return values.join("|");
    }
    let name = arg
        .get_value_names()
        .and_then(|names| names.first().map(ToString::to_string))
        .unwrap_or_else(|| arg.get_id().to_string());
    if name
        .chars()
        .all(|character| character.is_ascii_uppercase() || character == '_')
    {
        name.to_lowercase().replace('_', "-")
    } else {
        name
    }
}

/// An argument as a synopsis writes it: `<路径>`, `[--limit <limit>]`.
fn synopsis(arg: &Arg) -> String {
    let repeat = if arg
        .get_num_args()
        .is_some_and(|range| range.max_values() > 1)
        || matches!(arg.get_action(), clap::ArgAction::Append)
    {
        "…"
    } else {
        ""
    };
    if arg.is_positional() {
        let name = value_name(arg);
        return if arg.is_required_set() {
            format!("<{name}>{repeat}")
        } else {
            format!("[{name}{repeat}]")
        };
    }
    let flag = match (arg.get_short(), arg.get_long()) {
        (Some(short), Some(long)) => format!("-{short}|--{long}"),
        (None, Some(long)) => format!("--{long}"),
        (Some(short), None) => format!("-{short}"),
        (None, None) => arg.get_id().to_string(),
    };
    let text = if !arg.get_action().takes_values() {
        flag
    } else if arg
        .get_num_args()
        .is_some_and(|range| range.min_values() == 0)
    {
        format!("{flag}[=<{}>]", value_name(arg))
    } else {
        format!("{flag} <{}>{repeat}", value_name(arg))
    };
    if arg.is_required_set() {
        text
    } else {
        format!("[{text}]")
    }
}

fn entry(path: &str, command: &clap::Command) -> String {
    let mut words = vec![path.to_owned()];
    words.extend(
        command
            .get_positionals()
            .filter(|arg| !arg.is_hide_set())
            .map(synopsis),
    );
    words.extend(
        command
            .get_arguments()
            .filter(|arg| !arg.is_positional() && !arg.is_hide_set() && !arg.is_global_set())
            .map(synopsis),
    );
    let about = command
        .get_long_about()
        .or_else(|| command.get_about())
        .map(ToString::to_string)
        .unwrap_or_default();
    let mut text = format!("- `{}`\n  {about}\n", words.join(" "));
    let meta = super::cmd::meta_of(path);
    let mut notes = Vec::new();
    if meta.output == super::cmd::OutputKind::Bytes {
        notes.push("raw bytes on stdout".to_owned());
    }
    if meta.confirm.is_some() {
        notes.push("asks first; `-y` in scripts".to_owned());
    }
    if meta.lasting {
        notes.push("needs a case backend, shell or batch".to_owned());
    }
    if !meta.incomplete.is_empty() {
        notes.push(format!("exit 2 when set: {}", meta.incomplete.join(", ")));
    }
    if !notes.is_empty() {
        text.push_str(&format!("  ({})\n", notes.join("; ")));
    }
    text
}

/// Every runnable command under `command`, depth first.
fn entries(path: &str, command: &clap::Command, out: &mut String) {
    let children: Vec<&clap::Command> = command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
        .collect();
    if children.is_empty()
        || !command.is_subcommand_required_set()
            && !path.is_empty()
            && command.get_positionals().count() > 0
    {
        out.push_str(&entry(path, command));
    }
    for child in children {
        let child_path = format!("{path} {}", child.get_name()).trim().to_owned();
        entries(&child_path, child, out);
    }
}

fn render() -> String {
    let root = Cli::command();
    let mut text = String::from(
        "# lovelycarve-cli command reference\n\n\
         Generated from the CLI command tree by `just update-cli-skill`; do not edit by hand.\n\n\
         Each entry is a command's synopsis and its description. Positional arguments come first, \
         then options; `<…>` is required, `[…]` optional, `a|b` lists the accepted values and `…` \
         marks a repeatable value. A group given without a subcommand runs the subcommand marked \
         as its default. Evidence addresses start at the workspace root: `/` lists the evidence, \
         `/<alias>` an evidence item's partitions and volumes, `/<alias>/<volume>/<path>` a file, \
         and `/<alias>/@<node id>` any node by its ID. Run `lovelycarve-cli <command> --help` for \
         option descriptions and defaults, and `lovelycarve-cli schema <command>` for the JSON \
         description.\n\n\
         ## Options on every command\n\n",
    );
    for arg in root
        .get_arguments()
        .filter(|arg| arg.is_global_set() && !arg.is_hide_set())
    {
        let synopsis = synopsis(arg);
        let flag = synopsis
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .unwrap_or(&synopsis);
        text.push_str(&format!("- `{flag}`\n  {}\n", help(arg)));
    }
    text.push_str("\n## Contents\n\n");
    for (title, _) in SECTIONS {
        text.push_str(&format!("- [{title}](#{title})\n"));
    }
    for (title, names) in SECTIONS {
        text.push_str(&format!("\n## {title}\n\n"));
        for name in *names {
            if let Some(command) = root.find_subcommand(name) {
                let children = command
                    .get_subcommands()
                    .filter(|child| !child.is_hide_set())
                    .count();
                if children == 0
                    || !command.is_subcommand_required_set()
                        && command.get_positionals().count() > 0
                {
                    text.push_str(&entry(name, command));
                }
                for child in command
                    .get_subcommands()
                    .filter(|child| !child.is_hide_set())
                {
                    entries(&format!("{name} {}", child.get_name()), child, &mut text);
                }
            }
        }
    }
    text
}

#[test]
fn command_reference_matches_the_command_tree() {
    let expected = render();
    let actual = read(REFERENCE);
    if actual != expected {
        let line = actual
            .lines()
            .zip(expected.lines())
            .position(|(actual, expected)| actual != expected)
            .unwrap_or_else(|| actual.lines().count().min(expected.lines().count()))
            + 1;
        panic!(
            "skills/lovelycarve-cli/{REFERENCE} differs from the command tree from line {line}; \
             run `just update-cli-skill`"
        );
    }
}

#[test]
#[ignore = "rewrites the skill's command reference; run `just update-cli-skill`"]
fn update_command_reference() {
    let path = skill_file(REFERENCE);
    std::fs::write(&path, render()).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}

#[test]
fn reference_lists_every_runnable_command() {
    let reference = render();
    let all = super::schema::describe(&[]).unwrap();
    for command in all["commands"].as_array().unwrap() {
        let path = command["command"].as_str().unwrap();
        let entry = format!("- `{path}");
        assert!(
            reference.lines().any(|line| line
                .strip_prefix(&entry)
                .is_some_and(|rest| rest.starts_with([' ', '`']))),
            "{path} is not in the reference"
        );
    }
}

/// The words of a batch request's argv, `$ref` values replaced by an
/// address-shaped placeholder.
fn argv_words(request: &Value) -> Vec<String> {
    request["argv"]
        .as_array()
        .unwrap_or_else(|| panic!("no argv in {request}"))
        .iter()
        .map(|word| match word {
            Value::String(text) => text.clone(),
            Value::Object(object) if object.contains_key("$ref") => "/ref".to_owned(),
            other => other.to_string(),
        })
        .collect()
}

#[test]
fn skill_examples_parse_as_commands() {
    let mut examples = 0;
    for page in PAGES {
        let text = read(page);
        for line in text
            .lines()
            .filter(|line| line.starts_with("lovelycarve-cli "))
        {
            let mut words = shell::tokenize(line)
                .unwrap_or_else(|error| panic!("{page}: {line}: {}", error.message));
            // A redirection or a pipe ends the command.
            if let Some(end) = words
                .iter()
                .position(|word| matches!(word.as_str(), ">" | ">>" | "2>" | "<" | "|"))
            {
                words.truncate(end);
            }
            if let Err(error) = super::runtime::parse(&words[1..]) {
                panic!("{page}: {line}: {}", error.message);
            }
            examples += 1;
        }
        for line in text.lines().filter(|line| line.starts_with("{\"id\":")) {
            let request: Value = serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("{page}: {line}: {error}"));
            if let Err(error) = super::runtime::parse(&argv_words(&request)) {
                panic!("{page}: {line}: {}", error.message);
            }
            examples += 1;
        }
    }
    assert!(
        examples > 0,
        "the skill must contain executable command examples"
    );
}
