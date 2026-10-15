//! 自动识别检材的平台与备份类型。
//!
//! 按检材形态分三路:
//!
//! 1. **磁盘镜像路**:逐分区(LVM 展开到逻辑卷)按 *文件系统类型门限 + 标志文件
//!    表* 定平台:严格匹配判"系统分区",再由四平台复核赋平台串
//!    ([`fstype`]);
//! 2. **归档/备份文件路**:扫描条目名与特征文件,识别各
//!    厂商手机备份与取证工具导出包,外加 adb/小米备份魔数([`backup`]);
//! 3. **手机镜像路**:小米固件容器、雷电模拟器备份这类"镜像本身就是手机"的
//!    形态,分区扫描之后兜底为 Android。
//!
//! 结论回写由命令层完成:`evidence.platform` / `evidence.type` /
//! `evidence.extra_info`(厂商、显示名、备份特征)/ `evidence.partitions`。平台
//! 未命中时字段保持空,任务照常提交。

pub mod backup;
pub mod fstype;

use std::collections::VecDeque;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::core::analysis::identify::os_detect_adapters::{
    kind_str, supports_os_detect, Fingerprint,
};
use crate::core::evidence::image::{ImageFormat, Partition, PartitionKind};
use crate::core::filesystem::evidence_session::EvidenceSession;
use crate::core::filesystem::tree::fs_tree::{self, NodeKind};
use crate::core::filesystem::volumes::lvm::{LvmLogicalVolume, LvmVolume};

pub use backup::BackupConclusion;
pub use fstype::MarkerCheck;

/// 检材平台词表,存储在 `evidence.platform`。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvidencePlatform {
    Android,
    Ios,
    Windows,
    MacOs,
    Linux,
    Harmony,
    HuaweiHmos,
    OtherMobile,
}

impl EvidencePlatform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Android => "android",
            Self::Ios => "ios",
            Self::Windows => "windows",
            Self::MacOs => "macos",
            Self::Linux => "linux",
            Self::Harmony => "harmony",
            Self::HuaweiHmos => "huaweihmos",
            Self::OtherMobile => "othermobile",
        }
    }
}

/// 检材走的识别路。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectRoute {
    DiskImage,
    Archive,
    PhoneImage,
}

/// 磁盘/手机镜像的 `evidence.type`。
pub const EVIDENCE_TYPE_DISK: &str = "disk";
/// 非备份归档/目录检材的 `evidence.type`。
pub const EVIDENCE_TYPE_ARCHIVE: &str = "compressfile";

/// 归档条目走查上限:层数与条目数(备份特征文件都在包根或一两层内)。
const ARCHIVE_WALK_MAX_DEPTH: usize = 3;
const ARCHIVE_WALK_MAX_ENTRIES: usize = backup::KEYWORD_SCAN_LIMIT;
/// 备份魔数探测读取的文件头长度(覆盖小米 `.bak` 私有前缀窗口)。
const BACKUP_MAGIC_HEAD_BYTES: usize = 64 * 1024;
/// `huaweicheck.json` 读取上限(随包资源只有 38 字节)。
const HUAWEI_CHECK_READ_CAP: usize = 4096;

/// 一个分区(或 LVM 逻辑卷 / 归档合成根)的平台判定。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartitionPlatform {
    pub partition_id: u8,
    pub label: String,
    pub kind: String,
    /// 平台识别使用的文件系统类型码;不在门限表内为 `None`。
    pub ftype: Option<u64>,
    /// 严格匹配结论("系统分区")。
    pub native_platform: Option<EvidencePlatform>,
    pub native_markers: Vec<MarkerCheck>,
    /// 平台复核结论。
    pub platform: Option<EvidencePlatform>,
    pub go_markers: Vec<MarkerCheck>,
    pub filesystem_opened: bool,
    pub bitlocker_locked: bool,
    pub error: Option<String>,
}

/// 自动识别结论(命令层原样返回前端,并据此回写检材行)。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AutoDetectReport {
    pub image_id: String,
    pub route: DetectRoute,
    /// 主平台(回写 `evidence.platform`)。
    pub platform: Option<EvidencePlatform>,
    /// 各分区命中平台去重。
    pub platforms: Vec<EvidencePlatform>,
    /// 回写 `evidence.type`:`disk` / `compressfile` / 备份类型枚举词。
    pub evidence_type: String,
    pub is_backup: bool,
    pub manufactor: Option<String>,
    /// 包含厂商与备份工具信息的平台显示名。
    pub display_platform_name: String,
    /// 平台的人读短名。
    pub friend_platform_name: String,
    pub partitions: Vec<PartitionPlatform>,
    pub backup: Option<BackupConclusion>,
    /// 不影响识别继续执行的警告,保留供排障。
    pub warnings: Vec<String>,
}

/// 识别因任务取消(卸载镜像或 `task_cancel`)而中止,不产出部分报告。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DetectCancelled;

impl std::fmt::Display for DetectCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("自动识别已取消")
    }
}

impl std::error::Error for DetectCancelled {}

/// 取消检查点:分区、逻辑卷与归档目录之间。单个探测本身有界,不再细分。
fn ensure_active(cancel: &CancellationToken) -> Result<(), DetectCancelled> {
    if cancel.is_cancelled() {
        Err(DetectCancelled)
    } else {
        Ok(())
    }
}

/// 入口:对已加载镜像跑完整识别。
pub fn detect_image(
    image: &Arc<EvidenceSession>,
    cancel: &CancellationToken,
) -> Result<AutoDetectReport, DetectCancelled> {
    ensure_active(cancel)?;
    let format = image.info().source_format;
    let mut warnings = Vec::new();

    // 字节魔数先行:adb `.ab` / 小米 `.bak` 以 raw 打开时既无分区也不是归档。
    let header_backup = if image.folder.is_some() {
        None
    } else {
        probe_backup_magic(image, &mut warnings)
    };
    if header_backup.is_some() {
        return Ok(finish(
            image,
            DetectRoute::Archive,
            Vec::new(),
            header_backup,
            None,
            warnings,
        ));
    }

    if is_archive_like(image) {
        return detect_archive(image, format, warnings, cancel);
    }
    let route = match format {
        ImageFormat::XiaomiFirmware | ImageFormat::Ldplayer => DetectRoute::PhoneImage,
        ImageFormat::Raw
        | ImageFormat::Ewf
        | ImageFormat::Vmdk
        | ImageFormat::Qcow2
        | ImageFormat::Phdi
        | ImageFormat::Vdi
        | ImageFormat::Vhd
        | ImageFormat::Vhdx
        | ImageFormat::Dmg
        | ImageFormat::Pmfx
        | ImageFormat::Docker
        | ImageFormat::Tar
        | ImageFormat::Zip
        | ImageFormat::Sevenz
        | ImageFormat::Ad1
        | ImageFormat::PhysicalDisk
        | ImageFormat::Folder
        | ImageFormat::Decrypted
        | ImageFormat::ShadowCopy
        | ImageFormat::Assembled => DetectRoute::DiskImage,
    };
    let partitions = walk_partitions(image, cancel)?;
    if partitions.is_empty() {
        // 未命中平台时记录诊断信息。
        warnings.push("dont know what's the image type".to_owned());
    }
    let phone_manufactor = match format {
        ImageFormat::XiaomiFirmware => Some("xiaomi".to_owned()),
        _ => None,
    };
    Ok(finish(
        image,
        route,
        partitions,
        None,
        phone_manufactor,
        warnings,
    ))
}

fn is_archive_like(image: &EvidenceSession) -> bool {
    image.tar.is_some()
        || image.zip.is_some()
        || image.sevenz.is_some()
        || image.ad1.is_some()
        || image.docker.is_some()
        || image.folder.is_some()
}

/// adb 备份:读文件头比对 `ANDROID BACKUP`。
fn probe_backup_magic(
    image: &Arc<EvidenceSession>,
    warnings: &mut Vec<String>,
) -> Option<BackupConclusion> {
    let len = usize::try_from(image.len().min(BACKUP_MAGIC_HEAD_BYTES as u64)).unwrap_or(0);
    if len == 0 {
        return None;
    }
    match image.read_range(0, len) {
        Ok(head) => backup::detect_android_backup_header(&head),
        Err(err) => {
            warnings.push(format!("open img failed: {err}"));
            None
        }
    }
}

// ----- 磁盘镜像路 ------------------------------------------------------------

fn walk_partitions(
    image: &Arc<EvidenceSession>,
    cancel: &CancellationToken,
) -> Result<Vec<PartitionPlatform>, DetectCancelled> {
    let mut out = Vec::new();
    for partition in &image.partitions {
        ensure_active(cancel)?;
        match partition.kind {
            PartitionKind::Lvm => match LvmVolume::open(image, partition) {
                Ok(lvm) if lvm.logical_volumes.is_empty() => {
                    out.push(unsupported_partition(partition));
                }
                Ok(lvm) => {
                    for logical_volume in &lvm.logical_volumes {
                        ensure_active(cancel)?;
                        out.push(probe_logical_volume(image, partition, logical_volume));
                    }
                }
                Err(err) => {
                    // LVM 卷组读取失败时保留分区和错误信息。
                    let mut entry = unsupported_partition(partition);
                    entry.error = Some(format!("get LVM VG failed: {err}"));
                    out.push(entry);
                }
            },
            PartitionKind::Bitlocker if image.bitlocker_unlocked(partition.id).is_none() => {
                let mut entry = unsupported_partition(partition);
                entry.bitlocker_locked = true;
                out.push(entry);
            }
            kind if supports_os_detect(kind) => {
                out.push(probe_root(
                    image,
                    format!("part:{}", partition.id),
                    partition.id,
                    partition_label(partition),
                    kind_str(kind).to_owned(),
                    fstype::partition_ftype(kind),
                ));
            }
            _ => out.push(unsupported_partition(partition)),
        }
    }
    Ok(out)
}

fn probe_logical_volume(
    image: &Arc<EvidenceSession>,
    partition: &Partition,
    logical_volume: &LvmLogicalVolume,
) -> PartitionPlatform {
    let label = format!(
        "LV {}/{} · {}",
        partition.id, logical_volume.index, logical_volume.name
    );
    let kind = format!("lvm:{}", kind_str(logical_volume.kind));
    if !supports_os_detect(logical_volume.kind) {
        return PartitionPlatform {
            partition_id: partition.id,
            label,
            kind,
            ftype: fstype::partition_ftype(logical_volume.kind),
            native_platform: None,
            native_markers: Vec::new(),
            platform: None,
            go_markers: Vec::new(),
            filesystem_opened: false,
            bitlocker_locked: false,
            error: None,
        };
    }
    probe_root(
        image,
        format!("lvm:{}:{}", partition.id, logical_volume.index),
        partition.id,
        label,
        kind,
        fstype::partition_ftype(logical_volume.kind),
    )
}

/// 对一个 fs_tree 根(`part:N` / `lvm:N:M` / 归档合成根)跑严格匹配与平台复核。
fn probe_root(
    image: &Arc<EvidenceSession>,
    root_id: String,
    partition_id: u8,
    label: String,
    kind: String,
    ftype: Option<u64>,
) -> PartitionPlatform {
    // 先列根目录:把"文件系统打不开"与"打开了但无标志命中"区分开
    if let Err(err) = fs_tree::children_for_image(image, &root_id) {
        return PartitionPlatform {
            partition_id,
            label,
            kind,
            ftype,
            native_platform: None,
            native_markers: Vec::new(),
            platform: None,
            go_markers: Vec::new(),
            filesystem_opened: false,
            bitlocker_locked: false,
            error: Some(format!("open filesystem with img failed: {err}")),
        };
    }
    tracing::debug!("autodetect: {root_id} fstype: {ftype:#x?}");
    let fingerprint = Fingerprint::new(Arc::clone(image), root_id);
    let (native_platform, native_markers) = match ftype {
        Some(code) => fstype::native_detect(code, &fingerprint),
        None => (None, Vec::new()),
    };
    let (platform, go_markers) = fstype::go_detect(ftype, &fingerprint);
    PartitionPlatform {
        partition_id,
        label,
        kind,
        ftype,
        native_platform,
        native_markers,
        platform,
        go_markers,
        filesystem_opened: true,
        bitlocker_locked: false,
        error: None,
    }
}

fn unsupported_partition(partition: &Partition) -> PartitionPlatform {
    PartitionPlatform {
        partition_id: partition.id,
        label: partition_label(partition),
        kind: kind_str(partition.kind).to_owned(),
        ftype: fstype::partition_ftype(partition.kind),
        native_platform: None,
        native_markers: Vec::new(),
        platform: None,
        go_markers: Vec::new(),
        filesystem_opened: false,
        bitlocker_locked: false,
        error: None,
    }
}

fn partition_label(partition: &Partition) -> String {
    partition
        .label
        .clone()
        .unwrap_or_else(|| format!("分区{}", partition.id))
}

// ----- 归档/备份文件路 -------------------------------------------------------

fn detect_archive(
    image: &Arc<EvidenceSession>,
    format: ImageFormat,
    mut warnings: Vec<String>,
    cancel: &CancellationToken,
) -> Result<AutoDetectReport, DetectCancelled> {
    if let Some(info) = image.folder.as_ref().and_then(|folder| folder.ios_info()) {
        ensure_active(cancel)?;
        return Ok(finish(
            image,
            DetectRoute::Archive,
            Vec::new(),
            Some(BackupConclusion {
                backup_type: "iosbackup".into(),
                tool: "iTunes/Finder/MobileSync 备份".into(),
                platform: EvidencePlatform::Ios,
                manufactor: Some("apple".into()),
                features: vec!["Manifest.plist + Manifest.db".into()],
                needs_password: info.locked,
            }),
            Some("apple".into()),
            warnings,
        ));
    }
    let roots = match fs_tree::children_for_image(image, "root") {
        Ok(roots) => roots,
        Err(err) => {
            warnings.push(format!("open img failed: {err}"));
            Vec::new()
        }
    };
    let kind = match serde_json::to_value(format) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "archive".to_owned(),
    };
    // 合成根跑无门限的平台复核:手机提取 tar/zip 常见 data/system 目录。
    let mut partitions = Vec::new();
    let mut entries = Vec::new();
    for (index, root) in roots.iter().enumerate() {
        ensure_active(cancel)?;
        partitions.push(probe_root(
            image,
            root.id.clone(),
            u8::try_from(index).unwrap_or(u8::MAX),
            root.name.clone(),
            kind.clone(),
            None,
        ));
        collect_entries(image, &root.id, &mut entries, &mut warnings, cancel)?;
    }
    let huaweicheck = read_huaweicheck(image, &entries);
    let relative: Vec<String> = entries.into_iter().map(|(_, path)| path).collect();
    let backup = backup::detect_archive_backup(&relative, huaweicheck.as_deref());
    Ok(finish(
        image,
        DetectRoute::Archive,
        partitions,
        backup,
        None,
        warnings,
    ))
}

/// 广度优先走查归档条目(带根 id,便于回读特征文件)。
fn collect_entries(
    image: &Arc<EvidenceSession>,
    root_id: &str,
    entries: &mut Vec<(String, String)>,
    warnings: &mut Vec<String>,
    cancel: &CancellationToken,
) -> Result<(), DetectCancelled> {
    let mut queue: VecDeque<(String, String, usize)> = VecDeque::new();
    queue.push_back((root_id.to_owned(), String::new(), 0));
    while let Some((node_id, prefix, depth)) = queue.pop_front() {
        ensure_active(cancel)?;
        if entries.len() >= ARCHIVE_WALK_MAX_ENTRIES {
            warnings.push(format!(
                "archive walk truncated at {ARCHIVE_WALK_MAX_ENTRIES} entries"
            ));
            return Ok(());
        }
        let children = match fs_tree::children_for_image(image, &node_id) {
            Ok(children) => children,
            Err(err) => {
                warnings.push(format!("list {node_id}: {err}"));
                continue;
            }
        };
        for child in children {
            let path = if prefix.is_empty() {
                child.name.clone()
            } else {
                format!("{prefix}/{}", child.name)
            };
            if child.kind == NodeKind::Dir && depth + 1 < ARCHIVE_WALK_MAX_DEPTH {
                queue.push_back((child.id.clone(), path.clone(), depth + 1));
            }
            entries.push((root_id.to_owned(), path));
            if entries.len() >= ARCHIVE_WALK_MAX_ENTRIES {
                break;
            }
        }
    }
    Ok(())
}

/// 包内 `huaweicheck.json` 有则读出文本。
fn read_huaweicheck(image: &Arc<EvidenceSession>, entries: &[(String, String)]) -> Option<String> {
    let (root_id, path) = entries
        .iter()
        .find(|(_, path)| path.rsplit('/').next() == Some("huaweicheck.json"))?;
    let fingerprint = Fingerprint::new(Arc::clone(image), root_id.clone());
    match fingerprint.read_limited(path, HUAWEI_CHECK_READ_CAP) {
        Ok(Some(bytes)) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Ok(None) => None,
        Err(err) => {
            tracing::debug!("autodetect: read huaweicheck.json: {err}");
            None
        }
    }
}

// ----- 聚合与收尾 -------------------------------------------------------------

/// 主平台与平台列表:优先严格匹配与平台复核一致的分区,其次首个平台复核
/// 命中,再次首个严格匹配命中。
pub fn aggregate_platforms(
    partitions: &[PartitionPlatform],
) -> (Option<EvidencePlatform>, Vec<EvidencePlatform>) {
    let mut platforms = Vec::new();
    for partition in partitions {
        for candidate in [partition.platform, partition.native_platform]
            .into_iter()
            .flatten()
        {
            if !platforms.contains(&candidate) {
                platforms.push(candidate);
            }
        }
    }
    let anchored = partitions
        .iter()
        .find(|partition| {
            partition.native_platform.is_some() && partition.platform == partition.native_platform
        })
        .and_then(|partition| partition.platform);
    let go_first = partitions.iter().find_map(|partition| partition.platform);
    let native_first = partitions
        .iter()
        .find_map(|partition| partition.native_platform);
    (anchored.or(go_first).or(native_first), platforms)
}

fn finish(
    image: &EvidenceSession,
    route: DetectRoute,
    partitions: Vec<PartitionPlatform>,
    backup: Option<BackupConclusion>,
    route_manufactor: Option<String>,
    mut warnings: Vec<String>,
) -> AutoDetectReport {
    let (mut platform, mut platforms) = aggregate_platforms(&partitions);
    let mut manufactor = route_manufactor;
    let evidence_type = match (&backup, route) {
        (Some(backup), _) => {
            platform = Some(backup.platform);
            if !platforms.contains(&backup.platform) {
                platforms.insert(0, backup.platform);
            }
            manufactor = backup.manufactor.clone().or(manufactor);
            if backup.needs_password {
                warnings.push("未预填密码或密码错误".to_owned());
            }
            backup.backup_type.clone()
        }
        (None, DetectRoute::PhoneImage) => {
            if platform.is_none() {
                platform = Some(EvidencePlatform::Android);
                platforms.push(EvidencePlatform::Android);
            }
            EVIDENCE_TYPE_DISK.to_owned()
        }
        (None, DetectRoute::DiskImage) => EVIDENCE_TYPE_DISK.to_owned(),
        (None, DetectRoute::Archive) => EVIDENCE_TYPE_ARCHIVE.to_owned(),
    };
    if platform.is_none() {
        warnings.push("PlatformIsEmpty".to_owned());
    }
    AutoDetectReport {
        image_id: image.id.clone(),
        route,
        platform,
        platforms,
        evidence_type,
        is_backup: backup.is_some(),
        display_platform_name: backup::gen_display_platform_name(
            platform,
            manufactor.as_deref(),
            backup.as_ref(),
        ),
        friend_platform_name: backup::gen_friend_platform_name(platform),
        manufactor,
        partitions,
        backup,
        warnings,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::collections::BTreeSet;

    use crate::core::analysis::identify::os_detect::OsFingerprint;

    /// 内存文件系统:路径统一为前导 `/`,注册文件时自动补父目录。
    #[derive(Default)]
    pub struct MockFs {
        files: BTreeSet<String>,
        dirs: BTreeSet<String>,
    }

    impl MockFs {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_dirs(mut self, paths: &[&str]) -> Self {
            for path in paths {
                let path = normalize(path);
                self.register_parents(&path);
                self.dirs.insert(path);
            }
            self
        }

        pub fn with_files(mut self, paths: &[&str]) -> Self {
            for path in paths {
                let path = normalize(path);
                self.register_parents(&path);
                self.files.insert(path);
            }
            self
        }

        fn register_parents(&mut self, path: &str) {
            let mut current = path;
            while let Some(index) = current.rfind('/') {
                if index == 0 {
                    break;
                }
                current = &current[..index];
                self.dirs.insert(current.to_owned());
            }
        }
    }

    impl OsFingerprint for MockFs {
        fn read(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
            Ok(self.files.contains(&normalize(path)).then(Vec::new))
        }

        fn exists(&self, path: &str) -> anyhow::Result<bool> {
            let key = normalize(path);
            Ok(self.files.contains(&key) || self.dirs.contains(&key))
        }

        fn is_dir(&self, path: &str) -> anyhow::Result<bool> {
            Ok(self.dirs.contains(&normalize(path)))
        }
    }

    fn normalize(path: &str) -> String {
        format!("/{}", path.replace('\\', "/").trim_matches('/'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detection_routes_preserve_ipc_and_stored_fact_values() {
        assert_eq!(
            serde_json::to_value([
                DetectRoute::DiskImage,
                DetectRoute::Archive,
                DetectRoute::PhoneImage,
            ])
            .expect("serialize detection routes"),
            serde_json::json!(["disk_image", "archive", "phone_image"])
        );
    }

    fn partition(
        id: u8,
        native: Option<EvidencePlatform>,
        go: Option<EvidencePlatform>,
    ) -> PartitionPlatform {
        PartitionPlatform {
            partition_id: id,
            label: format!("分区{id}"),
            kind: "ntfs".into(),
            ftype: Some(fstype::FTYPE_NTFS),
            native_platform: native,
            native_markers: Vec::new(),
            platform: go,
            go_markers: Vec::new(),
            filesystem_opened: true,
            bitlocker_locked: false,
            error: None,
        }
    }

    #[test]
    fn platform_serialises_to_reference_words() {
        for (platform, word) in [
            (EvidencePlatform::Android, "android"),
            (EvidencePlatform::Ios, "ios"),
            (EvidencePlatform::Windows, "windows"),
            (EvidencePlatform::MacOs, "macos"),
            (EvidencePlatform::Linux, "linux"),
            (EvidencePlatform::Harmony, "harmony"),
            (EvidencePlatform::HuaweiHmos, "huaweihmos"),
            (EvidencePlatform::OtherMobile, "othermobile"),
        ] {
            assert_eq!(platform.as_str(), word);
            assert_eq!(
                serde_json::to_string(&platform).expect("serialize"),
                format!("\"{word}\"")
            );
        }
    }

    #[test]
    fn aggregate_prefers_system_partition_agreed_by_both_layers() {
        // 分区 1:平台复核单独判 Linux(比如带 etc/issue 的数据盘);
        // 分区 2:C++ 全匹配 + Go 同意的 Windows 系统盘。
        let partitions = vec![
            partition(1, None, Some(EvidencePlatform::Linux)),
            partition(
                2,
                Some(EvidencePlatform::Windows),
                Some(EvidencePlatform::Windows),
            ),
        ];
        let (platform, platforms) = aggregate_platforms(&partitions);
        assert_eq!(platform, Some(EvidencePlatform::Windows));
        assert_eq!(
            platforms,
            vec![EvidencePlatform::Linux, EvidencePlatform::Windows]
        );
    }

    #[test]
    fn folder_evidence_takes_the_archive_route_until_cancelled() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("notes.txt"), b"evidence").expect("write fixture");
        let image = Arc::new(EvidenceSession::open(temp.path()).expect("open folder evidence"));

        let report = detect_image(&image, &CancellationToken::new()).expect("detect folder");
        assert_eq!(
            (report.route, report.platform, report.evidence_type.as_str()),
            (DetectRoute::Archive, None, EVIDENCE_TYPE_ARCHIVE)
        );

        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(detect_image(&image, &cancel), Err(DetectCancelled));
    }

    #[test]
    fn aggregate_falls_back_to_first_go_then_native_hit() {
        let go_only = vec![
            partition(1, None, None),
            partition(2, None, Some(EvidencePlatform::Android)),
        ];
        assert_eq!(
            aggregate_platforms(&go_only).0,
            Some(EvidencePlatform::Android)
        );
        let native_only = vec![partition(1, Some(EvidencePlatform::Linux), None)];
        assert_eq!(
            aggregate_platforms(&native_only),
            (Some(EvidencePlatform::Linux), vec![EvidencePlatform::Linux])
        );
        assert_eq!(
            aggregate_platforms(&[partition(1, None, None)]),
            (None, Vec::new())
        );
    }
}
