//! The process: parse, answer what needs no evidence directly, otherwise start
//! the backend (state, plugins, event sink) on a dedicated thread with its own
//! tokio runtime, run the command, wait for its background work and shut down.
//! No desktop runtime, window or WebView is ever created.
use std::{
    io::{IsTerminal, Write},
    sync::Arc,
};

use serde_json::{json, Map, Value};

use super::{
    argv::{self, Globals, Invocation, Parsed},
    catalog::{self, Catalog},
    complete,
    context::{self, Context, Exec, Mode, Request},
    dotenv, generated, interactive, native,
    output::{self, Role},
    session, shell, CliError, Output,
};
use crate::{host::AppHandle, state::AppState, task_manager::TaskKind};

pub fn run() -> i32 {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let catalog = match catalog::catalog() {
        Ok(catalog) => catalog,
        Err(error) => {
            output::report_error(&error);
            return error.exit_code;
        }
    };
    match argv::parse(catalog, &args) {
        Parsed::Splash => {
            splash(catalog);
            0
        }
        Parsed::Version { quiet } => {
            output::open_session(quiet);
            println!("{}", env!("CARGO_PKG_VERSION"));
            0
        }
        Parsed::Help(help) => {
            output::open_session(false);
            completions_hint(false);
            print!("{help}");
            0
        }
        Parsed::UsageError { help, error } => {
            output::open_session(false);
            if let Some(help) = help {
                eprint!("{help}");
            }
            output::report_error(&error);
            error.exit_code
        }
        Parsed::Complete(words) => {
            let mut stdout = std::io::stdout().lock();
            for (value, description) in complete::candidates(catalog, &words) {
                if writeln!(stdout, "{value}\t{}", output::sanitize(&description)).is_err() {
                    return 1;
                }
            }
            0
        }
        Parsed::Run {
            globals,
            invocation,
        } => match command(catalog, globals, invocation) {
            Ok(code) => code,
            Err(error) => {
                output::report_error(&error);
                error.exit_code
            }
        },
    }
}

fn splash(catalog: &Catalog) {
    let binary = &catalog.binary;
    let line = |command: &str, description: &str| {
        let text = format!("{binary} {command}");
        let padding = " ".repeat(48usize.saturating_sub(output::display_width(&text)));
        format!(
            "  {}{padding} {}",
            output::paint(Role::Code, &text),
            output::paint(Role::Muted, description)
        )
    };
    let lines = [
        output::headline(),
        String::new(),
        "LovelyCarve 取证命令行：打开检材、浏览与解析文件、搜索、恢复与导出。".into(),
        String::new(),
        output::bold("开始使用："),
        line("images open <检材路径>", "打开检材"),
        line("files list --path / --image <检材路径>", "浏览检材"),
        line("cli search \"<要完成的任务>\"", "按意图查找命令"),
        line("docs search \"<问题>\"", "搜索内置文档"),
        line("--help", "浏览全部命令"),
        line("<命令> --help", "查看单个命令"),
        line("complete powershell", "安装 shell 补全"),
        String::new(),
        format!(
            "{}  {}",
            output::bold("文档："),
            output::paint(Role::Brand, "https://github.com/LovelyTeams/LovelyCarve")
        ),
        String::new(),
    ];
    println!("{}", lines.join("\n"));
}

/// A one-time tip about shell completion on the first run at a terminal.
fn completions_hint(quiet: bool) {
    if quiet || !std::io::stderr().is_terminal() {
        return;
    }
    let Some(path) = crate::host::default_app_data_dir().map(|dir| dir.join("cli-state.json"))
    else {
        return;
    };
    let mut state: Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_else(|| json!({}));
    if state["completions"]["prompted"] == true {
        return;
    }
    eprintln!(
        "{}",
        output::paint(
            Role::Muted,
            "提示：运行 `lovelycarve-cli complete <bash|zsh|fish|powershell>` 安装 shell 补全。"
        )
    );
    state["completions"] = json!({"prompted": true});
    let saved = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&path, state.to_string()));
    if let Err(error) = saved {
        tracing::debug!("save CLI state: {error}");
    }
}

fn print(output: Output, invocation: &Invocation, quiet: bool) -> Result<i32, CliError> {
    let (_, done) = invocation.command.category.labels();
    output::print_output(output, done, quiet, invocation.text)?;
    Ok(0)
}

fn command(
    catalog: &'static Catalog,
    globals: Globals,
    invocation: Invocation,
) -> Result<i32, CliError> {
    let native = invocation.command.native.as_deref();
    // Completion output must stay parseable: no banner, no hint.
    if native == Some("complete") {
        if invocation.dry_run {
            let preview = context::preview(invocation.command, &invocation.values, None)?;
            return print(Output::Json(preview), &invocation, true);
        }
        return match invocation.values.get("shell").and_then(Value::as_str) {
            Some(shell) => {
                let script = complete::script(&catalog.binary, shell)
                    .ok_or_else(|| CliError::invalid(format!("不支持的 shell：{shell}")))?;
                print!("{script}");
                Ok(0)
            }
            None => {
                eprintln!(
                    "用法：{0} complete <bash|zsh|fish|powershell>\n       {0} complete bash >> ~/.bashrc",
                    catalog.binary
                );
                Ok(1)
            }
        };
    }
    output::open_session(globals.quiet);
    completions_hint(globals.quiet);
    if invocation.dry_run {
        let scope = globals
            .image
            .clone()
            .or_else(|| dotenv::variable("LOVELYCARVE_IMAGE"));
        let preview = context::preview(invocation.command, &invocation.values, scope.as_deref())?;
        return print(Output::Json(preview), &invocation, globals.quiet);
    }
    if let Some(output) = native::offline(invocation.command, &invocation.values, Mode::OneShot) {
        return print(output?, &invocation, globals.quiet);
    }
    match native {
        Some("session") => {
            let jobs = invocation
                .values
                .get("jobs")
                .and_then(Value::as_u64)
                .unwrap_or(1);
            if !(1..=64).contains(&jobs) {
                return Err(CliError::new("usage", "--jobs 的范围是 1–64"));
            }
            let events = invocation.values.get("events") == Some(&Value::Bool(true));
            Ok(backend(
                globals,
                Work::Session {
                    jobs: jobs as usize,
                    events,
                },
            ))
        }
        Some("shell") => Ok(backend(globals, Work::Shell)),
        _ => Ok(backend(globals, Work::OneShot(invocation))),
    }
}

enum Work {
    OneShot(Invocation),
    Session { jobs: usize, events: bool },
    Shell,
}

fn backend(globals: Globals, work: Work) -> i32 {
    // Deep parser recursion needs more than the default main-thread stack.
    let thread = std::thread::Builder::new()
        .name("lovelycarve-cli".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .thread_name("lovelycarve-cli-worker")
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    output::report_error(&super::internal_error(error));
                    return 1;
                }
            };
            let code = runtime.block_on(serve(globals, work));
            // Cancelled tasks clean up within `serve`; a blocking read that
            // has no cancellation (a stream to stdout) must not hold the exit.
            runtime.shutdown_timeout(std::time::Duration::from_secs(10));
            code
        });
    match thread.map(std::thread::JoinHandle::join) {
        Ok(Ok(code)) => code,
        Ok(Err(_)) => {
            output::report_error(&super::internal_error("CLI 工作线程异常退出"));
            1
        }
        Err(error) => {
            output::report_error(&super::internal_error(error));
            1
        }
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    // A subscriber may already be installed, as in tests.
    if let Err(error) = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(filter)
        .try_init()
    {
        tracing::debug!("tracing already initialised: {error}");
    }
}

/// Events go to stderr as JSONL with `session --events`, and nowhere otherwise.
fn host(events: bool) -> AppHandle {
    AppHandle::new(
        crate::host::default_app_data_dir(),
        move |event, payload| {
            if !events {
                return Ok(());
            }
            let line = format!(
                "{{\"event\":{},\"payload\":{payload}}}",
                serde_json::to_string(event).map_err(|error| error.to_string())?
            );
            let mut stderr = std::io::stderr().lock();
            writeln!(stderr, "{line}").map_err(|error| error.to_string())
        },
    )
}

async fn serve(globals: Globals, work: Work) -> i32 {
    init_tracing();
    let events = matches!(work, Work::Session { events: true, .. });
    let host = host(events);
    let state = Arc::new(AppState::new());
    if let Err(error) = crate::app::setup::initialize(&state, &host) {
        output::report_error(&super::internal_error(format!("初始化失败：{error}")));
        return 1;
    }
    let mode = match work {
        Work::OneShot(_) => Mode::OneShot,
        Work::Session { .. } => Mode::Session,
        Work::Shell => Mode::Shell,
    };
    let exec = Exec {
        host,
        state: Arc::clone(&state),
        mode,
        quiet: globals.quiet,
    };
    let (scope, scope_source) = match globals.image.clone() {
        Some(image) => (Some(image), None),
        None => match dotenv::lookup("LOVELYCARVE_IMAGE") {
            Some((image, source)) => (Some(image), Some(source.describe("LOVELYCARVE_IMAGE"))),
            None => (None, None),
        },
    };
    let mut context = Context {
        scope,
        scope_source,
        ..Context::default()
    };
    let case = match globals.case.clone() {
        Some(dir) => Some((dir, "--case".to_owned())),
        None => dotenv::lookup("LOVELYCARVE_CASE")
            .map(|(dir, source)| (dir, source.describe("LOVELYCARVE_CASE"))),
    };
    let outcome = tokio::select! {
        outcome = perform(&exec, &mut context, case, work) => outcome,
        () = interrupted() => {
            for task in state.tasks.list() {
                state.tasks.cancel(&task.id);
            }
            Err(CliError::exit(130))
        }
    };
    let code = match outcome {
        Ok(code) => code,
        Err(error) => {
            output::report_error(&error);
            error.exit_code
        }
    };
    shutdown(&state).await;
    code
}

/// The case to open before the command, with where it was given; announced
/// under `DEBUG`, as cf announces the account.
async fn perform(
    exec: &Exec,
    context: &mut Context,
    case: Option<(String, String)>,
    work: Work,
) -> Result<i32, CliError> {
    if let Some((dir, source)) = case {
        if interactive::debug() {
            eprintln!("使用案件：{}（来自 {source}）", output::sanitize(&dir));
        }
        let args = Map::from_iter([("dir".to_owned(), json!(dir))]);
        generated::dispatch(&exec.host, &exec.state, "case_open", args).await?;
    }
    let code = match work {
        Work::OneShot(invocation) => {
            let output = exec
                .execute(
                    context,
                    Request {
                        command: invocation.command,
                        values: invocation.values.clone(),
                        ask: invocation.ask.clone(),
                        force: invocation.force,
                        dry_run: false,
                        text: invocation.text,
                    },
                )
                .await?;
            print(output, &invocation, exec.quiet)?
        }
        Work::Session { jobs, .. } => {
            let failed = session::json_lines(exec.clone(), context, jobs).await?;
            i32::from(failed)
        }
        Work::Shell => shell::run(exec, context).await?,
    };
    finish(&exec.state).await?;
    Ok(code)
}

/// Before exit: indexes of caseless images are process-local and discarded
/// anyway, so stop building them; then wait for every other task.
async fn finish(state: &AppState) -> Result<(), CliError> {
    state.tasks.cancel_kind(TaskKind::SessionSnapshot);
    native::wait_idle(state).await
}

async fn interrupted() {
    #[cfg(windows)]
    {
        use tokio::signal::windows::{ctrl_break, ctrl_c};
        match (ctrl_c(), ctrl_break()) {
            (Ok(mut interrupt), Ok(mut console_break)) => {
                tokio::select! {
                    _ = interrupt.recv() => {}
                    _ = console_break.recv() => {}
                }
            }
            (Ok(mut interrupt), Err(_)) => {
                interrupt.recv().await;
            }
            (Err(error), _) => {
                tracing::warn!("Ctrl+C handler unavailable: {error}");
                std::future::pending::<()>().await;
            }
        }
    }
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        match (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
        ) {
            (Ok(mut interrupt), Ok(mut terminate)) => {
                tokio::select! {
                    _ = interrupt.recv() => {}
                    _ = terminate.recv() => {}
                }
            }
            _ => std::future::pending::<()>().await,
        }
    }
}

async fn shutdown(state: &AppState) {
    for task in state.tasks.list() {
        state.tasks.cancel(&task.id);
    }
    if let Err(error) =
        crate::app::workflows::casework::case_close(crate::host::State::new(state)).await
    {
        tracing::error!("CLI case close: {}", error.message);
    }
    crate::app::lifecycle::shutdown_plugins(state).await;
    crate::app::lifecycle::release_media(state).await;
}
