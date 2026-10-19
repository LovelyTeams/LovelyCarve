//! system command adapters.

pub mod mount;
#[expect(
    clippy::module_inception,
    reason = "Keep the existing CLI help group and command leaf name stable"
)]
pub mod system;
pub mod updater;
