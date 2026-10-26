//! Copying a file out of an image: the `export` verb every `fs.<fs>`
//! shares, written once so that no driver writes it again over a
//! whole-file buffer.
//!
//! A driver supplies a [`FileSource`] — the file's length and a positioned
//! read over its own API — and this module does the rest:
//!
//! - **Bounded.** The copy moves [`CHUNK`] bytes at a time through one
//!   buffer allocated once, so memory does not grow with the file
//!   (`tests/transfer_bounded.rs` measures it).
//! - **Evidence.** Every copy returns its byte count and the SHA-256 of the
//!   bytes delivered, and a file export reports both.
//! - **Complete or not at all.** A local destination is written under a
//!   hidden staging name in its own directory, synced, and only then
//!   published under its real name. A failed or short read, or a failed
//!   write, removes the staging file and leaves the destination as it was.
//!   A crash leaves at most `.<name>.<pid>-<n>.partial` beside it, never a
//!   half-written destination.
//! - **Never over a file unasked.** Without `--replace` an existing name —
//!   a file, a directory or a symlink, dangling or not — is refused before
//!   the image is read, and publication is a hard link, which fails rather
//!   than overwrite a name that appeared meanwhile. There is no fallback to
//!   check-then-rename: a filesystem without hard links is told to use
//!   `--replace`. With it, publication is a rename, which swaps the new file
//!   in atomically; a directory is still refused.
//! - **Missing data is never success.** A writer that fails (a closed pipe)
//!   fails the copy, saying how many bytes of the total were delivered.
//!
//! Verification of the published bytes, aliasing checks, trees, progress
//! and import build on this; each is its own piece of work.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::{Arg, ArgAction, ArgMatches};
use sha2::{Digest, Sha256};

use super::output::{CliError, Json, Outcome};

/// The bytes a transfer moves at a time. One buffer of this size (or of
/// the file's size, when smaller) is all the file data a copy holds.
pub const CHUNK: usize = 1 << 20;

/// A file a driver can read at a position: what `export` copies from.
pub trait FileSource {
    /// The file's length in bytes, read once before the copy starts.
    fn size(&self) -> u64;

    /// Read into `buf` from `offset`, returning how many bytes were placed
    /// there. Fewer than asked is allowed; none before [`size`](Self::size)
    /// is a short read, and fails the copy. An error is the driver's own,
    /// and is passed on as it is.
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, CliError>;
}

/// Where an export's bytes go.
pub enum Destination<'a> {
    /// A writer, such as stdout: bytes go out as they are read.
    Writer(&'a mut dyn Write),
    /// A local file, published only when complete.
    File {
        /// The name to publish.
        path: &'a Path,
        /// Replace an existing file rather than refuse it.
        replace: bool,
    },
}

/// What a copy delivered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copied {
    /// The bytes delivered.
    pub bytes: u64,
    /// The SHA-256 of those bytes.
    pub sha256: [u8; 32],
    /// Whether an existing local file was replaced.
    pub replaced: bool,
}

impl Copied {
    /// The digest in lowercase hex, as checksum tools print it.
    pub fn sha256_hex(&self) -> String {
        self.sha256.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Copy `source` to `destination`. See the module documentation for what
/// is promised about memory, publication and failures.
pub fn export_file(
    source: &mut dyn FileSource,
    destination: Destination<'_>,
) -> Result<Copied, CliError> {
    match destination {
        Destination::Writer(sink) => pump(source, sink),
        Destination::File { path, replace } => export_to_file(source, path, replace),
    }
}

/// The copy itself: chunk by chunk from `source` into `sink`, digesting
/// what passes.
fn pump(source: &mut dyn FileSource, sink: &mut dyn Write) -> Result<Copied, CliError> {
    let size = source.size();
    let mut buf = vec![0u8; usize::try_from(size).map_or(CHUNK, |s| s.min(CHUNK))];
    let mut digest = Sha256::new();
    let mut offset = 0u64;
    let delivered = |offset: u64, e: io::Error| {
        CliError::failed(format!(
            "output failed with {offset} of {size} bytes delivered: {e}"
        ))
    };
    while offset < size {
        let want = buf
            .len()
            .min(usize::try_from(size - offset).unwrap_or(usize::MAX));
        let got = source.read_at(offset, &mut buf[..want])?;
        if got == 0 {
            return Err(CliError::failed(format!(
                "short read: the source ended at {offset} of {size} bytes"
            )));
        }
        if got > want {
            return Err(CliError::failed(format!(
                "the source answered {got} bytes when {want} were asked for at byte {offset}"
            )));
        }
        digest.update(&buf[..got]);
        sink.write_all(&buf[..got])
            .map_err(|e| delivered(offset, e))?;
        offset += got as u64;
    }
    sink.flush().map_err(|e| delivered(offset, e))?;
    Ok(Copied {
        bytes: offset,
        sha256: digest.finalize().into(),
        replaced: false,
    })
}

fn export_to_file(
    source: &mut dyn FileSource,
    path: &Path,
    replace: bool,
) -> Result<Copied, CliError> {
    let name = path
        .file_name()
        .ok_or_else(|| CliError::usage(format!("{}: not a file name to create", path.display())))?;
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    // Everything that can be refused is refused before the image is read.
    let existing = fs::symlink_metadata(path).ok();
    if let Some(meta) = &existing {
        if meta.is_dir() {
            return Err(CliError::failed(format!(
                "{}: is a directory",
                path.display()
            )));
        }
        if !replace {
            return Err(exists(path));
        }
    }
    if !fs::metadata(parent).is_ok_and(|m| m.is_dir()) {
        return Err(CliError::failed(format!(
            "{}: no such directory to export into",
            parent.display()
        )));
    }

    let (staging, mut file) = create_staging(parent, name)?;
    let copied = pump(source, &mut file).and_then(|copied| {
        file.sync_all()
            .map_err(|e| CliError::failed(format!("sync {}: {e}", staging.display())))?;
        Ok(copied)
    });
    drop(file);
    let published = copied.and_then(|copied| {
        publish(&staging, path, replace)?;
        Ok(copied)
    });
    let mut copied = match published {
        Ok(copied) => copied,
        Err(e) => {
            let _ = fs::remove_file(&staging);
            return Err(e);
        }
    };
    sync_dir(parent).map_err(|e| {
        CliError::failed(format!(
            "{} is complete, but its directory could not be synced: {e}",
            path.display()
        ))
    })?;
    copied.replaced = existing.is_some();
    Ok(copied)
}

fn exists(path: &Path) -> CliError {
    CliError::failed(format!(
        "{}: exists; give --replace to replace it",
        path.display()
    ))
}

/// A new, empty file under a hidden name beside the destination. The
/// name is never one that exists: it is created exclusively, and the next
/// candidate is tried when it is taken.
fn create_staging(parent: &Path, name: &OsStr) -> Result<(PathBuf, File), CliError> {
    // The stem only has to be recognisable; it is shortened so that a
    // destination name near the length limit still leaves room for the
    // suffix.
    let stem: String = name.to_string_lossy().chars().take(100).collect();
    let pid = std::process::id();
    for n in 0..1000u32 {
        let staging = parent.join(format!(".{stem}.{pid}-{n}.partial"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging)
        {
            Ok(file) => return Ok((staging, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(CliError::failed(format!(
                    "create {}: {e}",
                    staging.display()
                )))
            }
        }
    }
    Err(CliError::failed(format!(
        "{}: no free staging name beside it",
        parent.join(name).display()
    )))
}

/// Give the complete staging file its real name.
fn publish(staging: &Path, path: &Path, replace: bool) -> Result<(), CliError> {
    if replace {
        return fs::rename(staging, path)
            .map_err(|e| CliError::failed(format!("replace {}: {e}", path.display())));
    }
    match fs::hard_link(staging, path) {
        Ok(()) => fs::remove_file(staging).map_err(|e| {
            CliError::failed(format!(
                "{} is published, but its staging name {} could not be removed: {e}",
                path.display(),
                staging.display()
            ))
        }),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(exists(path)),
        Err(e) => Err(CliError::failed(format!(
            "{}: cannot publish without replacing, which needs a hard link: {e}; \
             --replace publishes by rename instead",
            path.display()
        ))),
    }
}

/// Make the new name durable: on Unix a rename or link is only on disk
/// once its directory is synced.
#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_: &Path) -> io::Result<()> {
    Ok(())
}

// ---------------------------------------------------------------------------
// The shared command line
// ---------------------------------------------------------------------------

/// Where an `export` command line sends the file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// `-`: the bytes go to stdout, and nothing else does.
    Stdout,
    /// A local file to create (or, with `--replace`, to replace).
    File(PathBuf),
}

/// An `export` command line, parsed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportRequest {
    /// The file inside the image, as given.
    pub path: OsString,
    /// Where it goes.
    pub output: Output,
    /// `--replace`.
    pub replace: bool,
}

/// The arguments every driver's `export` takes, so they all parse alike:
/// `PATH DEST [--replace]`, DEST `-` meaning stdout.
pub fn export_args() -> [Arg; 3] {
    [
        Arg::new("path")
            .value_name("PATH")
            .help("The file inside the image")
            .required(true)
            .value_parser(clap::value_parser!(OsString)),
        Arg::new("dest")
            .value_name("DEST")
            .help("The local file to create, or - for stdout")
            .required(true)
            .value_parser(clap::value_parser!(OsString)),
        Arg::new("replace")
            .long("replace")
            .action(ArgAction::SetTrue)
            .help(
                "Replace DEST if it exists, atomically; without this an existing DEST is refused",
            ),
    ]
}

impl ExportRequest {
    /// The request [`export_args`] parsed. `--replace` with stdout is a
    /// usage error: there is nothing there to replace.
    pub fn from_matches(matches: &ArgMatches) -> Result<ExportRequest, CliError> {
        let given = |id: &str| {
            matches
                .get_one::<OsString>(id)
                .cloned()
                .ok_or_else(|| CliError::usage(format!("missing {}", id.to_uppercase())))
        };
        let path = given("path")?;
        let dest = given("dest")?;
        let replace = matches.get_flag("replace");
        let output = if dest == "-" {
            if replace {
                return Err(CliError::usage("--replace needs a file DEST, not stdout"));
            }
            Output::Stdout
        } else {
            Output::File(PathBuf::from(dest))
        };
        Ok(ExportRequest {
            path,
            output,
            replace,
        })
    }
}

/// Run an `export` the driver has opened `source` for. To stdout, the
/// bytes are the whole output; to a file, the result is a report of the
/// bytes and their digest.
pub fn export(request: &ExportRequest, source: &mut dyn FileSource) -> Result<Outcome, CliError> {
    let dest = match &request.output {
        Output::Stdout => {
            let mut stdout = io::stdout().lock();
            export_file(source, Destination::Writer(&mut stdout))?;
            return Ok(Outcome::done());
        }
        Output::File(dest) => dest,
    };
    let copied = export_file(
        source,
        Destination::File {
            path: dest,
            replace: request.replace,
        },
    )?;
    let path = request.path.to_string_lossy();
    let report = Json::object([
        ("path", Json::from(path.as_ref())),
        ("destination", Json::from(dest.display().to_string())),
        ("bytes", Json::from(copied.bytes)),
        ("sha256", Json::from(copied.sha256_hex())),
        ("replaced", Json::from(copied.replaced)),
    ]);
    let text = format!(
        "exported {path} to {} ({} bytes, sha256 {})",
        dest.display(),
        copied.bytes,
        copied.sha256_hex()
    );
    Ok(Outcome::report(report).with_text(text))
}
