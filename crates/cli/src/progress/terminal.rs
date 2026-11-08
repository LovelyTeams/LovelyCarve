//! Optional OSC 9;4 activity for terminal tabs/taskbars, owned by the monitor.
use super::{interactive, Options};
use std::io::{IsTerminal, Write};

pub(super) struct Terminal(bool);
impl Terminal {
    pub fn start(options: Options) -> Self {
        let program = std::env::var("TERM_PROGRAM")
            .unwrap_or_default()
            .to_ascii_lowercase();
        let supported = std::env::var("LOVELYCARVE_FORCE_OSC_PROGRESS")
            .is_ok_and(|value| value == "1")
            || ["ghostty", "iterm", "wezterm"]
                .iter()
                .any(|name| program.contains(name))
            || std::env::var_os("WT_SESSION").is_some()
            || std::env::var("ConEmuANSI").is_ok_and(|value| value == "ON");
        let enabled = supported
            && !options.quiet
            && !options.events
            && !interactive::quiet_env()
            && !std::env::var("LOVELYCARVE_NO_OSC_PROGRESS").is_ok_and(|value| value == "1")
            && std::io::stderr().is_terminal();
        if enabled {
            emit(3);
        }
        Self(enabled)
    }
    pub fn clear(&mut self) {
        if self.0 {
            self.0 = false;
            emit(0);
        }
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        self.clear();
    }
}
fn emit(state: u8) {
    let mut stderr = std::io::stderr().lock();
    if let Err(error) = write!(stderr, "\x1b]9;4;{state};0\x07").and_then(|()| stderr.flush()) {
        tracing::debug!("terminal progress: {error}");
    }
}
