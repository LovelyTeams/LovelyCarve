//! A backend serving overlapping clients over loopback connections. A
//! registered task keeps `tasks wait`, a command that runs alone, waiting
//! for as long as a test needs.
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::{mpsc as channel, Arc},
    time::{Duration, Instant},
};

use serde_json::json;

use super::run;
use crate::{
    cli::{
        backend::{
            protocol::{self, Header, Request, Work},
            registry::Descriptor,
        },
        exec::{Exec, Mode, RequestOptions},
        progress::{Event, Reporter},
        CliError, Output,
    },
    host::AppHandle,
    state::AppState,
    task_manager::TaskKind,
};

/// Command futures outgrow the test harness's stack, as in the CLI.
const STACK: usize = 16 * 1024 * 1024;

/// A backend serving on a loopback port until a client closes it.
struct Fixture {
    port: u16,
    token: String,
    reporter: Reporter,
    state: Arc<AppState>,
    server: tokio::task::JoinHandle<Result<(), CliError>>,
    _directory: tempfile::TempDir,
}

fn start(runtime: &tokio::runtime::Runtime) -> Fixture {
    let _context = runtime.enter();
    let directory = tempfile::tempdir().unwrap();
    let state = Arc::new(AppState::new());
    let exec = Exec {
        host: AppHandle::new(Some(directory.path().to_owned()), |_, _| Ok(())),
        state: Arc::clone(&state),
        mode: Mode::Backend,
        reporter: Reporter::silent(),
        background: Default::default(),
        workspace: Default::default(),
        request: Default::default(),
    };
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let token = "0123456789abcdef".repeat(4);
    let descriptor = Descriptor {
        protocol: protocol::VERSION,
        case_dir: directory.path().display().to_string(),
        port,
        token: token.clone(),
        pid: std::process::id(),
        started_at: 0,
    };
    let reporter = exec.reporter.clone();
    let server = runtime.spawn(async move { run(&exec, listener, descriptor, || ()).await });
    Fixture {
        port,
        token,
        reporter,
        state,
        server,
        _directory: directory,
    }
}

impl Fixture {
    /// Send `work`; the connection its answer arrives on.
    fn send(&self, work: Work, progress: bool) -> TcpStream {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port)).unwrap();
        let request = Request {
            token: self.token.clone(),
            work,
            progress,
        };
        serde_json::to_writer(&mut stream, &request).unwrap();
        stream.write_all(b"\n").unwrap();
        stream
    }

    /// Send `work` and read its answer, which must arrive within `patience`.
    fn ask(&self, work: Work, patience: Duration) -> Result<Output, CliError> {
        let stream = self.send(work, false);
        stream.set_read_timeout(Some(patience)).unwrap();
        protocol::receive(&mut BufReader::new(&stream), &mut std::io::sink(), None)
    }

    /// Emit progress of a task no request started until `ran` says the
    /// request that receives it runs.
    fn until(&self, ran: &channel::Receiver<()>) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            self.reporter
                .event("task-progress", json!({"task": {"id": "held"}}))
                .unwrap();
            if ran.recv_timeout(Duration::from_millis(20)).is_ok() {
                return;
            }
            assert!(Instant::now() < deadline, "the request never ran");
        }
    }
}

fn runtime(workers: usize) -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .thread_stack_size(STACK)
        .enable_all()
        .build()
        .unwrap()
}

fn on_cli_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(test)
        .unwrap()
        .join()
        .unwrap();
}

/// A command as a client sends it.
fn command(argv: &[&str]) -> Work {
    let words: Vec<String> = argv.iter().map(|word| (*word).to_owned()).collect();
    Work::Run {
        command: Box::new(crate::cli::runtime::parse(&words).unwrap().command.unwrap()),
        options: RequestOptions::default(),
    }
}

/// The progress events of a response, then how it ended.
fn frames(reader: &mut impl BufRead) -> (Vec<Event>, Result<(), CliError>) {
    let mut events = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        match serde_json::from_str(&line).unwrap() {
            Header::Event { event } => events.push(event),
            Header::Json { bytes } => {
                std::io::copy(&mut reader.take(bytes), &mut std::io::sink()).unwrap();
                return (events, Ok(()));
            }
            Header::Done => return (events, Ok(())),
            Header::Error { error } => return (events, Err(error.into())),
            Header::Stream => panic!("a stream where a result was due"),
        }
    }
}

#[test]
fn a_pending_close_leaves_the_backend_answering_others() {
    on_cli_stack(|| {
        let runtime = runtime(2);
        let backend = start(&runtime);
        let held = backend
            .state
            .tasks
            .register("held", TaskKind::ImageHash, None);
        let (running, ran) = channel::channel();
        let stream = backend.send(command(&["tasks", "wait"]), true);
        let waiter = std::thread::spawn(move || {
            let mut reader = BufReader::new(&stream);
            let mut first = String::new();
            reader.read_line(&mut first).unwrap();
            running.send(()).unwrap();
            (first, frames(&mut reader))
        });
        // The progress of a task no request started reaches the command
        // that runs alone.
        backend.until(&ran);
        let closer = {
            let stream = backend.send(Work::Close, false);
            std::thread::spawn(move || {
                protocol::receive(&mut BufReader::new(&stream), &mut std::io::sink(), None)
            })
        };
        // The close waits for `tasks wait`; meanwhile every client is
        // answered at once, and told why once the close is known.
        let deadline = Instant::now() + Duration::from_secs(30);
        let refused = loop {
            match backend.ask(Work::Info, Duration::from_secs(5)) {
                Ok(_) => assert!(Instant::now() < deadline, "the close never arrived"),
                Err(error) => break error,
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(refused.code, "caseBackendClosing", "{}", refused.message);
        let refused = backend
            .ask(command(&["case", "status"]), Duration::from_secs(5))
            .unwrap_err();
        assert_eq!(refused.code, "caseBackendClosing", "{}", refused.message);
        // Once the command that runs alone ends, the backend closes.
        drop(held);
        let (first, (_, waited)) = waiter.join().unwrap();
        assert!(first.contains(r#""kind":"event""#), "{first}");
        waited.unwrap();
        assert!(matches!(closer.join().unwrap(), Ok(Output::Written)));
        runtime.block_on(backend.server).unwrap().unwrap();
    });
}

#[test]
fn a_client_that_stops_reading_holds_no_runtime_worker() {
    on_cli_stack(|| {
        // One worker: a write that held it would stall the whole backend.
        let runtime = runtime(1);
        let backend = start(&runtime);
        let held = backend
            .state
            .tasks
            .register("held", TaskKind::ImageHash, None);
        let (running, ran) = channel::channel();
        let (resume, resumed) = channel::channel::<()>();
        let stream = backend.send(command(&["tasks", "wait"]), true);
        let client = std::thread::spawn(move || {
            let mut reader = BufReader::new(&stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            running.send(()).unwrap();
            // It stops reading until told to go on.
            resumed.recv().unwrap();
            frames(&mut reader)
        });
        backend.until(&ran);
        // The progress meant for it far outgrows what its connection holds.
        let sent = 4096;
        let message = "x".repeat(32 * 1024);
        let flood = Instant::now();
        for sequence in 1..=sent {
            backend
                .reporter
                .event(
                    "task-progress",
                    json!({"task": {"id": "held"}, "sequence": sequence, "message": message}),
                )
                .unwrap();
        }
        assert!(
            flood.elapsed() < Duration::from_secs(20),
            "relaying progress held the emitter for {:?}",
            flood.elapsed()
        );
        // The backend still answers on its only worker.
        let info = backend.ask(Work::Info, Duration::from_secs(5)).unwrap();
        assert!(matches!(info, Output::Json(_)));
        resume.send(()).unwrap();
        drop(held);
        let (events, waited) = client.join().unwrap();
        waited.unwrap();
        // The client then reads what was kept for it, in order: a bounded
        // share of the flood, never the whole of it.
        let delivered: Vec<u64> = events
            .iter()
            .filter_map(|event| event.payload["sequence"].as_u64())
            .collect();
        assert!(
            delivered.windows(2).all(|pair| pair[0] < pair[1]),
            "progress out of order"
        );
        assert!(
            !delivered.is_empty() && delivered.len() < sent as usize,
            "{} of {sent} events reached a client that stopped reading",
            delivered.len()
        );
        assert!(matches!(
            backend.ask(Work::Close, Duration::from_secs(30)),
            Ok(Output::Written)
        ));
        runtime.block_on(backend.server).unwrap().unwrap();
    });
}
