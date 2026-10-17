//! Read-only filesystem projection commands.

use crate::host::State;

use crate::app::error::AppError;
#[cfg(not(target_os = "windows"))]
use crate::app::error::ErrorCode;
use crate::state::AppState;

#[cfg(target_os = "windows")]
use crate::core::platform::dokan_mount;
#[cfg(target_os = "windows")]
pub use crate::core::platform::dokan_mount::{DokanMountView, MountShareView};
#[cfg(target_os = "windows")]
use crate::core::platform::windows_share;

#[cfg(target_os = "windows")]
use crate::app::access::loaded_image_app_error;
#[cfg(target_os = "windows")]
use uuid::Uuid;
#[cfg(target_os = "windows")]
use winapi::um::fileapi::GetLogicalDrives;

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DokanMountView {
    pub id: String,
    pub image_id: String,
    pub root_node_id: String,
    pub mount_point: String,
    pub running: bool,
    pub share: Option<MountShareView>,
}

#[cfg(not(target_os = "windows"))]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MountShareView {
    pub mount_id: String,
    pub share_name: String,
    pub local_path: String,
    pub unc_path: String,
    pub running: bool,
}

pub async fn fs_mount_readonly(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    mount_point: String,
) -> Result<DokanMountView, AppError> {
    mount_readonly_impl(state, image_id, node_id, mount_point).await
}

#[cfg(target_os = "windows")]
async fn mount_readonly_impl(
    state: State<'_, AppState>,
    image_id: String,
    node_id: String,
    mount_point: String,
) -> Result<DokanMountView, AppError> {
    let image = loaded_image_app_error(&state, &image_id).await?;
    let mount_id = Uuid::new_v4().to_string();
    let session = tokio::task::spawn_blocking({
        let mount_id = mount_id.clone();
        let image_id = image_id.clone();
        move || dokan_mount::start_readonly_mount(mount_id, image_id, image, node_id, mount_point)
    })
    .await
    .map_err(|err| AppError::task_panicked("Dokan mount", err))?
    .map_err(AppError::from)?;
    let view = session.view();
    state
        .dokan_mounts
        .lock()
        .await
        .insert(view.id.clone(), session);
    Ok(view)
}

#[cfg(not(target_os = "windows"))]
async fn mount_readonly_impl(
    _state: State<'_, AppState>,
    _image_id: String,
    _node_id: String,
    _mount_point: String,
) -> Result<DokanMountView, AppError> {
    Err(AppError::new(
        ErrorCode::UnsupportedFormat,
        "Dokan read-only mount is only available on Windows",
    ))
}

pub async fn fs_unmount_readonly(
    state: State<'_, AppState>,
    mount_id: String,
) -> Result<(), AppError> {
    unmount_readonly_impl(state, mount_id).await
}

#[cfg(target_os = "windows")]
async fn unmount_readonly_impl(
    state: State<'_, AppState>,
    mount_id: String,
) -> Result<(), AppError> {
    unshare_mount_by_id(&state, &mount_id).await?;
    let session = state.dokan_mounts.lock().await.remove(&mount_id);
    let Some(session) = session else {
        return Ok(());
    };
    tokio::task::spawn_blocking(move || session.stop())
        .await
        .map_err(|err| AppError::task_panicked("Dokan unmount", err))?
        .map_err(AppError::from)
}

#[cfg(not(target_os = "windows"))]
async fn unmount_readonly_impl(
    _state: State<'_, AppState>,
    _mount_id: String,
) -> Result<(), AppError> {
    Ok(())
}

pub async fn fs_mounts(state: State<'_, AppState>) -> Result<Vec<DokanMountView>, AppError> {
    mounts_impl(state).await
}

#[cfg(target_os = "windows")]
async fn mounts_impl(state: State<'_, AppState>) -> Result<Vec<DokanMountView>, AppError> {
    let shares = state.dokan_mount_shares.lock().await.clone();
    Ok(state
        .dokan_mounts
        .lock()
        .await
        .values()
        .map(|session| {
            let mut view = session.view();
            view.share = shares.get(&view.id).cloned();
            view
        })
        .collect())
}

#[cfg(not(target_os = "windows"))]
async fn mounts_impl(_state: State<'_, AppState>) -> Result<Vec<DokanMountView>, AppError> {
    Ok(Vec::new())
}

pub async fn fs_suggest_mount_point(state: State<'_, AppState>) -> Result<String, AppError> {
    let points = available_mount_points_impl(state).await?;
    points
        .into_iter()
        .next()
        .ok_or_else(|| AppError::resource_limit("没有可用盘符"))
}

#[cfg(target_os = "windows")]
async fn available_mount_points_impl(state: State<'_, AppState>) -> Result<Vec<String>, AppError> {
    let active_mounts = state
        .dokan_mounts
        .lock()
        .await
        .values()
        .filter_map(|session| session.view().mount_point.chars().next())
        .map(|ch| ch.to_ascii_uppercase())
        .collect::<std::collections::HashSet<_>>();
    let drives = unsafe { GetLogicalDrives() };
    let mut points = Vec::new();
    for letter in 'H'..='Y' {
        let bit = 1u32 << ((letter as u8 - b'A') as u32);
        if drives & bit == 0 && !active_mounts.contains(&letter) {
            points.push(format!("{letter}:\\"));
        }
    }
    for letter in 'A'..='Z' {
        let bit = 1u32 << ((letter as u8 - b'A') as u32);
        if !('H'..='Y').contains(&letter) && drives & bit == 0 && !active_mounts.contains(&letter) {
            points.push(format!("{letter}:\\"));
        }
    }
    Ok(points)
}

#[cfg(not(target_os = "windows"))]
async fn available_mount_points_impl(_state: State<'_, AppState>) -> Result<Vec<String>, AppError> {
    Err(AppError::new(
        ErrorCode::UnsupportedFormat,
        "Dokan read-only mount is only available on Windows",
    ))
}

pub async fn fs_available_mount_points(
    state: State<'_, AppState>,
) -> Result<Vec<String>, AppError> {
    available_mount_points_impl(state).await
}

pub async fn fs_share_mount(
    state: State<'_, AppState>,
    mount_id: String,
    share_name: Option<String>,
) -> Result<MountShareView, AppError> {
    share_mount_impl(state, mount_id, share_name).await
}

#[cfg(target_os = "windows")]
async fn share_mount_impl(
    state: State<'_, AppState>,
    mount_id: String,
    share_name: Option<String>,
) -> Result<MountShareView, AppError> {
    if let Some(existing) = state
        .dokan_mount_shares
        .lock()
        .await
        .get(&mount_id)
        .cloned()
    {
        return Ok(existing);
    }

    let mount = state
        .dokan_mounts
        .lock()
        .await
        .get(&mount_id)
        .map(|session| session.view())
        .ok_or_else(|| {
            AppError::not_found("挂载不存在，无法创建局域网共享")
                .with_context_value(serde_json::json!({ "mount_id": mount_id }))
        })?;
    let share_name = windows_share::sanitized_share_name(share_name.as_deref(), &mount.id);
    let local_path = mount.mount_point.clone();
    let share = tokio::task::spawn_blocking({
        let share_name = share_name.clone();
        move || windows_share::create_share(&share_name, &local_path)
    })
    .await
    .map_err(|err| AppError::task_panicked("SMB share", err))?
    .map_err(AppError::from)?;
    let share = MountShareView {
        mount_id: mount.id.clone(),
        share_name: share.share_name,
        local_path: share.local_path,
        unc_path: share.unc_path,
        running: true,
    };
    state
        .dokan_mount_shares
        .lock()
        .await
        .insert(mount.id, share.clone());
    Ok(share)
}

#[cfg(not(target_os = "windows"))]
async fn share_mount_impl(
    _state: State<'_, AppState>,
    _mount_id: String,
    _share_name: Option<String>,
) -> Result<MountShareView, AppError> {
    Err(AppError::new(
        ErrorCode::UnsupportedFormat,
        "LAN sharing for mounted volumes is only available on Windows",
    ))
}

pub async fn fs_unshare_mount(
    state: State<'_, AppState>,
    mount_id: String,
) -> Result<(), AppError> {
    unshare_mount_impl(state, mount_id).await
}

#[cfg(target_os = "windows")]
async fn unshare_mount_impl(state: State<'_, AppState>, mount_id: String) -> Result<(), AppError> {
    unshare_mount_by_id(&state, &mount_id).await
}

#[cfg(not(target_os = "windows"))]
async fn unshare_mount_impl(
    _state: State<'_, AppState>,
    _mount_id: String,
) -> Result<(), AppError> {
    Ok(())
}

#[cfg(target_os = "windows")]
pub(crate) async fn unshare_mount_by_id(state: &AppState, mount_id: &str) -> Result<(), AppError> {
    let share = state.dokan_mount_shares.lock().await.get(mount_id).cloned();
    let Some(share) = share else {
        return Ok(());
    };
    tokio::task::spawn_blocking({
        let share_name = share.share_name.clone();
        move || windows_share::delete_share(&share_name)
    })
    .await
    .map_err(|err| AppError::task_panicked("SMB unshare", err))?
    .map_err(AppError::from)?;
    state.dokan_mount_shares.lock().await.remove(mount_id);
    Ok(())
}

/// Create (or reuse) an empty subdirectory to serve as a folder mount point,
/// returning its absolute path. Layout is `<root>/<image_id>/<name>`: a
/// per-image grouping layer keeps every partition of one image together (and
/// apart from other images), under which the partition leaf lives. The leaf
/// name is sanitized from `name`; on collision with a non-empty entry we append
/// " (2)", " (3)", … An existing *empty* leaf is reused as-is, so re-mounting
/// the same partition doesn't pile up folders. `root` must already exist — the
/// image layer is created if missing (idempotent), only the leaf is the fresh
/// mount point. Cross-platform (`std::fs`); the Windows-only mount validates
/// NTFS/emptiness afterwards.
pub fn fs_create_mount_dir(
    root: String,
    image_id: String,
    name: String,
) -> Result<String, AppError> {
    create_mount_dir_impl(&root, &image_id, &name)
}

fn create_mount_dir_impl(root: &str, image_id: &str, name: &str) -> Result<String, AppError> {
    let trimmed_root = root.trim();
    if trimmed_root.is_empty() {
        return Err(AppError::invalid_input("默认挂载根目录为空"));
    }
    let root_path = std::path::Path::new(trimmed_root);
    if !root_path.is_dir() {
        return Err(AppError::not_found(format!(
            "默认挂载根目录不存在或不是文件夹：{trimmed_root}"
        )));
    }
    // 镜像分组层：用镜像 id 建一层，把同一镜像的各分区挂载目录归到一起。可能
    // 已被本镜像上一个分区的挂载建好，create_dir_all 幂等。
    let image_dir = root_path.join(sanitize_dir_name(image_id));
    std::fs::create_dir_all(&image_dir).map_err(|err| {
        AppError::from_io(
            format!("无法新建镜像挂载目录：{}", image_dir.display()),
            err,
        )
    })?;
    let base = sanitize_dir_name(name);
    for attempt in 1..=999u32 {
        let leaf = if attempt == 1 {
            base.clone()
        } else {
            format!("{base} ({attempt})")
        };
        let candidate = image_dir.join(&leaf);
        // 存在性用 symlink_metadata（lstat，不跟随重解析点）判：Dokan 文件夹挂载
        // 卸载后常留下一个悬空的重解析点目录，`Path::exists` 会跟随它指向已消失
        // 的目标而误判为「不存在」，随后 create_dir 撞上残留条目报「已存在」。
        match std::fs::symlink_metadata(&candidate) {
            Err(_) => {
                // 物理上确实没有此条目 → 新建挂载点。
                std::fs::create_dir(&candidate).map_err(|err| {
                    AppError::from_io(format!("无法新建挂载文件夹：{}", candidate.display()), err)
                })?;
                return Ok(candidate.to_string_lossy().into_owned());
            }
            Ok(_) => {
                // 已有同名条目（常见：上次挂载卸载后残留的空目录 / 悬空重解析点）。
                // 能当空目录读 → 直接复用（挂载点正需一个已存在的空目录）。
                let reusable_empty = candidate.is_dir()
                    && std::fs::read_dir(&candidate)
                        .map(|mut entries| entries.next().is_none())
                        .unwrap_or(false);
                if reusable_empty {
                    return Ok(candidate.to_string_lossy().into_owned());
                }
                // 否则尝试把它清掉再重建：残留的悬空重解析点目录用 remove_dir 可移除
                // （删的是重解析点本身，不是目标）。删成功就重建复用同名，避免每次
                // 卸载重挂都累加后缀。删不掉（真有内容 / 仍被占用 / 是文件）就让给
                // 下一个后缀名。
                if std::fs::remove_dir(&candidate).is_ok() {
                    std::fs::create_dir(&candidate).map_err(|err| {
                        AppError::from_io(
                            format!("无法重建挂载文件夹：{}", candidate.display()),
                            err,
                        )
                    })?;
                    return Ok(candidate.to_string_lossy().into_owned());
                }
            }
        }
    }
    Err(AppError::resource_limit(format!(
        "挂载文件夹名冲突过多，请清理 {} 下的旧挂载目录",
        image_dir.display()
    )))
}

/// Map a partition/image title into a safe single path segment: replace the
/// Windows-reserved characters and control chars with `_`, trim surrounding
/// whitespace and dots (trailing dots are illegal on Windows). Falls back to
/// `mount` when nothing usable remains.
fn sanitize_dir_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "mount".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{create_mount_dir_impl, sanitize_dir_name};

    fn scratch_root() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lovelycarve-mount-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("scratch root");
        dir
    }

    #[test]
    fn sanitize_strips_reserved_and_falls_back() {
        assert_eq!(sanitize_dir_name("分区 1 (NTFS)"), "分区 1 (NTFS)");
        assert_eq!(sanitize_dir_name("C:/bad\\name?"), "C__bad_name_");
        assert_eq!(sanitize_dir_name("  trailing.. "), "trailing");
        assert_eq!(sanitize_dir_name("***"), "___");
        assert_eq!(sanitize_dir_name("   "), "mount");
    }

    #[test]
    fn creates_then_reuses_empty_and_suffixes_nonempty() {
        let root = scratch_root();
        let root_str = root.to_string_lossy();
        let img = "img-abc";

        // First call creates <root>/<image_id>/<leaf>.
        let a = create_mount_dir_impl(&root_str, img, "分区 1").expect("create");
        assert!(std::path::Path::new(&a).is_dir());
        assert!(a.ends_with("分区 1"));
        // The image-id grouping layer sits between root and the leaf.
        assert!(root.join(img).join("分区 1").is_dir());

        // Second call reuses the same empty directory (no pile-up).
        let b = create_mount_dir_impl(&root_str, img, "分区 1").expect("reuse");
        assert_eq!(a, b);

        // Once it's non-empty, the next call suffixes instead of reusing.
        std::fs::write(std::path::Path::new(&a).join("occupied.txt"), b"x").expect("occupy");
        let c = create_mount_dir_impl(&root_str, img, "分区 1").expect("suffix");
        assert_ne!(a, c);
        assert!(c.ends_with("分区 1 (2)"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn separate_images_get_separate_layers() {
        let root = scratch_root();
        let root_str = root.to_string_lossy();
        let a = create_mount_dir_impl(&root_str, "img-1", "分区 0").expect("img1");
        let b = create_mount_dir_impl(&root_str, "img-2", "分区 0").expect("img2");
        assert_ne!(a, b);
        assert!(root.join("img-1").join("分区 0").is_dir());
        assert!(root.join("img-2").join("分区 0").is_dir());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_root_is_an_error() {
        let root = scratch_root();
        let missing = root.join("does-not-exist");
        let err = create_mount_dir_impl(&missing.to_string_lossy(), "img", "x").unwrap_err();
        assert!(err.message.contains("默认挂载根目录"));
        std::fs::remove_dir_all(&root).ok();
    }
}
