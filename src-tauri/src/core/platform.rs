//! platform domain modules.

#[cfg(any(not(feature = "reader-library"), feature = "desktop", test))]
pub mod components;
#[cfg(all(
    target_os = "windows",
    any(not(feature = "reader-library"), feature = "desktop", test)
))]
pub mod dokan_mount;
pub mod ffi;
pub mod resources;
pub mod windows_share;
