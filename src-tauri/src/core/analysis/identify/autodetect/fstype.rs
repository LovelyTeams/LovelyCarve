//! 磁盘镜像路的平台判定:文件系统类型门限 + 标志文件表。
//!
//! 判定分两层,按文件系统类型码选择规则:
//!
//! - **严格匹配**:按 ftype 选一张标志文件表,
//!   **全部**存在才算命中(AND)。命中即"系统分区"。
//! - **平台复核**:依次调 macOS → Windows → Linux → Android
//!   四个检查函数,首个返回真的赋平台串(短路)。四个函数各有自己的门限与
//!   标志路径,判定多为"任一存在"(OR),Android 例外(见 [`go_detect`])。
//!
//! [`PartitionKind`] 通过 [`partition_ftype`] 折算成平台识别使用的 ftype 码。
//! 未出现在任何门限表中的文件系统(FAT、exFAT、ISO、SquashFS、LUKS…)
//! 返回 `None`,只走无门限的 Linux/Android 检查。

use serde::{Deserialize, Serialize};

use crate::core::analysis::identify::os_detect::OsFingerprint;
use crate::core::evidence::image::PartitionKind;

use super::EvidencePlatform;

/// TSK `TSK_FS_TYPE_NTFS`。Windows 严格匹配与平台复核的门限之一。
pub const FTYPE_NTFS: u64 = 0x1;
/// TSK `TSK_FS_TYPE_EXT2`。Linux 严格匹配的门限之一。
pub const FTYPE_EXT2: u64 = 0x80;
/// TSK `TSK_FS_TYPE_EXT3`。Linux 严格匹配的门限之一。
pub const FTYPE_EXT3: u64 = 0x100;
/// TSK `TSK_FS_TYPE_HFS`(HFS+/HFSX)。macOS 严格匹配与平台复核的门限之一。
pub const FTYPE_HFS: u64 = 0x1000;
/// TSK `TSK_FS_TYPE_EXT4`。Linux 严格匹配的门限之一。
pub const FTYPE_EXT4: u64 = 0x2000;
/// TSK `TSK_FS_TYPE_HFS_LEGACY`。仅用于 macOS 平台复核。
pub const FTYPE_HFS_LEGACY: u64 = 0x8000;
/// `HFS | HFS_LEGACY` 组合位。仅用于 macOS 平台复核。
pub const FTYPE_HFS_ANY: u64 = 0x9000;
/// Linux 扩展类型码,XFS 映射到本码以使用 Linux 标志文件表。
pub const FTYPE_LINUX_EXT_A: u64 = 0x10000;
/// APFS 扩展类型码,用于 macOS 严格匹配与平台复核。
pub const FTYPE_APFS: u64 = 0x100000;
/// Linux 扩展类型码,Btrfs 映射到本码以使用 Linux 标志文件表。
pub const FTYPE_LINUX_EXT_B: u64 = 0x200000;
/// Windows 直接匹配类型码,ReFS 映射到本码,不检查标志文件。
pub const FTYPE_WINDOWS_DIRECT: u64 = 0x4000000;

/// Windows 严格匹配标志文件表(4 项,AND)。
pub const NATIVE_WINDOWS_MARKERS: [&str; 4] =
    ["/$MFT", "/Windows/System32", "/Users", "/Program Files"];
/// macOS 严格匹配标志文件表(8 项,AND)。
pub const NATIVE_MACOS_MARKERS: [&str; 8] = [
    "/bin",
    "/sbin",
    "/etc",
    "/var",
    "/usr",
    "/Applications",
    "/System/Library/CoreServices/SystemVersion.plist",
    "/private/var/db/dslocal/nodes/Default/users",
];
/// Linux 严格匹配标志文件表(6 项,AND)。
pub const NATIVE_LINUX_MARKERS: [&str; 6] =
    ["/boot", "/bin", "/etc/issue", "/home", "/usr", "/var"];

/// macOS 平台复核的标志路径(OR)。
pub const GO_MACOS_MARKERS: [&str; 2] = [
    "/System/Library/CoreServices/SystemVersion.plist",
    "/private/var/db/dslocal/nodes/Default/users",
];
/// Windows 平台复核的标志路径(OR,仅 ftype == NTFS 时查)。
pub const GO_WINDOWS_MARKERS: [&str; 4] =
    ["Windows", "WINDOWS", "Program Files", "Program Files (x86)"];
/// Linux 平台复核的标志路径(OR,无门限)。
pub const GO_LINUX_MARKERS: [&str; 2] = ["etc/issue", "boot/vmlinuz"];
/// Android 平台复核的快捷标志:存在即判 Android。
pub const GO_ANDROID_SHORTCUT_MARKER: &str = "data/com.android.camera";
/// Android 平台复核的目录组:快捷标志缺席时须**全部**存在(AND)。
pub const GO_ANDROID_DIR_MARKERS: [&str; 5] = ["system", "data", "misc", "app", "local"];

/// 一个标志路径的探测结果,供前端逐项展示。
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MarkerCheck {
    pub path: String,
    pub present: bool,
}

/// 分区类型 → 平台识别使用的 ftype 码。`None` 表示该文件系统不在
/// 任何门限表中。
pub fn partition_ftype(kind: PartitionKind) -> Option<u64> {
    match kind {
        PartitionKind::Ntfs => Some(FTYPE_NTFS),
        PartitionKind::Refs => Some(FTYPE_WINDOWS_DIRECT),
        PartitionKind::HfsPlus | PartitionKind::HfsX => Some(FTYPE_HFS),
        PartitionKind::ApfsContainer => Some(FTYPE_APFS),
        PartitionKind::Ext4 => Some(FTYPE_EXT4),
        PartitionKind::Xfs => Some(FTYPE_LINUX_EXT_A),
        PartitionKind::Btrfs => Some(FTYPE_LINUX_EXT_B),
        PartitionKind::Ufs => Some(FTYPE_EXT2),
        PartitionKind::Fat12
        | PartitionKind::Fat16
        | PartitionKind::Fat32
        | PartitionKind::ExFat
        | PartitionKind::Cdfs
        | PartitionKind::Squashfs
        | PartitionKind::Ubi
        | PartitionKind::Bitlocker
        | PartitionKind::BiosBoot
        | PartitionKind::Filevault
        | PartitionKind::Luks
        | PartitionKind::Lvm
        | PartitionKind::Apm
        | PartitionKind::Bsd
        | PartitionKind::Swap
        | PartitionKind::Extended
        | PartitionKind::Mdadm
        | PartitionKind::Zfs
        | PartitionKind::Unknown => None,
    }
}

/// 严格匹配规则:ftype → (平台, 标志文件表)。不在表中的 ftype 返回 `None`。
pub fn native_table(ftype: u64) -> Option<(EvidencePlatform, &'static [&'static str])> {
    match ftype {
        FTYPE_NTFS => Some((EvidencePlatform::Windows, &NATIVE_WINDOWS_MARKERS)),
        FTYPE_HFS | FTYPE_APFS => Some((EvidencePlatform::MacOs, &NATIVE_MACOS_MARKERS)),
        FTYPE_EXT2 | FTYPE_EXT3 | FTYPE_EXT4 | FTYPE_LINUX_EXT_A | FTYPE_LINUX_EXT_B => {
            Some((EvidencePlatform::Linux, &NATIVE_LINUX_MARKERS))
        }
        _ => None,
    }
}

/// 严格匹配判定:所选表内每个路径都存在才命中。返回
/// (命中平台, 逐项探测结果);ftype 不在表中时返回 `(None, [])`。
pub fn native_detect(
    ftype: u64,
    fs: &dyn OsFingerprint,
) -> (Option<EvidencePlatform>, Vec<MarkerCheck>) {
    let Some((platform, markers)) = native_table(ftype) else {
        return (None, Vec::new());
    };
    let checks = probe_all(markers, fs);
    let all_present = checks.iter().all(|check| check.present);
    (all_present.then_some(platform), checks)
}

/// 平台复核:macOS → Windows → Linux → Android 短路。返回
/// (平台, 实际探测过的标志)。`ftype` 为 `None` 时只有无门限的 Linux/Android
/// 检查会跑。
pub fn go_detect(
    ftype: Option<u64>,
    fs: &dyn OsFingerprint,
) -> (Option<EvidencePlatform>, Vec<MarkerCheck>) {
    let mut checks = Vec::new();
    if go_macos(ftype, fs, &mut checks) {
        return (Some(EvidencePlatform::MacOs), checks);
    }
    if go_windows(ftype, fs, &mut checks) {
        return (Some(EvidencePlatform::Windows), checks);
    }
    if go_linux(fs, &mut checks) {
        return (Some(EvidencePlatform::Linux), checks);
    }
    if go_android(fs, &mut checks) {
        return (Some(EvidencePlatform::Android), checks);
    }
    (None, checks)
}

/// macOS 检查:门限 {HFS, HFS_LEGACY, HFS_ANY, APFS},两个标志任一存在即真。
fn go_macos(ftype: Option<u64>, fs: &dyn OsFingerprint, checks: &mut Vec<MarkerCheck>) -> bool {
    if !matches!(
        ftype,
        Some(FTYPE_HFS | FTYPE_HFS_LEGACY | FTYPE_HFS_ANY | FTYPE_APFS)
    ) {
        return false;
    }
    probe_any(&GO_MACOS_MARKERS, fs, checks)
}

/// Windows 检查:ftype == 0x4000000 直接为真;ftype == NTFS 时
/// 四个标志任一存在即真;其余 ftype 为假。
fn go_windows(ftype: Option<u64>, fs: &dyn OsFingerprint, checks: &mut Vec<MarkerCheck>) -> bool {
    match ftype {
        Some(FTYPE_WINDOWS_DIRECT) => true,
        Some(FTYPE_NTFS) => probe_any(&GO_WINDOWS_MARKERS, fs, checks),
        Some(_) | None => false,
    }
}

/// Linux 检查:无门限,`etc/issue`/`boot/vmlinuz` 任一存在即真。
fn go_linux(fs: &dyn OsFingerprint, checks: &mut Vec<MarkerCheck>) -> bool {
    probe_any(&GO_LINUX_MARKERS, fs, checks)
}

/// Android 检查:`data/com.android.camera` 存在直接为真;否则
/// `system`/`data`/`misc`/`app`/`local` 须全部存在。
fn go_android(fs: &dyn OsFingerprint, checks: &mut Vec<MarkerCheck>) -> bool {
    let shortcut = exists(fs, GO_ANDROID_SHORTCUT_MARKER);
    checks.push(MarkerCheck {
        path: GO_ANDROID_SHORTCUT_MARKER.to_owned(),
        present: shortcut,
    });
    if shortcut {
        return true;
    }
    for marker in GO_ANDROID_DIR_MARKERS {
        let present = exists(fs, marker);
        checks.push(MarkerCheck {
            path: marker.to_owned(),
            present,
        });
        if !present {
            return false;
        }
    }
    true
}

/// 逐项探测整张表,完整记录 AND 判定以便前端显示缺了哪一项。
fn probe_all(markers: &[&str], fs: &dyn OsFingerprint) -> Vec<MarkerCheck> {
    markers
        .iter()
        .map(|marker| MarkerCheck {
            path: (*marker).to_owned(),
            present: exists(fs, marker),
        })
        .collect()
}

/// OR 判定:首个标志存在即返回,后面的标志不再探测。
fn probe_any(markers: &[&str], fs: &dyn OsFingerprint, checks: &mut Vec<MarkerCheck>) -> bool {
    for marker in markers {
        let present = exists(fs, marker);
        checks.push(MarkerCheck {
            path: (*marker).to_owned(),
            present,
        });
        if present {
            return true;
        }
    }
    false
}

/// 文件和目录都可作为标志,这里只问"是否存在"。
/// 探测出错按"不存在"处理并记 debug 日志。
fn exists(fs: &dyn OsFingerprint, path: &str) -> bool {
    match fs.exists(path) {
        Ok(present) => present,
        Err(err) => {
            tracing::debug!("autodetect: probe {path}: {err}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::analysis::identify::autodetect::test_support::MockFs;

    #[test]
    fn partition_ftype_maps_every_gated_kind() {
        assert_eq!(partition_ftype(PartitionKind::Ntfs), Some(FTYPE_NTFS));
        assert_eq!(
            partition_ftype(PartitionKind::Refs),
            Some(FTYPE_WINDOWS_DIRECT)
        );
        assert_eq!(partition_ftype(PartitionKind::HfsPlus), Some(FTYPE_HFS));
        assert_eq!(partition_ftype(PartitionKind::HfsX), Some(FTYPE_HFS));
        assert_eq!(
            partition_ftype(PartitionKind::ApfsContainer),
            Some(FTYPE_APFS)
        );
        assert_eq!(partition_ftype(PartitionKind::Ext4), Some(FTYPE_EXT4));
        assert_eq!(partition_ftype(PartitionKind::Xfs), Some(FTYPE_LINUX_EXT_A));
        assert_eq!(
            partition_ftype(PartitionKind::Btrfs),
            Some(FTYPE_LINUX_EXT_B)
        );
        assert_eq!(partition_ftype(PartitionKind::Ufs), Some(FTYPE_EXT2));
        assert_eq!(partition_ftype(PartitionKind::Fat32), None);
        assert_eq!(partition_ftype(PartitionKind::Lvm), None);
    }

    #[test]
    fn native_table_follows_reference_switch() {
        assert_eq!(
            native_table(FTYPE_NTFS).map(|(platform, _)| platform),
            Some(EvidencePlatform::Windows)
        );
        for ftype in [FTYPE_HFS, FTYPE_APFS] {
            assert_eq!(
                native_table(ftype).map(|(platform, _)| platform),
                Some(EvidencePlatform::MacOs)
            );
        }
        for ftype in [
            FTYPE_EXT2,
            FTYPE_EXT3,
            FTYPE_EXT4,
            FTYPE_LINUX_EXT_A,
            FTYPE_LINUX_EXT_B,
        ] {
            assert_eq!(
                native_table(ftype).map(|(platform, _)| platform),
                Some(EvidencePlatform::Linux)
            );
        }
        // 0x4000000 与 HFS_LEGACY 仅用于平台复核,不参与严格匹配。
        assert_eq!(native_table(FTYPE_WINDOWS_DIRECT), None);
        assert_eq!(native_table(FTYPE_HFS_LEGACY), None);
    }

    #[test]
    fn native_detect_requires_every_marker() {
        let full =
            MockFs::new().with_dirs(&["/$MFT", "/Windows/System32", "/Users", "/Program Files"]);
        let (platform, checks) = native_detect(FTYPE_NTFS, &full);
        assert_eq!(platform, Some(EvidencePlatform::Windows));
        assert_eq!(checks.len(), 4);
        assert!(checks.iter().all(|check| check.present));

        let partial = MockFs::new().with_dirs(&["/$MFT", "/Windows/System32", "/Users"]);
        let (platform, checks) = native_detect(FTYPE_NTFS, &partial);
        assert_eq!(platform, None);
        assert_eq!(
            checks,
            vec![
                MarkerCheck {
                    path: "/$MFT".into(),
                    present: true
                },
                MarkerCheck {
                    path: "/Windows/System32".into(),
                    present: true
                },
                MarkerCheck {
                    path: "/Users".into(),
                    present: true
                },
                MarkerCheck {
                    path: "/Program Files".into(),
                    present: false
                },
            ]
        );
    }

    #[test]
    fn native_detect_skips_ungated_ftype() {
        let fs = MockFs::new().with_dirs(&["/boot", "/bin", "/etc/issue", "/home", "/usr", "/var"]);
        assert_eq!(native_detect(FTYPE_WINDOWS_DIRECT, &fs), (None, Vec::new()));
        assert_eq!(
            native_detect(FTYPE_EXT4, &fs).0,
            Some(EvidencePlatform::Linux)
        );
    }

    #[test]
    fn go_macos_needs_gate_and_any_marker() {
        let fs = MockFs::new().with_files(&["/System/Library/CoreServices/SystemVersion.plist"]);
        assert_eq!(
            go_detect(Some(FTYPE_APFS), &fs).0,
            Some(EvidencePlatform::MacOs)
        );
        assert_eq!(
            go_detect(Some(FTYPE_HFS_ANY), &fs).0,
            Some(EvidencePlatform::MacOs)
        );
        // 同样的标志放在 NTFS 上不算 macOS(门限不过),也不是 Windows(无 Windows 标志)。
        assert_eq!(go_detect(Some(FTYPE_NTFS), &fs).0, None);
    }

    #[test]
    fn go_windows_direct_ftype_skips_markers() {
        let empty = MockFs::new();
        let (platform, checks) = go_detect(Some(FTYPE_WINDOWS_DIRECT), &empty);
        assert_eq!(platform, Some(EvidencePlatform::Windows));
        assert!(checks.is_empty());

        let ntfs = MockFs::new().with_dirs(&["/Program Files (x86)"]);
        let (platform, checks) = go_detect(Some(FTYPE_NTFS), &ntfs);
        assert_eq!(platform, Some(EvidencePlatform::Windows));
        // OR 短路:前三个探测为假,第四个命中即停。
        assert_eq!(checks.len(), 4);
        assert!(checks[3].present);
    }

    #[test]
    fn go_linux_has_no_gate() {
        let fs = MockFs::new().with_files(&["/boot/vmlinuz"]);
        assert_eq!(go_detect(None, &fs).0, Some(EvidencePlatform::Linux));
        assert_eq!(
            go_detect(Some(FTYPE_NTFS), &fs).0,
            Some(EvidencePlatform::Linux)
        );
    }

    #[test]
    fn go_android_shortcut_or_all_dirs() {
        let camera = MockFs::new().with_dirs(&["/data/com.android.camera"]);
        assert_eq!(go_detect(None, &camera).0, Some(EvidencePlatform::Android));

        let all_dirs = MockFs::new().with_dirs(&["/system", "/data", "/misc", "/app", "/local"]);
        assert_eq!(
            go_detect(None, &all_dirs).0,
            Some(EvidencePlatform::Android)
        );

        let missing_local = MockFs::new().with_dirs(&["/system", "/data", "/misc", "/app"]);
        let (platform, checks) = go_detect(None, &missing_local);
        assert_eq!(platform, None);
        // 快捷标志 + 5 目录都被探测过,最后一项为假。
        let android_checks: Vec<&MarkerCheck> = checks
            .iter()
            .filter(|check| !GO_LINUX_MARKERS.contains(&check.path.as_str()))
            .collect();
        assert_eq!(android_checks.len(), 6);
        assert!(!android_checks[5].present);
    }

    #[test]
    fn go_mapper_prefers_macos_over_linux_on_apfs() {
        // APFS 根同时带 macOS 与 Linux 风格标志时优先判定 macOS。
        let fs = MockFs::new().with_files(&[
            "/System/Library/CoreServices/SystemVersion.plist",
            "/etc/issue",
        ]);
        assert_eq!(
            go_detect(Some(FTYPE_APFS), &fs).0,
            Some(EvidencePlatform::MacOs)
        );
    }
}
