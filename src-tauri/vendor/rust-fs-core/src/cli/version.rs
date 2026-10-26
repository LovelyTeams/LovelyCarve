//! The identifying `--version` line: `<tool> (<crate>) <version>`.
//!
//! One shape for every name the binary answers to, because the question
//! it exists to answer is "is the program PATH found ours?", asked by
//! `doctor` and by every test suite before it trusts a tool.

use super::family::Family;

/// The version clap prints after a command's name: `(<crate>) <version>`,
/// so `fs.<fs> --version` reads `fs.<fs> (rust-fs-<fs>) 1.2.3`.
pub fn clap_version(family: &Family) -> &'static str {
    // clap takes a 'static str; one per process, a few dozen bytes.
    Box::leak(format!("({}) {}", family.crate_name, family.version).into_boxed_str())
}

/// The whole line for `tool`.
pub fn line(family: &Family, tool: &str) -> String {
    format!("{tool} ({}) {}", family.crate_name, family.version)
}

/// What a version line says, if it has our shape.
#[derive(Debug, PartialEq, Eq)]
pub struct Identity {
    pub tool: String,
    pub crate_name: String,
    pub version: String,
}

/// Read a `--version` answer: the first line, as `<tool> (<crate>)
/// <version>`. Anything else — another package's banner, a usage message,
/// nothing at all — is `None`.
pub fn parse(output: &str) -> Option<Identity> {
    let first = output.lines().next()?.trim();
    let (tool, rest) = first.split_once(" (")?;
    let (crate_name, version) = rest.split_once(") ")?;
    let well_formed = !tool.is_empty()
        && !tool.contains(char::is_whitespace)
        && !crate_name.is_empty()
        && !crate_name.contains(char::is_whitespace)
        && version.split('.').count() >= 3
        && !version.contains(char::is_whitespace);
    well_formed.then(|| Identity {
        tool: tool.to_string(),
        crate_name: crate_name.to_string(),
        version: version.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(tool: &str, crate_name: &str, version: &str) -> Option<Identity> {
        Some(Identity {
            tool: tool.into(),
            crate_name: crate_name.into(),
            version: version.into(),
        })
    }

    #[test]
    fn our_shape_is_read() {
        assert_eq!(
            parse("mkfs.ext4 (am-fs-ext4) 0.6.0\n"),
            identity("mkfs.ext4", "am-fs-ext4", "0.6.0")
        );
        assert_eq!(
            parse("img.qcow2 (am-img-qcow2) 0.4.5\ntrailing lines are ignored\n"),
            identity("img.qcow2", "am-img-qcow2", "0.4.5")
        );
    }

    #[test]
    fn another_package_in_our_shape_is_read_and_names_itself() {
        // erofs-utils prints our shape and names its own package, which is
        // how doctor tells it apart: by the crate, not the shape.
        assert_eq!(
            parse("mkfs.erofs (erofs-utils) 1.9.1\n"),
            identity("mkfs.erofs", "erofs-utils", "1.9.1")
        );
    }

    #[test]
    fn other_banners_and_usage_messages_are_nobodys() {
        for foreign in [
            "mke2fs 1.47.0 (5-Feb-2023)",
            "Usage: mkfs.ext4 [-c|-l filename] device",
            "mkntfs v2022.10.3 (libntfs-3g)",
            "Usage: mkntfs [options] device [number-of-sectors]",
            "mkfs.erofs 1.8.10",
            "Usage: mkfs.erofs [OPTIONS] FILE SOURCE",
            "unsquashfs version 4.6.1 (2023/03/25)",
            "btrfs-progs v6.6.3",
            "xfs_db version 6.1.0",
            "",
            // Each half of the shape, broken on its own.
            "fs.x () 1.2.3",
            " (am-fs-x) 1.2.3",
            "two words (am-fs-x) 1.2.3",
            "fs.x (am fs x) 1.2.3",
            "fs.x (am-fs-x) 1.2",
            "fs.x (am-fs-x) 1.2.3 beta",
        ] {
            assert_eq!(parse(foreign), None, "{foreign:?}");
        }
    }

    #[test]
    fn the_line_clap_prints_is_the_line_parse_reads() {
        let family = super::super::Family {
            repo: "rust-fs-x",
            crate_name: "am-fs-x",
            version: "1.2.3",
            about: "",
            install_hints: &[],
            tools: &[],
        };
        assert_eq!(clap_version(&family), "(am-fs-x) 1.2.3");
        assert_eq!(line(&family, "fs.x"), "fs.x (am-fs-x) 1.2.3");
        assert_eq!(
            parse(&line(&family, "fs.x")),
            identity("fs.x", "am-fs-x", "1.2.3")
        );
    }
}
