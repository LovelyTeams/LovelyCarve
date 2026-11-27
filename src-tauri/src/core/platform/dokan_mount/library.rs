//! Dokany's user-mode library, loaded at run time.
//!
//! The executables do not link `dokan2.dll`, so they start without Dokany on
//! every Windows architecture. The vendored `dokan-sys` loads the library on
//! first use: the copy Dokany installed in System32, which matches the
//! installed driver, then one beside the executable (the Windows x64 packages
//! ship one for driver-only installations), then one in the Cargo profile
//! directory above a test or example binary.

use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::core::common::error::Failure;
use crate::core::platform::components;

/// The loaded Dokany library, after checking that its driver answers.
pub fn require() -> Result<&'static dokan_sys::Library> {
    let library = dokan_sys::load(&fallbacks()).context(Failure::unavailable(
        "未找到可用的 Dokany 2 用户态库 dokan2.dll，只读挂载需要先安装 Dokany 2",
    ))?;
    if library.driver_version() == 0 {
        bail!(Failure::unavailable(format!(
            "Dokany 驱动未安装或未运行（已加载 {}）",
            library.path().display()
        )));
    }
    Ok(library)
}

/// `dokan2.dll` beside the executable or in the Cargo profile directory above
/// it; an executable whose location is unknown has no fallbacks.
fn fallbacks() -> Vec<PathBuf> {
    components::search_directories()
        .map(|directories| {
            directories
                .into_iter()
                .map(|directory| directory.join(dokan_sys::LIBRARY_NAME))
                .collect()
        })
        .unwrap_or_default()
}
