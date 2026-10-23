//! Forensic engines organized by responsibility. Application state and IPC live outside this layer.

#[cfg(any(not(feature = "reader-library"), feature = "desktop", test))]
pub mod analysis;
#[cfg(any(not(feature = "reader-library"), feature = "desktop", test))]
pub mod carving;
#[cfg(any(not(feature = "reader-library"), feature = "desktop", test))]
pub mod casework;
#[cfg(any(not(feature = "reader-library"), feature = "desktop", test))]
pub mod catalog;
pub mod common;
pub mod evidence;
#[cfg(any(not(feature = "reader-library"), feature = "desktop", test))]
pub mod extensions;
pub mod filesystem;
pub mod platform;
#[cfg(any(not(feature = "reader-library"), feature = "desktop", test))]
pub mod preview;
#[cfg(any(not(feature = "reader-library"), feature = "desktop", test))]
pub mod search;
#[cfg(any(not(feature = "reader-library"), feature = "desktop", test))]
pub mod timeline;
