//! Progress for a running command, drawn as cf's clack spinner: a gray guide
//! line, then a braille frame, the verb-only label and an elapsed `[Ns]`
//! timer on stderr, ending as a green `◇` line (a red `▲` one on failure);
//! and OSC 9;4 progress in the tab or taskbar of terminals that support it.
//! The spinner needs colour terminals on stdout and stderr and stays off with
//! `LOVELYCARVE_QUIET=1`; `--quiet` turns only the terminal progress off.
use std::{
    future::Future,
    io::{IsTerminal, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

use super::{
    interactive,
    output::{self, Tone},
};

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const FRAME_DELAY: Duration = Duration::from_millis(80);

/// How a command may show that it is running.
#[derive(Clone, Copy)]
pub struct Progress {
    /// `--quiet`: no terminal progress.
    pub quiet: bool,
    /// Whether the spinner may draw at all (never in a JSONL session).
    pub spinner: bool,
}

fn can_animate(progress: Progress) -> bool {
    progress.spinner
        && !interactive::quiet_env()
        && output::styled()
        && std::io::stdout().is_terminal()
        && std::io::stderr().is_terminal()
}

fn osc_supported(quiet: bool) -> bool {
    if quiet
        || interactive::quiet_env()
        || std::env::var("LOVELYCARVE_NO_OSC_PROGRESS").is_ok_and(|value| value == "1")
        || !std::io::stderr().is_terminal()
    {
        return false;
    }
    if std::env::var("LOVELYCARVE_FORCE_OSC_PROGRESS").is_ok_and(|value| value == "1") {
        return true;
    }
    let program = std::env::var("TERM_PROGRAM")
        .unwrap_or_default()
        .to_ascii_lowercase();
    ["ghostty", "iterm", "wezterm"]
        .iter()
        .any(|name| program.contains(name))
        || std::env::var_os("WT_SESSION").is_some()
        || std::env::var("ConEmuANSI").is_ok_and(|value| value == "ON")
}

fn osc(state: u8) {
    let mut stderr = std::io::stderr().lock();
    if let Err(error) = write!(stderr, "\x1b]9;4;{state};0\x07").and_then(|()| stderr.flush()) {
        tracing::debug!("terminal progress: {error}");
    }
}

/// `[12s]`, or `[1m 5s]` past a minute.
fn timer(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds >= 60 {
        format!("[{}m {}s]", seconds / 60, seconds % 60)
    } else {
        format!("[{seconds}s]")
    }
}

/// Run `task` with a live spinner labelled `label`.
pub async fn with_progress<T, E>(
    label: &str,
    progress: Progress,
    task: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let osc_enabled = osc_supported(progress.quiet);
    if osc_enabled {
        osc(3);
    }
    if !can_animate(progress) {
        let result = task.await;
        if osc_enabled {
            osc(0);
        }
        return result;
    }
    output::open_session(progress.quiet);
    let started = Instant::now();
    eprintln!("{}", output::tone(Tone::Gray, "│"));
    let stop = Arc::new(AtomicBool::new(false));
    let spinner = {
        let stop = Arc::clone(&stop);
        let label = label.to_owned();
        std::thread::Builder::new()
            .name("lovelycarve-cli-progress".into())
            .spawn(move || {
                let mut frame = 0usize;
                while !stop.load(Ordering::SeqCst) {
                    let mut stderr = std::io::stderr().lock();
                    let line = format!(
                        "\r\x1b[2K{}  {label} {}",
                        output::tone(Tone::Magenta, FRAMES[frame % FRAMES.len()]),
                        timer(started.elapsed())
                    );
                    if write!(stderr, "{line}")
                        .and_then(|()| stderr.flush())
                        .is_err()
                    {
                        return;
                    }
                    drop(stderr);
                    frame += 1;
                    std::thread::sleep(FRAME_DELAY);
                }
            })
    };
    let result = task.await;
    stop.store(true, Ordering::SeqCst);
    if let Ok(spinner) = spinner {
        if spinner.join().is_err() {
            tracing::debug!("progress thread panicked");
        }
    }
    if osc_enabled {
        osc(0);
    }
    let marker = if result.is_ok() {
        output::tone(Tone::Green, "◇")
    } else {
        output::tone(Tone::Red, "▲")
    };
    eprintln!("\r\x1b[2K{marker}  {label} {}", timer(started.elapsed()));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timer_counts_whole_seconds_and_minutes() {
        assert_eq!(timer(Duration::from_millis(900)), "[0s]");
        assert_eq!(timer(Duration::from_secs(59)), "[59s]");
        assert_eq!(timer(Duration::from_secs(65)), "[1m 5s]");
    }
}
