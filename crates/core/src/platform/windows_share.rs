//! Windows SMB 共享（`net share`）封装：把一个已挂载的只读卷发布为局域网共享。
//!
//! GUI 和 lovelycarve-cli 的 `fs_share_mount` / `fs_unshare_mount` 命令共用这一份
//! net share 调用逻辑，保持共享名清洗与错误文案单一事实源。创建共享需要进程
//! 以管理员权限运行并启用 Windows 文件和打印机共享，失败细节由
//! [`net_share_failure`] 拼装成可操作的中文提示。

#![cfg(target_os = "windows")]

use std::process::Command;

use anyhow::{Context, Result};

/// 一次成功的 `net share` 创建结果。
#[derive(Debug, Clone)]
pub struct WindowsShare {
    pub share_name: String,
    pub local_path: String,
    pub unc_path: String,
}

/// 把 `local_path`（盘符根或文件夹挂载点）发布为只读局域网共享。
pub fn create_share(share_name: &str, local_path: &str) -> Result<WindowsShare> {
    let output = Command::new("net")
        .args([
            "share",
            &format!("{share_name}={local_path}"),
            "/CACHE:None",
            "/REMARK:LovelyCarve read-only mount",
        ])
        .output()
        .context("无法启动 net share")?;
    if !output.status.success() {
        return Err(net_share_failure("创建局域网共享失败", &output));
    }
    Ok(WindowsShare {
        share_name: share_name.to_string(),
        local_path: local_path.to_string(),
        unc_path: format!("\\\\{}\\{share_name}", local_host_name()),
    })
}

/// 删除一个由 [`create_share`] 创建的局域网共享。
pub fn delete_share(share_name: &str) -> Result<()> {
    let output = Command::new("net")
        .args(["share", share_name, "/delete", "/y"])
        .output()
        .context("无法启动 net share /delete")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(net_share_failure("取消局域网共享失败", &output))
    }
}

/// 把 net share 的退出输出转成可操作的失败原因。共享名清洗规则与 GUI 一致：
/// `input` 为空时回退 `LC_<mount_id 前 8 字符>`，Windows 保留字符替换为 `_`，
/// 去首尾空白与点号，最长 80 字符。
pub fn sanitized_share_name(input: Option<&str>, mount_id: &str) -> String {
    let fallback = format!("LC_{}", mount_id.chars().take(8).collect::<String>());
    let raw = input.unwrap_or("").trim();
    let source = if raw.is_empty() {
        fallback.as_str()
    } else {
        raw
    };
    let mut name = source
        .chars()
        .map(|ch| {
            if ch.is_control()
                || matches!(
                    ch,
                    '"' | '/'
                        | '\\'
                        | '['
                        | ']'
                        | ':'
                        | '|'
                        | '<'
                        | '>'
                        | '+'
                        | '='
                        | ';'
                        | ','
                        | '?'
                        | '*'
                )
            {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_string();
    if name.is_empty() {
        name = fallback;
    }
    if name.chars().count() > 80 {
        name = name.chars().take(80).collect();
    }
    name
}

fn local_host_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}

fn net_share_failure(operation: &str, output: &std::process::Output) -> anyhow::Error {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = format!("{stdout}\n{stderr}").trim().to_string();
    let message = if detail.is_empty() {
        format!(
            "{operation}：net share 退出码 {}。请确认以管理员权限运行，并启用 Windows 文件和打印机共享。",
            output.status
        )
    } else {
        format!("{operation}：{detail}。请确认以管理员权限运行，并启用 Windows 文件和打印机共享。")
    };
    anyhow::anyhow!(message)
}

#[cfg(test)]
mod tests {
    use super::sanitized_share_name;

    #[test]
    fn share_name_sanitizes_windows_reserved_chars() {
        assert_eq!(
            sanitized_share_name(Some("  分区:0 / FAT32?  "), "abcdef12-3456"),
            "分区_0 _ FAT32_"
        );
        assert_eq!(
            sanitized_share_name(Some("..."), "abcdef12-3456"),
            "LC_abcdef12"
        );
        assert_eq!(sanitized_share_name(None, "abcdef12-3456"), "LC_abcdef12");
    }
}
