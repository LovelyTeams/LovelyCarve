//! 案件进程锁:同一案件同一时间只被一个 LovelyCarve 进程打开。
//!
//! 锁是案件目录下的 `case.lock` 上的操作系统独占文件锁(std `File::try_lock`:
//! Windows 为 `LockFileEx`,其它平台为 `flock`),案件打开期间持有,关闭时随
//! 句柄释放;进程崩溃时由操作系统回收,不会留下需要人工清理的陈旧锁。锁按句柄
//! 生效,同一进程里重复打开同一案件同样被拒绝。
//!
//! 逻辑锁(用户在界面上「锁定案件」)是另一回事,存在台账的案件档案里。

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::casework::case_paths;

/// 持有中的案件进程锁;`Drop` 时释放。
pub struct CaseLock {
    file: File,
    path: PathBuf,
}

impl CaseLock {
    /// 锁住案件目录;已被其它进程(或本进程的另一次打开)持有时立即失败。
    pub fn acquire(case_dir: &Path) -> anyhow::Result<Self> {
        let path = case_paths::lock_path(case_dir);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("open case lock {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file, path }),
            Err(TryLockError::WouldBlock) => anyhow::bail!(
                "case {} is already open in another LovelyCarve window or process",
                case_dir.display()
            ),
            Err(TryLockError::Error(error)) => {
                Err(error).with_context(|| format!("lock case {}", path.display()))
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for CaseLock {
    fn drop(&mut self) {
        // 关闭句柄必然释放锁;显式解锁只是缩短窗口。
        if let Err(error) = self.file.unlock() {
            tracing::debug!("case lock: unlock {} failed: {error}", self.path.display());
        }
    }
}

impl std::fmt::Debug for CaseLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CaseLock")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_holder_is_refused_until_release() {
        let directory = tempfile::tempdir().expect("tempdir");
        let first = CaseLock::acquire(directory.path()).expect("first lock");
        let error = CaseLock::acquire(directory.path()).expect_err("held");
        assert!(format!("{error:#}").contains("already open"));
        drop(first);
        let second = CaseLock::acquire(directory.path()).expect("relock after release");
        assert!(second.path().ends_with(case_paths::LOCK_FILE));
    }
}
