//! platform domain modules.

#[cfg(all(target_os = "windows", any(not(feature = "reader-library"), test)))]
pub mod dokan_mount;
pub mod ffi;
pub mod resources;
pub mod windows_share;
