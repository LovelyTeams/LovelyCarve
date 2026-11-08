//! Exporting a file or a directory from evidence to the host. Files go to a
//! chosen path; directories become a folder tree or one ZIP archive, with
//! progress in the task center. Every name from evidence is sanitized.
use std::{collections::HashSet, path::PathBuf};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants as _},
        h_flex, v_flex, WindowExt as _,
    },
    App, ParentElement as _, Styled as _, Window,
};
use lovelycarve_backend::{
    commands::filesystem::filesystem_export::{self as export_commands, ExportDirMode},
    core::{
        evidence::image::ReadFaultReport,
        filesystem::tree::fs_tree::{FsNode, NodeKind},
    },
    host::State,
};

use crate::{backend::Backend, format, ui};

fn default_directory() -> PathBuf {
    dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn fault_note(report: &ReadFaultReport) -> String {
    if report.read_faults_detected {
        format!(
            " · 含 {} 补零字节（{} 个坏区）",
            format::bytes(report.synthetic_bytes),
            report.bad_regions.len()
        )
    } else {
        String::new()
    }
}

/// `name`, or `stem (n).ext` when the folder already has it (case-insensitive,
/// as Windows compares names).
fn unique_name(name: String, used: &mut HashSet<String>) -> String {
    if used.insert(name.to_lowercase()) {
        return name;
    }
    let (stem, extension) = match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => (stem.to_owned(), format!(".{extension}")),
        _ => (name.clone(), String::new()),
    };
    let mut index = 2;
    loop {
        let candidate = format!("{stem} ({index}){extension}");
        if used.insert(candidate.to_lowercase()) {
            return candidate;
        }
        index += 1;
    }
}

/// Export several checked nodes into one chosen folder: files keep their
/// sanitized names (made unique there) and directories become folder trees.
pub fn export_many(image_id: String, nodes: Vec<FsNode>, window: &mut Window, cx: &mut App) {
    let prompt = ui::prompt_directory("选择导出目标目录", cx);
    let backend = Backend::global(cx);
    window
        .spawn(cx, async move |cx| {
            let Some(directory) = prompt.await else {
                return;
            };
            let mut used: HashSet<String> = match std::fs::read_dir(&directory) {
                Ok(entries) => entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().to_lowercase())
                    .collect(),
                Err(error) => {
                    cx.update(|window, cx| {
                        ui::notify_error(window, cx, "导出失败", error.to_string())
                    })
                    .ok();
                    return;
                }
            };
            let total = nodes.len();
            cx.update(|window, cx| {
                ui::notify_success(
                    window,
                    cx,
                    "开始导出",
                    format!("{total} 项 → {}", directory.display()),
                )
            })
            .ok();
            let (mut exported, mut bytes, mut synthetic, mut failed) = (0usize, 0u64, 0u64, 0usize);
            let mut last_error = None;
            for node in nodes {
                let name = unique_name(
                    lovelycarve_backend::embed::sanitize_export_name(&node.name),
                    &mut used,
                );
                let target = directory.join(name).display().to_string();
                let (image_id, node_id) = (image_id.clone(), node.id.clone());
                let outcome = if node.kind == NodeKind::Dir {
                    backend
                        .run(move |host, state| async move {
                            export_commands::fs_export_dir(
                                host,
                                State::new(&state),
                                image_id,
                                node_id,
                                target,
                                uuid::Uuid::new_v4().to_string(),
                                ExportDirMode::Tree,
                            )
                            .await
                        })
                        .await
                        .map(|summary| (summary.bytes_written, summary.read_faults.synthetic_bytes))
                } else {
                    backend
                        .run(move |_, state| async move {
                            export_commands::fs_export(
                                State::new(&state),
                                image_id,
                                node_id,
                                target,
                            )
                            .await
                        })
                        .await
                        .map(|result| (result.bytes_written, result.read_faults.synthetic_bytes))
                };
                match outcome {
                    Ok((written, filled)) => {
                        exported += 1;
                        bytes += written;
                        synthetic += filled;
                    }
                    Err(error) => {
                        failed += 1;
                        last_error = Some(error.message);
                    }
                }
            }
            cx.update(|window, cx| {
                let mut parts = vec![format!("{exported} 项"), format::bytes(bytes)];
                if synthetic > 0 {
                    parts.push(format!("含 {} 补零字节", format::bytes(synthetic)));
                }
                let summary = format!("{} · {}", directory.display(), parts.join(" · "));
                match last_error {
                    Some(error) => ui::notify_error(
                        window,
                        cx,
                        "导出完成（部分失败）",
                        format!("{summary} · 失败 {failed}（{error}）"),
                    ),
                    None => ui::notify_success(window, cx, "导出完成", summary),
                }
            })
            .ok();
        })
        .detach();
}

/// Export `node` of evidence `image_id`, asking where to put it.
pub fn export_node(image_id: String, node: FsNode, window: &mut Window, cx: &mut App) {
    match node.kind {
        NodeKind::Dir => choose_directory_mode(image_id, node, window, cx),
        NodeKind::File | NodeKind::Symlink | NodeKind::Unallocated => {
            export_file(image_id, node, window, cx)
        }
    }
}

fn export_file(image_id: String, node: FsNode, window: &mut Window, cx: &mut App) {
    let name = lovelycarve_backend::embed::sanitize_export_name(&node.name);
    let destination = cx.prompt_for_new_path(&default_directory(), Some(&name));
    let backend = Backend::global(cx);
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(destination))) = destination.await else {
                return;
            };
            let target = destination.display().to_string();
            let node_id = node.id.clone();
            let exported = backend
                .run(move |_, state| async move {
                    export_commands::fs_export(State::new(&state), image_id, node_id, target).await
                })
                .await;
            cx.update(|window, cx| match exported {
                Ok(result) => ui::notify_success(
                    window,
                    cx,
                    if result.read_faults.read_faults_detected {
                        "导出成功（含补零）"
                    } else {
                        "导出成功"
                    },
                    format!(
                        "{}（{}）{}",
                        destination.display(),
                        format::bytes(result.bytes_written),
                        fault_note(&result.read_faults)
                    ),
                ),
                Err(error) => ui::notify_error(window, cx, "导出失败", error.message),
            })
            .ok();
        })
        .detach();
}

fn choose_directory_mode(image_id: String, node: FsNode, window: &mut Window, cx: &mut App) {
    let description = format!(
        "「{}」可以按原目录结构导出为文件夹，或打包为一个 ZIP 文件。",
        node.name
    );
    window.open_dialog(cx, move |dialog, _, cx| {
        let tree_image = image_id.clone();
        let tree_node = node.clone();
        let zip_image = image_id.clone();
        let zip_node = node.clone();
        dialog
            .centered()
            .title(ui::dialog_heading(
                gpui_kit::assets::IconName::Download,
                "导出目录",
                description.clone(),
                cx,
            ))
            .w(gpui_kit::px(440.))
            .child(
                v_flex().gap_4().child(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("export-cancel")
                                .ghost()
                                .label("取消")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(Button::new("export-zip").label("导出为 ZIP…").on_click(
                            move |_, window, cx| {
                                window.close_dialog(cx);
                                export_directory(
                                    zip_image.clone(),
                                    zip_node.clone(),
                                    ExportDirMode::Zip,
                                    window,
                                    cx,
                                );
                            },
                        ))
                        .child(
                            Button::new("export-tree")
                                .primary()
                                .label("导出为文件夹…")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    export_directory(
                                        tree_image.clone(),
                                        tree_node.clone(),
                                        ExportDirMode::Tree,
                                        window,
                                        cx,
                                    );
                                }),
                        ),
                ),
            )
    });
}

fn export_directory(
    image_id: String,
    node: FsNode,
    mode: ExportDirMode,
    window: &mut Window,
    cx: &mut App,
) {
    let name = lovelycarve_backend::embed::sanitize_export_name(&node.name);
    let backend = Backend::global(cx);
    let tree_prompt =
        matches!(mode, ExportDirMode::Tree).then(|| ui::prompt_directory("选择导出目标目录", cx));
    let zip_prompt = matches!(mode, ExportDirMode::Zip)
        .then(|| cx.prompt_for_new_path(&default_directory(), Some(&format!("{name}.zip"))));
    window
        .spawn(cx, async move |cx| {
            let target = match (tree_prompt, zip_prompt) {
                (Some(prompt), _) => match prompt.await {
                    Some(directory) => directory.join(&name),
                    None => return,
                },
                (None, Some(prompt)) => match prompt.await {
                    Ok(Ok(Some(path)))
                        if path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("zip")) =>
                    {
                        path
                    }
                    Ok(Ok(Some(path))) => path.with_extension("zip"),
                    _ => return,
                },
                (None, None) => return,
            };
            let destination = target.display().to_string();
            let task_id = uuid::Uuid::new_v4().to_string();
            let node_id = node.id.clone();
            cx.update(|window, cx| {
                ui::notify_success(
                    window,
                    cx,
                    "开始导出",
                    format!("{destination} · 进度见任务中心"),
                );
            })
            .ok();
            let exported = backend
                .run(move |host, state| async move {
                    export_commands::fs_export_dir(
                        host,
                        State::new(&state),
                        image_id,
                        node_id,
                        destination,
                        task_id,
                        mode,
                    )
                    .await
                })
                .await;
            cx.update(|window, cx| match exported {
                Ok(summary) => {
                    let mut parts = vec![
                        format!("{} 个文件", format::count(summary.file_count)),
                        format!("{} 个目录", format::count(summary.dir_count)),
                        format::bytes(summary.bytes_written),
                    ];
                    if summary.skipped_count > 0 {
                        parts.push(format!("跳过 {}", summary.skipped_count));
                    }
                    if summary.truncated {
                        parts.push("已截断".to_owned());
                    }
                    let title = if summary.cancelled {
                        "导出已取消"
                    } else if summary.read_faults.read_faults_detected {
                        "导出完成（含补零）"
                    } else {
                        "导出完成"
                    };
                    ui::notify_success(
                        window,
                        cx,
                        title,
                        format!(
                            "{} · {}{}",
                            target.display(),
                            parts.join(" · "),
                            fault_note(&summary.read_faults)
                        ),
                    );
                }
                Err(error) => ui::notify_error(window, cx, "导出失败", error.message),
            })
            .ok();
        })
        .detach();
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::unique_name;

    #[test]
    fn export_names_stay_unique_per_folder() {
        let mut used = HashSet::from(["report.txt".to_owned()]);
        assert_eq!(
            unique_name("Report.TXT".to_owned(), &mut used),
            "Report (2).TXT"
        );
        assert_eq!(
            unique_name("report.txt".to_owned(), &mut used),
            "report (3).txt"
        );
        assert_eq!(unique_name("data".to_owned(), &mut used), "data");
        assert_eq!(unique_name("data".to_owned(), &mut used), "data (2)");
        assert_eq!(unique_name(".bashrc".to_owned(), &mut used), ".bashrc");
    }
}
