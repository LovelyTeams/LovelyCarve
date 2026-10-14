use std::{
    io::{BufReader, IsTerminal, Write},
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
        Arc,
    },
};

use clap::ArgMatches;
use serde_json::{json, Map};
use tauri::{Listener, Manager};

use super::{catalog, input, shell::Shell, workflow, CliError, Output};
use crate::state::AppState;

pub fn run() -> i32 {
    match start() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}

fn start() -> Result<i32, CliError> {
    let matches = match input::parser()?.try_get_matches() {
        Ok(matches) => matches,
        Err(error) => {
            let code = error.exit_code();
            error.print()?;
            return Ok(code);
        }
    };
    if matches.subcommand_name() == Some("capabilities") {
        workflow::print_output(capabilities()?, matches.get_flag("json"))?;
        return Ok(0);
    }
    if let Some(("commands", args)) = matches.subcommand() {
        let commands = catalog::commands()?;
        let output = if let Some(name) = args.get_one::<String>("name") {
            serde_json::to_value(
                commands
                    .iter()
                    .find(|command| command.name == name.replace('-', "_"))
                    .ok_or_else(|| CliError::invalid("未知命令"))?,
            )?
        } else {
            serde_json::to_value(commands)?
        };
        workflow::print_output(Output::Json(output), matches.get_flag("json"))?;
        return Ok(0);
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .try_init()
        .map_err(super::internal_error)?;
    let mut context = tauri::generate_context!();
    context.config_mut().app.windows.clear();
    let app = crate::application_builder()
        .build(context)
        .map_err(super::internal_error)?;
    let handle = app.handle().clone();
    let interrupted = Arc::new(AtomicBool::new(false));
    // Wry's no-window loop may return zero independently of AppHandle::exit.
    // The CLI contract is the worker's outcome, never the GUI loop status.
    let exit_status = Arc::new(AtomicI32::new(1));
    #[cfg(target_os = "windows")]
    {
        let handle = handle.clone();
        let interrupted = interrupted.clone();
        ctrlc::set_handler(move || {
            interrupted.store(true, Ordering::SeqCst);
            for task in handle.state::<AppState>().tasks.list() {
                handle.state::<AppState>().tasks.cancel(&task.id);
            }
            handle.exit(130);
        })
        .map_err(super::internal_error)?;
    }
    if matches.get_flag("events") {
        for name in [
            "task-lifecycle",
            "task-progress",
            "image-load-progress",
            "image-hash-progress",
            "image-export-progress",
            "bitlocker-unlock-progress",
            "bitlocker-scan-progress",
            "snapshot-build-progress",
            "carve-scan-progress",
            "plugin-progress",
            "plugin-catalog-changed",
            "sched-task-event",
            "autodetect-finished",
            "autodetect-failed",
            "global-search-progress",
            "timeline-progress",
            "fs-export-dir-progress",
            "qr-scan-progress",
        ] {
            handle.listen_any(name, move |event| {
                let payload = serde_json::from_str::<serde_json::Value>(event.payload())
                    .unwrap_or_else(|_| json!(event.payload()));
                let mut stderr = std::io::stderr().lock();
                if let Err(error) = writeln!(stderr, "{}", json!({"event":name,"payload":payload}))
                {
                    tracing::warn!("write CLI event: {error}");
                }
            });
        }
    }
    let worker_status = exit_status.clone();
    // Tauri invokes setup on Ready, not build. Commands must not race discovery
    // and the other setup-owned services, especially in an open JSONL session.
    let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("lovelycarve-cli".into())
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            if let Err(error) = ready_receiver.recv() {
                tracing::error!("CLI 初始化未完成: {error}");
                handle.exit(1);
                return;
            }
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                tauri::async_runtime::block_on(execute(&handle, &matches))
            }));
            let code = match outcome {
                Ok(Ok(code)) => code,
                Ok(Err(error)) => {
                    if matches.get_flag("json") {
                        eprintln!("{}", json!({"error":error}));
                    } else {
                        eprintln!("{error}");
                    }
                    1
                }
                Err(_) => {
                    eprintln!("CLI 工作线程异常退出");
                    1
                }
            };
            worker_status.store(code, Ordering::SeqCst);
            handle.exit(code);
        })?;
    let mut ready_sender = Some(ready_sender);
    app.run_return(move |handle, event| match event {
        tauri::RunEvent::Ready => {
            if let Some(sender) = ready_sender.take() {
                if let Err(error) = sender.send(()) {
                    tracing::error!("CLI 工作线程未就绪: {error}");
                    handle.exit(1);
                }
            }
        }
        tauri::RunEvent::ExitRequested {
            code: None, api, ..
        } => api.prevent_exit(),
        tauri::RunEvent::Exit => {
            drop(ready_sender.take());
            tauri::async_runtime::block_on(shutdown(handle));
        }
        _ => {}
    });
    Ok(if interrupted.load(Ordering::SeqCst) {
        130
    } else {
        exit_status.load(Ordering::SeqCst)
    })
}

pub(super) fn capabilities() -> Result<Output, CliError> {
    Ok(Output::Json(json!({
        "version": env!("CARGO_PKG_VERSION"), "platform": std::env::consts::OS,
        "backend_commands": catalog::commands()?.len(), "windowless": true,
        "qrcode": cfg!(feature = "qrcode-wechat"), "ocr": cfg!(feature = "ocr"),
        "ocr_cuda": cfg!(feature = "ocr-cuda"), "dokan_mount": cfg!(target_os = "windows"),
        "session": {
            "protocol": "jsonl", "protocol_version": 1,
            "default_jobs": 1, "max_jobs": 64,
            "response_order": "completion", "correlation_field": "id",
            "references": "prior-successful-result-json-pointer",
            "non_parallel_commands": "barrier",
            "stdout": "responses", "events": "stderr",
            "max_line_bytes": 16 * 1024 * 1024, "max_requests": 10000
        }
    })))
}

async fn execute(app: &tauri::AppHandle, matches: &ArgMatches) -> Result<i32, CliError> {
    let mut shell = Shell::default();
    if let Some(path) = matches.get_one::<String>("case") {
        shell
            .invoke(
                app,
                "case_open",
                Map::from_iter([("dir".into(), json!(path))]),
            )
            .await?;
    }
    if let Some(path) = matches.get_one::<String>("image") {
        shell
            .invoke(
                app,
                "image_load",
                Map::from_iter([("path".into(), json!(path))]),
            )
            .await?;
    }
    let json_mode = matches.get_flag("json");
    let mut failed = false;
    match matches.subcommand() {
        None | Some(("shell", _)) => {
            let interactive = std::io::stdin().is_terminal();
            if interactive {
                eprintln!("LovelyCarve CLI — help 查看全部命令；open 打开检材；exit 退出。");
            }
            workflow::shell_lines(
                app,
                &mut shell,
                &mut std::io::stdin().lock(),
                interactive,
                json_mode,
            )
            .await?;
        }
        Some(("session", args)) => {
            let jobs = args.get_one::<u32>("jobs").copied().unwrap_or(1) as usize;
            failed = super::session::json_lines(app, &mut shell, jobs).await?;
        }
        Some(("run", args)) => {
            let path = args
                .get_one::<String>("script")
                .ok_or_else(|| CliError::invalid("缺少脚本路径"))?;
            let mut reader = BufReader::new(std::fs::File::open(path)?);
            workflow::shell_lines(app, &mut shell, &mut reader, false, json_mode).await?;
        }
        Some(_) => {
            if let Some(output) = shell.execute(app, matches).await? {
                workflow::print_output(output, json_mode)?;
            }
        }
    }
    workflow::wait_idle(app).await?;
    Ok(i32::from(failed))
}

async fn shutdown(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    for task in state.tasks.list() {
        state.tasks.cancel(&task.id);
    }
    if let Err(error) = crate::app::workflows::casework::case_close(state.clone()).await {
        tracing::error!("CLI case close: {}", error.message);
    }
    crate::app::lifecycle::shutdown_plugins(&state).await;
    crate::app::lifecycle::release_media(&state).await;
}
