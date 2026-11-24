//! The process: parse, answer what needs no evidence directly, forward to a
//! case backend when one serves the case, otherwise start the backend state
//! (AppState, plugins, event sink) on a dedicated thread with its own tokio
//! runtime, run the command, wait for its background work and shut down. No
//! desktop runtime, window or WebView is ever created.
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::Parser;
use serde_json::{json, Value};

use super::{
    backend,
    background::Background,
    cmd::{self, CaseCommand, Cli, Command, Globals},
    exec::{Exec, Executed, Mode, RequestOptions},
    help,
    incomplete::{self, EXIT_INCOMPLETE},
    interactive, output,
    progress::{Options, Reporter},
    projection, render,
    types::HostPath,
    workspace::Workspace,
    CliError, Output,
};
use crate::{host::AppHandle, state::AppState, task_manager::TaskKind};

pub fn run() -> i32 {
    let args: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    match main(&args) {
        Ok(code) => code,
        Err(error) => {
            output::report_error(&error);
            error.exit_code
        }
    }
}

/// Parse `words` (without the program name).
pub fn parse(words: &[String]) -> Result<Cli, CliError> {
    Cli::try_parse_from(std::iter::once("lovelycarve-cli".to_owned()).chain(words.iter().cloned()))
        .map_err(|error| help::usage_error(words, &error))
}

fn main(args: &[String]) -> Result<i32, CliError> {
    if let Some(code) = help::intercept(args) {
        return Ok(code);
    }
    // A line that does not parse still fails in the form it asked for.
    output::set_json_mode(
        args.iter()
            .any(|arg| arg == "--json" || arg.starts_with("--json=")),
    );
    let cli = parse(args)?;
    let mut globals = cli.globals;
    // JSON is for programs: progress for people stays off (--events reports it).
    globals.quiet |= globals.json.is_some();
    output::set_json_mode(globals.json.is_some());
    let Some(mut command) = cli.command else {
        help::splash();
        return Ok(0);
    };
    check_projection(&command, &globals)?;
    if let Some(code) = local(&command, &globals)? {
        return Ok(code);
    }
    if globals.dry_run {
        output::open_session(true);
        print_json(&dry_run(&command)?, None)?;
        return Ok(0);
    }
    output::open_session(globals.quiet || globals.events);
    for (name, secret) in command.secrets_mut() {
        secret.resolve(name, !interactive::is_non_interactive_or_ci())?;
    }
    let case = case_dir(&globals, &command)?;
    let options = RequestOptions {
        tz: globals.tz.clone(),
        yes: globals.yes,
    };
    // Process management.
    if let Command::Case(cmd::case::CaseArgs {
        command:
            Some(
                case_command @ (CaseCommand::Open(_)
                | CaseCommand::Close(_)
                | CaseCommand::Serve(_)),
            ),
    }) = &command
    {
        return manage(case_command, case.as_deref(), &globals);
    }
    let running = match case.as_deref() {
        Some(dir) => match backend::state(dir)? {
            backend::State::Running => Some(dir.to_owned()),
            backend::State::Exited => return Err(backend_exited(dir)),
            backend::State::NotRunning => None,
        },
        None => None,
    };
    if let Command::Shell = command {
        return match running {
            Some(dir) => runtime()?.block_on(super::shell::run_with(
                super::shell::Runner::Remote(dir),
                &globals,
            )),
            None => start(globals, case, Work::Shell),
        };
    }
    if let Command::Batch(args) = &command {
        if !(1..=64).contains(&args.jobs) {
            return Err(CliError::new("usage", "--jobs 的范围是 1–64"));
        }
        return match running {
            Some(dir) => super::batch::run_client(dir, args.jobs),
            None => start(globals, case, Work::Batch { jobs: args.jobs }),
        };
    }
    if let Some(dir) = running.as_deref() {
        return forward(dir, &command, options, &globals);
    }
    let new_case = match &command {
        Command::Case(cmd::case::CaseArgs {
            command: Some(CaseCommand::New(new)),
        }) if new.open => Some(new.dir.text()),
        _ => None,
    };
    let code = start(
        globals.clone(),
        case,
        Work::OneShot {
            command: Box::new(command),
            options,
        },
    )?;
    if let (0, Some(dir)) = (code, new_case) {
        let info = backend::open(&dir)?;
        output::note(
            &format!(
                "案件后台进程已启动（pid {}）：之后指向该案件的命令复用已打开的检材",
                info["pid"]
            ),
            globals.quiet,
        );
    }
    Ok(code)
}

fn backend_exited(dir: &str) -> CliError {
    CliError::new(
        "caseBackendExited",
        format!(
            "案件 {dir} 的后台进程已退出，残留记录已清理；运行 case open 重新启动\
             （解锁、组装卷和挂载需要重新建立）"
        ),
    )
}

/// Commands that need neither the backend nor evidence.
pub(super) fn local(command: &Command, globals: &Globals) -> Result<Option<i32>, CliError> {
    match command {
        Command::Schema(args) => {
            print_json(&super::schema::describe(&args.words)?, None)?;
            Ok(Some(0))
        }
        Command::Completion(args) => {
            print!("{}", super::complete::script(args.shell));
            Ok(Some(0))
        }
        Command::Complete(args) => {
            let mut stdout = std::io::stdout().lock();
            for (value, description) in super::complete::candidates(&args.words) {
                if writeln!(stdout, "{value}\t{}", output::sanitize(&description)).is_err() {
                    return Ok(Some(1));
                }
            }
            Ok(Some(0))
        }
        Command::System => {
            let value = cmd::system::capabilities();
            if globals.json.is_some() {
                print_json(&value, globals.json.as_deref())?;
            } else {
                output::print_text(&render::outline(&value))?;
            }
            Ok(Some(0))
        }
        _ => Ok(None),
    }
}

/// `--json=fields` is checked before anything runs: a malformed list, or one
/// for raw content, fails without side effects.
pub(super) fn check_projection(command: &Command, globals: &Globals) -> Result<(), CliError> {
    let fields = projection::parse(globals.json.as_deref())?;
    if !fields.is_empty() && command.meta().output == cmd::OutputKind::Bytes {
        return Err(CliError::new(
            "usage",
            format!(
                "{} 输出文件内容，不是列表：用 --json 取十六进制 JSON，不要写字段",
                command.path()
            ),
        ));
    }
    Ok(())
}

/// What `--dry-run` prints: the parsed request, secrets redacted.
pub(super) fn dry_run(command: &Command) -> Result<Value, CliError> {
    let mut value = serde_json::to_value(command)?;
    redact(&mut value);
    Ok(json!({
        "command": command.path(),
        "request": value,
        "executed": false,
    }))
}

/// Replace every secret value with `<redacted>`.
fn redact(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields.iter_mut() {
                if key == "value" && child.is_string() {
                    *child = json!("<redacted>");
                } else {
                    redact(child);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

/// `case.db` marks a case directory.
fn is_case(dir: &Path) -> bool {
    dir.join(crate::core::casework::case_paths::LEDGER_FILE)
        .is_file()
}

/// The case a command works in: `-C`, `LOVELYCARVE_CASE`, or the case the
/// working directory is in. Commands that name their case themselves keep it.
fn case_dir(globals: &Globals, command: &Command) -> Result<Option<String>, CliError> {
    if let Command::Case(cmd::case::CaseArgs {
        command: Some(case_command),
    }) = command
    {
        match case_command {
            CaseCommand::Open(dir) | CaseCommand::Close(dir) => {
                if let Some(dir) = &dir.dir {
                    return Ok(Some(dir.text()));
                }
            }
            CaseCommand::Serve(dir) => return Ok(Some(dir.dir.text())),
            // Creating, listing or removing cases happens outside any case.
            CaseCommand::New(_) | CaseCommand::Ls | CaseCommand::Rm(_) => return Ok(None),
            CaseCommand::Status
            | CaseCommand::Lock
            | CaseCommand::Unlock
            | CaseCommand::Link(_) => {}
        }
    }
    if let Some(dir) = &globals.case {
        return Ok(Some(dir.text()));
    }
    if let Some(dir) = std::env::var_os("LOVELYCARVE_CASE").filter(|dir| !dir.is_empty()) {
        let dir = super::types::parse_host_path(&dir.to_string_lossy())
            .map_err(|message| CliError::new("usage", message))?;
        return Ok(Some(dir.text()));
    }
    let Ok(current) = std::env::current_dir() else {
        return Ok(None);
    };
    let found: Option<PathBuf> = current
        .ancestors()
        .find(|dir| is_case(dir))
        .map(Path::to_owned);
    if let Some(dir) = &found {
        if !globals.quiet && !globals.events {
            output::note(
                &format!("使用案件：{}（当前目录所在的案件）", dir.display()),
                false,
            );
        }
    }
    Ok(found.map(|dir| HostPath::new(dir).text()))
}

fn manage(command: &CaseCommand, case: Option<&str>, globals: &Globals) -> Result<i32, CliError> {
    let dir = case
        .ok_or_else(|| CliError::new("usage", "需要案件目录：case open <目录>，或用 -C <目录>"))?;
    match command {
        CaseCommand::Open(_) => {
            let info = backend::open(dir)?;
            present(&Command::System, Output::json(info), globals).map(|_| 0)
        }
        CaseCommand::Close(_) => {
            let info = backend::close(dir)?;
            present(&Command::System, Output::json(info), globals).map(|_| 0)
        }
        CaseCommand::Serve(_) => {
            let prepared = backend::prepare(dir)?;
            start(
                globals.clone(),
                Some(dir.to_owned()),
                Work::Backend(prepared),
            )
        }
        _ => Err(super::internal_error("不是进程管理命令")),
    }
}

/// Run `command` in the case backend, confirming destructive commands here
/// where a person may be asked.
fn forward(
    dir: &str,
    command: &Command,
    mut options: RequestOptions,
    globals: &Globals,
) -> Result<i32, CliError> {
    let meta = command.meta();
    if let (Some(question), false) = (meta.confirm, options.yes) {
        if !super::prompt::confirm_delete(false, Some(question))? {
            eprintln!("Aborted.");
            return Err(CliError::aborted());
        }
        options.yes = true;
    }
    let reporter = Reporter::new(Options {
        quiet: globals.quiet,
        events: globals.events,
    });
    // Evidence named with --image joins the backend's workspace first.
    if !globals.images.is_empty() {
        let add = Command::Evidence(cmd::evidence::EvidenceArgs {
            command: Some(cmd::evidence::EvidenceCommand::Add(
                cmd::evidence::AddArgs {
                    paths: globals.images.clone(),
                    alias: None,
                    raw_read_mode: None,
                    qcow_password: None,
                    qcow_key: None,
                    wait: false,
                },
            )),
        });
        backend::forward(dir, &add, options.clone(), &reporter)?;
    }
    let mut monitor = reporter.start(command.path());
    let result = forwarded(dir, command, options, &reporter, globals);
    monitor.finish(&result);
    present(command, result?, globals)
}

/// Run `command` in the case backend: raw bytes stream to stdout, or with
/// `--json` come back to be printed as JSON.
pub(super) fn forwarded(
    dir: &str,
    command: &Command,
    options: RequestOptions,
    reporter: &Reporter,
    globals: &Globals,
) -> Result<Output, CliError> {
    if globals.json.is_none() || command.meta().output != cmd::OutputKind::Bytes {
        return backend::forward(dir, command, options, reporter);
    }
    let mut bytes = super::stream::Bounded::default();
    match backend::forward_into(dir, command, options, Some(reporter), &mut bytes)? {
        Output::Written => Ok(Output::Bytes(bytes.into_inner())),
        other => Ok(other),
    }
}

/// Print a result for people or, with `--json`, as JSON; returns the exit
/// status (2 for an incomplete result).
pub(super) fn present(
    command: &Command,
    output: Output,
    globals: &Globals,
) -> Result<i32, CliError> {
    let meta = command.meta();
    let incomplete = incomplete::check(meta.incomplete, &output);
    match output {
        Output::Json(json) => {
            if globals.json.is_some() {
                print_json(json.value(), globals.json.as_deref())?;
            } else if json.is_null() {
                output::confirm_done("完成", globals.quiet || globals.events);
            } else {
                let value = json.value();
                let text = cmd::render(command, value).unwrap_or_else(|| render::generic(value));
                output::print_text(&text)?;
            }
        }
        // JSON consumers get raw content as JSON too, as batch responses carry it.
        Output::Bytes(bytes) if globals.json.is_some() => {
            print_json(
                &json!({"encoding": "hex", "data": hex::encode(bytes)}),
                None,
            )?;
        }
        Output::Bytes(bytes) => output::print_bytes(&bytes)?,
        Output::Written => {}
        Output::Stream(_) => return Err(super::internal_error("流式结果没有写出")),
    }
    Ok(match incomplete {
        Some(incomplete) => {
            report_incomplete(&incomplete, globals.events);
            EXIT_INCOMPLETE
        }
        None => 0,
    })
}

fn print_json(value: &Value, fields: Option<&str>) -> Result<(), CliError> {
    let fields = projection::parse(fields)?;
    let output = projection::apply(Output::json(value.clone()), &fields)?;
    if let Output::Json(json) = output {
        output::print_text(&output::format_json(&json))?;
    }
    Ok(())
}

/// Say that the result just printed is incomplete: a framed warning, or a
/// `cli-warning` event under `--events`.
pub(super) fn report_incomplete(incomplete: &incomplete::Incomplete, events: bool) {
    if !events {
        output::report_incomplete(incomplete);
        return;
    }
    let reporter = Reporter::new(Options {
        quiet: true,
        events: true,
    });
    match serde_json::to_value(incomplete) {
        Ok(payload) => {
            if let Err(error) = reporter.event("cli-warning", payload) {
                eprintln!("无法写出 CLI 警告事件：{error}");
            }
        }
        Err(error) => eprintln!("无法写出 CLI 警告事件：{error}"),
    }
}

fn report_error(error: &CliError, events: bool) {
    if !events {
        output::report_error(error);
        return;
    }
    if error.reported {
        return;
    }
    let reporter = Reporter::new(Options {
        quiet: true,
        events: true,
    });
    let payload = json!({"code": error.code, "message": error.message, "details": error.details,
        "operation": match &error.origin { super::Origin::Backend(operation) => operation.as_deref(), super::Origin::Cli => None }});
    if let Err(error) = reporter.event("cli-error", payload) {
        eprintln!("无法写出 CLI 错误事件：{error}");
    }
}

pub(super) enum Work {
    OneShot {
        command: Box<Command>,
        options: RequestOptions,
    },
    Batch {
        jobs: usize,
    },
    Shell,
    Backend(backend::Prepared),
}

/// The stack of every thread that runs commands. Parser recursion and the
/// command futures (large in debug builds) need more than the defaults: a
/// batch request, for one, runs its whole command on a blocking thread.
const STACK_SIZE: usize = 16 * 1024 * 1024;

/// The tokio runtime commands run on.
pub(super) fn runtime() -> Result<tokio::runtime::Runtime, CliError> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("lovelycarve-cli-worker")
        .thread_stack_size(STACK_SIZE)
        .build()
        .map_err(super::internal_error)
}

/// Run `work` on a thread with a large stack and its own tokio runtime.
fn start(globals: Globals, case: Option<String>, work: Work) -> Result<i32, CliError> {
    let events = globals.events;
    let thread = std::thread::Builder::new()
        .name("lovelycarve-cli".into())
        .stack_size(STACK_SIZE)
        .spawn(move || {
            let runtime = match runtime() {
                Ok(runtime) => runtime,
                Err(error) => {
                    report_error(&error, events);
                    return 1;
                }
            };
            let code = runtime.block_on(serve(globals, case, work));
            // Cancelled tasks clean up within `serve`; a blocking read that
            // has no cancellation (a stream to stdout) must not hold the exit.
            runtime.shutdown_timeout(std::time::Duration::from_secs(10));
            code
        })?;
    match thread.join() {
        Ok(code) => Ok(code),
        Err(_) => Err(super::internal_error("CLI 工作线程异常退出")),
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error"));
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

/// Backend events feed human progress or explicit JSONL diagnostics on
/// stderr, and tell the background work how its tasks end.
fn host(reporter: Reporter, background: Background) -> AppHandle {
    AppHandle::new(
        crate::host::default_app_data_dir(),
        move |event, payload| {
            let payload = serde_json::from_str(&payload).map_err(|error| error.to_string())?;
            background.observe(event, &payload);
            reporter.event(event, payload)
        },
    )
}

/// A fresh backend state, initialized as the desktop app initializes its own.
pub(super) fn exec(mode: Mode, quiet: bool, events: bool) -> Result<Exec, CliError> {
    let reporter = Reporter::new(Options {
        quiet: quiet || matches!(mode, Mode::Batch | Mode::Backend),
        events,
    });
    let background = Background::default();
    let host = host(reporter.clone(), background.clone());
    let state = Arc::new(AppState::new());
    crate::app::setup::initialize(&state, &host)
        .map_err(|error| super::internal_error(format!("初始化失败：{error}")))?;
    Ok(Exec {
        host,
        state,
        mode,
        reporter,
        background,
        workspace: Workspace::default(),
    })
}

async fn serve(globals: Globals, case: Option<String>, work: Work) -> i32 {
    init_tracing();
    let mode = match &work {
        Work::OneShot { .. } => Mode::OneShot,
        Work::Batch { .. } => Mode::Batch,
        Work::Shell => Mode::Shell,
        Work::Backend(_) => Mode::Backend,
    };
    let exec = match exec(mode, globals.quiet, globals.events) {
        Ok(exec) => exec,
        Err(error) => {
            report_error(&error, globals.events);
            return 1;
        }
    };
    let outcome = tokio::select! {
        outcome = perform(&exec, &globals, case, work) => outcome,
        () = interrupted() => {
            for task in exec.state.tasks.list() {
                exec.state.tasks.cancel(&task.id);
            }
            Err(CliError::exit(130))
        }
    };
    let code = match outcome {
        Ok(code) => code,
        Err(error) => {
            report_error(&error, globals.events);
            error.exit_code
        }
    };
    shutdown(&exec.state).await;
    code
}

/// Open the case and the `--image` evidence, then run the work.
async fn perform(
    exec: &Exec,
    globals: &Globals,
    case: Option<String>,
    work: Work,
) -> Result<i32, CliError> {
    if let Some(dir) = case {
        let foreground = exec.background.foreground();
        let opened =
            crate::commands::casework::case::case_open(exec.host.clone(), exec.state(), dir)
                .await
                .map_err(|error| super::command_error(error).in_operation("case_open"));
        foreground.finish(&opened);
        opened?;
    }
    for image in &globals.images {
        exec.add(&image.text(), None, Default::default()).await?;
    }
    let code = match work {
        Work::OneShot { command, options } => {
            let command = *command;
            let executed = exec.run(command.clone(), &options).await?;
            deliver(exec, &command, executed, globals).await?
        }
        Work::Batch { jobs } => super::batch::run(exec.clone(), jobs).await?,
        Work::Shell => super::shell::run(exec, globals).await?,
        Work::Backend(prepared) => return backend::serve(exec, prepared).await,
    };
    // Indexes of caseless evidence are process-local and discarded anyway.
    exec.state.tasks.cancel_kind(TaskKind::SessionSnapshot);
    let mut monitor = exec
        .state
        .tasks
        .list()
        .iter()
        .any(|task| task.kind != TaskKind::SessionSnapshot)
        .then(|| exec.reporter.start("tasks wait（等待后台任务）"));
    let result = finish(exec).await;
    if let Some(monitor) = &mut monitor {
        monitor.finish(&result);
    }
    result?;
    Ok(code)
}

/// Print one command's result where it ran; a streamed read goes to stdout.
pub(super) async fn deliver(
    exec: &Exec,
    command: &Command,
    executed: Executed,
    globals: &Globals,
) -> Result<i32, CliError> {
    match executed.output {
        Output::Stream(plan) if globals.json.is_some() => {
            let bytes = super::stream::buffer(exec, plan).await?;
            present(command, Output::Bytes(bytes), globals)
        }
        Output::Stream(plan) => {
            super::stream::stream(exec, plan, |chunk| {
                let mut stdout = std::io::stdout().lock();
                stdout.write_all(chunk)?;
                stdout.flush()
            })
            .await?;
            Ok(0)
        }
        output => present(command, output, globals),
    }
}

/// Before exit: wait for every task the work left behind. A failed
/// background task fails the process, after the command's own output.
async fn finish(exec: &Exec) -> Result<(), CliError> {
    exec.state.tasks.cancel_kind(TaskKind::SessionSnapshot);
    exec.background.wait(&exec.state).await?.verdict(None)?;
    Ok(())
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

pub(super) async fn shutdown(state: &AppState) {
    // Record the tasks that already ended while their case is still open.
    crate::app::lifecycle::flush_task_journal(state).await;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_runs_redact_secrets_and_never_run() {
        let cli = parse(&[
            "unlock".to_owned(),
            "/nas/part:2".to_owned(),
            "--password".to_owned(),
            "@secret.txt".to_owned(),
        ])
        .unwrap();
        let mut command = cli.command.unwrap();
        for (_, secret) in command.secrets_mut() {
            *secret = super::super::types::Secret::Value("hunter2".into());
        }
        let shown = dry_run(&command).unwrap();
        assert_eq!(shown["command"], "unlock");
        assert_eq!(shown["executed"], false);
        assert!(!shown.to_string().contains("hunter2"));
        assert!(shown.to_string().contains("<redacted>"));
    }
}
