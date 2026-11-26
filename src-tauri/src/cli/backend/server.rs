//! A case backend serving other processes. It owns one `AppState` with the
//! case open; clients never recreate it. Read-only queries run side by side,
//! every other command runs alone. Each connection is answered by a task of
//! its own, so a request waiting for its turn, a close included, never keeps
//! the backend from answering others. A request whose client goes away is
//! cancelled: it stops waiting for its turn or its work, and the tasks it
//! started are cancelled. No write to a client runs on a runtime worker: a
//! client that stops reading stalls a thread of its own.
use std::{
    io::{BufReader, ErrorKind, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::json;
use tokio::sync::{mpsc, oneshot, RwLock};
use tokio_util::sync::CancellationToken;

use super::{
    protocol::{self, Header, Request, Work},
    registry::{self, Descriptor, Registration},
};
use crate::cli::{
    cmd::Command,
    exec::{Exec, RequestOptions, RequestTasks},
    progress::{Claims, Event},
    stream::ReadPlan,
    CliError, Output,
};

/// How often a request's watch looks up from its connection to see whether
/// the request has settled.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);
/// How long a command other than a query still runs once its client left:
/// time to finish, or to unwind the tasks cancelled for it, so a change is
/// not cut off half-way. One that only waits (`tasks wait`) is dropped then.
const UNWIND: Duration = Duration::from_secs(2);
/// How long the listener rests when no client is connecting.
const ACCEPT_INTERVAL: Duration = Duration::from_millis(25);
/// How long a client has to send its request once connected.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// How long one write may wait for a client that does not read before its
/// connection is given up.
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
/// Progress events kept for a client that reads slower than they arrive;
/// beyond that, newer ones are dropped.
const PROGRESS_QUEUE: usize = 256;

pub(crate) struct Prepared {
    listener: TcpListener,
    lock: std::fs::File,
    descriptor: Descriptor,
}

pub(in crate::cli) fn prepare(case_dir: &str) -> Result<Prepared, CliError> {
    let lock = registry::lock(case_dir)?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    listener.set_nonblocking(true)?;
    let descriptor = Descriptor {
        protocol: protocol::VERSION,
        case_dir: case_dir.to_owned(),
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
    };
    Ok(Prepared {
        listener,
        lock,
        descriptor,
    })
}

/// Serve until a client closes the backend. The case is already open.
pub(in crate::cli) async fn serve(exec: &Exec, prepared: Prepared) -> Result<i32, CliError> {
    let Prepared {
        listener,
        lock,
        descriptor,
    } = prepared;
    let registration = Registration::publish(lock, &descriptor)?;
    run(exec, listener, descriptor, move || drop(registration)).await?;
    Ok(0)
}

/// What the connections of one backend share.
struct Backend {
    exec: Exec,
    descriptor: Descriptor,
    /// Read-only queries share it; every other command holds it alone.
    gate: Arc<RwLock<()>>,
    /// A client asked to close: new requests are refused.
    closing: CancellationToken,
    /// The close has its turn: requests still waiting for theirs are refused.
    closed: CancellationToken,
}

/// Answer connections on `listener` until a client closes the backend;
/// `unpublish` runs once the case is closed, before the close is answered.
async fn run(
    exec: &Exec,
    listener: TcpListener,
    descriptor: Descriptor,
    unpublish: impl FnOnce(),
) -> Result<(), CliError> {
    let backend = Arc::new(Backend {
        exec: exec.clone(),
        descriptor,
        gate: Arc::new(RwLock::new(())),
        closing: CancellationToken::new(),
        closed: CancellationToken::new(),
    });
    let (closers, mut close_requests) = mpsc::unbounded_channel();
    // Polled from the first close request on, beside the listener, so the
    // backend keeps answering while the close waits for its turn.
    let close = close(&backend, unpublish);
    tokio::pin!(close);
    let mut waiting = Vec::new();
    let report = loop {
        tokio::select! {
            report = &mut close, if !waiting.is_empty() => break report,
            Some(connection) = close_requests.recv() => {
                // What runs or waits already goes first; later requests are
                // refused.
                backend.closing.cancel();
                waiting.push(connection);
            }
            connection = accept(&listener) => {
                tokio::spawn(answer(Arc::clone(&backend), connection?, closers.clone()));
            }
        }
    };
    // A failure nobody waited for goes to the first client that asked.
    let mut report = Some(report);
    for connection in waiting {
        let output = report.take().unwrap_or(Ok(())).map(|()| Output::Written);
        if let Err(error) = reply(connection, output).await {
            tracing::debug!("backend close response: {error}");
        }
    }
    Ok(())
}

/// Close once the requests that run or wait for their turn already have
/// ended: those still waiting then are refused, the remaining tasks are
/// cancelled and the case closed. A failure nobody waited for is still
/// reported.
async fn close(backend: &Backend, unpublish: impl FnOnce()) -> Result<(), CliError> {
    let _alone = Arc::clone(&backend.gate).write_owned().await;
    backend.closed.cancel();
    let report = backend
        .exec
        .background
        .take()
        .verdict(Some("案件后台进程已关闭"));
    crate::cli::runtime::shutdown(&backend.exec.state).await;
    unpublish();
    report.map(|_| ())
}

/// The next client; the listener does not block.
async fn accept(listener: &TcpListener) -> Result<TcpStream, CliError> {
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Ok(stream),
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                tokio::time::sleep(ACCEPT_INTERVAL).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Read the request a client sends on `stream` and answer it. A close goes
/// to `closers`, which answer it once the backend has closed.
async fn answer(
    backend: Arc<Backend>,
    stream: TcpStream,
    closers: mpsc::UnboundedSender<Arc<TcpStream>>,
) {
    let (stream, request) = match receive(stream).await {
        Ok(received) => received,
        Err(error) => {
            tracing::debug!("backend connection: {error}");
            return;
        }
    };
    let request = match request {
        Ok(request) if request.token == backend.descriptor.token => request,
        Ok(_) => {
            return refuse(
                stream,
                CliError::new("permissionDenied", "后台进程凭据无效"),
            )
            .await;
        }
        Err(error) => return refuse(stream, error).await,
    };
    match request.work {
        Work::Close => {
            if closers.send(stream).is_err() {
                tracing::debug!("backend close request after the backend closed");
            }
        }
        Work::Info | Work::Run { .. } if backend.closing.is_cancelled() => {
            refuse(stream, closing()).await;
        }
        Work::Info => {
            let evidence = backend.exec.state.images.lock().await.len();
            let descriptor = &backend.descriptor;
            let info = json!({
                "caseDir": descriptor.case_dir,
                "pid": descriptor.pid,
                "startedAt": descriptor.started_at,
                "protocol": descriptor.protocol,
                "openEvidence": evidence,
                "running": true,
            });
            if let Err(error) = reply(stream, Ok(Output::json(info))).await {
                tracing::debug!("backend info response: {error}");
            }
        }
        Work::Run { command, options } => {
            handle(backend, stream, *command, options, request.progress).await;
        }
    }
}

/// The request a client sends once connected, read on a blocking thread.
async fn receive(
    stream: TcpStream,
) -> Result<(Arc<TcpStream>, Result<Request, CliError>), CliError> {
    // Windows inherits the listener's nonblocking mode on accepted
    // sockets; the framed transport uses blocking reads with timeouts.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    blocking(move || {
        let mut reader = BufReader::new(stream);
        let request = protocol::read_request(&mut reader);
        Ok((Arc::new(reader.into_inner()), request))
    })
    .await
}

/// Why a request is refused once a client asked to close the backend.
fn closing() -> CliError {
    CliError::new(
        "caseBackendClosing",
        "案件后台进程正在关闭，不再接受请求：它等已在运行和排队的命令结束后退出；之后命令各自运行，或再次 case open",
    )
}

async fn handle(
    backend: Arc<Backend>,
    stream: Arc<TcpStream>,
    command: Command,
    options: RequestOptions,
    progress: bool,
) {
    let path = command.path();
    let exec = Exec {
        request: RequestTasks::tracked(),
        ..backend.exec.clone()
    };
    let departure = match Departure::watch(&stream) {
        Ok(departure) => Some(departure),
        Err(error) => {
            tracing::warn!("backend request {path}: cannot watch its connection: {error}");
            None
        }
    };
    let gone = departure
        .as_ref()
        .map(|departure| departure.gone.clone())
        .unwrap_or_default();
    let concurrent = command.meta().concurrent;
    let gate = Arc::clone(&backend.gate);
    // Held until the response is sent.
    let (_shared, _alone) = tokio::select! {
        guards = async move {
            if concurrent {
                (Some(gate.read_owned().await), None)
            } else {
                (None, Some(gate.write_owned().await))
            }
        } => guards,
        () = gone.cancelled() => {
            tracing::info!("backend request {path}: its client left before it ran");
            return;
        }
        // Queued behind the close, it would never run.
        () = backend.closed.cancelled() => return refuse(stream, closing()).await,
    };
    // A query receives the progress of its own tasks; a command that runs
    // alone also everything nobody else claims.
    let claims = if concurrent {
        Claims::Own
    } else {
        Claims::Unclaimed
    };
    let (relay, subscription) = match progress.then(|| Relay::start(Arc::clone(&stream))) {
        Some(Ok((relay, events))) => (
            Some(relay),
            Some(
                exec.reporter
                    .subscribe(events, exec.request.clone(), claims),
            ),
        ),
        Some(Err(error)) => {
            tracing::warn!("backend request {path}: cannot relay its progress: {error}");
            (None, None)
        }
        None => (None, None),
    };
    let result = {
        let work = exec.run(command, &options);
        tokio::pin!(work);
        tokio::select! {
            biased;
            () = gone.cancelled() => {
                // The work still holds its tasks: cancel them first. A query
                // is dropped at once, anything else after it settles or
                // [`UNWIND`] passes; its result goes nowhere.
                let cancelled = exec.request.cancel(&exec.state);
                let settled =
                    !concurrent && tokio::time::timeout(UNWIND, &mut work).await.is_ok();
                tracing::info!(
                    "backend request {path}: its client left; cancelled {cancelled} task(s), {}",
                    if settled { "the command settled" } else { "the command was dropped" }
                );
                return;
            }
            result = &mut work => result,
        }
    };
    // The events relayed so far go out before the response.
    drop(subscription);
    if let Some(relay) = relay {
        if let Err(error) = relay.finish().await {
            // The connection broke: nothing more reaches the client.
            tracing::debug!("backend request {path}: progress delivery: {error}");
            return;
        }
    }
    let delivered = match result.map(|executed| executed.output) {
        Ok(Output::Stream(plan)) => stream_plan(&exec, stream, plan).await,
        other => reply(stream, other).await,
    };
    if let Err(error) = delivered {
        // A disconnected client never tears down the shared evidence and
        // never causes a mutation to be retried.
        tracing::debug!("backend response delivery: {error}");
    }
}

/// Writes a request's progress events to its connection while the command
/// runs, on a thread of its own: a client that stops reading stalls that
/// thread, never a runtime worker. Events wait in a bounded queue, and a
/// client that falls that far behind loses the newer ones.
struct Relay {
    /// Whether every event was written, once the last sender has dropped.
    written: oneshot::Receiver<Result<(), CliError>>,
}

impl Relay {
    /// Relay to `stream` the events sent through the returned sender.
    fn start(stream: Arc<TcpStream>) -> std::io::Result<(Self, mpsc::Sender<Event>)> {
        let (sender, mut events) = mpsc::channel(PROGRESS_QUEUE);
        let (report, written) = oneshot::channel();
        std::thread::Builder::new()
            .name("lovelycarve-cli-request-progress".into())
            .spawn(move || {
                let mut connection = &*stream;
                let mut result = Ok(());
                while let Some(event) = events.blocking_recv() {
                    if let Err(error) = protocol::header(&mut connection, &Header::Event { event })
                    {
                        // Nothing more reaches this client.
                        events.close();
                        result = Err(error);
                        break;
                    }
                }
                if report.send(result).is_err() {
                    tracing::debug!("backend progress relay outlived its request");
                }
            })?;
        Ok((Self { written }, sender))
    }

    /// Wait until the events sent before the last sender dropped are written.
    async fn finish(self) -> Result<(), CliError> {
        self.written.await.map_err(crate::cli::internal_error)?
    }
}

/// Watches a request's connection for its client going away (Ctrl+C, a
/// killed process). A client sends nothing after its request, so whatever a
/// read returns, the end of the stream, a reset or stray bytes, means the
/// request is abandoned.
struct Departure {
    gone: CancellationToken,
    settled: Arc<AtomicBool>,
}

impl Departure {
    fn watch(stream: &TcpStream) -> std::io::Result<Self> {
        let mut connection = stream.try_clone()?;
        // Nothing else reads from the connection after the request.
        connection.set_read_timeout(Some(WATCH_INTERVAL))?;
        let gone = CancellationToken::new();
        let settled = Arc::new(AtomicBool::new(false));
        let (left, done) = (gone.clone(), Arc::clone(&settled));
        std::thread::Builder::new()
            .name("lovelycarve-cli-request-watch".into())
            .spawn(move || {
                let mut byte = [0; 1];
                while !done.load(Ordering::Relaxed) {
                    match connection.read(&mut byte) {
                        Err(error)
                            if matches!(
                                error.kind(),
                                ErrorKind::WouldBlock
                                    | ErrorKind::TimedOut
                                    | ErrorKind::Interrupted
                            ) => {}
                        // A connection that ends reads as its end or a reset;
                        // a copy that thinks itself unconnected tells nothing.
                        Err(error) if error.kind() == ErrorKind::NotConnected => {
                            tracing::warn!("backend request connection cannot be watched: {error}");
                            return;
                        }
                        read => {
                            tracing::debug!("backend request connection ended: {read:?}");
                            left.cancel();
                            return;
                        }
                    }
                }
            })?;
        Ok(Self { gone, settled })
    }
}

impl Drop for Departure {
    /// The request settled: its watch ends within [`WATCH_INTERVAL`].
    fn drop(&mut self) {
        self.settled.store(true, Ordering::Relaxed);
    }
}

/// Run `work`, a read or a write of a connection, on a blocking thread: a
/// client that stalls holds that thread, never a runtime worker.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, CliError> + Send + 'static,
) -> Result<T, CliError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(crate::cli::internal_error)?
}

/// Send a JSON, byte or empty result, or an error.
async fn reply(stream: Arc<TcpStream>, output: Result<Output, CliError>) -> Result<(), CliError> {
    blocking(move || protocol::send(&mut &*stream, output)).await
}

/// Answer with `error`.
async fn refuse(stream: Arc<TcpStream>, error: CliError) {
    if let Err(delivery) = reply(stream, Err(error)).await {
        tracing::debug!("backend refusal delivery: {delivery}");
    }
}

/// Stream a file range in chunks; once the stream has started, its errors
/// travel in the stream's final status.
async fn stream_plan(exec: &Exec, stream: Arc<TcpStream>, plan: ReadPlan) -> Result<(), CliError> {
    let start = Arc::clone(&stream);
    blocking(move || protocol::header(&mut &*start, &Header::Stream)).await?;
    let sink = Arc::clone(&stream);
    // The chunks are written where they are read, on a blocking thread.
    let result = crate::cli::stream::stream(exec, plan, move |chunk| {
        let mut sink = &*sink;
        protocol::chunk(&mut sink, chunk)?;
        sink.flush()
    })
    .await
    .map(|_| ());
    blocking(move || protocol::end_stream(&mut &*stream, result)).await
}

#[cfg(test)]
mod tests;
