//! Static catalogue surfaced to the dashboard's "module status" cards.
//!
//! Each entry advertises whether the corresponding feature is wired
//! end-to-end (`Ready`) or still has gaps (`Planned`), and what the user
//! gets when they navigate into it. Update this when shipping or removing
//! a major capability.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleSummary {
    pub key: String,
    pub label: String,
    pub status: ModuleStatus,
    pub frontend: bool,
    pub backend: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleStatus {
    Ready,
    Planned,
}

pub fn summary() -> Vec<ModuleSummary> {
    vec![
        module(
            "cases",
            "案件工作区",
            ModuleStatus::Ready,
            "case_list/create/delete IPC",
        ),
        module(
            "image",
            "镜像加载",
            ModuleStatus::Ready,
            "raw/dd/img + E01/EWF + VMDK + QCOW2 + VDI + VHD/VHDX",
        ),
        module(
            "os_detect",
            "OS 识别",
            ModuleStatus::Ready,
            "Windows / Linux / macOS / ESXi / MikroTik / OpenWrt / 钉钉",
        ),
        module(
            "files",
            "文件树",
            ModuleStatus::Ready,
            "partition listing + Ext4 / NTFS / Btrfs / FAT / CDFS lazy directory tree",
        ),
        module(
            "search",
            "搜索",
            ModuleStatus::Ready,
            "ASCII/regex-like alternation scan over loaded image",
        ),
        module(
            "carve",
            "文件雕刻",
            ModuleStatus::Ready,
            "background signature scan + paged findings + DFXML recovery",
        ),
        module(
            "report",
            "报告",
            ModuleStatus::Planned,
            "report renderer not connected yet",
        ),
    ]
}

fn module(key: &str, label: &str, status: ModuleStatus, detail: &str) -> ModuleSummary {
    ModuleSummary {
        key: key.into(),
        label: label.into(),
        status,
        frontend: true,
        backend: true,
        detail: detail.into(),
    }
}
