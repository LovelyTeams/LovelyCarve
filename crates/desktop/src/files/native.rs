//! Outside In runs in a separate process. The backend holds the evidence
//! preview lease until the process exits, including on cancellation or error.

mod surface;
mod window;

pub use surface::{finish_frame, viewport, ViewerLayer, ViewerWindow};

use std::{
    future::Future,
    io,
    os::windows::io::{FromRawHandle as _, OwnedHandle},
    path::{Path, PathBuf},
    process::Stdio,
    ptr,
    time::Duration,
};

use futures::channel::oneshot;
use lovelycarve_backend::{
    commands::{app_error::AppError, preview::filesystem_preview as preview},
    core::filesystem::tree::fs_tree::FsNode,
    host::{LeaseOwner, State},
};
use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    process::{Child, Command},
};
use tokio_util::sync::CancellationToken;
use winapi::um::{
    jobapi2::{AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject},
    winnt::{
        JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
};

use crate::backend::{Backend, CommandError};

const CHILD_ARG: &str = "--outside-in-preview";
const READY: &str = "ready ";
const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

pub fn run_child_if_requested() -> Option<Result<(), String>> {
    let mut args = std::env::args_os().skip(1);
    if args.next()?.as_os_str() != CHILD_ARG {
        return None;
    }
    let arguments = (
        args.next(),
        args.next(),
        args.next(),
        args.next(),
        args.next(),
    );
    Some(match arguments {
        (Some(parent), Some(directory), Some(file), Some(name), None) => parent
            .to_str()
            .and_then(ViewerWindow::parse)
            .ok_or_else(|| "原类型视图缺少 GPUI 窗口句柄".to_owned())
            .and_then(|parent| window::run(parent, Path::new(&directory), Path::new(&file), &name)),
        _ => Err("原类型视图启动参数无效".to_owned()),
    })
}

pub fn runtime_directory(configured: Option<PathBuf>) -> Result<PathBuf, String> {
    let directory = configured
        .or_else(|| std::env::var_os("LOVELYCARVE_OUTSIDE_IN_DIR").map(PathBuf::from))
        .or_else(|| Some(std::env::current_exe().ok()?.parent()?.join("viewer")))
        .ok_or("请选择 Outside In x64 Viewer 目录")?;
    validate_runtime(&directory)?;
    std::path::absolute(&directory).map_err(|error| format!("无法解析 Viewer 目录：{error}"))
}

pub fn validate_runtime(directory: &Path) -> Result<(), String> {
    if !directory.join("sccvw.dll").is_file() || !directory.join("oswin64.dll").is_file() {
        return Err("请选择包含 sccvw.dll 和 oswin64.dll 的 Outside In x64 Viewer 目录".into());
    }
    Ok(())
}

pub fn open(
    backend: Backend,
    parent: ViewerWindow,
    runtime: PathBuf,
    image_id: String,
    node: FsNode,
    cancel: CancellationToken,
    ready: oneshot::Sender<ViewerWindow>,
) -> impl Future<Output = Result<(), CommandError>> + use<> {
    let task_id = uuid::Uuid::new_v4().to_string();
    backend.run(move |_, state| async move {
        let owner_cancel = cancel.clone();
        let owner = LeaseOwner::window(task_id.clone(), move || owner_cancel.is_cancelled());
        let extension = Path::new(&node.name)
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_owned);
        let materialize = preview::fs_media_temp(
            owner,
            State::new(&state),
            image_id,
            node.id,
            extension,
            task_id.clone(),
        );
        tokio::pin!(materialize);
        let file = tokio::select! {
            biased;
            result = &mut materialize => result?,
            _ = cancel.cancelled() => {
                state.tasks.cancel(&task_id);
                materialize.await?
            }
        };
        let result = if cancel.is_cancelled() {
            Ok(())
        } else {
            run_viewer(parent, &runtime, &file, &node.name, &cancel, ready).await
        };
        preview::fs_media_release(State::new(&state), task_id).await?;
        result
    })
}

async fn run_viewer(
    parent: ViewerWindow,
    runtime: &Path,
    file: &str,
    name: &str,
    cancel: &CancellationToken,
    ready: oneshot::Sender<ViewerWindow>,
) -> Result<(), AppError> {
    let executable = std::env::current_exe()
        .map_err(|error| AppError::from_io("定位原类型视图程序失败", error))?;
    let mut child = Command::new(executable)
        .arg(CHILD_ARG)
        .arg(parent.number().to_string())
        .arg(runtime)
        .arg(file)
        .arg(name.replace('\0', "�"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| AppError::from_io("启动原类型视图失败", error))?;
    let result = monitor_viewer(&mut child, cancel, ready).await;
    // Wait for Windows to close any handles the viewer still has on the
    // preview before returning the lease, even after a failed handshake.
    if let Err(error) = child.kill().await {
        tracing::warn!("结束原类型视图进程失败：{error}");
    }
    result
}

async fn monitor_viewer(
    child: &mut Child,
    cancel: &CancellationToken,
    ready: oneshot::Sender<ViewerWindow>,
) -> Result<(), AppError> {
    let _job =
        attach_job(child).map_err(|error| AppError::from_io("保护原类型视图进程失败", error))?;
    let output = child
        .stdout
        .take()
        .ok_or_else(|| AppError::invalid_input("原类型视图进程缺少状态通道"))?;
    let mut output = BufReader::new(output);
    let mut response = String::new();
    tokio::select! {
        biased;
        _ = cancel.cancelled() => return Ok(()),
        result = tokio::time::timeout(STARTUP_TIMEOUT, output.read_line(&mut response)) => {
            result
                .map_err(|_| AppError::invalid_input("原类型视图启动超时"))?
                .map_err(|error| AppError::from_io("读取原类型视图状态失败", error))?;
        }
    }
    let response = response.trim();
    let Some(viewer) = response.strip_prefix(READY).and_then(ViewerWindow::parse) else {
        let message = if response.is_empty() {
            "原类型视图进程在启动时退出"
        } else {
            response
        };
        return Err(AppError::invalid_input(message));
    };
    if ready.send(viewer).is_err() {
        return Ok(());
    }
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Ok(()),
        status = child.wait() => {
            let status = status.map_err(|error| AppError::from_io("等待原类型视图退出失败", error))?;
            if !status.success() {
                return Err(AppError::invalid_input(format!("原类型视图进程异常退出（{status}）")));
            }
            Ok(())
        }
    }
}

/// Close the viewer even if the GPUI host itself crashes or exits early.
fn attach_job(child: &Child) -> io::Result<OwnedHandle> {
    let process = child
        .raw_handle()
        .ok_or_else(|| io::Error::other("缺少原类型视图进程句柄"))?;
    // SAFETY: The null arguments request a private job with default security.
    let handle = unsafe { CreateJobObjectW(ptr::null_mut(), ptr::null()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateJobObjectW returned an owned handle.
    let job = unsafe { OwnedHandle::from_raw_handle(handle.cast()) };
    // SAFETY: This Win32 structure is a plain record of integer fields.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: Both handles are live and the limits buffer has the specified layout.
    let configured = unsafe {
        SetInformationJobObject(
            handle,
            JobObjectExtendedLimitInformation,
            (&raw mut limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        ) != 0
            && AssignProcessToJobObject(handle, process.cast()) != 0
    };
    if !configured {
        return Err(io::Error::last_os_error());
    }
    Ok(job)
}
