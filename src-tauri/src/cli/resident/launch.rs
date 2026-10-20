//! Detach the server without retaining a client's redirected output pipes.
use std::process::{Child, Command};

use crate::cli::CliError;

pub(super) fn spawn(command: &mut Command) -> Result<Child, CliError> {
    #[cfg(windows)]
    let _standard_handles = windows::StandardHandles::exclude_from_inheritance()?;
    Ok(command.spawn()?)
}

#[cfg(windows)]
mod windows {
    use std::os::windows::io::{AsRawHandle, RawHandle};

    use winapi::um::handleapi::{GetHandleInformation, SetHandleInformation};

    use crate::cli::CliError;

    const INHERIT: u32 = 1; // HANDLE_FLAG_INHERIT

    /// Used only by the management client, before any backend threads start.
    /// Rust's Windows launcher inherits every inheritable handle, including
    /// the original redirected stdio handles even when new stdio is supplied.
    pub(super) struct StandardHandles(Vec<(RawHandle, u32)>);

    impl StandardHandles {
        pub fn exclude_from_inheritance() -> Result<Self, CliError> {
            let mut handles = Self(Vec::new());
            for handle in [
                std::io::stdin().as_raw_handle(),
                std::io::stdout().as_raw_handle(),
                std::io::stderr().as_raw_handle(),
            ] {
                if handle.is_null()
                    || handle as isize == -1
                    || handles.0.iter().any(|(existing, _)| *existing == handle)
                {
                    continue;
                }
                let mut flags = 0;
                // SAFETY: stdio owns each handle throughout spawning; only its
                // inheritance flag is changed and the original is restored.
                if unsafe { GetHandleInformation(handle.cast(), &mut flags) } == 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                if flags & INHERIT != 0 {
                    // SAFETY: same live standard handle, preserving other flags.
                    if unsafe { SetHandleInformation(handle.cast(), INHERIT, 0) } == 0 {
                        return Err(std::io::Error::last_os_error().into());
                    }
                    handles.0.push((handle, flags));
                }
            }
            Ok(handles)
        }
    }

    impl Drop for StandardHandles {
        fn drop(&mut self) {
            for &(handle, flags) in &self.0 {
                // SAFETY: original handles still belong to the management client.
                if unsafe { SetHandleInformation(handle.cast(), INHERIT, flags & INHERIT) } == 0 {
                    tracing::warn!(
                        "restore CLI standard handle inheritance: {}",
                        std::io::Error::last_os_error()
                    );
                }
            }
        }
    }
}
