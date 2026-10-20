//! Progress stays on stderr when stdout is a pipe. A heartbeat proves CLI
//! liveness; only backend observations describe completed work.
use super::{interactive, output, CliError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    io::{IsTerminal, Write},
    sync::{mpsc, Arc, Mutex, Weak},
    thread::JoinHandle,
    time::{Duration, Instant},
};

#[cfg(not(test))]
const HEARTBEAT: Duration = Duration::from_secs(5);
#[cfg(test)]
const HEARTBEAT: Duration = Duration::from_millis(20);
mod terminal;
const FRAME_DELAY: Duration = Duration::from_millis(80);
const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

#[derive(Clone, Copy, Default)]
pub struct Options {
    pub quiet: bool,
    pub events: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Event {
    pub event: String,
    pub payload: Value,
}
type EventSink = dyn Fn(&Event) -> Result<(), String> + Send + Sync;
struct Shared {
    options: Options,
    active: Mutex<Weak<Active>>,
    remote: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Event>>>,
    sink: Box<EventSink>,
}

/// The observation sink shared by a CLI's host and command executor.
#[derive(Clone)]
pub struct Reporter(Arc<Shared>);
impl Reporter {
    pub fn new(options: Options) -> Self {
        Self::with_sink(options, |event| {
            let mut stderr = std::io::stderr().lock();
            serde_json::to_writer(&mut stderr, event).map_err(|error| error.to_string())?;
            writeln!(stderr)
                .and_then(|()| stderr.flush())
                .map_err(|error| error.to_string())
        })
    }
    #[cfg(test)]
    pub fn silent() -> Self {
        Self::new(Options {
            quiet: true,
            events: false,
        })
    }
    fn with_sink(
        options: Options,
        sink: impl Fn(&Event) -> Result<(), String> + Send + Sync + 'static,
    ) -> Self {
        Self(Arc::new(Shared {
            options,
            active: Mutex::new(Weak::new()),
            remote: Mutex::new(None),
            sink: Box::new(sink),
        }))
    }
    /// Preserve backend payloads; never put diagnostics in stdout.
    pub fn event(&self, event: &str, payload: Value) -> Result<(), String> {
        let event = Event {
            event: event.into(),
            payload,
        };
        if let Some(sender) = self
            .0
            .remote
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            if sender.send(event.clone()).is_err() {
                tracing::debug!("resident progress receiver disconnected");
            }
        }
        if self.0.options.events {
            (self.0.sink)(&event)?;
        }
        let active = self
            .0
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .upgrade();
        if let (Some(active), Some(stage)) = (active, stage(&event)) {
            active.observe(stage);
        }
        Ok(())
    }
    pub fn subscribe(&self, sender: tokio::sync::mpsc::UnboundedSender<Event>) -> Subscription {
        *self
            .0
            .remote
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sender);
        Subscription(self.clone())
    }
    pub fn start(&self, command: &str) -> Monitor {
        let options = self.0.options;
        let human = !options.quiet && !options.events && !interactive::quiet_env();
        let active = Arc::new(Active {
            command: command.into(),
            id: uuid::Uuid::new_v4().to_string(),
            started: Instant::now(),
            human,
            animated: human && output::styled() && std::io::stderr().is_terminal(),
            stage: Mutex::new((String::from("处理中"), Instant::now())),
            reporter: self.clone(),
        });
        *self
            .0
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::downgrade(&active);
        active.lifecycle("started");
        if human {
            active.print("开始", "处理中");
        }
        let (stop, receiver) = mpsc::channel();
        let worker = (human || options.events)
            .then(|| {
                let active = Arc::clone(&active);
                std::thread::Builder::new()
                    .name("lovelycarve-cli-progress".into())
                    .spawn(move || {
                        let interval = if active.animated {
                            FRAME_DELAY
                        } else {
                            HEARTBEAT
                        };
                        let mut heartbeat = Instant::now();
                        let mut frame = 0usize;
                        while matches!(
                            receiver.recv_timeout(interval),
                            Err(mpsc::RecvTimeoutError::Timeout)
                        ) {
                            let stage = active
                                .stage
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .0
                                .clone();
                            if active.animated {
                                active.print(FRAMES[frame % FRAMES.len()], &stage);
                                frame += 1;
                            }
                            if heartbeat.elapsed() >= HEARTBEAT {
                                active.lifecycle("running");
                                if active.human && !active.animated {
                                    active.print("仍在执行", &stage);
                                }
                                heartbeat = Instant::now();
                            }
                        }
                    })
            })
            .and_then(|result| match result {
                Ok(worker) => Some(worker),
                Err(error) => {
                    tracing::warn!("CLI progress worker: {error}");
                    None
                }
            });
        Monitor {
            terminal: terminal::Terminal::start(options),
            active,
            stop: Some(stop),
            worker,
        }
    }
}
pub struct Subscription(Reporter);
impl Drop for Subscription {
    fn drop(&mut self) {
        *self
            .0
             .0
            .remote
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}
struct Active {
    command: String,
    id: String,
    started: Instant,
    human: bool,
    animated: bool,
    stage: Mutex<(String, Instant)>,
    reporter: Reporter,
}
impl Active {
    fn lifecycle(&self, phase: &str) {
        if self.reporter.0.options.events {
            let stage = self
                .stage
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0
                .clone();
            let event = Event {
                event: "cli-command".into(),
                payload: json!({"command": self.command, "commandId": self.id,
                "phase": phase, "elapsedMs": self.started.elapsed().as_millis() as u64, "stage": stage}),
            };
            if let Err(error) = (self.reporter.0.sink)(&event) {
                tracing::debug!("CLI lifecycle delivery: {error}");
            }
        }
    }
    fn print(&self, marker: &str, stage: &str) {
        let mut stderr = std::io::stderr().lock();
        let prefix = if self.animated { "\r\x1b[2K" } else { "" };
        let end = if self.animated { "" } else { "\n" };
        let marker = if self.animated {
            output::tone(
                match marker {
                    "完成" => output::Tone::Green,
                    "失败" | "已取消" => output::Tone::Red,
                    _ => output::Tone::Magenta,
                },
                marker,
            )
        } else {
            marker.to_owned()
        };
        let result = write!(
            stderr,
            "{prefix}{marker} [{}] {} {}{end}",
            output::sanitize(&self.command),
            output::sanitize(stage),
            timer(self.started.elapsed())
        )
        .and_then(|()| stderr.flush());
        if let Err(error) = result {
            tracing::debug!("CLI progress delivery: {error}");
        }
    }
    fn observe(&self, stage: String) {
        let mut latest = self
            .stage
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed = latest.0 != stage;
        latest.0 = stage.clone();
        if self.human
            && !self.animated
            && changed
            && latest.1.elapsed() >= Duration::from_millis(200)
        {
            latest.1 = Instant::now();
            drop(latest);
            self.print("进度", &stage);
        }
    }
}
/// RAII cleanup also handles a command future dropped on interruption.
pub struct Monitor {
    terminal: terminal::Terminal,
    active: Arc<Active>,
    stop: Option<mpsc::Sender<()>>,
    worker: Option<JoinHandle<()>>,
}
impl Monitor {
    pub fn finish<T>(&mut self, result: &Result<T, CliError>) {
        self.end(match result {
            Ok(_) => "completed",
            Err(error) if error.exit_code == 130 || error.code == "cancelled" => "cancelled",
            Err(_) => "failed",
        });
    }
    fn end(&mut self, phase: &str) {
        let Some(stop) = self.stop.take() else {
            return;
        };
        if stop.send(()).is_err() {
            tracing::debug!("CLI progress worker already exited");
        }
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                tracing::warn!("CLI progress worker panicked");
            }
        }
        self.terminal.clear();
        self.active.lifecycle(phase);
        if self.active.human {
            let stage = self
                .active
                .stage
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0
                .clone();
            self.active.print(
                match phase {
                    "completed" => "完成",
                    "cancelled" => "已取消",
                    _ => "失败",
                },
                &stage,
            );
            if self.active.animated {
                eprintln!();
            }
        }
        let mut latest = self
            .active
            .reporter
            .0
            .active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if latest
            .upgrade()
            .is_some_and(|active| Arc::ptr_eq(&active, &self.active))
        {
            *latest = Weak::new();
        }
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.end("cancelled");
    }
}
fn timer(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds >= 60 {
        format!("[{}m {}s]", seconds / 60, seconds % 60)
    } else {
        format!("[{seconds}s]")
    }
}
fn stage(event: &Event) -> Option<String> {
    let payload = &event.payload;
    let label = match event.event.as_str() {
        "image-load-progress" => payload.get("stage")?.as_str()?.to_owned(),
        "raid-detect-progress" => match payload.get("stage")?.as_str()? {
            "lvmMetadata" => "读取 LVM 成员元数据",
            "mdMetadata" => "探测 md RAID 成员",
            "mdArrays" => "组装 md 阵列",
            "firmware" => "探测固件 RAID",
            "lvmVolumes" => "组装 LVM 逻辑卷",
            "heuristic" => "采样推定无元数据 RAID（总量未知）",
            "registering" => "注册组装卷",
            "complete" => "卷检测与注册完成",
            _ => return None,
        }
        .into(),
        "task-progress" => payload.get("message")?.as_str()?.to_owned(),
        _ => return None,
    };
    let current = payload
        .get("current")
        .or_else(|| payload.get("done"))
        .and_then(Value::as_u64);
    let total = payload
        .get("total")
        .and_then(Value::as_u64)
        .filter(|total| *total > 0);
    Some(match (current, total) {
        (Some(current), Some(total)) => format!("{label} ({current}/{total})"),
        _ => label,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_stops_monitor_and_keeps_stages_distinct_from_percentages() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&events);
        let (sender, heartbeat) = mpsc::channel();
        let reporter = Reporter::with_sink(
            Options {
                quiet: true,
                events: true,
            },
            move |event| {
                captured.lock().unwrap().push(event.clone());
                if event.payload["phase"] == "running" {
                    sender.send(()).unwrap();
                }
                Ok(())
            },
        );
        {
            let _monitor = reporter.start("raid detect");
            reporter
                .event(
                    "raid-detect-progress",
                    json!({"stage":"heuristic","current":0,"total":null}),
                )
                .unwrap();
            assert!(
                heartbeat.recv_timeout(Duration::from_secs(1)).is_ok(),
                "live commands must emit a heartbeat before finishing"
            );
        }
        let events = events.lock().unwrap();
        assert_eq!(events.first().unwrap().payload["phase"], "started");
        assert_eq!(events.last().unwrap().payload["phase"], "cancelled");
        assert!(events.last().unwrap().payload["stage"]
            .as_str()
            .unwrap()
            .contains("总量未知"));
        assert!(reporter.0.active.lock().unwrap().upgrade().is_none());
        assert_eq!(timer(Duration::from_secs(65)), "[1m 5s]");
    }
}
