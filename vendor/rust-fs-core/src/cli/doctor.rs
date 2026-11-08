//! `<repo> doctor`: is every name this repository installs, as PATH
//! resolves it, our program?
//!
//! A dotted name is a shared namespace. `mkfs.<fs>` may also be the
//! reference tools' program, a distribution's `/usr/sbin/mkfs.<fs>`, or an
//! older install of ours, and
//! whichever PATH finds first is the one a user runs. When that is not
//! ours the symptom is a tool that "behaves unexpectedly" — so this says,
//! per name, what wins, whose it is, and the exact fix.
//!
//! "Ours" is decided by asking: each program found is run with
//! `--version` and must answer `<name> (<crate>) <version>` with this
//! binary's crate AND version. A same-crate program at another version is
//! a second install, stale, and reported as such.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::time::Duration;

use clap::Command as Cmd;

use super::family::Family;
use super::output::{Json, Outcome};
use super::version;

/// The clap command for `doctor`.
pub fn command(family: &Family) -> Cmd {
    Cmd::new("doctor")
        .about("Check that every tool PATH resolves is this program, and say how to fix one that is not")
        .args(super::format_args())
        .after_help(format!(
            "Exits 0 when every name resolves to this program, 1 otherwise.\n\n\
             Examples:\n  {repo} doctor\n  {repo} doctor --text",
            repo = family.repo
        ))
}

/// What one name resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// This program, at this version.
    Ours,
    /// Nothing on PATH answers to the name.
    Missing,
    /// Something else wins: another package's program, or a file that
    /// does not answer `--version` in our shape.
    Foreign,
    /// Ours, but another version: a second install is ahead on PATH.
    Stale,
}

impl Status {
    fn as_str(&self) -> &'static str {
        match self {
            Status::Ours => "ours",
            Status::Missing => "missing",
            Status::Foreign => "foreign",
            Status::Stale => "stale",
        }
    }
}

/// One name's diagnosis.
#[derive(Debug, Clone)]
pub struct Finding {
    pub name: String,
    pub status: Status,
    /// The program PATH runs for the name.
    pub path: Option<PathBuf>,
    /// The first line of its `--version`, as it answered.
    pub version: Option<String>,
    /// The Homebrew formula the winning program belongs to, when it lives
    /// in a Cellar.
    pub formula: Option<String>,
    /// Every later program of the same name on PATH, in PATH order.
    pub shadowed: Vec<PathBuf>,
    /// What to do, when there is something to do.
    pub fix: Option<String>,
}

/// Diagnose every name, against the process's PATH.
pub fn diagnose(family: &Family) -> Vec<Finding> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    diagnose_path(family, &path)
}

/// Diagnose every name against an explicit PATH value.
pub fn diagnose_path(family: &Family, path: &OsStr) -> Vec<Finding> {
    let dirs: Vec<PathBuf> = std::env::split_paths(path).collect();
    family
        .tools
        .iter()
        .map(|tool| diagnose_one(family, tool.name, &dirs))
        .collect()
}

fn diagnose_one(family: &Family, name: &str, dirs: &[PathBuf]) -> Finding {
    let found: Vec<PathBuf> = dirs
        .iter()
        .map(|dir| dir.join(program_file(name)))
        .filter(|candidate| is_executable(candidate))
        .collect();
    let Some(winner) = found.first().cloned() else {
        return Finding {
            name: name.to_string(),
            status: Status::Missing,
            path: None,
            version: None,
            formula: None,
            shadowed: Vec::new(),
            fix: Some(format!(
                "{name} is not on PATH. Install it: {}",
                family.install_hints.join(", or ")
            )),
        };
    };
    let answer = ask_version(&winner);
    let identity = answer.as_deref().and_then(version::parse);
    let status = match &identity {
        Some(id) if id.crate_name == family.crate_name && id.tool == name => {
            if id.version == family.version {
                Status::Ours
            } else {
                Status::Stale
            }
        }
        _ => Status::Foreign,
    };
    let formula = cellar_formula(&winner);
    // Only asked when the winner is not ours: the later programs are never
    // run otherwise (a CI runner may have stubbed them to fail loudly).
    let ours_later = found[1..]
        .iter()
        .filter(|_| status != Status::Ours)
        .find(|p| {
            ask_version(p)
                .as_deref()
                .and_then(version::parse)
                .is_some_and(|id| {
                    id.crate_name == family.crate_name
                        && id.tool == name
                        && id.version == family.version
                })
        })
        .cloned();
    let fix = match status {
        Status::Ours => None,
        Status::Missing => unreachable!("handled above"),
        Status::Foreign | Status::Stale => Some(fix_for(
            family,
            name,
            &winner,
            &status,
            identity.as_ref().map(|id| id.version.as_str()),
            formula.as_deref(),
            ours_later.as_deref(),
        )),
    };
    Finding {
        name: name.to_string(),
        status,
        path: Some(winner),
        version: answer.map(|a| a.lines().next().unwrap_or("").trim().to_string()),
        formula,
        shadowed: found[1..].to_vec(),
        fix,
    }
}

fn fix_for(
    family: &Family,
    name: &str,
    winner: &Path,
    status: &Status,
    their_version: Option<&str>,
    formula: Option<&str>,
    ours_later: Option<&Path>,
) -> String {
    let winner_dir = winner.parent().unwrap_or(Path::new("")).display();
    let mut fixes = Vec::new();
    if let Some(formula) = formula {
        fixes.push(format!("`brew unlink {formula}`"));
    }
    match ours_later {
        Some(ours) => fixes.push(format!(
            "put {} before {winner_dir} on PATH",
            ours.parent().unwrap_or(Path::new("")).display()
        )),
        None => fixes.push(format!(
            "install ours ({}) in a directory before {winner_dir} on PATH",
            family.install_hints.join(", or ")
        )),
    }
    let what = match status {
        Status::Stale => format!(
            "{} is {} {}, not {}",
            winner.display(),
            family.crate_name,
            their_version.unwrap_or("?"),
            family.version
        ),
        _ => format!("{} is not {}'s {name}", winner.display(), family.crate_name),
    };
    format!("{what}: {}", fixes.join(", or "))
}

/// How long `--version` may take, as a number of polls this far apart:
/// five seconds.
const PROBE_POLL: Duration = Duration::from_millis(10);
const PROBE_POLLS: u32 = 500;

/// How much of a `--version` answer is kept: the identity is its first
/// line. The rest is read and dropped, so a long answer cannot fill the
/// pipe and stall the program.
const PROBE_KEEP: usize = 64 * 1024;

/// Run `program --version` with no input, and give up after a few
/// seconds: a program that is not ours may wait for a terminal.
///
/// Its stdout is READ WHILE IT RUNS, on a thread. Read only after it
/// exits, a program answering more than a pipe holds blocks on the full
/// pipe, runs out the time and is taken for someone else's.
fn ask_version(program: &Path) -> Option<String> {
    let mut child = spawn_version(program)?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        use std::io::Read;
        let mut kept = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let room = PROBE_KEEP.saturating_sub(kept.len());
                    kept.extend_from_slice(&chunk[..n.min(room)]);
                }
            }
        }
        kept
    });
    // Polled rather than timed: a count of short sleeps needs no clock.
    let mut polls_left = PROBE_POLLS;
    let exited = loop {
        match child.try_wait() {
            Ok(Some(_)) => break true,
            Ok(None) if polls_left > 0 => {
                polls_left -= 1;
                std::thread::sleep(PROBE_POLL);
            }
            _ => break false,
        }
    };
    if !exited {
        // Killing it closes the pipe, which ends the reader.
        let _ = child.kill();
        let _ = child.wait();
        let _ = reader.join();
        return None;
    }
    let kept = reader.join().ok()?;
    Some(String::from_utf8_lossy(&kept).into_owned())
}

/// Start `program --version`, asking again while the file is busy.
///
/// Linux refuses to run a file some process still holds open for writing
/// (ETXTBSY): an install still copying the binary into place, or any
/// program that forked while it had the file open. That is momentary, and
/// the program is whatever it is the moment the writer lets go, so a busy
/// file is asked again, for up to five seconds, rather than
/// taken for somebody else's.
fn spawn_version(program: &Path) -> Option<std::process::Child> {
    let mut polls_left = PROBE_POLLS;
    loop {
        match Process::new(program)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => return Some(child),
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && polls_left > 0 => {
                polls_left -= 1;
                std::thread::sleep(PROBE_POLL);
            }
            Err(_) => return None,
        }
    }
}

/// The file a program called `name` is in a PATH directory: `name` itself,
/// or on Windows `name.exe`, which is how a shell there finds it (PATHEXT),
/// and how a Windows release ships each dotted name.
fn program_file(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file())
}

/// The Homebrew formula a program belongs to: the directory after
/// `Cellar` in its resolved path (`.../Cellar/<formula>/<version>/bin/...`).
pub fn cellar_formula(program: &Path) -> Option<String> {
    let resolved = std::fs::canonicalize(program).ok()?;
    let mut components = resolved.components().map(|c| c.as_os_str());
    components.find(|c| *c == "Cellar")?;
    components.next().map(|f| f.to_string_lossy().into_owned())
}

/// Run the diagnosis and turn it into a result: exit 0 when every name is
/// ours, 1 otherwise.
pub fn run(family: &Family) -> Outcome {
    let path = std::env::var_os("PATH").unwrap_or_default();
    run_path(family, &path)
}

/// [`run`] against an explicit PATH value.
pub fn run_path(family: &Family, path: &OsStr) -> Outcome {
    let findings = diagnose_path(family, path);
    let ok = findings.iter().all(|f| f.status == Status::Ours);
    let tools: Vec<Json> = findings
        .iter()
        .map(|f| {
            Json::object([
                ("name", Json::from(f.name.as_str())),
                ("status", Json::from(f.status.as_str())),
                (
                    "path",
                    Json::from(f.path.as_ref().map(|p| p.display().to_string())),
                ),
                ("version", Json::from(f.version.clone())),
                ("formula", Json::from(f.formula.clone())),
                (
                    "shadowed",
                    Json::from(
                        f.shadowed
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>(),
                    ),
                ),
                ("fix", Json::from(f.fix.clone())),
            ])
        })
        .collect();
    let report = Json::object([
        ("ok", Json::from(ok)),
        ("crate", Json::from(family.crate_name)),
        ("version", Json::from(family.version)),
        ("tools", Json::Arr(tools)),
    ]);
    let mut text = Vec::new();
    for f in &findings {
        let at = f
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "not on PATH".to_string());
        text.push(format!("{}: {} ({at})", f.name, f.status.as_str()));
        for later in &f.shadowed {
            text.push(format!("  also on PATH, not run: {}", later.display()));
        }
        if let Some(fix) = &f.fix {
            text.push(format!("  fix: {fix}"));
        }
    }
    text.push(if ok {
        format!("every tool is {} {}", family.crate_name, family.version)
    } else {
        "some tools on PATH are not this program; see the fixes above".to_string()
    });
    Outcome::report(report)
        .with_text(text.join("\n"))
        .with_code(u8::from(!ok))
}
