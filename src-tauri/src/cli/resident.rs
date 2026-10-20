//! Named resident sessions: lifecycle management stays outside the backend;
//! only the detached server owns AppState and open evidence handles.
mod attach;
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

use super::{
    argv::{Globals, Invocation},
    output, CliError, Output,
};

pub(super) use client::forward;
pub(super) use server::{prepare, serve, Prepared};

pub(super) fn selected(globals: &Globals) -> Option<String> {
    globals
        .session
        .clone()
        .or_else(|| super::dotenv::variable("LOVELYCARVE_SESSION"))
}

pub(super) fn name(invocation: &Invocation) -> Result<&str, CliError> {
    let name = invocation
        .values
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| CliError::invalid("缺少会话名"))?;
    registry::validate_name(name)?;
    Ok(name)
}

pub(super) fn manage(globals: &Globals, invocation: &Invocation) -> Result<i32, CliError> {
    if invocation.command.native.as_deref() != Some("sessions-open")
        && (globals.case.is_some() || globals.image.is_some())
    {
        return Err(CliError::invalid("--case / --image 仅用于 sessions open 首次启动；连接后用 cases open / context use 修改上下文"));
    }
    let result = match invocation.command.native.as_deref() {
        Some("sessions-open") => open(name(invocation)?, globals)?,
        Some("sessions-get") => client::info(name(invocation)?)?,
        Some("sessions-list") => list()?,
        Some("sessions-close") => {
            let name = name(invocation)?;
            client::exchange(name, "@close", Default::default(), false, false, false)?;
            json!({"name": name, "closed": true})
        }
        Some("sessions-attach") => {
            return attach::run(name(invocation)?, globals.timezone.as_deref())
        }
        _ => return Err(CliError::invalid("未知的会话管理命令")),
    };
    let (_, done) = invocation.command.category.labels();
    output::print_output(Output::json(result), done, globals.quiet, false)?;
    Ok(0)
}

fn open(name: &str, globals: &Globals) -> Result<Value, CliError> {
    match registry::lock(name) {
        Ok(lock) => drop(lock),
        Err(error) if error.code == "sessionAlreadyRunning" => {
            if globals.image.is_some() || globals.case.is_some() || globals.timezone.is_some() {
                return Err(CliError::invalid(
                    "会话已运行；用 context use / images open / cases open 修改已有上下文",
                ));
            }
            return client::info(name);
        }
        Err(error) => return Err(error),
    }
    let log_path = registry::directory()?.join(format!("session-{name}.log"));
    let log = registry::private_options().open(&log_path)?;
    let mut command = Command::new(std::env::current_exe()?);
    command.arg("--quiet");
    if let Some(timezone) = &globals.timezone {
        super::timezone::validate(timezone)?;
        command.args(["--timezone", timezone]);
    }
    if let Some(image) = &globals.image {
        command.args(["--image", image]);
    }
    if let Some(case) = &globals.case {
        command.args(["--case", case]);
    }
    command
        .args(["sessions", "serve", name])
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
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(status) = child.try_wait()? {
            // Another launcher may have won the name lock. Reuse its owner.
            if let Ok(info) = client::info(name) {
                return Ok(info);
            }
            return Err(CliError::new(
                "sessionStartFailed",
                format!(
                    "会话 {name} 启动失败（{status}）；查看日志 {}",
                    log_path.display()
                ),
            ));
        }
        if let Ok(info) = client::info(name) {
            if info["pid"] == child.id() {
                return Ok(info);
            }
        }
        if Instant::now() >= deadline {
            return Err(CliError::new(
                "sessionStarting",
                format!(
                    "会话 {name} 仍在后台打开检材；稍后运行 sessions get {name}，日志 {}",
                    log_path.display()
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn list() -> Result<Value, CliError> {
    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(registry::directory()?)? {
        let path = entry?.path();
        let Some(file) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(name) = file
            .strip_prefix("session-")
            .and_then(|file| file.strip_suffix(".json"))
        else {
            continue;
        };
        match client::info(name) {
            Ok(info) => sessions.push(info),
            Err(error) => sessions.push(json!({"name": name, "running": false, "error": {"code": error.code, "message": error.message}})),
        }
    }
    sessions.sort_by(|left, right| left["name"].as_str().cmp(&right["name"].as_str()));
    Ok(json!({"sessions": sessions, "total": sessions.len()}))
}
