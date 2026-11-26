//! The case backend: a background `lovelycarve-cli` process that holds one
//! case, so every command for that case shares the open evidence, unlocked
//! volumes, assembled arrays, mounts and running tasks. It is started and
//! stopped explicitly (`case open` / `case close`); a command never starts
//! one, and a backend that died is reported instead of silently replaced.
//! The case directory is its name: a case is opened by one process at a time.
mod client;
mod launch;
mod protocol;
mod registry;
mod server;

use std::{
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde_json::{json, Value};

use super::{cmd, exec::RequestOptions, progress::Reporter, CliError, Output};

pub(super) use client::guard_interrupts;
pub(super) use server::{prepare, serve, Prepared};
pub(super) const PROTOCOL_VERSION: u32 = protocol::VERSION;

/// Whether `case_dir` has a backend, and in what state.
pub(super) enum State {
    NotRunning,
    Running,
    /// Its record outlived the process; the record is gone now.
    Exited,
}

pub(super) fn state(case_dir: &str) -> Result<State, CliError> {
    match registry::read(case_dir) {
        Ok(None) => Ok(State::NotRunning),
        Ok(Some(_)) => {
            if registry::clear_stale(case_dir)? {
                Ok(State::Exited)
            } else {
                Ok(State::Running)
            }
        }
        Err(_) if registry::clear_stale(case_dir)? => Ok(State::Exited),
        Err(error) => Err(error),
    }
}

/// Whether a backend serves `case_dir` (for `case ls` and `case rm`).
pub(crate) fn running(case_dir: &str) -> bool {
    matches!(state(case_dir), Ok(State::Running))
}

/// The last lines of the backend's log, for a failure to start.
fn log_tail(case_dir: &str) -> String {
    let Ok(path) = registry::log_path(case_dir) else {
        return String::new();
    };
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().rev().take(6).collect();
    let tail: Vec<&str> = lines.into_iter().rev().collect();
    format!("\n日志 {}：\n{}", path.display(), tail.join("\n"))
}

/// Start the backend of `case_dir`; report the running one when it already
/// runs.
pub(super) fn open(case_dir: &str) -> Result<Value, CliError> {
    match registry::lock(case_dir) {
        Ok(lock) => {
            registry::remove_record(&lock, case_dir)?;
            drop(lock);
        }
        Err(error) if error.code == "caseBackendRunning" => {
            let mut info = client::info(case_dir)?;
            info["alreadyRunning"] = json!(true);
            return Ok(info);
        }
        Err(error) => return Err(error),
    }
    let log_path = registry::log_path(case_dir)?;
    let log = registry::private_options().truncate(true).open(&log_path)?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["-q", "case", "serve", case_dir])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = launch::spawn(&mut command)?;
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(status) = child.try_wait()? {
            // Another launcher may have won the lock: report its backend.
            if let Ok(info) = client::info(case_dir) {
                return Ok(info);
            }
            return Err(CliError::new(
                "caseBackendStartFailed",
                format!("案件后台进程启动失败（{status}）{}", log_tail(case_dir)),
            ));
        }
        if let Ok(info) = client::info(case_dir) {
            if info["pid"] == child.id() {
                return Ok(info);
            }
        }
        if Instant::now() >= deadline {
            return Err(CliError::new(
                "caseBackendStarting",
                format!(
                    "案件后台进程仍在打开案件；稍后运行 case status，日志 {}",
                    log_path.display()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Stop the backend of `case_dir`.
pub(super) fn close(case_dir: &str) -> Result<Value, CliError> {
    match state(case_dir)? {
        State::NotRunning => Ok(json!({"caseDir": case_dir, "closed": false, "running": false})),
        State::Exited => Ok(json!({"caseDir": case_dir, "closed": true, "exited": true})),
        State::Running => {
            client::exchange(case_dir, protocol::Work::Close, None, &mut std::io::sink())?;
            Ok(json!({"caseDir": case_dir, "closed": true}))
        }
    }
}

/// Run `command` in the backend of `case_dir`, writing raw bytes to `sink`
/// instead of stdout.
pub(super) fn forward_into(
    case_dir: &str,
    command: &cmd::Command,
    options: RequestOptions,
    reporter: Option<&Reporter>,
    sink: &mut impl std::io::Write,
) -> Result<Output, CliError> {
    client::exchange(
        case_dir,
        protocol::Work::Run {
            command: Box::new(command.clone()),
            options,
        },
        reporter,
        sink,
    )
}

/// Run `command` in the backend of `case_dir`; raw bytes go straight to
/// stdout.
pub(super) fn forward(
    case_dir: &str,
    command: &cmd::Command,
    options: RequestOptions,
    reporter: &Reporter,
) -> Result<Output, CliError> {
    let mut stdout = std::io::stdout().lock();
    client::exchange(
        case_dir,
        protocol::Work::Run {
            command: Box::new(command.clone()),
            options,
        },
        Some(reporter),
        &mut stdout,
    )
}
