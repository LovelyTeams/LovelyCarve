//! 外置资源（模型等）定位。模型不再 `include_bytes!` 内嵌进二进制，而是作为文件随包发布：
//! 安装版经 `bundle.resources` 落到可执行文件旁的 `resources/`；便携版（7z）同样把 `resources/`
//! 放在 exe 旁；开发时从仓库 `src-tauri/resources/` 取。这里按「可执行文件目录」与「当前工作
//! 目录」各自向上若干级搜索每级下的 `resources/` 与 `src-tauri/resources/`，镜像 `core::platform::ffi`
//! 的 DLL 查找（`dll_search_path`），覆盖以上三种运行场景。

use std::path::{Path, PathBuf};

use crate::core::common::error::Failure;

/// 解析一个相对 `resources/` 的资源文件（如 `"ocr/pp-ocrv6_tiny_det.onnx"`）的磁盘路径。
/// 命中第一个存在的候选；都找不到则返回带查找说明的错误。
pub fn resource_path(rel: &str) -> anyhow::Result<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            push_roots(dir, &mut roots);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        push_roots(&cwd, &mut roots);
    }
    for root in &roots {
        let candidate = root.join(rel);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    anyhow::bail!(Failure::not_found(format!(
        "找不到外置资源 `resources/{rel}`：请确保 `resources/` 目录在可执行文件旁\
         （安装 / 便携版）或仓库 `src-tauri/` 下（开发）。已查找 {} 个候选位置。",
        roots.len()
    )))
}

/// 从 `start` 起向上 7 级，每级收集 `resources/` 与 `src-tauri/resources/` 作为候选根。
fn push_roots(start: &Path, roots: &mut Vec<PathBuf>) {
    let mut here: Option<&Path> = Some(start);
    for _ in 0..7 {
        let Some(dir) = here else { break };
        roots.push(dir.join("resources"));
        roots.push(dir.join("src-tauri").join("resources"));
        here = dir.parent();
    }
}
