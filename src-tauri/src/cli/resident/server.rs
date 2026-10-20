//! A serialized executor owning one live CLI context. Clients never recreate
//! AppState, evidence sessions, assembled devices or their caches.
use std::{
    io::{BufReader, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{json, Map, Value};

use super::{
    protocol::{self, Header},
    registry::{self, Descriptor, Registration},
};
use crate::cli::{
    catalog,
    context::{Context, Exec, Request},
    native, CliError, Output,
};

pub(crate) struct Prepared {
    listener: TcpListener,
    lock: std::fs::File,
    descriptor: Descriptor,
}

pub(in crate::cli) fn prepare(name: &str) -> Result<Prepared, CliError> {
    let lock = registry::lock(name)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let descriptor = Descriptor {
        protocol: protocol::VERSION,
        name: name.into(),
        port: listener.local_addr()?.port(),
        token: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
        pid: std::process::id(),
        started_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(crate::cli::internal_error)?
            .as_secs(),
        working_directory: std::env::current_dir()?,
    };
    Ok(Prepared {
        listener,
        lock,
        descriptor,
    })
}

pub(in crate::cli) async fn serve(
    exec: &Exec,
    context: &mut Context,
    prepared: Prepared,
) -> Result<i32, CliError> {
    let Prepared {
        listener,
        lock,
        descriptor,
    } = prepared;
    // Open the initial evidence exactly once, before advertising readiness.
    exec.current_image(context).await?;
    let registration = Registration::publish(lock, &descriptor)?;
    loop {
        let (stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        // Windows inherits the listener's nonblocking mode on accepted sockets.
        // The framed transport uses blocking reads/writes with timeouts.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        let (mut stream, request) = tokio::task::spawn_blocking(move || {
            let mut reader = BufReader::new(stream);
            let request = protocol::read_request(&mut reader);
            (reader.into_inner(), request)
        })
        .await
        .map_err(crate::cli::internal_error)?;
        let request = match request {
            Ok(request) if request.token == descriptor.token => request,
            Ok(_) => {
                if let Err(error) = protocol::send(
                    &mut stream,
                    Err(CliError::new("permissionDenied", "会话凭据无效")),
                ) {
                    tracing::debug!("resident authentication response: {error}");
                }
                continue;
            }
            Err(error) => {
                if let Err(error) = protocol::send(&mut stream, Err(error)) {
                    tracing::debug!("resident request response: {error}");
                }
                continue;
            }
        };
        if request.command == "@close" {
            crate::cli::runtime::shutdown(&exec.state).await;
            drop(registration);
            protocol::send(&mut stream, Ok(Output::Written))?;
            return Ok(0);
        }
        let result = if request.command == "@info" {
            let output =
                execute(exec, context, "context get", Map::new(), false, false, None).await;
            match output {
                Ok(Output::Json(context)) => Ok(Output::json(json!({
                    "name": descriptor.name, "pid": descriptor.pid, "startedAt": descriptor.started_at,
                    "workingDirectory": descriptor.working_directory, "running": true,
                    "context": context.value(), "imageCount": exec.state.images.lock().await.len()
                }))),
                Ok(_) => Err(crate::cli::internal_error("context get 返回非 JSON 结果")),
                Err(error) => Err(error),
            }
        } else if request.command == "files read" && !request.dry_run {
            // Handle before the generic JSONL read cap: one logical read streams
            // bounded chunks while holding this request's image/node identity.
            match stream_file(exec, context, &request.args, &mut stream).await {
                Ok(()) => continue,
                Err(error) => Err(error),
            }
        } else {
            let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
            let subscription = request.progress.then(|| exec.reporter.subscribe(sender));
            let result = {
                let work = async {
                    if let Some(reference) = &request.initial_image {
                        if request.command != "raid detect" {
                            return Err(CliError::invalid(
                                "initialImage 仅用于 RAID 检测的初始检材",
                            ));
                        }
                        if !request.dry_run {
                            exec.resolve_image(context, reference).await?;
                        }
                    }
                    execute(
                        exec,
                        context,
                        &request.command,
                        request.args,
                        request.force,
                        request.dry_run,
                        request.timezone,
                    )
                    .await
                };
                tokio::pin!(work);
                loop {
                    tokio::select! {
                        biased;
                        event = events.recv(), if subscription.is_some() => {
                            if let Some(event) = event {
                                if let Err(error) = protocol::header(&mut stream, &Header::Event { event }) { tracing::debug!("resident progress delivery: {error}"); }
                            }
                        }
                        result = &mut work => break result,
                    }
                }
            };
            drop(subscription);
            while let Ok(event) = events.try_recv() {
                if let Err(error) = protocol::header(&mut stream, &Header::Event { event }) {
                    tracing::debug!("resident final progress delivery: {error}");
                }
            }
            result
        };
        if let Err(error) = protocol::send(&mut stream, result) {
            // A disconnected client never tears down the shared evidence and
            // never causes a mutation to be retried automatically.
            tracing::debug!("resident response delivery: {error}");
        }
    }
}

async fn execute(
    exec: &Exec,
    context: &mut Context,
    path: &str,
    values: Map<String, Value>,
    force: bool,
    dry_run: bool,
    timezone: Option<String>,
) -> Result<Output, CliError> {
    let command = catalog::catalog()?
        .find_path(path)
        .ok_or_else(|| CliError::invalid(format!("未知命令：{path}")))?;
    if command.native.as_deref().is_some_and(|native| {
        matches!(native, "session" | "shell" | "complete") || native.starts_with("sessions-")
    }) {
        return Err(CliError::invalid("不能在常驻会话内嵌套会话管理命令"));
    }
    command.validate(&values)?;
    exec.execute(
        context,
        Request {
            timezone,
            fields: None,
            command,
            values,
            ask: vec![],
            force,
            dry_run,
            text: false,
        },
    )
    .await
}

async fn stream_file(
    exec: &Exec,
    context: &mut Context,
    args: &Map<String, Value>,
    stream: &mut TcpStream,
) -> Result<(), CliError> {
    let command = catalog::catalog()?
        .find_path("files read")
        .ok_or_else(|| crate::cli::internal_error("缺少 files read 命令"))?;
    command.validate(args)?;
    let image_id = match args.get("imageId").and_then(Value::as_str) {
        Some(reference) => exec.resolve_image(context, reference).await?,
        None => exec
            .current_image(context)
            .await?
            .ok_or_else(|| CliError::new("noImage", "需要检材"))?,
    };
    let mut values = args.clone();
    values.insert("imageId".into(), json!(image_id));
    let node_id = exec
        .target(context, command, "nodeId", &mut values)
        .await?
        .node_id;
    let image = exec
        .state
        .images
        .lock()
        .await
        .get(&image_id)
        .cloned()
        .ok_or_else(|| CliError::new("noImage", "检材已关闭"))?;
    let stat_node = node_id.clone();
    let size = tokio::task::spawn_blocking(move || {
        native::require_source(&image)?;
        crate::core::filesystem::tree::fs_tree::FsSession::new(&image)
            .file_stat(&stat_node)
            .map(|stat| stat.size)
            .map_err(crate::cli::internal_error)
    })
    .await
    .map_err(crate::cli::internal_error)??;
    let mut offset = args.get("offset").and_then(Value::as_u64).unwrap_or(0);
    if offset > size {
        return Err(CliError::invalid(format!(
            "偏移 {offset} 超过文件大小 {size}"
        )));
    }
    let length = args
        .get("length")
        .and_then(Value::as_u64)
        .unwrap_or(size - offset)
        .min(size - offset);
    if let Err(error) = protocol::header(stream, &Header::Stream) {
        tracing::debug!("resident file stream header: {error}");
        return Ok(());
    }
    let result = async {
        let end = offset + length;
        while offset < end {
            let length = (end - offset).min(protocol::CHUNK as u64);
            let values =
                json!({"imageId": image_id, "nodeId": node_id, "offset": offset, "length": length})
                    .as_object()
                    .cloned()
                    .ok_or_else(|| crate::cli::internal_error("文件读取参数无效"))?;
            let Output::Bytes(bytes) =
                execute(exec, context, "files read", values, false, false, None).await?
            else {
                return Err(crate::cli::internal_error("files read 返回非字节结果"));
            };
            if bytes.len() as u64 != length {
                return Err(CliError::new(
                    "shortRead",
                    "文件未完整读取，源检材可能已变化",
                ));
            }
            protocol::chunk(stream, &bytes)?;
            stream.flush()?;
            offset += length;
        }
        Ok(())
    }
    .await;
    // Once a stream starts its errors travel in the stream's final status.
    // A socket failure must not be followed by a second response header.
    if let Err(error) = protocol::end_stream(stream, result) {
        tracing::debug!("resident file stream: {error}");
    }
    Ok(())
}
