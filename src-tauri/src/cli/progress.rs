//! Progress for a running command: a braille spinner with an elapsed timer on
//! stderr, and OSC 9;4 progress in the tab or taskbar of terminals that support
//! it. Both stay silent for pipes, CI and `--quiet`, so results remain
//! machine-readable.
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
    output::{self, paint, Role},
};

const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const FRAME_DELAY: Duration = Duration::from_millis(80);

fn can_animate(quiet: bool) -> bool {
    !quiet
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

/// Run `task` with a live spinner labelled `label`; on completion the line
/// becomes `◇ label`, or a red `■ label` when the task fails.
pub async fn with_progress<T, E>(
    label: &str,
    quiet: bool,
    task: impl Future<Output = Result<T, E>>,
) -> Result<T, E> {
    let osc_enabled = osc_supported(quiet);
    if osc_enabled {
        osc(3);
    }
    if !can_animate(quiet) {
        let result = task.await;
        if osc_enabled {
            osc(0);
        }
        return result;
    }
    output::open_session(quiet);
    let stop = Arc::new(AtomicBool::new(false));
    let spinner = {
        let stop = Arc::clone(&stop);
        let label = label.to_owned();
        std::thread::Builder::new()
            .name("lovelycarve-cli-progress".into())
            .spawn(move || {
                let started = Instant::now();
                let mut frame = 0usize;
                while !stop.load(Ordering::SeqCst) {
                    let mut stderr = std::io::stderr().lock();
                    let line = format!(
                        "\r\x1b[2K{}  {}  {} {}",
                        paint(Role::Muted, "│"),
                        paint(Role::Brand, FRAMES[frame % FRAMES.len()]),
                        label,
                        paint(
                            Role::Muted,
                            &format!("({:.1}s)", started.elapsed().as_secs_f64())
                        )
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
        paint(Role::Muted, "◇")
    } else {
        paint(Role::Error, "■")
    };
    eprintln!("\r\x1b[2K{marker}  {label}");
    result
}
