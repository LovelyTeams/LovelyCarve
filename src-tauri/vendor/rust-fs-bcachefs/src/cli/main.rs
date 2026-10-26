//! `rust-fs-bcachefs`: the command-line tools for bcachefs, one multi-call
//! binary. Installed as `rust-fs-bcachefs` and linked as `fs.bcachefs`.
//! The dispatch and output contract are `fs_core::cli` (rust-fs-core's
//! `cli` feature); `fs` is the tool itself.

mod device;
mod fs;
mod fs_write;
mod fsck;

use fs_core::cli;
use std::process::ExitCode;

static FAMILY: cli::Family = cli::Family {
    repo: "rust-fs-bcachefs",
    crate_name: env!("CARGO_PKG_NAME"),
    version: env!("CARGO_PKG_VERSION"),
    about: "bcachefs tools: read a bcachefs image or device directly, without mounting it",
    install_hints: &[
        "`cargo install rust-fs-bcachefs --features cli` from a checkout of this repository",
    ],
    tools: &[fs::TOOL, fsck::TOOL],
};

fn main() -> ExitCode {
    cli::main(&FAMILY)
}
