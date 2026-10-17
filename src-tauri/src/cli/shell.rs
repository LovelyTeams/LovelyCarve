//! `shell`: one command per line against a process that keeps the case and
//! images open. `cd`, `pwd` and `exit` are shell conveniences; everything else
//! is the ordinary command grammar. Without a terminal it runs a script and
//! stops at the first failure.
use std::io::{IsTerminal, Write};

use super::{
    argv::{self, Parsed},
    catalog,
    context::{Context, Exec, Request},
    output, CliError,
};

/// Split a line into words. Quotes group words; backslashes in Windows paths
/// stay literal; nothing is expanded or executed.
pub fn tokenize(line: &str) -> Result<Vec<String>, CliError> {
    let mut result = Vec::new();
    let mut token = String::new();
    let mut quote = None;
    let mut started = false;
    for character in line.chars() {
        if let Some(delimiter) = quote {
            if character == delimiter {
                quote = None;
            } else {
                token.push(character);
            }
        } else if character == '\'' || character == '"' {
            quote = Some(character);
            started = true;
        } else if character.is_whitespace() {
            if started {
                result.push(std::mem::take(&mut token));
                started = false;
            }
        } else {
            token.push(character);
            started = true;
        }
    }
    if quote.is_some() {
        return Err(CliError::invalid("引号未闭合"));
    }
    if started {
        result.push(token);
    }
    Ok(result)
}

async fn line(exec: &Exec, context: &mut Context, words: Vec<String>) -> Result<(), CliError> {
    let catalog = catalog::catalog()?;
    match words.first().map(String::as_str) {
        Some("pwd") if words.len() == 1 => {
            println!("{}", context.pwd());
            return Ok(());
        }
        Some("cd") if words.len() == 2 => {
            let command = catalog
                .find(&["context", "set"])
                .ok_or_else(|| super::internal_error("缺少 context set 命令"))?;
            let values = serde_json::Map::from_iter([("dir".to_owned(), words[1].clone().into())]);
            exec.execute(
                context,
                Request {
                    command,
                    values,
                    ask: Vec::new(),
                    force: false,
                    dry_run: false,
                    text: false,
                },
            )
            .await?;
            return Ok(());
        }
        _ => {}
    }
    match argv::parse(catalog, &words) {
        Parsed::Run {
            globals,
            invocation,
        } => {
            if globals.case.is_some() {
                return Err(CliError::invalid(
                    "--case 只能在启动时使用；会话中请用 cases open",
                ));
            }
            if matches!(
                invocation.command.native.as_deref(),
                Some("session" | "shell" | "complete")
            ) {
                return Err(CliError::invalid("不能在 shell 中嵌套会话"));
            }
            let mut values = invocation.values;
            if let (Some(image), Some(param)) = (
                globals.image,
                invocation.command.params.iter().find(|param| param.scope),
            ) {
                values.entry(param.name.clone()).or_insert(image.into());
            }
            let (_, done) = invocation.command.category.labels();
            let output = exec
                .execute(
                    context,
                    Request {
                        command: invocation.command,
                        values,
                        ask: invocation.ask,
                        force: invocation.force,
                        dry_run: invocation.dry_run,
                        text: invocation.text,
                    },
                )
                .await?;
            output::print_output(output, done, globals.quiet, invocation.text)
        }
        Parsed::Help(help) => {
            print!("{help}");
            Ok(())
        }
        Parsed::UsageError { error, .. } => Err(error),
        Parsed::Splash | Parsed::Version { .. } | Parsed::Complete(_) => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }
}

/// Reads one line of stdin when asked, on its own thread: a pending read never
/// holds up Ctrl+C handling, and nothing reads the console while a command
/// runs, so the command's own prompts get the keyboard.
struct LineReader {
    requests: std::sync::mpsc::Sender<()>,
    lines: tokio::sync::mpsc::Receiver<std::io::Result<Option<String>>>,
}

impl LineReader {
    fn spawn() -> Result<Self, CliError> {
        let (requests, wanted) = std::sync::mpsc::channel::<()>();
        let (sender, lines) = tokio::sync::mpsc::channel(1);
        std::thread::Builder::new()
            .name("lovelycarve-cli-shell-input".into())
            .spawn(move || {
                let stdin = std::io::stdin();
                while wanted.recv().is_ok() {
                    let mut text = String::new();
                    let line = stdin
                        .read_line(&mut text)
                        .map(|bytes| (bytes != 0).then_some(text));
                    if sender.blocking_send(line).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self { requests, lines })
    }

    async fn next(&mut self) -> Result<Option<String>, CliError> {
        self.requests
            .send(())
            .map_err(|_| super::internal_error("shell 输入线程已退出"))?;
        match self.lines.recv().await {
            Some(line) => Ok(line?),
            None => Ok(None),
        }
    }
}

pub async fn run(exec: &Exec, context: &mut Context) -> Result<i32, CliError> {
    let interactive = std::io::stdin().is_terminal();
    if interactive {
        eprintln!(
            "{}",
            output::hint(
                "输入命令（与命令行相同，例如 images open <路径>），help 查看命令，exit 退出"
            )
        );
    }
    let mut input = LineReader::spawn()?;
    loop {
        if interactive {
            eprint!("lovelycarve:{}> ", output::sanitize(&context.pwd()));
            std::io::stderr().flush()?;
        }
        let Some(text) = input.next().await? else {
            if interactive {
                eprintln!();
            }
            break;
        };
        let text = text.trim_start_matches('\u{feff}').trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        if matches!(text, "exit" | "quit") {
            break;
        }
        let words = match tokenize(text) {
            Ok(words) if words == ["help"] => vec!["--help".to_owned()],
            Ok(words) => words,
            Err(error) => {
                if !interactive {
                    return Err(error);
                }
                output::report_error(&error);
                continue;
            }
        };
        if let Err(error) = line(exec, context, words).await {
            if !interactive {
                return Err(error);
            }
            output::report_error(&error);
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_preserves_windows_paths_and_json() {
        assert_eq!(
            tokenize(r#"images open "D:\Evidence Files\disk.raw""#).unwrap(),
            ["images", "open", "D:\\Evidence Files\\disk.raw"]
        );
        assert_eq!(
            tokenize(r#"carve scans create --options '{"scope":"whole"}'"#).unwrap(),
            [
                "carve",
                "scans",
                "create",
                "--options",
                r#"{"scope":"whole"}"#
            ]
        );
        assert!(tokenize("images open 'missing").is_err());
        assert_eq!(tokenize("cd ''").unwrap(), ["cd", ""]);
    }
}
