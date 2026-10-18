//! X-Ways-style discovery of the contiguous prefix starting at the selected file.
use crate::{invalid, Discovery, MAX_SEGMENTS};
use std::io;
use std::path::{Path, PathBuf};

/// Discover only consecutive numeric extensions, stopping at the first gap.
/// A nonnumeric first extension may continue at `.001`, or at `.002` when
/// `.001` is absent. Later numbered members may be selected deliberately.
/// Unlike strict discovery this does not search backward or scan the directory.
pub fn discover_contiguous(path: impl AsRef<Path>) -> io::Result<Discovery> {
    let path = path.as_ref();
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(invalid("forced RAW input must be a regular file"));
    }
    let mut paths = vec![path.to_path_buf()];
    let extension = path.extension().and_then(|value| value.to_str());
    let numbered = extension.is_some_and(|extension| {
        !extension.is_empty() && extension.bytes().all(|byte| byte.is_ascii_digit())
    });
    let (mut number, width) = if numbered {
        let extension = extension.ok_or_else(|| invalid("RAW extension missing"))?;
        let number = extension
            .parse::<u64>()
            .map_err(|_| invalid("RAW segment number overflow"))?;
        (number.checked_add(1), extension.len().clamp(3, 5))
    } else if path.with_extension("001").try_exists()? {
        (Some(1), 3)
    } else {
        (Some(2), 3)
    };
    while let Some(next) = number {
        let candidate: PathBuf = path.with_extension(format!("{next:0width$}"));
        if !candidate.try_exists()? {
            break;
        }
        if paths.len() >= MAX_SEGMENTS {
            return Err(invalid("RAW segment count limit exceeded"));
        }
        paths.push(candidate);
        number = next.checked_add(1);
    }
    Ok(Discovery {
        numbered: numbered || paths.len() > 1,
        paths,
    })
}
