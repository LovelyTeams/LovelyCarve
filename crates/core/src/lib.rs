//! LovelyCarve's forensic engines, organized by responsibility. Application
//! state, tasks and the command functions live in `lovelycarve-backend`; the
//! engines depend on none of them.
#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

#[cfg(any(not(feature = "reader-library"), test))]
pub mod analysis;
#[cfg(any(not(feature = "reader-library"), test))]
pub mod carving;
#[cfg(any(not(feature = "reader-library"), test))]
pub mod casework;
#[cfg(any(not(feature = "reader-library"), test))]
pub mod catalog;
pub mod common;
pub mod evidence;
#[cfg(any(not(feature = "reader-library"), test))]
pub mod extensions;
pub mod filesystem;
pub mod platform;
#[cfg(any(not(feature = "reader-library"), test))]
pub mod preview;
#[cfg(any(not(feature = "reader-library"), test))]
pub mod search;
#[cfg(any(not(feature = "reader-library"), test))]
pub mod timeline;
