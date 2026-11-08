//! Windows read-only Dokan projection for parsed filesystem trees.
//!
//! The mounted volume is a view over `fs_tree`; no writes are ever forwarded
//! to the evidence image.

#![cfg(target_os = "windows")]

mod filesystem;
use crate::filesystem::evidence_session::EvidenceSession;
use filesystem::*;
mod callbacks;
use callbacks::*;
mod read;
use read::*;
mod paths;
use paths::*;

use std::{
    num::NonZeroUsize,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use dokan::{
    CreateFileInfo, DiskSpaceInfo, FileInfo, FileSystemHandler, FileSystemMounter, FindData,
    MountFlags, MountOptions, OperationInfo, OperationResult, VolumeInfo, IO_SECURITY_CONTEXT,
};
use dokan_sys::win32::{
    FILE_CREATE, FILE_DELETE_ON_CLOSE, FILE_OVERWRITE, FILE_OVERWRITE_IF, FILE_SUPERSEDE,
};
use dokan_sys::win32::{FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF};
use lru::LruCache;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use widestring::{U16CStr, U16CString};
use winapi::{
    shared::{
        ntdef::NTSTATUS,
        ntstatus::{
            STATUS_ACCESS_DENIED, STATUS_FILE_IS_A_DIRECTORY, STATUS_INVALID_PARAMETER,
            STATUS_NOT_A_DIRECTORY, STATUS_NOT_IMPLEMENTED, STATUS_NO_SUCH_FILE,
            STATUS_OBJECT_NAME_COLLISION,
        },
    },
    um::{
        fileapi::GetVolumeInformationW,
        winnt::{
            ACCESS_MASK, DELETE, FILE_APPEND_DATA, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
            FILE_ATTRIBUTE_READONLY, FILE_CASE_PRESERVED_NAMES, FILE_READ_ONLY_VOLUME,
            FILE_UNICODE_ON_DISK, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, FILE_WRITE_EA,
            GENERIC_ALL, GENERIC_WRITE, PSECURITY_DESCRIPTOR,
        },
    },
};

use crate::{
    evidence::image::PartitionKind,
    filesystem::docker::host_docker,
    filesystem::tree::fs_tree::{self, FsNode, NodeKind},
    filesystem::volumes::lvm::{parse_lvm_id, LvmVolume},
};

const MAX_VOLUME_LABEL_CHARS: usize = 32;
const DOKAN_STREAM_CHUNK: usize = 1024 * 1024;
const READONLY_SECURITY_DESCRIPTOR: &[u8] = &[
    0x01, 0x00, 0x04, 0x90, 0x14, 0x00, 0x00, 0x00, 0x24, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x34, 0x00, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x20, 0x00, 0x00, 0x00,
    0x20, 0x02, 0x00, 0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x20, 0x00, 0x00, 0x00,
    0x20, 0x02, 0x00, 0x00, 0x02, 0x00, 0x60, 0x00, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, 0x14, 0x00,
    0x89, 0x00, 0x12, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x12, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x18, 0x00, 0x89, 0x00, 0x12, 0x00, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05,
    0x20, 0x00, 0x00, 0x00, 0x20, 0x02, 0x00, 0x00, 0x00, 0x00, 0x18, 0x00, 0x89, 0x00, 0x12, 0x00,
    0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x05, 0x20, 0x00, 0x00, 0x00, 0x21, 0x02, 0x00, 0x00,
    0x00, 0x00, 0x14, 0x00, 0x89, 0x00, 0x12, 0x00, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
    0x00, 0x00, 0x00, 0x00,
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountShareView {
    pub mount_id: String,
    pub share_name: String,
    pub local_path: String,
    pub unc_path: String,
    pub running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DokanMountView {
    pub id: String,
    pub image_id: String,
    pub root_node_id: String,
    pub mount_point: String,
    pub running: bool,
    pub share: Option<MountShareView>,
}

pub struct DokanMountSession {
    view: DokanMountView,
    stop: Arc<AtomicBool>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl DokanMountSession {
    pub fn view(&self) -> DokanMountView {
        self.view.clone()
    }

    pub fn stop(&self) -> Result<()> {
        self.stop.store(true, Ordering::SeqCst);
        let mount_point =
            U16CString::from_str(&self.view.mount_point).context("invalid Dokan mount point")?;
        if !dokan::unmount(&mount_point) {
            tracing::debug!("Dokan unmount returned false for {}", self.view.mount_point);
        }
        if let Some(handle) = self.join.lock().take() {
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("Dokan mount thread panicked"))?;
        }
        Ok(())
    }
}

impl Drop for DokanMountSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Ok(mount_point) = U16CString::from_str(&self.view.mount_point) {
            let _ = dokan::unmount(&mount_point);
        }
        if let Some(handle) = self.join.get_mut().take() {
            let _ = handle.join();
        }
    }
}

/// Dokan exposes `init` / `shutdown` as process-global APIs (they manipulate
/// the user-mode driver's singleton state), so calling them per-session would
/// have one mount's `shutdown` tear down state another live mount still
/// depends on. We refcount mounts here and only init / shutdown at the
/// transitions 0→1 and 1→0.
static DOKAN_REFCOUNT: AtomicUsize = AtomicUsize::new(0);
static DOKAN_INIT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn dokan_init_lock() -> &'static Mutex<()> {
    DOKAN_INIT_LOCK.get_or_init(|| Mutex::new(()))
}

fn dokan_acquire() {
    let _guard = dokan_init_lock().lock();
    if DOKAN_REFCOUNT.fetch_add(1, Ordering::SeqCst) == 0 {
        dokan::init();
    }
}

fn dokan_release() {
    let _guard = dokan_init_lock().lock();
    if DOKAN_REFCOUNT.fetch_sub(1, Ordering::SeqCst) == 1 {
        dokan::shutdown();
    }
}

pub fn start_readonly_mount(
    id: String,
    image_id: String,
    image: Arc<EvidenceSession>,
    root_node_id: String,
    mount_point: String,
) -> Result<Arc<DokanMountSession>> {
    if root_node_id.trim().is_empty() {
        anyhow::bail!("mount requires a concrete filesystem node id");
    }
    let normalized_mount = normalize_mount_point(&mount_point)?;
    let is_drive = is_drive_letter_mount(&normalized_mount);
    // Directory mount points must be an existing, empty NTFS folder. Validate
    // up front so the common mistakes surface as clear messages instead of an
    // opaque Dokan "Start" failure. Drive-letter mounts skip this.
    if !is_drive {
        validate_folder_mount(&normalized_mount)?;
    }
    let mount_point_u16 =
        U16CString::from_str(&normalized_mount).context("Dokan mount point is not UTF-16 safe")?;
    let root = root_node(&image, &root_node_id)?;
    if root.kind != NodeKind::Dir {
        anyhow::bail!("Dokan mount root must be a directory-like node");
    }

    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let thread_mount = normalized_mount.clone();
    let thread_mount_point = mount_point_u16.clone();
    let thread_root_node_id = root_node_id.clone();

    let join = thread::spawn(move || {
        dokan_acquire();
        let volume_name = volume_name_for_mount(&image, &thread_root_node_id);
        let fs_kind = fs_kind_for_root(&image, &thread_root_node_id);
        // Docker / OCI container filesystems and plain Linux/Android tar
        // extractions are case-sensitive; FAT-derived images stay
        // case-insensitive. We special-case the synthetic `docker:` / `tar:` /
        // `zip:` / `host-docker:` roots before falling back to the regular
        // PartitionKind table because none has a PartitionKind representation.
        let is_docker_root = is_docker_mount_root(&thread_root_node_id);
        let is_tar_root = thread_root_node_id == "tar:" || thread_root_node_id.starts_with("tar:");
        let is_zip_root = thread_root_node_id == "zip:" || thread_root_node_id.starts_with("zip:");
        // AD1 captures are usually of Windows volumes, so the projection stays
        // case-insensitive like the NTFS source it came from.
        let is_ad1_root = thread_root_node_id.starts_with("ad1:");
        let fs_name = if is_docker_root {
            "DOCKER".to_string()
        } else if is_tar_root {
            "TAR".to_string()
        } else if is_zip_root {
            "ZIP".to_string()
        } else if is_ad1_root {
            "AD1".to_string()
        } else {
            fs_kind
                .map(fs_label_for_kind)
                .unwrap_or("LCARVE")
                .to_string()
        };
        let case_sensitive = is_docker_root
            || is_tar_root
            || is_zip_root
            || fs_kind.map(is_case_sensitive_fs).unwrap_or(false);
        let handler = LovelyCarveDokanFs::new(
            image,
            root,
            thread_root_node_id,
            volume_name,
            fs_name,
            fs_kind,
            case_sensitive,
        );
        // MOUNT_MANAGER only for drive-letter mounts: it registers the volume
        // with the Windows Mount Manager (so Get-Volume / Storage WMI see the
        // drive) but its reparse-point registration is rejected for folder
        // targets on many systems, so directory mounts must omit it.
        let mut flags = MountFlags::WRITE_PROTECT | MountFlags::CURRENT_SESSION;
        if is_drive {
            flags |= MountFlags::MOUNT_MANAGER;
        }
        if case_sensitive {
            flags |= MountFlags::CASE_SENSITIVE;
        }
        // `LOVELYCARVE_DOKAN_DEBUG=1` flips on Dokan's kernel-side debug
        // logging + stderr forwarding so the driver's actual complaint
        // (sector-size mismatch, bad volume name, missing privilege, ...)
        // shows up on the console instead of getting swallowed behind the
        // generic "Start" / "the driver responds that something is wrong"
        // message Dokan returns to user-mode.
        if std::env::var_os("LOVELYCARVE_DOKAN_DEBUG").is_some() {
            flags |= MountFlags::DEBUG | MountFlags::STDERR;
        }
        let options = MountOptions {
            // CASE_SENSITIVE is enabled for filesystems that are case-sensitive
            // at rest (ext4, xfs, btrfs, zfs, apfs, hfsx, ufs, squashfs) so the
            // forensic semantics of the original path are preserved.
            // MOUNT_MANAGER (drive-letter mounts only) is set above.
            flags,
            timeout: Duration::from_secs(30),
            allocation_unit_size: 4096,
            sector_size: 4096,
            ..Default::default()
        };
        let mut mounter = FileSystemMounter::new(&handler, &thread_mount_point, &options);
        match mounter.mount() {
            Ok(file_system) => {
                let _ = ready_tx.send(Ok(()));
                drop(file_system);
            }
            Err(err) => {
                let _ = ready_tx.send(Err(format!("{err}")));
            }
        }
        if !thread_stop.load(Ordering::SeqCst) {
            tracing::info!("Dokan mount {} closed", thread_mount);
        }
        dokan_release();
    });

    match ready_rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(())) => Ok(Arc::new(DokanMountSession {
            view: DokanMountView {
                id,
                image_id,
                root_node_id,
                mount_point: normalized_mount,
                running: true,
                share: None,
            },
            stop,
            join: Mutex::new(Some(join)),
        })),
        Ok(Err(err)) => {
            let _ = join.join();
            anyhow::bail!("Dokan mount failed: {err}");
        }
        Err(err) => {
            stop.store(true, Ordering::SeqCst);
            let _ = dokan::unmount(&mount_point_u16);
            let _ = join.join();
            anyhow::bail!("Dokan mount did not become ready: {err}");
        }
    }
}

#[cfg(test)]
mod tests;
