//! Man pages and shell completions, generated from the clap commands the
//! tools actually parse with, so the documentation cannot describe a flag
//! a program does not take.
//!
//! Written into a `share/` directory in the layout every tarball in the
//! family uses, which is also where Homebrew links them from:
//!
//! ```text
//! share/man/man8/mkfs.<fs>.8             section 8: mkfs.* and fsck.*
//! share/man/man1/fs.<fs>.1               section 1: everything else,
//! share/man/man1/<repo>.1                  the repository entry point too
//! share/zsh/site-functions/_<name>
//! share/bash-completion/completions/<name>
//! share/fish/vendor_completions.d/<name>.fish
//! ```
//!
//! One page and one completion per name, the repository's included, and a
//! page per subcommand (`fs.<fs>-ls.1`, `rust-fs-<fs>-doctor.1`).

use std::io;
use std::path::{Path, PathBuf};

use clap::Command as Cmd;
/// The shells [`completion_path`] knows, from the generator itself.
pub use clap_complete::Shell;

use super::family::Family;

/// Every name and its command, the repository entry point last, with its
/// manual section.
fn named_commands(family: &'static Family) -> Vec<(String, u8, Cmd)> {
    let mut all: Vec<(String, u8, Cmd)> = family
        .tools
        .iter()
        .map(|tool| {
            (
                tool.name.to_string(),
                tool.section,
                super::tool_command(family, tool),
            )
        })
        .collect();
    all.push((family.repo.to_string(), 1, super::repo_command(family)));
    all
}

/// Write a man page per name under `share/man/man<section>/`, and one per
/// subcommand beside it (`fs.<fs>-ls.1`), which is what the parent page's
/// SUBCOMMANDS list refers to. Returns the paths written.
pub fn man_pages(family: &'static Family, share: &Path) -> io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (name, section, cmd) in named_commands(family) {
        let dir = share.join("man").join(format!("man{section}"));
        std::fs::create_dir_all(&dir)?;
        // The page's own VERSION line: the version alone, not the
        // `(crate) version` clap prints after a name.
        let cmd = cmd.version(family.version);
        // The entry point's tool verbs are documented once, under their
        // dotted names (its SUBCOMMANDS list says which); only its own
        // subcommands (`doctor`) get pages of their own.
        let skip: Vec<&str> = if name == family.repo {
            family.tools.iter().map(|t| t.verb).collect()
        } else {
            Vec::new()
        };
        write_pages(family, &dir, section, &name, cmd, &skip, &mut written)?;
    }
    Ok(written)
}

fn write_pages(
    family: &Family,
    dir: &Path,
    section: u8,
    name: &str,
    cmd: Cmd,
    skip: &[&str],
    written: &mut Vec<PathBuf>,
) -> io::Result<()> {
    let path = dir.join(format!("{name}.{section}"));
    let mut page = Vec::new();
    // clap names are 'static; a generator run leaks a few dozen bytes.
    let leaked: &'static str = Box::leak(name.to_string().into_boxed_str());
    clap_mangen::Man::new(cmd.clone().name(leaked))
        .title(name.to_uppercase())
        .section(section.to_string())
        .source(format!("{} {}", family.crate_name, family.version))
        .manual(family.repo)
        .render(&mut page)?;
    std::fs::write(&path, page)?;
    written.push(path);
    for sub in cmd.get_subcommands() {
        if sub.is_hide_set() || sub.get_name() == "help" || skip.contains(&sub.get_name()) {
            continue;
        }
        let sub_name = format!("{name}-{}", sub.get_name());
        write_pages(family, dir, section, &sub_name, sub.clone(), &[], written)?;
    }
    Ok(())
}

/// Where each shell's completion for `name` goes under `share/`.
pub fn completion_path(share: &Path, shell: Shell, name: &str) -> PathBuf {
    match shell {
        Shell::Zsh => share.join("zsh/site-functions").join(format!("_{name}")),
        Shell::Bash => share.join("bash-completion/completions").join(name),
        Shell::Fish => share
            .join("fish/vendor_completions.d")
            .join(format!("{name}.fish")),
        other => share.join(other.to_string()).join(name),
    }
}

/// The shells completions are written for.
pub const SHELLS: [Shell; 3] = [Shell::Zsh, Shell::Bash, Shell::Fish];

/// Write zsh, bash and fish completions per name. Returns the paths
/// written.
pub fn completions(family: &'static Family, share: &Path) -> io::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for (name, _, mut cmd) in named_commands(family) {
        for shell in SHELLS {
            let path = completion_path(share, shell, &name);
            std::fs::create_dir_all(path.parent().expect("a completion has a directory"))?;
            let mut script = Vec::new();
            clap_complete::generate(shell, &mut cmd, name.clone(), &mut script);
            std::fs::write(&path, script)?;
            written.push(path);
        }
    }
    Ok(written)
}
