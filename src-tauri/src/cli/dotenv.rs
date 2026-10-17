//! Context defaults from the environment: a variable set in the process wins
//! over the same variable in `./.env`.
//! Only the CLI's own context variables are read from the file, and they are
//! never exported to child processes such as plugin hosts.
use std::path::Path;

/// The context variables a `.env` file may provide.
pub const VARIABLES: &[&str] = &["LOVELYCARVE_CASE", "LOVELYCARVE_IMAGE"];

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

fn parse(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return None;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line.split_once('=')?;
        (key.trim() == name).then(|| unquote(value))
    })
}

/// `name` from the process environment, else from `directory/.env`.
pub fn variable_in(directory: &Path, name: &str) -> Option<String> {
    if let Some(value) = std::env::var(name).ok().filter(|value| !value.is_empty()) {
        return Some(value);
    }
    if !VARIABLES.contains(&name) {
        return None;
    }
    let text = std::fs::read_to_string(directory.join(".env")).ok()?;
    parse(&text, name).filter(|value| !value.is_empty())
}

pub fn variable(name: &str) -> Option<String> {
    let directory = std::env::current_dir().ok()?;
    variable_in(&directory, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_context_variables_from_the_file() {
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
        assert_eq!(variable_in(directory.path(), "LOVELYCARVE_UNLISTED"), None);
    }
}
