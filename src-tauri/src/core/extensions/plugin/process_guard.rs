use tokio::process::Child;

use super::Result;

/// Keeps a plugin inside the host-owned process resource boundary.
///
/// Windows Job Objects provide kill-on-close, a single-process ceiling, and a
/// per-process memory limit (the package's `memory`). Other platforms retain
/// Tokio's parent `kill_on_drop` behaviour until their native sandbox
/// adapters are added.
pub(super) struct ProcessGuard {
    #[cfg(windows)]
    job: winapi::um::winnt::HANDLE,
}

impl ProcessGuard {
    #[cfg(windows)]
    pub(super) fn attach(child: &Child, memory_limit: u64) -> Result<Self> {
        use std::{mem::size_of, ptr};

        use winapi::{
            shared::minwindef::DWORD,
            um::{
                jobapi2::{AssignProcessToJobObject, CreateJobObjectW, SetInformationJobObject},
                winnt::{
                    JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                    JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                    JOB_OBJECT_LIMIT_PROCESS_MEMORY,
                },
            },
        };

        let process = child.raw_handle().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "plugin process handle is unavailable",
            )
        })?;
        let job = unsafe { CreateJobObjectW(ptr::null_mut(), ptr::null()) };
        if job.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;
        limits.ProcessMemoryLimit = usize::try_from(memory_limit).unwrap_or(usize::MAX);
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&raw const limits).cast_mut().cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as DWORD,
            )
        };
        if configured == 0 {
            unsafe { winapi::um::handleapi::CloseHandle(job) };
            return Err(std::io::Error::last_os_error().into());
        }
        let assigned = unsafe { AssignProcessToJobObject(job, process.cast()) };
        if assigned == 0 {
            unsafe { winapi::um::handleapi::CloseHandle(job) };
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(Self { job })
    }

    #[cfg(not(windows))]
    pub(super) fn attach(_child: &Child, _memory_limit: u64) -> Result<Self> {
        Ok(Self {})
    }
}

#[cfg(windows)]
unsafe impl Send for ProcessGuard {}

#[cfg(windows)]
unsafe impl Sync for ProcessGuard {}

#[cfg(windows)]
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        unsafe { winapi::um::handleapi::CloseHandle(self.job) };
    }
}
