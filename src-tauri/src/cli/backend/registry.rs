//! Per-user rendezvous files, one set per case directory. A held file lock
//! owns the case's backend; a PID alone is never treated as proof that a
//! backend is still running.
use std::{
    fs::{File, OpenOptions},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{cli::CliError, core::casework::case::evidence_path_key};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct Descriptor {
    pub protocol: u32,
    pub case_dir: String,
    pub port: u16,
    pub token: String,
    pub pid: u32,
    pub started_at: u64,
}

/// The file name stem of a case's records: stable for one directory however
/// it is spelled (case and separators on Windows).
fn key(case_dir: &str) -> String {
    let identity = evidence_path_key(case_dir.trim_end_matches(['/', '\\']));
    format!(
        "case-{}",
        &hex::encode(Sha256::digest(identity.as_bytes()))[..24]
    )
}

pub(super) fn directory() -> Result<PathBuf, CliError> {
    let path = std::env::var_os("LOVELYCARVE_BACKEND_DIR")
        .map(PathBuf::from)
        .or_else(|| crate::host::default_app_data_dir().map(|path| path.join("cli-backends")))
        .ok_or_else(|| CliError::new("noDataDirectory", "找不到案件后台进程的记录目录"))?;
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

fn path(case_dir: &str) -> Result<PathBuf, CliError> {
    Ok(directory()?.join(format!("{}.json", key(case_dir))))
}

pub(super) fn log_path(case_dir: &str) -> Result<PathBuf, CliError> {
    Ok(directory()?.join(format!("{}.log", key(case_dir))))
}

/// Remove the record a backend left behind when it exited without closing
/// (a crash, a killed process, a restarted machine). The backend holds the
/// lock for as long as it runs, so taking the lock here proves it is gone.
/// Returns whether a record was removed.
pub(super) fn clear_stale(case_dir: &str) -> Result<bool, CliError> {
    match lock(case_dir) {
        Ok(lock) => remove_record(&lock, case_dir),
        Err(error) if error.code == "caseBackendRunning" => Ok(false),
        Err(error) => Err(error),
    }
}

/// Remove the record of `case_dir`, whose lock the caller holds.
pub(super) fn remove_record(_lock: &File, case_dir: &str) -> Result<bool, CliError> {
    match std::fs::remove_file(path(case_dir)?) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// The failure for a backend that exited and whose record was removed.
pub(super) fn exited(case_dir: &str) -> CliError {
    CliError::new(
        "caseBackendExited",
        format!(
            "案件 {case_dir} 的后台进程已退出，残留记录已清理；运行 case open 重新启动\
             （解锁、组装卷和挂载需要重新建立）"
        ),
    )
}

/// The descriptor of a backend that published one; `None` when none did.
pub(super) fn read(case_dir: &str) -> Result<Option<Descriptor>, CliError> {
    let path = path(case_dir)?;
    let file = match File::open(&path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let descriptor: Descriptor = serde_json::from_reader(std::io::Read::take(file, 64 * 1024))?;
    if descriptor.protocol != super::protocol::VERSION
        || descriptor.port == 0
        || descriptor.token.len() < 32
    {
        return Err(CliError::invalid("案件后台进程记录无效或协议不兼容"));
    }
    Ok(Some(descriptor))
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

pub(super) fn lock(case_dir: &str) -> Result<File, CliError> {
    let file = private_options().open(directory()?.join(format!("{}.lock", key(case_dir))))?;
    fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
        CliError::new(
            "caseBackendRunning",
            format!("案件 {case_dir} 的后台进程已运行或正在启动"),
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
        let path = path(&descriptor.case_dir)?;
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
        // Do not unlink a record replaced by another owner.
        let owned = File::open(&self.path)
            .ok()
            .and_then(|file| serde_json::from_reader::<_, Descriptor>(file).ok())
            .is_some_and(|record| record.token == self.token);
        if owned {
            if let Err(error) = std::fs::remove_file(&self.path) {
                tracing::warn!("remove case backend record: {error}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_case_directory_has_one_key_however_it_is_written() {
        let plain = key("D:/cases/nas");
        assert!(plain.starts_with("case-") && plain.len() == 29);
        assert_eq!(key("D:/cases/nas/"), plain);
        #[cfg(windows)]
        assert_eq!(key(r"d:\Cases\NAS"), plain);
        assert_ne!(key("D:/cases/other"), plain);
    }
}
