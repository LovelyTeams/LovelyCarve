//! A case backend serving other processes. It owns one `AppState` with the
//! case open; clients never recreate it. Read-only queries run side by side,
//! every other command runs alone. A request whose client goes away is
//! cancelled: it stops waiting for its turn or its work, and the tasks it
//! started are cancelled.
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
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use super::{
    protocol::{self, Header, Work},
    registry::{self, Descriptor, Registration},
};
use crate::cli::{
    cmd::Command,
    exec::{Exec, RequestOptions, RequestTasks},
    CliError, Output,
};

/// How often a request's watch looks up from its connection to see whether
/// the request has settled.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);
/// How long a command other than a query still runs once its client left:
/// time to finish, or to unwind the tasks cancelled for it, so a change is
/// not cut off half-way. One that only waits (`tasks wait`) is dropped then.
const UNWIND: Duration = Duration::from_secs(2);

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
    let gate = Arc::new(RwLock::new(()));
    loop {
        let (stream, _) = match listener.accept() {
            Ok(connection) => connection,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                tokio::time::sleep(Duration::from_millis(25)).await;
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        // Windows inherits the listener's nonblocking mode on accepted
        // sockets; the framed transport uses blocking reads with timeouts.
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(60)))?;
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
                    Err(CliError::new("permissionDenied", "后台进程凭据无效")),
                ) {
                    tracing::debug!("backend authentication response: {error}");
                }
                continue;
            }
            Err(error) => {
                if let Err(error) = protocol::send(&mut stream, Err(error)) {
                    tracing::debug!("backend request response: {error}");
                }
                continue;
            }
        };
        match request.work {
            Work::Close => {
                // Wait for what still runs alone, then cancel the rest; a
                // failure nobody waited for is still reported.
                let _exclusive = gate.write().await;
                let report = exec.background.take().verdict(Some("案件后台进程已关闭"));
                crate::cli::runtime::shutdown(&exec.state).await;
                drop(registration);
                protocol::send(&mut stream, report.map(|_| Output::Written))?;
                return Ok(0);
            }
            Work::Info => {
                let evidence = exec.state.images.lock().await.len();
                let info = json!({
                    "caseDir": descriptor.case_dir,
                    "pid": descriptor.pid,
                    "startedAt": descriptor.started_at,
                    "protocol": descriptor.protocol,
                    "openEvidence": evidence,
                    "running": true,
                });
                if let Err(error) = protocol::send(&mut stream, Ok(Output::json(info))) {
                    tracing::debug!("backend info response: {error}");
                }
            }
            Work::Run { command, options } => {
                let exec = exec.clone();
                let gate = Arc::clone(&gate);
                let progress = request.progress;
                tokio::spawn(async move {
                    handle(exec, gate, stream, *command, options, progress).await;
                });
            }
        }
    }
}

async fn handle(
    exec: Exec,
    gate: Arc<RwLock<()>>,
    mut stream: TcpStream,
    command: Command,
    options: RequestOptions,
    progress: bool,
) {
    let path = command.path();
    let exec = Exec {
        request: RequestTasks::tracked(),
        ..exec
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
    };
    let (sender, mut events) = tokio::sync::mpsc::unbounded_channel();
    let subscription = progress.then(|| exec.reporter.subscribe(sender));
    let mut relaying = subscription.is_some();
    let result = {
        let work = exec.run(command, &options);
        tokio::pin!(work);
        loop {
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
                event = events.recv(), if relaying => match event {
                    Some(event) => {
                        if let Err(error) = protocol::header(&mut stream, &Header::Event { event }) {
                            tracing::debug!("backend progress delivery: {error}");
                        }
                    }
                    // Another request took the progress subscription over;
                    // a closed channel would answer at once, forever.
                    None => relaying = false,
                },
                result = &mut work => break result,
            }
        }
    };
    drop(subscription);
    while let Ok(event) = events.try_recv() {
        if let Err(error) = protocol::header(&mut stream, &Header::Event { event }) {
            tracing::debug!("backend final progress delivery: {error}");
        }
    }
    let delivered = match result.map(|executed| executed.output) {
        Ok(Output::Stream(plan)) => stream_plan(&exec, &mut stream, plan).await,
        other => protocol::send(&mut stream, other),
    };
    if let Err(error) = delivered {
        // A disconnected client never tears down the shared evidence and
        // never causes a mutation to be retried.
        tracing::debug!("backend response delivery: {error}");
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

/// Stream a file range in chunks; once the stream has started, its errors
/// travel in the stream's final status.
async fn stream_plan(
    exec: &Exec,
    stream: &mut TcpStream,
    plan: crate::cli::stream::ReadPlan,
) -> Result<(), CliError> {
    protocol::header(stream, &Header::Stream)?;
    let mut sink = stream.try_clone()?;
    let result = crate::cli::stream::stream(exec, plan, move |chunk| {
        protocol::chunk(&mut sink, chunk)?;
        sink.flush()
    })
    .await
    .map(|_| ());
    protocol::end_stream(stream, result)
}
