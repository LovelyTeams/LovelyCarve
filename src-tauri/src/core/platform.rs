//! platform domain modules.

#[cfg(any(not(feature = "reader-library"), feature = "desktop", test))]
pub mod components;
#[cfg(all(
    windows,
    any(not(feature = "reader-library"), feature = "desktop", test)
))]
pub mod dokan_mount;
pub mod ffi;
pub mod resources;
#[cfg(windows)]
pub mod windows_share;
