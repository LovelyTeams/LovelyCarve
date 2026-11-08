//! Legacy ReFS DLL oracle, compiled only for explicit comparison builds.

#[cfg(feature = "refs-dll-oracle")]
pub(crate) mod common;
#[cfg(feature = "refs-dll-oracle")]
pub mod librefs;
