//! platform domain modules.

#[cfg(any(not(feature = "reader-library"), feature = "desktop", test))]
pub mod components;
#[cfg(all(
    windows,
    not(target_arch = "aarch64"),
    any(not(feature = "reader-library"), feature = "desktop", test)
))]
pub mod dokan_mount;
pub mod ffi;
pub mod resources;
#[cfg(all(windows, not(target_arch = "aarch64")))]
pub mod windows_share;
