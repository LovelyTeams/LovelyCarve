//! `shell`: one command per line, with a working directory inside the
//! workspace. `cd`, `pwd`, `help` and `exit` belong to the shell; every other
//! line is an ordinary command line whose relative addresses start from the
//! shell's directory. With a case backend running, lines go to it; without a
//! terminal the shell runs a script and stops at the first failure.
use std::io::{IsTerminal, Write};

use super::{
    cmd::{browse::StatArgs, CaseCommand, Command, Globals},
    exec::{Exec, RequestOptions},
    help,
    incomplete::EXIT_INCOMPLETE,
    output,
    progress::{Options, Reporter},
    types::{self, Address},
    CliError, Output,
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

/// The directory `cd target` leads to from `directory`. Like a shell's
/// logical `cd`, `..` goes up one level of the path as typed.
pub fn change(directory: &str, target: &str) -> String {
    let mut parts: Vec<String> = if target.starts_with('/') {
        Vec::new()
    } else {
        directory
            .split('/')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    };
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            name => parts.push(name.to_owned()),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Where the shell's commands run.
pub enum Runner<'a> {
    /// In this process.
    Local(&'a Exec),
    /// In the backend of a case directory.
    Remote(String),
}

impl Runner<'_> {
    async fn run(
        &self,
        command: Command,
        options: RequestOptions,
        globals: &Globals,
    ) -> Result<i32, CliError> {
        match self {
            Self::Local(exec) => {
                let executed = exec.run(command.clone(), &options).await?;
                super::runtime::deliver(exec, &command, executed, globals).await
            }
            Self::Remote(dir) => {
                let reporter = Reporter::new(Options {
                    quiet: globals.quiet,
                    events: globals.events,
                });
                let mut monitor = reporter.start(command.path());
                let result = super::runtime::forwarded(dir, &command, options, &reporter, globals);
                monitor.finish(&result);
                super::runtime::present(&command, result?, globals)
            }
        }
    }

    /// The JSON a query returns, without printing it.
    async fn query(&self, command: Command) -> Result<serde_json::Value, CliError> {
        let output = match self {
            Self::Local(exec) => exec.run(command, &RequestOptions::default()).await?.output,
            Self::Remote(dir) => super::backend::forward_into(
                dir,
                &command,
                RequestOptions::default(),
                None,
                &mut std::io::sink(),
            )?,
        };
        match output {
            Output::Json(json) => Ok(json.value().clone()),
            _ => Err(super::internal_error("查询返回了非 JSON 结果")),
        }
    }
}

/// Run one line; the exit status of the command it ran.
async fn line(
    runner: &Runner<'_>,
    directory: &mut String,
    words: Vec<String>,
    globals: &Globals,
) -> Result<i32, CliError> {
    match words.first().map(String::as_str) {
        Some("pwd") if words.len() == 1 => {
            println!("{directory}");
            return Ok(0);
        }
        Some("cd") if words.len() <= 2 => {
            let target = change(directory, words.get(1).map_or("/", String::as_str));
            if target != "/" {
                let node = runner
                    .query(Command::Stat(StatArgs {
                        path: Address::new(target.clone()),
                        content: false,
                    }))
                    .await?;
                if node["kind"] != "dir" {
                    return Err(CliError::invalid(format!("{target} 不是目录")));
                }
            }
            *directory = target;
            return Ok(0);
        }
        Some("help") => {
            print!("{}", help::render(&words[1..]));
            return Ok(0);
        }
        _ => {}
    }
    let cli = types::with_shell_directory(Some(directory), || super::runtime::parse(&words))?;
    let line_globals = cli.globals;
    if line_globals.case.is_some() || !line_globals.images.is_empty() {
        return Err(CliError::new(
            "usage",
            "shell 中不能用 -C / --image：案件在启动 shell 时指定，检材用 evidence add 打开",
        ));
    }
    let Some(mut command) = cli.command else {
        return Ok(0);
    };
    // Where a command would start from the workspace root, the shell starts
    // from its directory, as ls(1) and grep -r do.
    let here = || Some(Address::new(directory.clone()));
    match &mut command {
        Command::Ls(args) if args.path.is_none() => args.path = here(),
        Command::Grep(args) if args.path.is_none() => args.path = here(),
        Command::Find(args) if args.under.is_none() && directory != "/" => args.under = here(),
        _ => {}
    }
    if matches!(
        command,
        Command::Shell
            | Command::Batch(_)
            | Command::Case(super::cmd::case::CaseArgs {
                command: Some(CaseCommand::Open(_) | CaseCommand::Close(_) | CaseCommand::Serve(_))
            })
    ) {
        return Err(CliError::new(
            "usage",
            format!("{} 不能在 shell 中运行", command.path()),
        ));
    }
    let json = line_globals.json.or_else(|| globals.json.clone());
    let globals = Globals {
        quiet: line_globals.quiet || globals.quiet || json.is_some(),
        json,
        tz: line_globals.tz.or_else(|| globals.tz.clone()),
        yes: line_globals.yes,
        dry_run: line_globals.dry_run,
        events: globals.events,
        ..Globals::default()
    };
    output::set_json_mode(globals.json.is_some());
    super::runtime::check_projection(&command, &globals)?;
    if let Some(code) = super::runtime::local(&command, &globals)? {
        return Ok(code);
    }
    if globals.dry_run {
        output::print_text(&output::format_json(&super::Json::from(
            super::runtime::dry_run(&command)?,
        )))?;
        return Ok(0);
    }
    for (name, secret) in command.secrets_mut() {
        secret.resolve(name, std::io::stdin().is_terminal())?;
    }
    let options = RequestOptions {
        tz: globals.tz.clone(),
        yes: globals.yes,
    };
    runner.run(command, options, &globals).await
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

/// The shell's status: a script stops at the first failure, and one that
/// produced an incomplete result ends with status 2; a person at a terminal
/// sees every report and ends with 0.
pub async fn run_with(runner: Runner<'_>, globals: &Globals) -> Result<i32, CliError> {
    let interactive = std::io::stdin().is_terminal();
    let mut incomplete = false;
    let mut directory = "/".to_owned();
    if interactive {
        eprintln!(
            "{}",
            output::hint("输入命令（与命令行相同，例如 ls、cd /<别名>/part:0、cat 文件），help 查看命令，exit 退出")
        );
    }
    let mut input = LineReader::spawn()?;
    loop {
        if interactive {
            eprint!("lovelycarve:{}> ", output::sanitize(&directory));
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
            Ok(words) => words,
            Err(error) => {
                if !interactive {
                    return Err(error);
                }
                output::report_error(&error);
                continue;
            }
        };
        match line(&runner, &mut directory, words, globals).await {
            Ok(code) => incomplete |= code == EXIT_INCOMPLETE,
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

pub async fn run(exec: &Exec, globals: &Globals) -> Result<i32, CliError> {
    run_with(Runner::Local(exec), globals).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_preserves_windows_paths_and_json() {
        assert_eq!(
            tokenize(r#"evidence add "D:\Evidence Files\disk.raw""#).unwrap(),
            ["evidence", "add", "D:\\Evidence Files\\disk.raw"]
        );
        assert_eq!(
            tokenize(r#"rpc fs_size_tree --body '{"imageId":"nas"}'"#).unwrap(),
            ["rpc", "fs_size_tree", "--body", r#"{"imageId":"nas"}"#]
        );
        assert!(tokenize("evidence add 'missing").is_err());
        assert_eq!(tokenize("cd ''").unwrap(), ["cd", ""]);
    }

    #[test]
    fn cd_moves_through_the_path_as_typed() {
        assert_eq!(change("/", "nas"), "/nas");
        assert_eq!(change("/nas/part:0/Users", ".."), "/nas/part:0");
        assert_eq!(change("/nas", "../other/part:1"), "/other/part:1");
        assert_eq!(change("/nas/part:0", "/"), "/");
        assert_eq!(change("/nas", "./part:0/./etc/"), "/nas/part:0/etc");
        assert_eq!(change("/", ".."), "/");
    }
}
