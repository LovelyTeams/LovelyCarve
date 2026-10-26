//! `fsck.bcachefs`: check a bcachefs filesystem without changing it.
//!
//! What it checks is this crate's checker (`fs_bcachefs::check`), a subset
//! of what the reference checker checks, and it says so: a filesystem this
//! calls clean is one on which none of THESE invariants is broken.
//!
//! It never writes. `-n` is accepted because scripts pass it; `-y` and `-p`
//! (repair) are refused, because there is no repair.
//!
//! EXIT STATUS IS fsck(8)'s: 0 clean, 4 errors left uncorrected, 8 an
//! operational error (the target could not be opened, or is not bcachefs,
//! or cannot be checked), 16 a wrong command line.

use std::ffi::OsString;

use clap::{value_parser, Arg, ArgAction, ArgMatches, Command as Cmd};
use fs_core::cli::{CliError, Json, Outcome, Tool};
use fs_core::FileDevice;

/// fsck(8): filesystem errors left uncorrected.
pub const UNCORRECTED: u8 = 4;
/// fsck(8): operational error.
pub const OPERATIONAL: u8 = 8;
/// fsck(8): usage or syntax error.
pub const USAGE: u8 = 16;

pub const TOOL: Tool = Tool {
    name: "fsck.bcachefs",
    verb: "fsck",
    section: 8,
    usage_exit: USAGE,
    about: "Check a bcachefs filesystem without changing it",
    command,
    run,
};

fn command() -> Cmd {
    Cmd::new("fsck.bcachefs")
        .about("Check a bcachefs filesystem without changing it")
        .long_about(
            "Check a bcachefs image or device and report what is wrong with it. Nothing is \
             written, and nothing is repaired.\n\n\
             Checked: the superblock; every node of every btree (magic, sequence, checksums, \
             key order); every directory entry against the inode it names; every inode's link \
             count; every extent against its inode; and every data checksum. A filesystem not \
             shut down cleanly is checked through a replay of its journal, in memory. \
             Allocation, accounting and backpointers are not checked.\n\n\
             Exit status is fsck(8)'s: 0 clean, 4 errors found (and left), 8 the target could \
             not be checked, 16 a wrong command line.",
        )
        .arg(
            Arg::new("target")
                .value_name("TARGET")
                .help("Block device or image file to check")
                .required(true)
                .value_parser(value_parser!(OsString)),
        )
        .arg(
            Arg::new("no-change")
                .short('n')
                .help("Check only (the default, and the only mode)")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("repair")
                .short('y')
                .help("Refused: this checker does not repair")
                .action(ArgAction::SetTrue),
        )
        .arg(
            Arg::new("preen")
                .short('p')
                .help("Refused: this checker does not repair")
                .action(ArgAction::SetTrue),
        )
        .args(fs_core::cli::format_args())
        .after_help(
            "Examples:\n  \
             fsck.bcachefs disk.img           the report, as JSON\n  \
             fsck.bcachefs --text disk.img    the findings, one per line",
        )
}

fn run(m: &ArgMatches) -> Result<Outcome, CliError> {
    if m.get_flag("repair") || m.get_flag("preen") {
        return Err(
            CliError::usage("this checker does not repair: run it without -y or -p")
                .with_code(USAGE),
        );
    }
    let target = m.get_one::<OsString>("target").expect("required");
    let dev = FileDevice::open(target)
        .map_err(|e| CliError::failed(e.to_string()).with_code(OPERATIONAL))?;
    let r = fs_bcachefs::check::check(&dev)
        .map_err(|e| CliError::failed(e.to_string()).with_code(OPERATIONAL))?;
    let problems: Vec<Json> = r
        .problems
        .iter()
        .map(|p| {
            Json::object([
                ("kind", Json::Str(p.kind.into())),
                ("detail", Json::Str(p.detail.clone())),
            ])
        })
        .collect();
    let text: String = if r.clean() {
        format!(
            "clean: {} inodes, {} directory entries, {} extents{}",
            r.inodes,
            r.dirents,
            r.extents,
            if r.replayed {
                " (through a replay of the journal)"
            } else {
                ""
            }
        )
    } else {
        r.problems
            .iter()
            .map(|p| format!("{}: {}", p.kind, p.detail))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let code = if r.clean() { 0 } else { UNCORRECTED };
    Ok(Outcome::report(Json::object([
        ("clean", Json::Bool(r.clean())),
        ("replayed", Json::Bool(r.replayed)),
        ("inodes", Json::UInt(r.inodes)),
        ("dirents", Json::UInt(r.dirents)),
        ("extents", Json::UInt(r.extents)),
        ("problems", Json::Arr(problems)),
        ("exit", Json::UInt(u64::from(code))),
    ]))
    .with_text(text)
    .with_code(code))
}
