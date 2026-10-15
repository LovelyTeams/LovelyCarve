//! Plugin log files.
//!
//! A plugin's `Context::log` line reaches the host either as an ABI callback
//! (`isolation = "in_process"`) or as a `LEVEL message` line on the loader's
//! stderr (`isolation = "process"`). Both paths hand the line to a
//! [`PluginLogStore`], which appends it to that plugin's own file under the
//! app data directory, so plugin output outlives the session and no longer
//! depends on the `warn` default of the tracing subscriber.
//!
//! `record` never blocks its caller: it queues the line for a writer thread and
//! returns, so a slow disk cannot stall a plugin call. The queue is bounded, and
//! the lines dropped when it fills up are counted and reported in the file.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{ErrorKind, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{sync_channel, Receiver, SyncSender, TrySendError},
        Arc,
    },
    time::SystemTime,
};

use lovelycarve_plugin::abi;

use super::Result;

/// Log lines waiting for the writer thread before new ones are dropped.
const QUEUE_CAPACITY: usize = 4096;
/// A plugin's file is rotated once it reaches this size; one previous
/// generation is kept as `<name>.log.1`.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Longest plugin id fragment kept in a file name.
const MAX_FILE_NAME_CHARS: usize = 120;

/// Severity of one plugin log line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginLogLevel {
    Error,
    Warn,
    Info,
    Debug,
    /// A diagnostic the host wrote about the plugin instead of one the plugin
    /// sent: an unprefixed loader stderr line.
    Host,
}

impl PluginLogLevel {
    /// The level of an `abi::LOG_*` callback argument.
    pub(super) fn from_abi(level: u32) -> Self {
        match level {
            abi::LOG_ERROR => Self::Error,
            abi::LOG_WARN => Self::Warn,
            abi::LOG_INFO => Self::Info,
            _ => Self::Debug,
        }
    }

    /// Splits the `LEVEL message` prefix `lc-plugin-host` writes on stderr.
    fn from_stderr_line(line: &str) -> (Self, &str) {
        for (prefix, level) in [
            ("ERROR ", Self::Error),
            ("WARN ", Self::Warn),
            ("INFO ", Self::Info),
            ("DEBUG ", Self::Debug),
        ] {
            if let Some(message) = line.strip_prefix(prefix) {
                return (level, message);
            }
        }
        (Self::Host, line)
    }

    fn label(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
            Self::Host => "HOST",
        }
    }
}

/// One queued log line.
struct Record {
    plugin_id: String,
    level: PluginLogLevel,
    message: String,
    at: SystemTime,
}

enum Message {
    Line(Record),
    /// Test hook: lets a test wait until everything queued before it has been
    /// written.
    #[cfg(test)]
    Ack(std::sync::mpsc::Sender<()>),
}

/// One plugin's open log file and how many bytes it holds.
struct OpenLog {
    file: File,
    bytes: u64,
}

impl OpenLog {
    fn open(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let bytes = file.metadata().map(|meta| meta.len()).unwrap_or(0);
        Ok(Self { file, bytes })
    }

    /// Appends one line, rotating first so a plugin's file stays bounded.
    fn write_line(&mut self, path: &Path, line: &str, max_file_bytes: u64) -> std::io::Result<()> {
        if self.bytes > 0 && self.bytes + line.len() as u64 > max_file_bytes {
            self.rotate(path)?;
        }
        self.file.write_all(line.as_bytes())?;
        self.bytes += line.len() as u64;
        Ok(())
    }

    /// Moves the current file to `<name>.log.1`, replacing the previous
    /// generation, and starts a new one.
    fn rotate(&mut self, path: &Path) -> std::io::Result<()> {
        let previous = path.with_extension("log.1");
        match fs::remove_file(&previous) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        self.file.flush()?;
        fs::rename(path, &previous)?;
        self.file = OpenOptions::new().create(true).append(true).open(path)?;
        self.bytes = 0;
        Ok(())
    }
}

/// Appends plugin log lines to one file per plugin.
///
/// One writer thread owns every file, so a plugin call only ever pays for
/// cloning its line.
pub struct PluginLogStore {
    root: PathBuf,
    sender: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
}

impl PluginLogStore {
    /// Opens `root`, creating it, and starts the writer thread.
    pub fn open(root: impl Into<PathBuf>) -> Result<Arc<Self>> {
        Self::with_limits(root.into(), MAX_FILE_BYTES, QUEUE_CAPACITY)
    }

    fn with_limits(root: PathBuf, max_file_bytes: u64, queue_capacity: usize) -> Result<Arc<Self>> {
        fs::create_dir_all(&root)?;
        let (sender, receiver) = sync_channel(queue_capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let writer_root = root.clone();
        let writer_dropped = Arc::clone(&dropped);
        std::thread::Builder::new()
            .name("plugin-log".to_owned())
            .spawn(move || writer_loop(&writer_root, max_file_bytes, &receiver, &writer_dropped))?;
        Ok(Arc::new(Self {
            root,
            sender,
            dropped,
        }))
    }

    /// The file the plugin's lines are appended to.
    pub fn file_path(&self, plugin_id: &str) -> PathBuf {
        self.root.join(log_file_name(plugin_id))
    }

    /// Queues one `Context::log` line. Returns immediately, also when the writer
    /// falls behind: a plugin is never blocked by its own log file.
    pub(super) fn record(&self, plugin_id: &str, level: PluginLogLevel, message: &str) {
        for line in message.lines() {
            let line = line.trim_end();
            if !line.is_empty() {
                self.push(plugin_id, level, line);
            }
        }
    }

    /// Queues a chunk of the isolated loader's stderr, which carries one
    /// `LEVEL message` line per plugin log call.
    pub(super) fn record_stderr(&self, plugin_id: &str, chunk: &str) {
        for line in chunk.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            let (level, message) = PluginLogLevel::from_stderr_line(line);
            self.push(plugin_id, level, message);
        }
    }

    fn push(&self, plugin_id: &str, level: PluginLogLevel, message: &str) {
        let record = Record {
            plugin_id: plugin_id.to_owned(),
            level,
            message: message.to_owned(),
            at: SystemTime::now(),
        };
        match self.sender.try_send(Message::Line(record)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {}
        }
    }
}

fn writer_loop(
    root: &Path,
    max_file_bytes: u64,
    receiver: &Receiver<Message>,
    dropped: &AtomicU64,
) {
    let mut files: HashMap<String, Option<OpenLog>> = HashMap::new();
    let mut reported_drops = 0_u64;
    while let Ok(message) = receiver.recv() {
        #[cfg(test)]
        let record = match message {
            Message::Line(record) => record,
            Message::Ack(ack) => {
                let _ = ack.send(());
                continue;
            }
        };
        #[cfg(not(test))]
        let Message::Line(record) = message;
        let path = root.join(log_file_name(&record.plugin_id));
        let open = files.entry(record.plugin_id.clone()).or_insert_with(|| {
            OpenLog::open(&path).map_or_else(
                |error| {
                    tracing::debug!(
                        plugin_id = %record.plugin_id,
                        "plugin log file {} is unavailable: {error}",
                        path.display()
                    );
                    None
                },
                Some,
            )
        });
        let Some(open) = open.as_mut() else {
            continue;
        };
        let total_drops = dropped.load(Ordering::Relaxed);
        if total_drops > reported_drops {
            reported_drops = total_drops;
            // The dropped lines cannot be attributed to one plugin any more,
            // so the notice goes to the file being written.
            let notice = format!(
                "{} {} {total_drops} plugin log lines were dropped: the writer queue was full\n",
                format_time(record.at),
                PluginLogLevel::Warn.label(),
            );
            if let Err(error) = open.write_line(&path, &notice, max_file_bytes) {
                tracing::debug!(plugin_id = %record.plugin_id, "plugin log write failed: {error}");
            }
        }
        let line = format!(
            "{} {} {}\n",
            format_time(record.at),
            record.level.label(),
            record.message
        );
        if let Err(error) = open.write_line(&path, &line, max_file_bytes) {
            tracing::debug!(plugin_id = %record.plugin_id, "plugin log write failed: {error}");
        }
    }
}

fn format_time(at: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(at).to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// A plugin id is already restricted to ASCII letters, digits, `.`, `-` and
/// `_`, but the name is rebuilt defensively and never left as a Win32 device
/// name.
fn log_file_name(plugin_id: &str) -> String {
    let cleaned: String = plugin_id
        .chars()
        .take(MAX_FILE_NAME_CHARS)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    let stem = cleaned.split('.').next().unwrap_or_default();
    if stem.is_empty()
        || crate::core::filesystem::tree::fs_tree_export::is_windows_device_name(stem)
    {
        format!("_{cleaned}.log")
    } else {
        format!("{cleaned}.log")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLUGIN_ID: &str = "com.example.plugin";

    fn store(root: &Path) -> Arc<PluginLogStore> {
        PluginLogStore::with_limits(root.to_owned(), 512, 64).expect("plugin log store")
    }

    fn read(path: &Path) -> String {
        fs::read_to_string(path).expect("read log file")
    }

    /// The `line N` numbers of a log file, oldest first. `line` is the first
    /// word of the message, so the number is the fourth field of the line.
    fn line_numbers(path: &Path) -> Vec<u32> {
        read(path)
            .lines()
            .map(|line| {
                let number = line.split(' ').nth(3).expect("line number");
                number.parse().expect("line number")
            })
            .collect()
    }

    /// Waits until every line queued before the call has been written.
    fn settle(store: &PluginLogStore) {
        let (sender, receiver) = std::sync::mpsc::channel();
        store
            .sender
            .send(Message::Ack(sender))
            .expect("writer thread");
        receiver.recv().expect("writer thread");
    }

    #[test]
    fn each_plugin_writes_to_its_own_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(directory.path());
        store.record("com.example.one", PluginLogLevel::Info, "reading image");
        store.record("com.example.two", PluginLogLevel::Error, "no evidence");
        settle(&store);
        let one = read(&store.file_path("com.example.one"));
        let two = read(&store.file_path("com.example.two"));
        assert_eq!(one.lines().count(), 1);
        assert!(one.contains(" INFO reading image"), "{one}");
        assert!(!one.contains("no evidence"), "{one}");
        assert!(two.contains(" ERROR no evidence"), "{two}");
    }

    #[test]
    fn a_line_carries_its_level_and_an_rfc3339_timestamp() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(directory.path());
        store.record(PLUGIN_ID, PluginLogLevel::Debug, "scanning roots");
        settle(&store);
        let content = read(&store.file_path(PLUGIN_ID));
        let line = content.lines().next().expect("one line");
        let mut fields = line.splitn(3, ' ');
        let timestamp = fields.next().expect("timestamp");
        assert!(timestamp.ends_with('Z'), "{line}");
        assert!(
            chrono::DateTime::parse_from_rfc3339(timestamp).is_ok(),
            "{line}"
        );
        assert_eq!(fields.next(), Some("DEBUG"));
        assert_eq!(fields.next(), Some("scanning roots"));
    }

    #[test]
    fn loader_stderr_lines_keep_the_level_of_their_prefix() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(directory.path());
        store.record_stderr(
            PLUGIN_ID,
            "WARN os-release is empty\r\nINFO 2 fields\nplain loader note",
        );
        settle(&store);
        let content = read(&store.file_path(PLUGIN_ID));
        let levels: Vec<&str> = content
            .lines()
            .map(|line| line.split(' ').nth(1).expect("level"))
            .collect();
        assert_eq!(levels, ["WARN", "INFO", "HOST"]);
        assert!(content.contains(" WARN os-release is empty\n"), "{content}");
        assert!(content.contains(" HOST plain loader note\n"), "{content}");
    }

    #[test]
    fn a_full_file_is_rotated_and_the_previous_generation_is_kept() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(directory.path());
        for index in 0..32 {
            store.record(
                PLUGIN_ID,
                PluginLogLevel::Info,
                &format!("line {index} {}", "x".repeat(24)),
            );
        }
        settle(&store);
        let path = store.file_path(PLUGIN_ID);
        let previous = path.with_extension("log.1");
        assert!(previous.is_file(), "no rotation happened");
        assert!(fs::metadata(&path).expect("metadata").len() <= 512);
        let current = line_numbers(&path);
        let kept = line_numbers(&previous);
        assert_eq!(
            current.last(),
            Some(&31),
            "newest line missing: {current:?}"
        );
        assert!(
            kept.iter().max() < current.iter().min(),
            "the kept generation must be older: {kept:?} {current:?}"
        );
        assert!(
            !kept.contains(&0),
            "only the last generation is kept: {kept:?}"
        );
    }

    #[test]
    fn reopening_the_store_appends_to_the_existing_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        {
            let store = store(directory.path());
            store.record(PLUGIN_ID, PluginLogLevel::Info, "first run");
            settle(&store);
        }
        {
            let store = store(directory.path());
            store.record(PLUGIN_ID, PluginLogLevel::Info, "second run");
            settle(&store);
        }
        let content = read(&directory.path().join("com.example.plugin.log"));
        assert!(content.contains("first run"), "{content}");
        assert!(content.contains("second run"), "{content}");
    }

    #[test]
    fn an_unsafe_plugin_id_still_produces_a_plain_file_name() {
        assert_eq!(
            log_file_name("com.example.plugin"),
            "com.example.plugin.log"
        );
        assert_eq!(
            log_file_name("com/example:plugin"),
            "com_example_plugin.log"
        );
        assert_eq!(log_file_name("con"), "_con.log");
        assert_eq!(log_file_name(""), "_.log");
    }
}
