//! Git Bash and MSYS2 rewrite an argument that starts with `/` into a path
//! below their installation before a native Windows program sees it:
//! `/分区1/etc` arrives as `C:/Program Files/Git/分区1/etc`, and one holding
//! `:/` as a path list, `/ev/folder:/a` as `C:\Git\ev\folder;C:\Git\a`. An
//! evidence path never starts with a drive letter, so such a path is turned
//! back into the one that was typed; one that cannot be is explained instead.
use std::borrow::Cow;

use super::CliError;

/// Whether `path` starts with a drive letter, as Windows host paths do.
fn drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'/' | b'\\')
}

/// The `/…` path MSYS rewrote into `path` below the root of an MSYS
/// installation (the directory holding `usr/bin/msys-2.0.dll`).
fn typed(path: &str) -> Option<String> {
    if !drive_path(path) {
        return None;
    }
    let components: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .collect();
    (1..=components.len()).find_map(|length| {
        // `C:` alone names the current directory of drive C, not its root.
        let root = format!("{}/", components[..length].join("/"));
        std::path::Path::new(&root)
            .join("usr/bin/msys-2.0.dll")
            .is_file()
            .then(|| format!("/{}", components[length..].join("/")))
    })
}

/// The evidence path that was typed, when MSYS rewrote it; other paths come
/// back unchanged.
pub(super) fn evidence_path(path: &str) -> Cow<'_, str> {
    if !cfg!(windows) {
        return Cow::Borrowed(path);
    }
    if path.contains(';') {
        let parts: Option<Vec<String>> = path.split(';').map(typed).collect();
        if let Some(parts) = parts {
            return Cow::Owned(parts.join(":"));
        }
    }
    typed(path).map_or(Cow::Borrowed(path), Cow::Owned)
}

const REWRITTEN: &str = "这是 Windows 主机路径，不是检材内路径。Git Bash / MSYS2 会把以 / 开头的参数改写成主机路径：\
     为该命令设置 MSYS2_ARG_CONV_EXCL='*'（Git Bash 也可设 MSYS_NO_PATHCONV=1），或在 PowerShell / cmd 中运行。";

/// Why `path`, given where an evidence address belongs, is not one.
pub(super) fn host_path(path: &str) -> Option<String> {
    (cfg!(windows) && drive_path(path)).then(|| format!("{path}：{REWRITTEN}"))
}

/// A failure to resolve a host-looking path says what most likely made it.
pub(super) fn explain(mut error: CliError, path: &str) -> CliError {
    if cfg!(windows) && drive_path(path) {
        error.message.push('\n');
        error.message.push_str(REWRITTEN);
    }
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn paths_below_an_msys_root_are_turned_back() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("Git");
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        std::fs::write(root.join("usr/bin/msys-2.0.dll"), b"").unwrap();
        let root = root.to_string_lossy().replace('\\', "/");
        assert_eq!(
            evidence_path(&format!("{root}/分区1/etc/os-release")),
            "/分区1/etc/os-release"
        );
        assert_eq!(evidence_path(&format!("{root}/")), "/");
        assert_eq!(
            evidence_path(&format!("{}\\分区1", root.replace('/', "\\"))),
            "/分区1"
        );
        // `:/` made it a path list: every part is turned back and rejoined.
        let windows_root = root.replace('/', "\\");
        assert_eq!(
            evidence_path(&format!(
                "{windows_root}\\ev\\folder;{windows_root}\\hello.txt"
            )),
            "/ev/folder:/hello.txt"
        );
        assert_eq!(
            evidence_path(&format!("{windows_root}\\ev\\tar;{windows_root}\\")),
            "/ev/tar:/"
        );
        // Not below an MSYS root, or not a host path at all: unchanged.
        let elsewhere = directory.path().join("other/分区1");
        let elsewhere = elsewhere.to_string_lossy();
        assert_eq!(evidence_path(&elsewhere), elsewhere);
        for path in ["/分区1/etc", "part:0/etc", "C:", "etc/C:/x"] {
            assert_eq!(evidence_path(path), path);
        }
        let error = explain(CliError::invalid("找不到"), &elsewhere);
        assert!(
            error.message.contains("MSYS2_ARG_CONV_EXCL"),
            "{}",
            error.message
        );
        let error = explain(CliError::invalid("找不到"), "/分区1");
        assert_eq!(error.message, "找不到");
    }
}
