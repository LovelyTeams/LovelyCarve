//! A terminal or line-oriented script attached to an existing session.
use std::io::{IsTerminal, Write};

use serde_json::{json, Map};

use super::client;
use crate::cli::{
    argv::{self, Parsed},
    catalog,
    incomplete::EXIT_INCOMPLETE,
    output, shell, CliError,
};

/// Like the shell: a script stops at the first failure and ends with
/// status 2 when a result was incomplete.
pub(super) fn run(name: &str, timezone: Option<&str>) -> Result<i32, CliError> {
    client::info(name)?;
    let interactive = std::io::stdin().is_terminal();
    let mut incomplete = false;
    if interactive {
        eprintln!("{}", output::hint("已连接常驻会话；输入普通命令、cd、pwd，exit 断开连接，sessions close 可关闭后台会话"));
    }
    loop {
        if interactive {
            eprint!("lovelycarve:{}> ", output::sanitize(name));
            std::io::stderr().flush()?;
        }
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            break;
        }
        let line = line.trim_start_matches('\u{feff}').trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if matches!(line, "exit" | "quit" | "detach") {
            break;
        }
        match execute_line(name, line, timezone) {
            Ok(partial) => incomplete |= partial,
            Err(error) if !interactive => return Err(error),
            Err(error) => output::report_error(&error),
        }
    }
    Ok(if incomplete && !interactive {
        EXIT_INCOMPLETE
    } else {
        0
    })
}

/// Runs one line; `true` when its result was incomplete (already reported).
fn execute_line(name: &str, line: &str, timezone: Option<&str>) -> Result<bool, CliError> {
    let mut words = shell::tokenize(line)?;
    match words.first().map(String::as_str) {
        Some("cd") if words.len() == 2 || (words.len() == 3 && words[1] == "--node-id") => {
            let (parameter, value) = if words.len() == 3 {
                ("nodeId", &words[2])
            } else {
                ("dir", &words[1])
            };
            client::exchange(
                name,
                "context set",
                Map::from_iter([(parameter.into(), json!(value))]),
                false,
                false,
                false,
            )?;
            return Ok(false);
        }
        Some("pwd") if words.len() == 1 => {
            let info = client::info(name)?;
            println!("{}", info["context"]["dir"].as_str().unwrap_or("/"));
            return Ok(false);
        }
        Some("help") if words.len() == 1 => words = vec!["--help".into()],
        _ => {}
    }
    match argv::parse(catalog::catalog()?, &words) {
        Parsed::Run {
            mut globals,
            invocation,
        } => {
            if !std::io::stdin().is_terminal() && !invocation.ask.is_empty() {
                return Err(CliError::invalid(
                    "连接脚本中的密码必须直接给值或用 @文件，不能从脚本标准输入询问",
                ));
            }
            if globals
                .session
                .as_deref()
                .is_some_and(|other| other != name)
            {
                return Err(CliError::invalid(
                    "attach 内不能切换命名会话；先 exit，再连接目标会话",
                ));
            }
            if globals.timezone.is_none() {
                globals.timezone = timezone.map(str::to_owned);
            }
            return Ok(client::forward(name, &globals, invocation)? == EXIT_INCOMPLETE);
        }
        Parsed::Help(help) => print!("{help}"),
        Parsed::UsageError { error, .. } => return Err(error),
        Parsed::Splash | Parsed::Version { .. } | Parsed::Complete(_) => {
            println!("{}", env!("CARGO_PKG_VERSION"))
        }
    }
    Ok(false)
}
