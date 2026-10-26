//! The command-line plumbing every tool in the family shares: dispatch on
//! `argv[0]`, `--version`, `doctor`, the JSON output and the structured
//! error, the `--json`/`--text` switch, and the man pages and shell
//! completions packaging needs. Behind the `cli` cargo feature; the default
//! build, and the static library, contain none of it.
//!
//! IT KNOWS NOTHING ABOUT ANY FORMAT, and must not learn. A crate describes
//! its tools once, as a [`Family`] — the repository name, the crate and
//! version, the install hints, and one [`Tool`] per dotted name, each a
//! clap command and a function that runs it — and hands that to [`main`].
//! Everything here reads only that description.
//!
//! ONE COPY, NOT ONE PER CRATE. This module began as a directory each
//! tool carried by hand (`src/cli/common/`), and the copies drifted within
//! weeks: one `doctor` learnt to drain a long `--version` answer while the
//! others still stalled on it, one entry point learnt to align its help,
//! and some could write man pages while others could not. Do not copy it
//! back out; fix it here.
//!
//! THE CONTRACT IT CARRIES, so each crate fills it in rather than defining
//! its own:
//!
//! - **One binary, many names.** Invoked as a tool's dotted name
//!   (`mkfs.<fs>`, `fs.<fs>`, `img.<fmt>`), it is that tool. Invoked under
//!   any other name — the repository's (`rust-fs-<fs>`), or a renamed copy
//!   — the first argument names the tool, by its verb (`mkfs`) or its full
//!   name (`mkfs.<fs>`). That second form is the one nothing on PATH can
//!   shadow.
//! - **`--version`** prints `<tool> (<crate>) <version>` for every name,
//!   which is how `doctor` and the test suites tell our binary from
//!   another package's program of the same name.
//! - **Output.** A result is JSON on stdout by default, `--text` for
//!   people; file content is raw bytes and is never wrapped. A failure is
//!   `{"error": "...", "code": N}` on STDERR, and `N` is the exit status,
//!   so stdout carries a result or nothing — never half of one, and never
//!   an error a pipe would take for data.
//! - **Exit statuses**, outside a tool that has its own scheme (`fsck`'s
//!   0/1/4/8): 0 done, 1 failed, 2 the command line was wrong, 3 the verb
//!   exists but this crate cannot do it (not implemented, or the format is
//!   read-only). A script moved between formats fails loudly on 3 instead
//!   of meaning something else.
//! - **`<repo> doctor`** resolves every dotted name on PATH and says, per
//!   name, whether the program found is ours, and if not, what wins and
//!   how to fix it.
//! - **`<repo> generate names|man|completions`** (hidden) prints the dotted
//!   names to link, and writes the man pages and shell completions, from
//!   the same clap commands the tools parse with.
//! - **`export`** ([`transfer`]) copies a file out of an image in bounded
//!   chunks, with its byte count and SHA-256, and publishes a local file
//!   only when it is complete. A driver supplies a positioned read; the
//!   copy, its flags and its report are the same for every format.
//!
//! ```no_run
//! use fs_core::cli::{self, CliError, Family, Json, Outcome, Tool};
//! use fs_core::cli::clap::{ArgMatches, Command};
//!
//! fn command() -> Command {
//!     Command::new("fs")
//!         .about("Work on an image")
//!         .args(cli::format_args())
//!         .after_help("Examples:\n  fs.demo disk.img")
//! }
//!
//! fn run(_: &ArgMatches) -> Result<Outcome, CliError> {
//!     Ok(Outcome::report(Json::object([("fs", Json::from("demo"))])))
//! }
//!
//! static FAMILY: Family = Family {
//!     repo: "rust-fs-demo",
//!     crate_name: "rust-fs-demo",
//!     version: "0.1.0",
//!     about: "Demo tools",
//!     install_hints: &["`cargo install rust-fs-demo --features cli`"],
//!     tools: &[Tool {
//!         name: "fs.demo",
//!         verb: "fs",
//!         section: 1,
//!         about: "Work on a demo image",
//!         usage_exit: cli::output::EXIT_USAGE,
//!         command,
//!         run,
//!     }],
//! };
//!
//! fn main() -> std::process::ExitCode {
//!     cli::main(&FAMILY)
//! }
//! ```

pub mod dispatch;
pub mod docs;
pub mod doctor;
pub mod family;
pub mod output;
pub mod transfer;
pub mod version;

// clap's `Command` is imported as `Cmd` throughout: it is not a process,
// and a reader scanning for `Command` constructors is looking for spawns.
// The one process this plumbing starts is doctor's `--version` probe
// (std's `Command`, imported there as `Process`), which runs a program by
// the name PATH gives it because that is the question doctor answers.

/// The argument parser the tools are written against, re-exported so a
/// crate's tools and this plumbing can never disagree about its version.
pub use clap;

pub use family::{Family, Tool};
pub use output::{CliError, Format, Json, Outcome};

use std::ffi::OsString;
use std::io::Write as _;
use std::process::ExitCode;

use clap::{Arg, ArgAction, Command as Cmd};

/// The whole program: work out which tool this is, parse its command
/// line, run it and print what it returned.
pub fn main(family: &'static Family) -> ExitCode {
    run(family, std::env::args_os().collect())
}

/// [`main`] over an explicit argument vector, `argv[0]` included.
pub fn run(family: &'static Family, argv: Vec<OsString>) -> ExitCode {
    respond(family, argv).emit()
}

/// What the plumbing prints for one invocation, and the exit status, before
/// any of it is written.
///
/// A tool that streams raw bytes writes them itself while it runs; this is
/// everything else — the report, the structured error, clap's help — which
/// is what makes the contract testable without spawning a process.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Response {
    /// Printed on stdout, as is.
    pub stdout: String,
    /// Printed on stderr, as is.
    pub stderr: String,
    /// The exit status.
    pub code: u8,
}

impl Response {
    fn out(text: String, code: u8) -> Response {
        Response {
            stdout: text,
            stderr: String::new(),
            code,
        }
    }

    fn err(text: String, code: u8) -> Response {
        Response {
            stdout: String::new(),
            stderr: text,
            code,
        }
    }

    /// Write both streams and turn the status into an [`ExitCode`]. A
    /// closed pipe (`| head`) is the reader's choice, not a failure.
    pub fn emit(self) -> ExitCode {
        if !self.stdout.is_empty() {
            let mut stdout = std::io::stdout().lock();
            let _ = stdout.write_all(self.stdout.as_bytes());
            let _ = stdout.flush();
        }
        if !self.stderr.is_empty() {
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(self.stderr.as_bytes());
            let _ = stderr.flush();
        }
        ExitCode::from(self.code)
    }
}

/// [`run`] without the writing: resolve, parse, run the tool, and return
/// what would be printed.
pub fn respond(family: &'static Family, argv: Vec<OsString>) -> Response {
    match dispatch::resolve(family, argv) {
        dispatch::Target::Tool(tool, argv) => respond_tool(family, tool, argv),
        dispatch::Target::Repo(argv) => respond_repo(family, argv),
    }
}

/// A tool's clap command, named and versioned for the name it runs under.
pub fn tool_command(family: &'static Family, tool: &Tool) -> Cmd {
    (tool.command)()
        .name(tool.name)
        .bin_name(tool.name)
        .version(version::clap_version(family))
}

/// The repository-named entry point's clap command: every tool as a
/// subcommand (so its help and its man page list them), plus `doctor`.
///
/// Parsing never reaches a tool's subcommand here: [`dispatch::resolve`]
/// hands `<repo> <verb> ...` to the tool itself before this runs.
pub fn repo_command(family: &'static Family) -> Cmd {
    let mut cmd = Cmd::new(family.repo)
        .bin_name(family.repo)
        .version(version::clap_version(family))
        .about(family.about)
        .subcommand_required(true)
        .arg_required_else_help(true)
        .after_help(repo_examples(family));
    for tool in family.tools {
        cmd = cmd.subcommand((tool.command)().name(tool.verb).about(format!(
            "{} (the same program as `{}`)",
            tool.about, tool.name
        )));
    }
    cmd.subcommand(doctor::command(family)).subcommand(
        Cmd::new("generate")
            .about("Print what packaging needs from the binary itself")
            .hide(true)
            .subcommand_required(true)
            .subcommand(
                Cmd::new("names").about("The dotted names to link to this binary, one per line"),
            )
            .subcommand(
                Cmd::new("man")
                    .about("Write a man page per name under SHARE/man/man<section>/")
                    .arg(share_arg()),
            )
            .subcommand(
                Cmd::new("completions")
                    .about("Write zsh, bash and fish completions per name under SHARE/")
                    .arg(share_arg()),
            ),
    )
}

/// `generate`'s SHARE: a path, and so taken as bytes. A path need not be
/// UTF-8, and a `String` argument would turn one that is not into a usage
/// error before the filesystem had a say.
fn share_arg() -> Arg {
    Arg::new("share")
        .value_name("SHARE")
        .value_parser(clap::value_parser!(OsString))
        .required(true)
}

/// The entry point's examples, one command per line with its explanation
/// in a column aligned on the longest command, however long the repository
/// name and the verbs are.
fn repo_examples(family: &Family) -> String {
    let mut rows: Vec<(String, String)> = family
        .tools
        .iter()
        .map(|tool| {
            (
                format!("{} {} --help", family.repo, tool.verb),
                format!("same as `{} --help`", tool.name),
            )
        })
        .collect();
    rows.push((
        format!("{} doctor", family.repo),
        "is every tool on PATH ours?".to_string(),
    ));
    let width = rows.iter().map(|(c, _)| c.len()).max().unwrap_or(0);
    let mut text = String::from("Examples:\n");
    for (cmd, what) in rows {
        text.push_str(&format!("  {cmd:<width$}    {what}\n"));
    }
    text
}

fn respond_tool(family: &'static Family, tool: &'static Tool, argv: Vec<OsString>) -> Response {
    let text_requested = output::text_requested(&argv);
    let argv_copy = argv.clone();
    match tool_command(family, tool).try_get_matches_from(argv) {
        Err(error) => clap_failure(tool.name, error, text_requested, tool.usage_exit),
        Ok(matches) => {
            let format = Format::of(&matches, &argv_copy);
            let result = (tool.run)(&matches);
            output::render(tool.name, format, result)
        }
    }
}

fn respond_repo(family: &'static Family, argv: Vec<OsString>) -> Response {
    let text_requested = output::text_requested(&argv);
    let argv_copy = argv.clone();
    let matches = match repo_command(family).try_get_matches_from(argv) {
        Ok(matches) => matches,
        Err(error) => return clap_failure(family.repo, error, text_requested, output::EXIT_USAGE),
    };
    match matches.subcommand() {
        Some(("doctor", sub)) => {
            let format = Format::of(sub, &argv_copy);
            output::render(family.repo, format, Ok(doctor::run(family)))
        }
        Some(("generate", sub)) => match sub.subcommand() {
            Some(("names", _)) => {
                let names: String = family
                    .tools
                    .iter()
                    .map(|tool| format!("{}\n", tool.name))
                    .collect();
                Response::out(names, 0)
            }
            Some((what @ ("man" | "completions"), args)) => {
                let share = std::path::Path::new(
                    args.get_one::<OsString>("share")
                        .expect("clap requires the share directory"),
                );
                let written = if what == "man" {
                    docs::man_pages(family, share)
                } else {
                    docs::completions(family, share)
                };
                let result = written
                    .map(|paths| {
                        Outcome::report(Json::Arr(
                            paths
                                .iter()
                                .map(|p| Json::from(p.display().to_string()))
                                .collect(),
                        ))
                    })
                    .map_err(|e| CliError::failed(format!("generate {what}: {e}")));
                output::render(family.repo, Format::Json, result)
            }
            _ => unreachable!("clap requires a generate subcommand"),
        },
        // A tool's verb never reaches here (dispatch took it), and clap
        // refuses anything else before this point.
        _ => unreachable!("clap requires a known subcommand"),
    }
}

/// Help and version go to stdout with status 0; anything else is a
/// command line that was wrong, status `usage_exit`, as a structured error
/// unless `--text` was asked for.
fn clap_failure(
    program: &str,
    error: clap::Error,
    text_requested: bool,
    usage_exit: u8,
) -> Response {
    use clap::error::ErrorKind;
    let rendered = error.render().to_string();
    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => Response::out(rendered, 0),
        // A bare `<repo>`: the help is the answer, but nothing was done.
        // clap prints this one on stderr.
        ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => Response::err(rendered, usage_exit),
        _ if text_requested => Response::err(rendered, usage_exit),
        _ => {
            let message = rendered
                .trim()
                .strip_prefix("error: ")
                .unwrap_or(rendered.trim())
                .to_string();
            output::render(
                program,
                Format::Json,
                Err(CliError::usage(message).with_code(usage_exit)),
            )
        }
    }
}

/// `--json` and `--text`, for any command that reports. The last one
/// given wins, so an alias or a wrapper can add either without breaking a
/// command line that already has the other.
pub fn format_args() -> [Arg; 2] {
    [
        Arg::new("json")
            .long("json")
            .help("Report as JSON on stdout (the default)")
            .action(ArgAction::SetTrue)
            .overrides_with("text"),
        Arg::new("text")
            .long("text")
            .help("Report as text for a person, instead of JSON")
            .action(ArgAction::SetTrue)
            .overrides_with("json"),
    ]
}
