//! Bounded segment discovery. It scans the family to detect holes instead of
//! stopping silently at the first missing filename.
use crate::invalid;
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const MAX_SEGMENTS: usize = 65_536;
const MAX_DIRECTORY_ENTRIES: usize = 1_000_000;

#[derive(Debug, Clone)]
pub struct Discovery {
    pub paths: Vec<PathBuf>,
    /// A recognized numbered/alphabetic family, including a single member.
    pub numbered: bool,
}

#[derive(Debug)]
enum Numbering {
    Decimal { width: usize },
    Alphabetic { width: usize },
}

#[derive(Debug)]
struct Family {
    prefix: String,
    numbering: Numbering,
}

impl Family {
    fn parse(name: &str, parent: &Path) -> Option<Self> {
        if let Some((prefix, suffix)) = name.rsplit_once('.') {
            if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                let value = suffix.parse::<u64>().ok()?;
                let width = (1..=suffix.len())
                    .rev()
                    .find(|width| {
                        format!("{value:0width$}") == suffix
                            && [0, 1].iter().any(|first| {
                                parent.join(format!("{prefix}.{first:0width$}")).exists()
                            })
                    })
                    .unwrap_or(suffix.len());
                return Some(Self {
                    prefix: format!("{prefix}."),
                    numbering: Numbering::Decimal { width },
                });
            }
        }
        // No-dot split(1) families use two or three trailing digits/letters.
        // Keep plain .raw/.img/.dd/.dmg filenames outside this heuristic.
        if name.rsplit_once('.').is_some_and(|(_, suffix)| {
            matches!(
                suffix.to_ascii_lowercase().as_str(),
                "raw" | "img" | "dd" | "dmg"
            )
        }) {
            return None;
        }
        for width in [3, 2] {
            let Some(index) = name.len().checked_sub(width) else {
                continue;
            };
            let Some(suffix) = name.get(index..) else {
                continue;
            };
            let numbering = if suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                Numbering::Decimal { width }
            } else if suffix.bytes().all(|byte| byte.is_ascii_lowercase()) {
                Numbering::Alphabetic { width }
            } else {
                continue;
            };
            let prefix = &name[..index];
            let first_suffix = match numbering {
                Numbering::Decimal { width } => "0".repeat(width),
                Numbering::Alphabetic { width } => "a".repeat(width),
            };
            if !parent.join(format!("{prefix}{first_suffix}")).exists() {
                continue;
            }
            return Some(Self {
                prefix: prefix.to_owned(),
                numbering,
            });
        }
        None
    }

    fn identifier(&self, name: &str) -> Option<u64> {
        let prefix = name.get(..self.prefix.len())?;
        #[cfg(windows)]
        let matches = prefix.eq_ignore_ascii_case(&self.prefix);
        #[cfg(not(windows))]
        let matches = prefix == self.prefix;
        if !matches {
            return None;
        }
        let suffix = name.get(self.prefix.len()..)?;
        match self.numbering {
            Numbering::Decimal { width } => {
                if suffix.len() < width
                    || suffix.len() > 9
                    || !suffix.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return None;
                }
                let value = suffix.parse::<u64>().ok()?;
                // Canonical fixed padding, expanding only after carry.
                (format!("{value:0width$}") == suffix).then_some(value)
            }
            Numbering::Alphabetic { width } => {
                if suffix.len() != width || !suffix.bytes().all(|byte| byte.is_ascii_lowercase()) {
                    return None;
                }
                Some(
                    suffix
                        .bytes()
                        .fold(0, |value, byte| value * 26 + u64::from(byte - b'a')),
                )
            }
        }
    }
}

/// Discover an existing path or a basename with a known RAW suffix.
/// Any member may be supplied; returned paths always begin at .0/.1 or aa.
/// Numeric families with a missing first/interior member fail explicitly.
/// Alphabetic heuristics require an aa/aaa first member to avoid hijacking
/// ordinary filenames. Non-UTF8 filenames can use `RawImage::open_segments`.
pub fn discover(path: impl AsRef<Path>) -> io::Result<Discovery> {
    let path = path.as_ref();
    if !path.exists() {
        // Upstream basename suffix priority, including split(1) names.
        for suffix in [
            ".raw", ".dmg", ".img", ".dd", ".000", ".001", ".00", ".01", ".0", ".1", "aa", "00",
            "aaa", "000",
        ] {
            let mut candidate = path.as_os_str().to_os_string();
            candidate.push(suffix);
            let candidate = PathBuf::from(candidate);
            if candidate.try_exists()? {
                return discover(candidate);
            }
        }
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("RAW image not found: {}", path.display()),
        ));
    }
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return Ok(Discovery {
            paths: vec![path.to_path_buf()],
            numbered: false,
        });
    };
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let Some(family) = Family::parse(name, parent) else {
        return Ok(Discovery {
            paths: vec![path.to_path_buf()],
            numbered: false,
        });
    };
    let mut members = BTreeMap::new();
    for (index, entry) in fs::read_dir(parent)?.enumerate() {
        if index >= MAX_DIRECTORY_ENTRIES {
            return Err(invalid("RAW discovery directory entry limit exceeded"));
        }
        let entry = entry?;
        let filename = entry.file_name();
        let Some(filename) = filename.to_str() else {
            continue;
        };
        if let Some(identifier) = family.identifier(filename) {
            if members.len() >= MAX_SEGMENTS {
                return Err(invalid("RAW segment count limit exceeded"));
            }
            members.insert(identifier, entry.path());
        }
    }
    let alphabetic = matches!(family.numbering, Numbering::Alphabetic { .. });
    if alphabetic && !members.contains_key(&0) {
        return Ok(Discovery {
            paths: vec![path.to_path_buf()],
            numbered: false,
        });
    }
    let first = if members.contains_key(&0) { 0 } else { 1 };
    if !members.contains_key(&first) {
        return Err(invalid(format!(
            "missing first RAW segment for {}",
            path.display()
        )));
    }
    let mut paths = Vec::with_capacity(members.len());
    for (index, (identifier, member)) in members.into_iter().enumerate() {
        if identifier != first + index as u64 {
            return Err(invalid(format!(
                "missing RAW segment {} before {}",
                first + index as u64,
                member.display()
            )));
        }
        paths.push(member);
    }
    Ok(Discovery {
        paths,
        numbered: true,
    })
}
