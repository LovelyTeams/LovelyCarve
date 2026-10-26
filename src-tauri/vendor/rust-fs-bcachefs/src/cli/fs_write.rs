//! `fs.bcachefs`'s write verbs: write (alias put), mkdir, rm, rmdir, mv, ln, chmod, chown,
//! setfattr, rmfattr. EXPERIMENTAL: each is one transaction of
//! `fs_bcachefs::write::Writer`, which the tests hold to the reference
//! checker and the reference mount; `--journal` commits it through the
//! journal instead of in place.

use std::ffi::OsString;
use std::io::Read;

use super::device::{Device, Target};
use clap::{value_parser, Arg, ArgAction, ArgMatches, Command as Cmd};
use fs_bcachefs::write::Writer;
use fs_bcachefs::Filesystem;
use fs_core::cli::{CliError, Json, Outcome};

const VERBS: &[&str] = &[
    "write", "mkdir", "rm", "rmdir", "mv", "ln", "chmod", "chown", "setfattr", "rmfattr",
];

fn journal() -> Arg {
    Arg::new("journal")
        .long("journal")
        .help("Commit through the journal: the filesystem is replayed when next opened")
        .action(ArgAction::SetTrue)
}

fn path(name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).value_name("PATH").help(help).required(true)
}

/// The write verbs, added to `fs.bcachefs`'s command.
pub fn subcommands(cmd: Cmd) -> Cmd {
    cmd.subcommand(
        Cmd::new("write")
            .visible_alias("put")
            .about("(experimental) Create or replace a file with stdin, or --from FILE")
            .arg(path("path", "The file to write"))
            .arg(
                Arg::new("from")
                    .long("from")
                    .value_name("FILE")
                    .value_parser(value_parser!(OsString))
                    .help("Read the contents from FILE instead of stdin"),
            )
            .arg(
                Arg::new("mode")
                    .long("mode")
                    .value_name("OCTAL")
                    .default_value("644"),
            )
            .arg(journal())
            .after_help(
                "Examples:\n  printf 'hello\\n' | fs.bcachefs disk.img write /hello.txt\n  fs.bcachefs disk.img write /hello.txt --from hello.txt\n  fs.bcachefs disk.img write /hello.txt --from hello.txt --mode 640 --journal\n\nput is an identical alias. --mode sets permissions only when creating a file.",
            ),
    )
    .subcommand(
        Cmd::new("mkdir")
            .about("(experimental) Create a directory")
            .arg(path("path", "The directory to create"))
            .arg(
                Arg::new("mode")
                    .long("mode")
                    .value_name("OCTAL")
                    .default_value("755"),
            )
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("rm")
            .about("(experimental) Remove a file or symlink")
            .arg(path("path", "The name to remove"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("rmdir")
            .about("(experimental) Remove an empty directory")
            .arg(path("path", "The directory to remove"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("mv")
            .about("(experimental) Rename: FROM to TO, which must not exist")
            .arg(path("from", "The name to move"))
            .arg(path("to", "Its new name"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("ln")
            .about("(experimental) Make a hard link, or with -s a symlink to TARGET")
            .arg(
                Arg::new("symbolic")
                    .short('s')
                    .action(ArgAction::SetTrue)
                    .help("Make a symlink whose target is TARGET, as given"),
            )
            .arg(Arg::new("target").value_name("TARGET").required(true))
            .arg(path("path", "The new name"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("chmod")
            .about("(experimental) Set permissions")
            .arg(Arg::new("mode").value_name("OCTAL").required(true))
            .arg(path("path", "The path to change"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("chown")
            .about("(experimental) Set owner and group: UID:GID, UID: or :GID")
            .arg(Arg::new("owner").value_name("UID:GID").required(true))
            .arg(path("path", "The path to change"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("setfattr")
            .about("(experimental) Set an extended attribute (user.* or trusted.*)")
            .arg(Arg::new("name").value_name("NAME").required(true))
            .arg(Arg::new("value").value_name("VALUE").required(true))
            .arg(path("path", "The path to change"))
            .arg(journal()),
    )
    .subcommand(
        Cmd::new("rmfattr")
            .about("(experimental) Remove an extended attribute")
            .arg(Arg::new("name").value_name("NAME").required(true))
            .arg(path("path", "The path to change"))
            .arg(journal()),
    )
}

fn failed(e: fs_bcachefs::Error) -> CliError {
    match e {
        fs_bcachefs::Error::Unsupported(m) => CliError::not_implemented(m),
        e => CliError::failed(e.to_string()),
    }
}

/// A path's parent directory, by inode, and its last component.
fn split(target: &Target<'_>, p: &str) -> Result<(u64, Vec<u8>), CliError> {
    let trimmed = p.trim_end_matches('/');
    let (dir, name) = match trimmed.rfind('/') {
        Some(i) => (&trimmed[..i], &trimmed[i + 1..]),
        None => ("", trimmed),
    };
    if name.is_empty() {
        return Err(CliError::usage(format!("{p:?} names no entry")));
    }
    let fs = Filesystem::open(target.open()?).map_err(failed)?;
    let dir = fs
        .lookup(if dir.is_empty() { "/" } else { dir })
        .map_err(failed)?;
    Ok((dir, name.as_bytes().to_vec()))
}

fn lookup(target: &Target<'_>, p: &str) -> Result<u64, CliError> {
    let fs = Filesystem::open(target.open()?).map_err(failed)?;
    fs.lookup(p).map_err(failed)
}

fn octal(s: &str) -> Result<u32, CliError> {
    u32::from_str_radix(s, 8).map_err(|_| CliError::usage(format!("{s:?} is not an octal mode")))
}

fn writer(dev: Device, journal: bool) -> Result<Writer<Device>, CliError> {
    if journal {
        Writer::open_journalled(dev).map_err(failed)
    } else {
        Writer::open(dev).map_err(failed)
    }
}

/// Resolve and validate through the library's reader before any writable
/// handle is opened. Only a missing final entry is a create; unsupported
/// paths and damaged metadata must retain their own errors.
fn write_path(
    target: &Target<'_>,
    p: &str,
    journal: bool,
) -> Result<(u64, Vec<u8>, Option<u64>), CliError> {
    let (parent, name) = p.rsplit_once('/').unwrap_or(("", p));
    if name.is_empty() || name.len() > 255 || p.split('/').any(|s| s == "." || s == "..") {
        return Err(CliError::usage(format!("{p:?} is not a valid file path")));
    }
    // These constructors only read. A read-only device lets the library
    // check its existing capabilities and journal without risking writes.
    writer(target.open()?, journal)?;
    let fs = Filesystem::open(target.open()?).map_err(failed)?;
    if fs.root().snapshot != u32::MAX {
        return Err(CliError::not_implemented(
            "writing a snapshotted root subvolume",
        ));
    }
    let dir = fs
        .lookup(if parent.is_empty() { "/" } else { parent })
        .map_err(failed)?;
    let parent_inode = fs.inode(dir).map_err(failed)?;
    if !parent_inode.is_dir() {
        return Err(CliError::failed(format!("{parent}: not a directory")));
    }
    let existing = match fs.lookup(p) {
        Ok(ino) => Some(ino),
        Err(fs_bcachefs::Error::NotFound(_)) => None,
        Err(e) => return Err(failed(e)),
    };
    let inode = match existing {
        Some(ino) => {
            let inode = fs.inode(ino).map_err(failed)?;
            if !inode.is_file() {
                return Err(CliError::failed(format!("{p}: not a regular file")));
            }
            inode
        }
        None => {
            if parent_inode.is_casefolded()
                || fs_bcachefs::inode::name_hash(
                    parent_inode.hash_type(),
                    parent_inode.hash_seed,
                    name.as_bytes(),
                )
                .is_none()
            {
                return Err(CliError::not_implemented(
                    "creating entries in a casefolded directory or one with an unknown string hash",
                ));
            }
            parent_inode
        }
    };
    let opts = inode.options;
    if opts.data_replicas > 2 {
        return Err(CliError::not_implemented(
            "writing more than one data replica on one device",
        ));
    }
    let checksum = if opts.data_checksum == 0 {
        u64::from(fs.superblock().data_checksum_opt())
    } else {
        opts.data_checksum - 1
    };
    let compression = if opts.compression == 0 {
        u64::from(fs.superblock().compression_opt())
    } else {
        opts.compression - 1
    };
    if checksum > 3 || compression > 3 || (checksum == 0 && compression != 0) {
        return Err(CliError::not_implemented(
            "the file's data checksum or compression",
        ));
    }
    Ok((dir, name.as_bytes().to_vec(), existing))
}

/// Run a write verb, or `None` when `verb` is not one.
pub fn run(target: &Target<'_>, verb: &str, m: &ArgMatches) -> Option<Result<Outcome, CliError>> {
    if !VERBS.contains(&verb) {
        return None;
    }
    Some(run_verb(target, verb, m))
}

fn run_verb(target: &Target<'_>, verb: &str, m: &ArgMatches) -> Result<Outcome, CliError> {
    let s = |k: &str| m.get_one::<String>(k).cloned().unwrap_or_default();
    let open = || -> Result<Writer<Device>, CliError> {
        let dev = target.open_rw()?;
        if m.get_flag("journal") {
            // Continues a journal an earlier --journal left for replay.
            Writer::open_journalled(dev).map_err(failed)
        } else {
            Writer::open(dev).map_err(failed)
        }
    };
    let mut report = vec![("verb", Json::Str(verb.into()))];
    match verb {
        "write" => {
            let p = s("path");
            let mode = octal(&s("mode"))?;
            if mode > 0o7777 {
                return Err(CliError::usage("mode must be between 0 and 7777 octal"));
            }
            let (dir, name, existing) = write_path(target, &p, m.get_flag("journal"))?;
            let mut data = Vec::new();
            match m.get_one::<OsString>("from") {
                Some(f) => {
                    data = std::fs::read(f).map_err(|e| CliError::failed(format!("{f:?}: {e}")))?;
                }
                None => {
                    std::io::stdin()
                        .read_to_end(&mut data)
                        .map_err(|e| CliError::failed(e.to_string()))?;
                }
            }
            let mut w = open()?;
            let ino = match existing {
                Some(ino) => {
                    w.write_file(ino, &data).map_err(failed)?;
                    ino
                }
                None => w.create_file(dir, &name, &data, mode).map_err(failed)?,
            };
            report.push(("ino", Json::UInt(ino)));
            report.push(("size", Json::UInt(data.len() as u64)));
        }
        "mkdir" => {
            let (dir, name) = split(target, &s("path"))?;
            let mode = octal(&s("mode"))?;
            let ino = open()?.mkdir(dir, &name, mode).map_err(failed)?;
            report.push(("ino", Json::UInt(ino)));
        }
        "rm" => {
            let (dir, name) = split(target, &s("path"))?;
            open()?.unlink(dir, &name).map_err(failed)?;
        }
        "rmdir" => {
            let (dir, name) = split(target, &s("path"))?;
            open()?.rmdir(dir, &name).map_err(failed)?;
        }
        "mv" => {
            let (from_dir, from) = split(target, &s("from"))?;
            let (to_dir, to) = split(target, &s("to"))?;
            open()?
                .rename(from_dir, &from, to_dir, &to)
                .map_err(failed)?;
        }
        "ln" => {
            let (dir, name) = split(target, &s("path"))?;
            let t = s("target");
            if m.get_flag("symbolic") {
                let ino = open()?.symlink(dir, &name, t.as_bytes()).map_err(failed)?;
                report.push(("ino", Json::UInt(ino)));
            } else {
                let ino = lookup(target, &t)?;
                open()?.link(ino, dir, &name).map_err(failed)?;
                report.push(("ino", Json::UInt(ino)));
            }
        }
        "chmod" => {
            let ino = lookup(target, &s("path"))?;
            open()?
                .set_attributes(ino, Some(octal(&s("mode"))?), None, None)
                .map_err(failed)?;
        }
        "chown" => {
            let ino = lookup(target, &s("path"))?;
            let owner = s("owner");
            let (u, g) = owner
                .split_once(':')
                .ok_or_else(|| CliError::usage("owner is UID:GID, UID: or :GID"))?;
            let num = |v: &str| -> Result<Option<u32>, CliError> {
                if v.is_empty() {
                    Ok(None)
                } else {
                    v.parse()
                        .map(Some)
                        .map_err(|_| CliError::usage(format!("{v:?} is not a number")))
                }
            };
            open()?
                .set_attributes(ino, None, num(u)?, num(g)?)
                .map_err(failed)?;
        }
        "setfattr" => {
            let ino = lookup(target, &s("path"))?;
            open()?
                .set_xattr(ino, s("name").as_bytes(), s("value").as_bytes())
                .map_err(failed)?;
        }
        "rmfattr" => {
            let ino = lookup(target, &s("path"))?;
            open()?
                .remove_xattr(ino, s("name").as_bytes())
                .map_err(failed)?;
        }
        _ => unreachable!("checked against VERBS"),
    }
    report.push(("journal", Json::Bool(m.get_flag("journal"))));
    Ok(Outcome::report(Json::object(report)))
}
