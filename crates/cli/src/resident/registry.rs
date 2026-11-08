//! Per-user rendezvous files. A held file lock owns the session name; a PID
//! alone is never treated as proof that a session is still running.
use std::{
    fs::{File, OpenOptions},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

use crate::CliError;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Descriptor {
    pub protocol: u32,
    pub name: String,
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub started_at: u64,
    pub working_directory: PathBuf,
}

pub(super) fn validate_name(name: &str) -> Result<(), CliError> {
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(CliError::invalid(
            "会话名须为 1–64 个字母、数字、下划线或连字符",
        ));
    }
    Ok(())
}

pub(super) fn directory() -> Result<PathBuf, CliError> {
    let path = std::env::var_os("LOVELYCARVE_SESSION_DIR")
        .map(PathBuf::from)
        .or_else(|| crate::host::default_app_data_dir().map(|path| path.join("cli-sessions")))
        .ok_or_else(|| CliError::new("noDataDirectory", "找不到常驻会话目录"))?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&path)?;
    Ok(path)
}

pub(super) fn path(name: &str) -> Result<PathBuf, CliError> {
    validate_name(name)?;
    Ok(directory()?.join(format!("session-{name}.json")))
}

pub(super) fn log_path(name: &str) -> Result<PathBuf, CliError> {
    validate_name(name)?;
    Ok(directory()?.join(format!("session-{name}.log")))
}

/// Remove the record a session left behind when its server exited without
/// closing (a crash, a killed process, a restarted machine). The server holds
/// the name's lock for as long as it runs, so taking the lock here proves it
/// is gone; the record is removed under the lock, before a new server can
/// publish its own. Returns whether a record was removed.
pub(super) fn clear_stale(name: &str) -> Result<bool, CliError> {
    match lock(name) {
        Ok(lock) => remove_record(&lock, name),
        Err(error) if error.code == "sessionAlreadyRunning" => Ok(false),
        Err(error) => Err(error),
    }
}

/// Remove the record of `name`, whose lock the caller holds: no server owns
/// the name, so a record is stale.
pub(super) fn remove_record(_lock: &File, name: &str) -> Result<bool, CliError> {
    match std::fs::remove_file(path(name)?) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// The failure for a session whose server exited and whose record was removed.
pub(super) fn exited(name: &str) -> CliError {
    CliError::new(
        "sessionExited",
        format!(
            "会话 {name} 的后台进程已退出，残留记录已清理；运行 sessions open {name} 重新启动\
             （已打开的检材、解锁、组装卷与当前目录需要重新建立）"
        ),
    )
}

pub(super) fn read(name: &str) -> Result<Descriptor, CliError> {
    let path = path(name)?;
    let file = File::open(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            CliError::new(
                "sessionNotFound",
                format!("会话 {name} 未启动；先运行 sessions open {name}"),
            )
        } else {
            error.into()
        }
    })?;
    let descriptor: Descriptor = serde_json::from_reader(std::io::Read::take(file, 64 * 1024))?;
    if descriptor.name != name
        || descriptor.protocol != super::protocol::VERSION
        || descriptor.port == 0
        || descriptor.token.len() < 32
    {
        return Err(CliError::invalid("常驻会话记录无效或协议不兼容"));
    }
    Ok(descriptor)
}

pub(super) fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

pub(super) fn lock(name: &str) -> Result<File, CliError> {
    validate_name(name)?;
    let file = private_options().open(directory()?.join(format!("session-{name}.lock")))?;
    fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
        CliError::new(
            "sessionAlreadyRunning",
            format!("会话 {name} 已运行或正在启动；用 sessions attach {name} 连接"),
        )
    })?;
    Ok(file)
}

pub(super) struct Registration {
    _lock: File,
    path: PathBuf,
    token: String,
}

impl Registration {
    pub fn publish(lock: File, descriptor: &Descriptor) -> Result<Self, CliError> {
        let path = path(&descriptor.name)?;
        let mut file = tempfile::NamedTempFile::new_in(directory()?)?;
        serde_json::to_writer(file.as_file_mut(), descriptor)?;
        file.as_file().sync_all()?;
        file.persist(&path)
            .map_err(|error| CliError::from(error.error))?;
        Ok(Self {
            _lock: lock,
            path,
            token: descriptor.token.clone(),
        })
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // Do not unlink a record replaced by another owner or an examiner.
        let owned = File::open(&self.path)
            .ok()
            .and_then(|file| serde_json::from_reader::<_, Descriptor>(file).ok())
            .is_some_and(|record| record.token == self.token);
        if owned {
            if let Err(error) = std::fs::remove_file(&self.path) {
                tracing::warn!("remove resident session record: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_names_cannot_escape_the_registry_directory() {
        for name in ["", "../other", "a/b", "a\\b", "a:b", "a b", ".", "中文"] {
            assert!(validate_name(name).is_err(), "{name}");
        }
        assert!(validate_name(&"a".repeat(65)).is_err());
        assert!(validate_name("exam_26-questions").is_ok());
    }
}
