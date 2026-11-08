//! Context defaults from the environment, loaded the way cf loads them: the
//! process environment wins over `./.env.local`, which wins over `./.env`.
//! Only the CLI's own context variables are read from the files, and they
//! are never exported to child processes such as plugin hosts.
use std::path::Path;

/// The context variables a `.env` file may provide.
pub const VARIABLES: &[&str] = &["LOVELYCARVE_CASE", "LOVELYCARVE_IMAGE"];

/// Files read in the working directory, the later one winning.
const FILES: &[&str] = &[".env", ".env.local"];

/// Where a context value came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Process,
    File(&'static str),
}

impl Source {
    /// How a notice names the origin of `name`.
    pub fn describe(self, name: &str) -> String {
        match self {
            Self::Process => name.to_owned(),
            Self::File(file) => format!("{file} 中的 {name}"),
        }
    }
}

fn unquote(value: &str) -> String {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|rest| rest.strip_suffix(quote))
        {
            return inner.to_owned();
        }
    }
    value
        .split_once(" #")
        .map_or(value, |(before, _)| before)
        .trim_end()
        .to_owned()
}

/// The last assignment of `name` in `text`.
fn parse(text: &str, name: &str) -> Option<String> {
    text.lines().rev().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return None;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line.split_once('=')?;
        (key.trim() == name).then(|| unquote(value))
    })
}

/// `name` from `directory/.env.local`, else `directory/.env`.
fn from_files(directory: &Path, name: &str) -> Option<(String, Source)> {
    if !VARIABLES.contains(&name) {
        return None;
    }
    FILES.iter().rev().find_map(|file| {
        let text = std::fs::read_to_string(directory.join(file)).ok()?;
        parse(&text, name)
            .filter(|value| !value.is_empty())
            .map(|value| (value, Source::File(file)))
    })
}

/// `name` and where it came from: the process environment, else the files
/// in the working directory.
pub fn lookup(name: &str) -> Option<(String, Source)> {
    if let Some(value) = std::env::var(name).ok().filter(|value| !value.is_empty()) {
        return Some((value, Source::Process));
    }
    from_files(&std::env::current_dir().ok()?, name)
}

pub fn variable(name: &str) -> Option<String> {
    lookup(name).map(|(value, _)| value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_context_variables_from_the_files() {
        let text = "# evidence\nexport LOVELYCARVE_IMAGE=\"D:/evidence/disk one.E01\"\nLOVELYCARVE_CASE=D:/cases/demo # current\nLOVELYCARVE_UNLISTED=1\n";
        assert_eq!(
            parse(text, "LOVELYCARVE_IMAGE").as_deref(),
            Some("D:/evidence/disk one.E01")
        );
        assert_eq!(
            parse(text, "LOVELYCARVE_CASE").as_deref(),
            Some("D:/cases/demo")
        );
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(".env"), text).unwrap();
        assert_eq!(from_files(directory.path(), "LOVELYCARVE_UNLISTED"), None);
        assert_eq!(
            from_files(directory.path(), "LOVELYCARVE_CASE"),
            Some(("D:/cases/demo".to_owned(), Source::File(".env")))
        );
        std::fs::write(
            directory.path().join(".env.local"),
            "LOVELYCARVE_CASE=D:/cases/mine\n",
        )
        .unwrap();
        assert_eq!(
            from_files(directory.path(), "LOVELYCARVE_CASE"),
            Some(("D:/cases/mine".to_owned(), Source::File(".env.local")))
        );
        assert_eq!(
            from_files(directory.path(), "LOVELYCARVE_IMAGE"),
            Some(("D:/evidence/disk one.E01".to_owned(), Source::File(".env")))
        );
        assert_eq!(
            Source::File(".env.local").describe("LOVELYCARVE_CASE"),
            ".env.local 中的 LOVELYCARVE_CASE"
        );
    }
}
