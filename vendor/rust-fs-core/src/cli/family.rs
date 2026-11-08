//! How a driver describes its tools to the shared plumbing.

use clap::{ArgMatches, Command as Cmd};

use super::output::{CliError, Outcome};

/// One dotted name, and the program behind it.
pub struct Tool {
    /// The name it is installed and invoked under: `mkfs.<fs>`.
    pub name: &'static str,
    /// The word that selects it after the repository name:
    /// `rust-fs-<fs> mkfs ...`.
    pub verb: &'static str,
    /// Its manual section: 8 for `mkfs.*` and `fsck.*`, 1 otherwise.
    pub section: u8,
    /// One line for the repository's help.
    pub about: &'static str,
    /// The exit status of a wrong command line: 2 by the shared contract,
    /// but a tool whose name promises another scheme keeps that scheme's
    /// (`fsck.*` answers 16, as fsck(8) documents, because 2 there means
    /// "reboot").
    pub usage_exit: u8,
    /// Its arguments. The name and version are set by the caller.
    pub command: fn() -> Cmd,
    /// Run it. `Ok` is printed on stdout in the format asked for; `Err`
    /// as a structured error on stderr, with its code as the exit status.
    pub run: fn(&ArgMatches) -> Result<Outcome, CliError>,
}

/// Everything a repository says about itself, once.
pub struct Family {
    /// The repository name, which is also the multi-call binary's own
    /// name and the one entry point nothing else installs.
    pub repo: &'static str,
    /// The crate the binary is built from, as `--version` names it.
    pub crate_name: &'static str,
    /// The crate's version.
    pub version: &'static str,
    /// One line for the repository-named entry point's help.
    pub about: &'static str,
    /// How a missing tool gets installed, most specific first. `doctor`
    /// and the test suites print these when a name does not resolve.
    pub install_hints: &'static [&'static str],
    /// Every dotted name, in the order help lists them.
    pub tools: &'static [Tool],
}

impl Family {
    /// The tool installed under `name`.
    pub fn by_name(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|tool| tool.name == name)
    }

    /// The tool a word after the repository name selects: its verb or its
    /// full dotted name.
    pub fn by_word(&self, word: &str) -> Option<&Tool> {
        self.tools
            .iter()
            .find(|tool| tool.verb == word || tool.name == word)
    }
}
