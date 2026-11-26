//! Talking to a case backend without ever starting one implicitly.
use std::{
    future::Future,
    io::{BufReader, Write},
    net::{Ipv4Addr, Shutdown, SocketAddr, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard, Once,
    },
    time::{Duration, Instant},
};

use serde_json::Value;

use super::{
    protocol::{self, Request, Work},
    registry,
};
use crate::cli::{progress::Reporter, CliError, Output};

/// Send `work` to the backend of `case_dir`. Raw bytes go to `sink`,
/// progress events to `reporter`.
pub(super) fn exchange(
    case_dir: &str,
    work: Work,
    reporter: Option<&Reporter>,
    sink: &mut impl Write,
) -> Result<Output, CliError> {
    let descriptor = match registry::read(case_dir) {
        Ok(Some(descriptor)) => descriptor,
        Ok(None) => {
            return Err(CliError::new(
                "caseBackendNotRunning",
                format!("案件 {case_dir} 没有运行中的后台进程；用 case open 启动"),
            ))
        }
        // An unreadable or incompatible record a dead backend left behind.
        Err(error) if registry::clear_stale(case_dir)? => {
            tracing::debug!("stale case backend record {case_dir}: {}", error.message);
            return Err(registry::exited(case_dir));
        }
        Err(error) => return Err(error),
    };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, descriptor.port));
    let stream = match TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
        Ok(stream) => stream,
        Err(_) if registry::clear_stale(case_dir)? => return Err(registry::exited(case_dir)),
        Err(error) => {
            return Err(CliError::new(
                "caseBackendUnavailable",
                format!(
                    "案件后台进程仍在运行但没有应答（{error}）：可能还在打开案件，或已无响应；日志见 {}",
                    registry::log_path(case_dir)?.display()
                ),
            ))
        }
    };
    // Shared rather than duplicated: on Windows a duplicate of a socket
    // connected with a timeout can report itself unconnected, and its
    // shutdown would then close nothing.
    let stream = Arc::new(stream);
    let _open = Open::register(&stream);
    let request = Request {
        token: descriptor.token,
        work,
        progress: reporter.is_some(),
    };
    let exchanged = round_trip(&stream, &request, reporter, sink);
    if OPEN.interrupted.load(Ordering::SeqCst) {
        // Ctrl+C closed the connection under it; the backend cancels the
        // request when it sees the connection close.
        return Err(CliError::cancelled());
    }
    exchanged
}

/// Send `request` and read the response to it.
fn round_trip(
    stream: &TcpStream,
    request: &Request,
    reporter: Option<&Reporter>,
    sink: &mut impl Write,
) -> Result<Output, CliError> {
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    if matches!(request.work, Work::Info) {
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    }
    let mut writer = stream;
    serde_json::to_writer(&mut writer, request)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    protocol::receive(&mut BufReader::new(stream), sink, reporter)
}

/// The running backend's state.
pub(super) fn info(case_dir: &str) -> Result<Value, CliError> {
    match exchange(case_dir, Work::Info, None, &mut std::io::sink())? {
        Output::Json(value) => Ok(value.value().clone()),
        _ => Err(crate::cli::internal_error("后台进程状态返回了非 JSON 结果")),
    }
}

/// How long an interruption waits for the open exchanges to return, so their
/// commands can report that they were cancelled.
const GRACE: Duration = Duration::from_secs(3);
/// How long it then leaves the commands to finish that report.
const SETTLE: Duration = Duration::from_millis(250);

/// The connections this process has open to case backends. Closing one is
/// how a client cancels its request: the backend sees its client go away
/// and cancels what the request started.
struct Connections {
    interrupted: AtomicBool,
    next: AtomicU64,
    open: Mutex<Vec<(u64, Arc<TcpStream>)>>,
}

static OPEN: Connections = Connections {
    interrupted: AtomicBool::new(false),
    next: AtomicU64::new(0),
    open: Mutex::new(Vec::new()),
};

impl Connections {
    fn lock(&self) -> MutexGuard<'_, Vec<(u64, Arc<TcpStream>)>> {
        self.open
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// One exchange's connection while the exchange runs.
struct Open(u64);

impl Open {
    fn register(stream: &Arc<TcpStream>) -> Self {
        let mut open = OPEN.lock();
        if OPEN.interrupted.load(Ordering::SeqCst) {
            drop(open);
            // The process is about to end with 130. Nothing new starts in
            // the meantime, or a batch would answer every request it reads.
            loop {
                std::thread::park();
            }
        }
        let id = OPEN.next.fetch_add(1, Ordering::Relaxed);
        open.push((id, Arc::clone(stream)));
        Self(id)
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        OPEN.lock().retain(|(id, _)| *id != self.0);
    }
}

/// From now on, Ctrl+C / Ctrl+Break ends this client of a case backend with
/// 130 instead of the console's default termination: every open connection
/// closes, so the backend cancels its request, and the exchanges return
/// [`CliError::cancelled`] for their commands to report. A second
/// interruption ends the process at once, as in a process that holds the
/// case itself.
pub(in crate::cli) fn guard_interrupts() {
    static GUARD: Once = Once::new();
    GUARD.call_once(|| {
        if let Err(error) = listen() {
            tracing::warn!("Ctrl+C handler unavailable: {error}");
        }
    });
}

fn listen() -> std::io::Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let mut interrupt = Box::pin(crate::cli::runtime::interrupted());
    // The first poll installs the handlers, before any request is sent.
    let first = {
        let _context = runtime.enter();
        interrupt
            .as_mut()
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    };
    std::thread::Builder::new()
        .name("lovelycarve-cli-interrupt".into())
        .spawn(move || {
            if first.is_pending() {
                runtime.block_on(interrupt);
            }
            close_and_exit()
        })?;
    Ok(())
}

/// Close every open connection, give the exchanges a moment to return, then
/// end with 130.
fn close_and_exit() -> ! {
    let waiting = {
        let open = OPEN.lock();
        OPEN.interrupted.store(true, Ordering::SeqCst);
        for (_, connection) in open.iter() {
            if let Err(error) = connection.shutdown(Shutdown::Both) {
                tracing::debug!("close case backend connection: {error}");
            }
        }
        !open.is_empty()
    };
    if waiting {
        let deadline = Instant::now() + GRACE;
        while !OPEN.lock().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(SETTLE);
    }
    std::process::exit(130)
}
