//! `schema`: a command's arguments, options and result as JSON, read from
//! the same clap tree that parses the command line. `schema` alone lists
//! every command. The JSON keys are the ones a batch request's argv and the
//! `--json` results use; option names are kebab-case on the command line.
use clap::{Arg, CommandFactory};
use serde_json::{json, Value};

use super::{
    cmd::{meta_of, Cli, OutputKind},
    CliError,
};

const BINARY: &str = "lovelycarve-cli";

fn values(arg: &Arg) -> Vec<String> {
    arg.get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| value.get_name().to_owned())
        .collect()
}

fn defaults(arg: &Arg) -> Value {
    let defaults: Vec<String> = arg
        .get_default_values()
        .iter()
        .map(|value| value.to_string_lossy().into_owned())
        .collect();
    match defaults.as_slice() {
        [] => Value::Null,
        [one] => json!(one),
        many => json!(many),
    }
}

fn describe_arg(arg: &Arg) -> Value {
    let takes_value = arg.get_action().takes_values();
    let mut value = json!({
        "help": arg.get_help().map(ToString::to_string).unwrap_or_default(),
        "required": arg.is_required_set(),
    });
    if arg.is_positional() {
        value["name"] = json!(arg
            .get_value_names()
            .and_then(|names| names.first().map(ToString::to_string))
            .unwrap_or_else(|| arg.get_id().to_string()));
        value["multiple"] = json!(arg
            .get_num_args()
            .is_some_and(|range| range.max_values() > 1));
    } else {
        value["flag"] = json!(arg.get_long().map(|long| format!("--{long}")));
        value["short"] = json!(arg.get_short().map(|short| format!("-{short}")));
        value["takesValue"] = json!(takes_value);
        if takes_value {
            value["valueName"] = json!(arg
                .get_value_names()
                .and_then(|names| names.first().map(ToString::to_string)));
            value["optionalValue"] = json!(arg
                .get_num_args()
                .is_some_and(|range| range.min_values() == 0));
            value["repeatable"] = json!(matches!(arg.get_action(), clap::ArgAction::Append));
        }
    }
    let choices = values(arg);
    if !choices.is_empty() && takes_value {
        value["values"] = json!(choices);
    }
    let default = defaults(arg);
    if !default.is_null() {
        value["default"] = default;
    }
    value
}

/// Every runnable command path, depth first.
fn paths(command: &clap::Command, prefix: &[String], out: &mut Vec<(String, String)>) {
    for child in command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
    {
        let mut path = prefix.to_vec();
        path.push(child.get_name().to_owned());
        let about = child
            .get_about()
            .map(ToString::to_string)
            .unwrap_or_default();
        if child.has_subcommands() {
            if !child.is_subcommand_required_set() {
                out.push((path.join(" "), about.clone()));
            }
            paths(child, &path, out);
        } else {
            out.push((path.join(" "), about));
        }
    }
}

pub fn describe(words: &[String]) -> Result<Value, CliError> {
    let root = Cli::command();
    if words.is_empty() {
        let mut list = Vec::new();
        paths(&root, &[], &mut list);
        return Ok(json!({
            "commands": list.iter().map(|(path, about)| json!({"command": path, "about": about})).collect::<Vec<_>>(),
            "globalOptions": root.get_arguments().filter(|arg| arg.is_global_set() && !arg.is_hide_set()).map(describe_arg).collect::<Vec<_>>(),
        }));
    }
    let mut command = &root;
    for word in words {
        command = command.find_subcommand(word).ok_or_else(|| {
            CliError::new(
                "usage",
                format!(
                    "没有命令 {}；用 {BINARY} schema 列出全部命令",
                    words.join(" ")
                ),
            )
        })?;
    }
    let path = words.join(" ");
    let subcommands: Vec<Value> = command
        .get_subcommands()
        .filter(|child| !child.is_hide_set())
        .map(|child| json!({"command": format!("{path} {}", child.get_name()), "about": child.get_about().map(ToString::to_string)}))
        .collect();
    let meta = meta_of(&path);
    let mut value = json!({
        "command": path,
        "about": command.get_long_about().or_else(|| command.get_about()).map(ToString::to_string),
        "arguments": command.get_positionals().filter(|arg| !arg.is_hide_set()).map(describe_arg).collect::<Vec<_>>(),
        "options": command.get_arguments().filter(|arg| !arg.is_positional() && !arg.is_hide_set() && !arg.is_global_set()).map(describe_arg).collect::<Vec<_>>(),
        "output": match meta.output {
            OutputKind::Json => "json",
            OutputKind::Bytes => "bytes",
        },
        "concurrent": meta.concurrent,
        "confirm": meta.confirm,
        "incompleteFields": meta.incomplete,
        "batchRequest": {"id": "1", "argv": path.split(' ').collect::<Vec<_>>()},
    });
    if !subcommands.is_empty() {
        value["subcommands"] = json!(subcommands);
    }
    if path == "rpc" {
        value["operations"] = super::cmd::system::operations()?;
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_describes_arguments_options_and_results() {
        let all = describe(&[]).unwrap();
        let commands: Vec<&str> = all["commands"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|command| command["command"].as_str())
            .collect();
        for expected in ["ls", "case status", "sqlite query", "evidence add"] {
            assert!(commands.contains(&expected), "{expected}");
        }
        assert!(!commands.contains(&"case serve"));
        let ls = describe(&["ls".into()]).unwrap();
        assert_eq!(ls["output"], "json");
        assert_eq!(ls["arguments"][0]["name"], "路径");
        assert!(ls["options"]
            .as_array()
            .unwrap()
            .iter()
            .any(|option| option["flag"] == "--limit" && option["default"] == "200"));
        let cat = describe(&["cat".into()]).unwrap();
        assert_eq!(cat["output"], "bytes");
        let hash = describe(&["hash".into()]).unwrap();
        assert_eq!(hash["incompleteFields"][0], "items.readFaultsDetected");
        let rm = describe(&["case".into(), "rm".into()]).unwrap();
        assert!(rm["confirm"].is_string());
        assert!(describe(&["nope".into()]).is_err());
    }
}
