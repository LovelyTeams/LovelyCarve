//! Plugin log files.
//!
//! A plugin's `Context::log` line reaches the host either as an ABI callback
//! (`isolation = "in_process"`) or as a `LEVEL message` line on the loader's
//! stderr (`isolation = "process"`). Both paths hand the line to a
//! [`PluginLogStore`], which appends it to that plugin's own file under the
//! executable directory, so plugin output outlives the session and no longer
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
use parking_lot::{Mutex, RwLock};
use sha2::{Digest as _, Sha256};

use super::Result;

/// Log lines waiting for the writer thread before new ones are dropped.
const QUEUE_CAPACITY: usize = 4096;
/// A plugin's file is rotated once it reaches this size; one previous
/// generation is kept as `<name>.log.1`.
const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;
/// Longest plugin id fragment kept in a file name.
const MAX_FILE_NAME_CHARS: usize = 120;
const MAX_MESSAGE_BYTES: usize = 64 * 1024;

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

/// One plugin's open log file and how many bytes it holds.
struct OpenLog {
    file: File,
    bytes: u64,
}

impl OpenLog {
    fn open(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let bytes = file.metadata()?.len();
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
    sender: RwLock<Option<SyncSender<Record>>>,
    writer: Mutex<Option<std::thread::JoinHandle<()>>>,
    dropped: Arc<AtomicU64>,
}

impl PluginLogStore {
    /// Uses the running desktop or CLI executable's directory, independent of
    /// the working directory and the per-user application data directory.
    pub fn beside_executable() -> Result<Arc<Self>> {
        let executable = std::env::current_exe()?;
        let directory = executable.parent().ok_or_else(|| {
            std::io::Error::other("plugin logs require an executable parent directory")
        })?;
        let root = directory.join("logs").join("plugins");
        Self::open(&root).map_err(|error| {
            std::io::Error::other(format!("plugin log directory {}: {error}", root.display()))
                .into()
        })
    }

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
        let writer = std::thread::Builder::new()
            .name("plugin-log".to_owned())
            .spawn(move || writer_loop(&writer_root, max_file_bytes, &receiver, &writer_dropped))?;
        Ok(Arc::new(Self {
            root,
            sender: RwLock::new(Some(sender)),
            writer: Mutex::new(Some(writer)),
            dropped,
        }))
    }

    /// The file the plugin's lines are appended to.
    pub fn file_path(&self, plugin_id: &str) -> PathBuf {
        self.root.join(log_file_name(plugin_id))
    }

    /// Stops accepting records and drains the queue. Blocking: call off the
    /// async workers after shutting down plugin instances.
    pub fn shutdown(&self) {
        self.sender.write().take();
        if self
            .writer
            .lock()
            .take()
            .is_some_and(|writer| writer.join().is_err())
        {
            tracing::warn!("plugin log writer panicked");
        }
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
        let sender = self.sender.read();
        let Some(sender) = sender.as_ref() else {
            return;
        };
        let message = if message.len() > MAX_MESSAGE_BYTES {
            let end = message.floor_char_boundary(MAX_MESSAGE_BYTES);
            format!("{} [truncated]", &message[..end])
        } else {
            message.to_owned()
        };
        let record = Record {
            plugin_id: plugin_id.to_owned(),
            level,
            message,
            at: SystemTime::now(),
        };
        match sender.try_send(record) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Disconnected(_)) => {
                tracing::warn!("plugin log writer disconnected");
            }
        }
    }
}

impl Drop for PluginLogStore {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn writer_loop(root: &Path, max_file_bytes: u64, receiver: &Receiver<Record>, dropped: &AtomicU64) {
    let mut files: HashMap<String, Option<OpenLog>> = HashMap::new();
    let mut reported_drops = 0_u64;
    while let Ok(record) = receiver.recv() {
        let path = root.join(log_file_name(&record.plugin_id));
        let open = files.entry(record.plugin_id.clone()).or_insert_with(|| {
            OpenLog::open(&path).map_or_else(
                |error| {
                    tracing::warn!(
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
                tracing::warn!(plugin_id = %record.plugin_id, "plugin log write failed: {error}");
            }
        }
        let line = format!(
            "{} {} {}\n",
            format_time(record.at),
            record.level.label(),
            record.message
        );
        if let Err(error) = open.write_line(&path, &line, max_file_bytes) {
            tracing::warn!(plugin_id = %record.plugin_id, "plugin log write failed: {error}");
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
    let cleaned = if stem.is_empty()
        || crate::core::filesystem::tree::fs_tree_export::is_windows_device_name(stem)
    {
        format!("_{cleaned}")
    } else {
        cleaned
    };
    // Keep distinct ids separate after truncation, sanitization, or Windows
    // case folding. Ordinary lowercase ids retain their readable file names.
    if cleaned != plugin_id || plugin_id.bytes().any(|byte| byte.is_ascii_uppercase()) {
        let digest = hex::encode(Sha256::digest(plugin_id.as_bytes()));
        format!("{cleaned}-{digest}.log")
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

    #[test]
    fn default_root_is_beside_the_running_executable() {
        let executable = std::env::current_exe().expect("executable");
        let store = PluginLogStore::beside_executable().expect("log store beside executable");
        assert_eq!(
            store.file_path(PLUGIN_ID),
            executable
                .parent()
                .expect("directory")
                .join("logs")
                .join("plugins")
                .join("com.example.plugin.log")
        );
        store.shutdown();
    }

    #[test]
    fn each_plugin_writes_to_its_own_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(directory.path());
        store.record("com.example.one", PluginLogLevel::Info, "reading image");
        store.record("com.example.two", PluginLogLevel::Error, "no evidence");
        store.shutdown();
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
        store.shutdown();
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
        store.shutdown();
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
        store.shutdown();
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
            store.shutdown();
        }
        {
            let store = store(directory.path());
            store.record(PLUGIN_ID, PluginLogLevel::Info, "second run");
            store.shutdown();
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
        for id in ["com/example:plugin", "con", "", "..", "CON.txt"] {
            let name = log_file_name(id);
            assert!(!name.contains(['/', '\\', ':']));
            assert!(name.ends_with(".log"));
        }
        for (left, right) in [
            ("con", "_con"),
            ("Plugin", "plugin"),
            ("com/example:plugin", "com_example_plugin"),
        ] {
            assert_ne!(
                log_file_name(left).to_lowercase(),
                log_file_name(right).to_lowercase()
            );
        }
        let prefix = "a".repeat(MAX_FILE_NAME_CHARS);
        assert_ne!(
            log_file_name(&format!("{prefix}1")),
            log_file_name(&format!("{prefix}2"))
        );
    }

    #[test]
    fn dropping_the_store_drains_queued_records() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = store(directory.path());
        let path = store.file_path(PLUGIN_ID);
        store.record(PLUGIN_ID, PluginLogLevel::Info, "last record");
        drop(store);
        assert!(read(&path).contains(" INFO last record\n"));
    }

    #[test]
    fn full_queue_drops_records_and_reports_the_count() {
        let directory = tempfile::tempdir().expect("temp dir");
        let (sender, receiver) = sync_channel(1);
        let store = PluginLogStore {
            root: directory.path().to_owned(),
            sender: RwLock::new(Some(sender)),
            writer: Mutex::new(None),
            dropped: Arc::new(AtomicU64::new(0)),
        };
        store.record(PLUGIN_ID, PluginLogLevel::Info, "accepted");
        store.record(PLUGIN_ID, PluginLogLevel::Info, "dropped");
        assert_eq!(store.dropped.load(Ordering::Relaxed), 1);
        store.shutdown();
        writer_loop(directory.path(), MAX_FILE_BYTES, &receiver, &store.dropped);
        let content = read(&store.file_path(PLUGIN_ID));
        assert!(content.contains("1 plugin log lines were dropped"));
        assert!(content.contains(" INFO accepted\n"));
        assert!(!content.contains(" INFO dropped\n"));
    }

    #[test]
    fn oversized_messages_keep_valid_utf8_and_a_truncation_marker() {
        let directory = tempfile::tempdir().expect("temp dir");
        let store = PluginLogStore::open(directory.path()).expect("store");
        store.record(
            PLUGIN_ID,
            PluginLogLevel::Info,
            &"日志".repeat(MAX_MESSAGE_BYTES),
        );
        store.shutdown();
        let content = read(&store.file_path(PLUGIN_ID));
        assert!(content.ends_with(" [truncated]\n"));
        assert!(content.len() < MAX_MESSAGE_BYTES + 100);
        store.record(PLUGIN_ID, PluginLogLevel::Info, "after shutdown");
        store.shutdown();
        assert_eq!(read(&store.file_path(PLUGIN_ID)), content);
    }
}
