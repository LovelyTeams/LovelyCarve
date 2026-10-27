//! Git Bash and MSYS2 rewrite an argument that starts with `/` into a path
//! below their installation before a native Windows program sees it:
//! `/分区1/etc` arrives as `C:/Program Files/Git/分区1/etc`. An evidence path
//! never starts with a drive letter, so such a path is turned back into the
//! one that was typed; one that cannot be is explained instead.
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

/// The evidence path that was typed, when MSYS rewrote it below the root of
/// an MSYS installation (the directory holding `usr/bin/msys-2.0.dll`).
/// Other paths come back unchanged.
pub(super) fn evidence_path(path: &str) -> Cow<'_, str> {
    if !cfg!(windows) || !drive_path(path) {
        return Cow::Borrowed(path);
    }
    let components: Vec<&str> = path
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .collect();
    for length in 1..=components.len() {
        // `C:` alone names the current directory of drive C, not its root.
        let root = format!("{}/", components[..length].join("/"));
        if std::path::Path::new(&root)
            .join("usr/bin/msys-2.0.dll")
            .is_file()
        {
            return Cow::Owned(format!("/{}", components[length..].join("/")));
        }
    }
    Cow::Borrowed(path)
}

/// A failure to resolve a host-looking path says what most likely made it.
pub(super) fn explain(mut error: CliError, path: &str) -> CliError {
    if cfg!(windows) && drive_path(path) {
        error.message.push_str(
            "\n这是 Windows 主机路径，不是镜像内路径。Git Bash / MSYS2 会把以 / 开头的参数改写成主机路径：\
             为该命令设置 MSYS2_ARG_CONV_EXCL='*'，或改用节点 ID（--node-id）。",
        );
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
