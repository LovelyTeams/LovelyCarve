//! Hands a `lovelycarve://` link from a desktop process the system has just
//! started to one that is already running. Every running desktop process
//! publishes a record (`desktop-links/<id>.json`: port, token and process ID)
//! beside a lock file it holds for as long as it runs, and takes links on a
//! loopback socket. A process started with a link offers it to the running
//! ones: first to the one that has the link's case open, then to one with no
//! case open; when none takes it, the new process opens the link itself. A
//! link that does not parse goes to any running process, which reports it.
use std::{
    fs::{File, OpenOptions, TryLockError},
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

use serde::{Deserialize, Serialize};

use super::plugin_install_link::PluginInstallLink;
use crate::core::casework::evidence_link::EvidenceLink;

const PROTOCOL: u32 = 1;
const RECORD_DIRECTORY: &str = "desktop-links";
/// One offer or reply line; links stay far below it.
const MAX_MESSAGE: u64 = 512 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const REPLY_TIMEOUT: Duration = Duration::from_secs(3);

/// Where running desktop processes publish their records.
pub(crate) fn directory() -> Option<PathBuf> {
    crate::host::default_app_data_dir().map(|directory| directory.join(RECORD_DIRECTORY))
}

/// The link the system started this process with: its only argument.
pub(crate) fn launch_link() -> Option<String> {
    let mut arguments = std::env::args_os().skip(1);
    let first = arguments.next()?;
    if arguments.next().is_some() {
        return None;
    }
    let text = first.into_string().ok()?;
    EvidenceLink::has_scheme(&text).then_some(text)
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Record {
    protocol: u32,
    port: u16,
    token: String,
    pid: u32,
}

/// Which running process may take a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Accept {
    /// The process that has the link's case open.
    Case,
    /// A process with no case open.
    Idle,
    /// Any process: the link does not parse, and the process says so.
    Any,
    /// A process that understands plugin installs. Older desktops do not
    /// recognize this offer and leave it for the sending process.
    PluginInstall,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Offer {
    protocol: u32,
    token: String,
    accept: Accept,
    url: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Reply {
    accepted: bool,
}

/// What a running process has open when a link is offered to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Workspace {
    Empty,
    /// The case uid.
    Case(String),
    /// A case is being opened or closed; links that depend on it wait for
    /// another process.
    Busy,
}

impl Workspace {
    fn takes(&self, accept: Accept, url: &str) -> bool {
        match (accept, self) {
            (Accept::Any, _) | (Accept::Idle, Self::Empty) => true,
            (Accept::PluginInstall, _) => PluginInstallLink::matches(url),
            (Accept::Case, Self::Case(case_id)) => {
                EvidenceLink::parse(url).is_ok_and(|link| link.case_id() == case_id)
            }
            (Accept::Idle, Self::Case(_) | Self::Busy)
            | (Accept::Case, Self::Empty | Self::Busy) => false,
        }
    }
}

/// This process's link endpoint, published until it is dropped.
pub(crate) struct Relay {
    record: PathBuf,
    token: String,
    _lock: File,
}

impl Relay {
    /// Publish this process in `directory` and take links on a loopback
    /// socket: `workspace` says what the process has open when a link is
    /// offered, `deliver` receives every link it takes.
    pub(crate) fn start(
        directory: &Path,
        workspace: impl Fn() -> Workspace + Send + 'static,
        deliver: impl Fn(String) + Send + 'static,
    ) -> std::io::Result<Self> {
        create_private_directory(directory)?;
        sweep(directory);
        let id = uuid::Uuid::new_v4().simple().to_string();
        let lock = private_options().open(directory.join(format!("{id}.lock")))?;
        lock.try_lock().map_err(|error| match error {
            TryLockError::Error(error) => error,
            TryLockError::WouldBlock => std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "link relay lock is held by another process",
            ),
        })?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let record = directory.join(format!("{id}.json"));
        write_record(
            directory,
            &record,
            &Record {
                protocol: PROTOCOL,
                port: listener.local_addr()?.port(),
                token: token.clone(),
                pid: std::process::id(),
            },
        )?;
        let expected = token.clone();
        std::thread::Builder::new()
            .name("evidence-link-relay".into())
            .spawn(move || serve(&listener, &expected, &workspace, &deliver))?;
        Ok(Self {
            record,
            token,
            _lock: lock,
        })
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        // Never unlink a record another process wrote under the same name.
        let owned = read_record(&self.record).is_some_and(|record| record.token == self.token);
        if owned {
            if let Err(error) = std::fs::remove_file(&self.record) {
                tracing::warn!("remove link relay record: {error}");
            }
            remove_stale(&self.record.with_extension("lock"));
        }
    }
}

fn serve(
    listener: &TcpListener,
    token: &str,
    workspace: &impl Fn() -> Workspace,
    deliver: &impl Fn(String),
) {
    for stream in listener.incoming() {
        let answered = stream.and_then(|stream| answer(&stream, token, workspace, deliver));
        if let Err(error) = answered {
            tracing::debug!("link offer: {error}");
        }
    }
}

fn answer(
    stream: &TcpStream,
    token: &str,
    workspace: &impl Fn() -> Workspace,
    deliver: &impl Fn(String),
) -> std::io::Result<()> {
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
    stream.set_write_timeout(Some(REPLY_TIMEOUT))?;
    let offer: Offer = serde_json::from_str(&read_line(stream)?)?;
    let accepted = offer.protocol == PROTOCOL
        && offer.token == token
        && workspace().takes(offer.accept, &offer.url);
    send(stream, &Reply { accepted })?;
    // Only a link whose acceptance reached the sender is delivered, so the
    // sender never also opens it itself.
    if accepted {
        deliver(offer.url);
    }
    Ok(())
}

/// Offer `url` to the running desktop processes; whether one took it.
pub(crate) fn forward(directory: &Path, url: &str) -> bool {
    let records = live_records(directory);
    if records.is_empty() {
        return false;
    }
    let passes: &[Accept] = if PluginInstallLink::matches(url) {
        &[Accept::PluginInstall]
    } else if EvidenceLink::parse(url).is_ok() {
        &[Accept::Case, Accept::Idle]
    } else {
        &[Accept::Any]
    };
    for accept in passes {
        for record in &records {
            match offer(record, *accept, url) {
                Ok(true) => return true,
                Ok(false) => {}
                Err(error) => tracing::debug!("offer a link to process {}: {error}", record.pid),
            }
        }
    }
    false
}

fn offer(record: &Record, accept: Accept, url: &str) -> std::io::Result<bool> {
    allow_foreground(record.pid);
    let stream = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, record.port)),
        CONNECT_TIMEOUT,
    )?;
    stream.set_read_timeout(Some(REPLY_TIMEOUT))?;
    stream.set_write_timeout(Some(REPLY_TIMEOUT))?;
    send(
        &stream,
        &Offer {
            protocol: PROTOCOL,
            token: record.token.clone(),
            accept,
            url: url.to_owned(),
        },
    )?;
    let reply: Reply = serde_json::from_str(&read_line(&stream)?)?;
    Ok(reply.accepted)
}

fn send(mut stream: &TcpStream, message: &impl Serialize) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(message)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    stream.flush()
}

fn read_line(stream: &TcpStream) -> std::io::Result<String> {
    let mut line = String::new();
    BufReader::new(Read::take(stream, MAX_MESSAGE)).read_line(&mut line)?;
    Ok(line)
}

/// The receiving window may only come to the front with the starting
/// process's permission on Windows; without it the taskbar button flashes.
#[cfg(windows)]
fn allow_foreground(pid: u32) {
    // SAFETY: AllowSetForegroundWindow takes a process ID and touches no
    // memory of ours.
    let allowed = unsafe { winapi::um::winuser::AllowSetForegroundWindow(pid) };
    if allowed == 0 {
        tracing::debug!("AllowSetForegroundWindow({pid}) failed");
    }
}

#[cfg(not(windows))]
fn allow_foreground(_pid: u32) {}

/// Whether the process that wrote a record still holds its lock.
enum Liveness {
    Running,
    Gone,
    Unknown,
}

fn liveness(lock: &Path) -> Liveness {
    let file = match File::open(lock) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Liveness::Gone,
        Err(_) => return Liveness::Unknown,
    };
    match file.try_lock_shared() {
        Ok(()) => Liveness::Gone,
        Err(TryLockError::WouldBlock) => Liveness::Running,
        Err(TryLockError::Error(_)) => Liveness::Unknown,
    }
}

/// Records of running processes, the most recently started first. Records
/// whose process has exited are removed.
fn live_records(directory: &Path) -> Vec<Record> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut records = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let lock = path.with_extension("lock");
        match liveness(&lock) {
            Liveness::Running | Liveness::Unknown => {}
            Liveness::Gone => {
                remove_stale(&path);
                remove_stale(&lock);
                continue;
            }
        }
        let Some(record) = read_record(&path).filter(|record| record.protocol == PROTOCOL) else {
            continue;
        };
        let started = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok();
        records.push((started, record));
    }
    records.sort_by_key(|(started, _)| std::cmp::Reverse(*started));
    records.into_iter().map(|(_, record)| record).collect()
}

/// Remove what processes that exited without cleaning up left behind.
fn sweep(directory: &Path) {
    live_records(directory);
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let orphan = path
            .extension()
            .is_some_and(|extension| extension == "lock")
            && !path.with_extension("json").exists();
        if orphan && matches!(liveness(&path), Liveness::Gone) {
            remove_stale(&path);
        }
    }
}

fn remove_stale(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::debug!("remove stale link relay file {}: {error}", path.display()),
    }
}

fn read_record(path: &Path) -> Option<Record> {
    let file = File::open(path).ok()?;
    serde_json::from_reader(Read::take(file, MAX_MESSAGE)).ok()
}

fn write_record(directory: &Path, path: &Path, record: &Record) -> std::io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    serde_json::to_writer(file.as_file_mut(), record)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn create_private_directory(directory: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory)
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    const LINK_C1: &str = "lovelycarve://open?case=c1&evidence=e&node=n";
    const LINK_C2: &str = "lovelycarve://open?case=c2";

    #[test]
    fn release_page_install_reaches_a_running_workspace_without_opening_a_case() {
        const LINK: &str = "lovelycarve://install-plugin?id=org.example.demo&version=1.0.0";
        let directory = tempfile::tempdir().expect("tempdir");
        let (_holder, holder_links) = relay(directory.path(), Workspace::Case("c1".into()));
        assert!(forward(directory.path(), LINK));
        assert_eq!(received(&holder_links).as_deref(), Some(LINK));
        assert!(!Workspace::Busy.takes(Accept::PluginInstall, LINK_C1));
    }

    fn relay(directory: &Path, workspace: Workspace) -> (Relay, mpsc::Receiver<String>) {
        let (sender, receiver) = mpsc::channel();
        let relay = Relay::start(
            directory,
            move || workspace.clone(),
            move |url| {
                if sender.send(url).is_err() {
                    tracing::debug!("test receiver dropped");
                }
            },
        )
        .expect("start relay");
        (relay, receiver)
    }

    fn received(receiver: &mpsc::Receiver<String>) -> Option<String> {
        receiver.recv_timeout(Duration::from_secs(5)).ok()
    }

    /// Every link the receivers get within a moment: delivery follows the
    /// reply, so it may land just after `forward` returns.
    fn all_received(receivers: &[&mpsc::Receiver<String>]) -> Vec<String> {
        std::thread::sleep(Duration::from_millis(300));
        receivers
            .iter()
            .flat_map(|receiver| receiver.try_iter())
            .collect()
    }

    #[test]
    fn a_link_goes_to_its_case_then_to_an_idle_process() {
        let directory = tempfile::tempdir().expect("tempdir");
        assert!(!forward(directory.path(), LINK_C1));
        let (holder, holder_links) = relay(directory.path(), Workspace::Case("c1".into()));
        assert!(
            !forward(directory.path(), LINK_C2),
            "no process may take c2"
        );
        let (_idle, idle_links) = relay(directory.path(), Workspace::Empty);
        let (_busy, busy_links) = relay(directory.path(), Workspace::Busy);

        assert!(forward(directory.path(), LINK_C1));
        assert_eq!(received(&holder_links).as_deref(), Some(LINK_C1));
        assert!(forward(directory.path(), LINK_C2));
        assert_eq!(received(&idle_links).as_deref(), Some(LINK_C2));
        assert!(forward(directory.path(), "lovelycarve://unknown"));
        assert_eq!(
            all_received(&[&holder_links, &idle_links, &busy_links]),
            ["lovelycarve://unknown"],
        );

        drop(holder);
        assert!(
            forward(directory.path(), LINK_C1),
            "the idle process takes it"
        );
        assert_eq!(received(&idle_links).as_deref(), Some(LINK_C1));
        assert!(all_received(&[&holder_links, &busy_links]).is_empty());
    }

    #[test]
    fn records_of_exited_processes_are_removed_and_tokens_checked() {
        let directory = tempfile::tempdir().expect("tempdir");
        let stale = directory.path().join("gone.json");
        write_record(
            directory.path(),
            &stale,
            &Record {
                protocol: PROTOCOL,
                port: 9,
                token: "t".repeat(64),
                pid: 1,
            },
        )
        .expect("record");
        std::fs::write(directory.path().join("gone.lock"), b"").expect("lock");
        std::fs::write(directory.path().join("orphan.lock"), b"").expect("orphan");
        assert!(!forward(directory.path(), LINK_C2));
        assert!(!stale.exists() && !directory.path().join("gone.lock").exists());

        let (relay, links) = relay(directory.path(), Workspace::Empty);
        assert!(!directory.path().join("orphan.lock").exists());
        let mut record = read_record(&relay.record).expect("published");
        record.token = "x".repeat(64);
        assert!(!offer(&record, Accept::Any, LINK_C2).expect("reply"));
        assert!(links.try_recv().is_err());
        drop(relay);
        assert!(live_records(directory.path()).is_empty());
    }

    #[test]
    fn processes_take_links_by_what_they_have_open() {
        let holder = Workspace::Case("c1".into());
        assert!(holder.takes(Accept::Case, LINK_C1));
        assert!(!holder.takes(Accept::Case, LINK_C2));
        assert!(!holder.takes(Accept::Case, "lovelycarve://open"));
        assert!(!holder.takes(Accept::Idle, LINK_C2));
        assert!(Workspace::Empty.takes(Accept::Idle, LINK_C2));
        assert!(!Workspace::Busy.takes(Accept::Idle, LINK_C2));
        assert!(Workspace::Busy.takes(Accept::Any, "lovelycarve://unknown"));
    }
}
