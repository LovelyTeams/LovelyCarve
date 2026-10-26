//! Which tool a process is, from the name it was started under.

use std::ffi::OsString;
use std::path::Path;

use super::family::{Family, Tool};

/// What an invocation turned out to be.
pub enum Target {
    /// A tool, with its own argument vector: `argv[0]` is the tool's name
    /// whichever way it was reached.
    Tool(&'static Tool, Vec<OsString>),
    /// The repository-named entry point itself (`--help`, `--version`,
    /// `doctor`), with the arguments as given.
    Repo(Vec<OsString>),
}

/// Resolve an argument vector, `argv[0]` included.
///
/// `argv[0]`'s file name decides first: a dotted name is that tool. Any
/// other name — the repository's, a renamed copy, cargo's test binary —
/// is the repository entry point, whose first argument may then name a
/// tool by verb or by full name. Unknown names fall through to the entry
/// point rather than failing, so a copy renamed by a packager still
/// answers `--help` and `doctor`.
pub fn resolve(family: &'static Family, mut argv: Vec<OsString>) -> Target {
    let invoked = argv
        .first()
        .map(|arg0| invoked_name(Path::new(arg0)))
        .unwrap_or_default();
    if let Some(tool) = family.by_name(&invoked) {
        argv[0] = OsString::from(tool.name);
        return Target::Tool(tool, argv);
    }
    if let Some(tool) = argv
        .get(1)
        .and_then(|word| word.to_str())
        .and_then(|word| family.by_word(word))
    {
        let mut rest = argv.split_off(2);
        rest.insert(0, OsString::from(tool.name));
        return Target::Tool(tool, rest);
    }
    if argv.is_empty() {
        argv.push(OsString::from(family.repo));
    }
    Target::Repo(argv)
}

/// The name a program was started under: the file name of `argv[0]`,
/// without a Windows `.exe`.
pub fn invoked_name(arg0: &Path) -> String {
    let name = arg0
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match name.strip_suffix(".exe") {
        Some(stem) => stem.to_string(),
        None => name,
    }
}
