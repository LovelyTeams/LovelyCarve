//! Shared native-library loading and Windows path helpers.
use std::path::{Path, PathBuf};

/// Standard DLL search list used by all of our `core::lib*` shims: filename
/// only (lets the loader walk PATH), executable directory, bundled `binaries/`
/// directories, then up to six ancestors of the current working directory so
/// `cargo run` from any subdir finds the DLLs sitting at the project root or
/// under its `binaries/` directory.
pub(crate) fn dll_search_path(name: &str) -> Vec<PathBuf> {
    let mut candidates: Vec<PathBuf> = vec![PathBuf::from(name)];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(name));
            candidates.push(dir.join("binaries").join(name));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut here: Option<&Path> = Some(cwd.as_path());
        for _ in 0..6 {
            let Some(dir) = here else { break };
            candidates.push(dir.join(name));
            candidates.push(dir.join("binaries").join(name));
            here = dir.parent();
        }
    }
    candidates
}
