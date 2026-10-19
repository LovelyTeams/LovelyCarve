#![cfg_attr(not(test), warn(clippy::unwrap_used, clippy::expect_used))]

pub(crate) mod app;
pub mod cli;
pub mod commands;
pub mod core;
pub mod host;
pub mod state;
pub mod task_manager;

/// Desktop entry point. The standalone CLI (`cli::run`) shares the command
/// registry below but never starts Tauri.
#[cfg(feature = "desktop")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let app = application_builder().build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(err) => {
            eprintln!("error while building lovelycarve: {err}");
            std::process::exit(1);
        }
    };
    app.run(crate::app::lifecycle::on_event);
}

/// The desktop builder and the backend command registry. The CLI build script
/// reads this `generate_handler!` list to generate its dispatch, so every
/// command registered here is also mapped (or explicitly excluded) in
/// `src/cli/commands.toml`.
#[cfg(feature = "desktop")]
fn application_builder() -> tauri::Builder<tauri::Wry> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state::AppState::new())
        .setup(crate::app::setup::setup)
        .invoke_handler(tauri::generate_handler![
            commands::analysis::bcp::bcp_catalog,
            commands::analysis::bcp::bcp_rows,
            commands::evidence::image::image_load,
            commands::evidence::image::image_info,
            commands::evidence::image::image_bad_regions,
            commands::evidence::image::image_unload,
            commands::evidence::image::image_list,
            commands::evidence::image::image_export_raw,
            commands::evidence::image::raid_auto_detect,
            commands::evidence::image::raid_manual_assemble,
            commands::evidence::image::physical_disks_list,
            commands::carving::carve::carve_formats,
            commands::carving::carve::carve_start,
            commands::carving::carve::carve_scan_status,
            commands::carving::carve::carve_scan_pause,
            commands::carving::carve::carve_scan_resume,
            commands::carving::carve::carve_latest_scan,
            commands::carving::carve_history::carve_scan_list,
            commands::carving::carve_history::carve_scan_remove,
            commands::carving::carve::carve_results_page,
            commands::carving::carve::carve_preview,
            commands::carving::carve_recovery::carve_recover,
            commands::carving::carve_filesystem::carve_filesystem_results,
            commands::filesystem::filesystem::fs_children,
            commands::filesystem::filesystem::fs_children_page,
            commands::filesystem::filesystem::fs_child_counts,
            commands::filesystem::filedb::filedb_volumes,
            commands::filesystem::filedb::filedb_query_files,
            commands::filesystem::filedb::filedb_resolve_meta_addr,
            commands::filesystem::filesystem::fs_subtree_empty_dirs,
            commands::filesystem::filesystem_flat::fs_flat_page,
            commands::filesystem::filesystem_flat::fs_flat_ext_counts,
            commands::search::global_search::global_search_start,
            commands::search::global_search::global_search_hits_page,
            commands::search::global_search::global_search_history_list,
            commands::search::global_search::global_search_history_delete,
            commands::search::global_search::global_search_export,
            commands::timeline::timeline::timeline_status,
            commands::timeline::timeline::timeline_start,
            commands::timeline::timeline::timeline_types,
            commands::timeline::timeline::timeline_count,
            commands::timeline::timeline::timeline_datas,
            commands::timeline::timeline::timeline_range,
            commands::timeline::timeline::timeline_export,
            commands::filesystem::filesystem::fs_node_locate,
            commands::filesystem::filesystem_source::fs_source_locate,
            commands::filesystem::filesystem_docker::fs_docker_prepare,
            commands::preview::filesystem_preview::fs_read,
            commands::preview::filesystem_preview::fs_read_range,
            commands::preview::filesystem_preview::fs_read_large,
            commands::preview::filesystem_preview::fs_media_temp,
            commands::preview::filesystem_preview::fs_media_release,
            commands::preview::filesystem_preview::fs_preview,
            commands::preview::filesystem_preview::fs_preview_text,
            commands::preview::filesystem_preview::fs_stat,
            commands::filesystem::filesystem_export::fs_export,
            commands::filesystem::filesystem_export::fs_export_dir,
            commands::filesystem::filesystem_export::fs_timeline_export,
            commands::filesystem::filesystem_streams::fs_streams,
            commands::filesystem::filesystem_streams::fs_stream_preview,
            commands::filesystem::filesystem_streams::fs_stream_export,
            commands::filesystem::filesystem_streams::fs_dir_stats,
            commands::filesystem::filesystem_analysis::fs_digest,
            commands::filesystem::filesystem_analysis::fs_identify_type,
            commands::filesystem::digests::fs_digest_load,
            commands::filesystem::digests::fs_digest_load_nodes,
            commands::filesystem::digests::fs_digest_persist,
            commands::filesystem::ftypes::fs_ftype_load,
            commands::filesystem::ftypes::fs_ftype_load_nodes,
            commands::filesystem::ftypes::fs_ftype_persist,
            commands::casework::tags::fs_tags_load,
            commands::casework::tags::fs_tags_load_nodes,
            commands::casework::tags::fs_tags_apply_changes,
            commands::casework::tags::fs_tags_save,
            commands::filesystem::filesystem_journal::fs_usn_journal,
            commands::filesystem::filesystem_journal::fs_usn_journal_page,
            commands::filesystem::filesystem_journal::fs_ext4_journal,
            commands::filesystem::filesystem_journal::fs_ext4_journal_blocks_page,
            commands::preview::filesystem_native_preview::fs_sqlite_preview,
            commands::preview::filesystem_native_preview::fs_sqlite_deleted_records,
            commands::preview::filesystem_native_preview::fs_sqlite_table_rows,
            commands::preview::filesystem_native_preview::fs_realm_preview,
            commands::preview::filesystem_native_preview::fs_realm_table_rows,
            commands::preview::filesystem_native_preview::fs_binary_preview,
            commands::preview::filesystem_native_preview::fs_gguf_preview,
            commands::preview::filesystem_native_preview::fs_zip_preview,
            commands::preview::filesystem_native_preview::fs_zip_member_read,
            commands::preview::filesystem_native_preview::fs_csv_preview,
            commands::preview::filesystem_native_preview::fs_vim_swap_preview,
            commands::system::mount::fs_mount_readonly,
            commands::system::mount::fs_unmount_readonly,
            commands::system::mount::fs_mounts,
            commands::system::mount::fs_share_mount,
            commands::system::mount::fs_unshare_mount,
            commands::system::mount::fs_suggest_mount_point,
            commands::system::mount::fs_available_mount_points,
            commands::system::mount::fs_create_mount_dir,
            commands::evidence::bitlocker::bitlocker_status,
            commands::evidence::bitlocker::bitlocker_metadata,
            commands::evidence::bitlocker::bitlocker_unlock,
            commands::evidence::encrypted_volume::encrypted_volume_unlock,
            commands::evidence::encrypted_volume::filevault_inspect,
            commands::evidence::ios_backup::ios_backup_unlock,
            commands::evidence::ios_backup::ios_backup_audit,
            commands::evidence::shadow_copy::shadow_copy_list,
            commands::evidence::shadow_copy::shadow_copy_open,
            commands::evidence::bitlocker::bitlocker_scan_memory,
            commands::evidence::bitlocker::bitlocker_unlock_with_vmk,
            commands::evidence::bitlocker::bitlocker_export_decrypted_image,
            commands::evidence::hash::image_hash_compute,
            commands::preview::qrcode::qr_scan,
            commands::preview::ocr::ocr_scan,
            commands::analysis::os_detect::os_detect,
            commands::analysis::autodetect::autodetect_evidence,
            commands::extensions::plugin::plugin_list,
            commands::extensions::plugin::plugin_restart,
            commands::extensions::plugin::plugin_analyze_file,
            commands::extensions::plugin::plugin_analysis_list,
            commands::analysis::analysis_results::analysis_result_tree,
            commands::analysis::analysis_results::analysis_records_page,
            commands::analysis::analysis_results::analysis_record_detail,
            commands::extensions::task_scheduler::sched_task_functions,
            commands::extensions::task_scheduler::sched_task_list,
            commands::extensions::task_scheduler::sched_task_create,
            commands::extensions::task_scheduler::sched_task_cancel,
            commands::extensions::task_scheduler::sched_task_pause,
            commands::extensions::task_scheduler::sched_task_start,
            commands::extensions::task_scheduler::sched_task_restart,
            commands::extensions::task_scheduler::sched_task_remove,
            commands::extensions::task_scheduler::sched_evidence_post_actions_set,
            commands::analysis::win_drive_letters::win_drive_letters,
            commands::system::system::task_cancel,
            commands::system::system::task_status,
            commands::system::system::task_list,
            commands::system::system::module_summary,
            commands::system::system::system_path_info,
            commands::system::system::reveal_path,
            commands::system::updater::release_update_check,
            commands::system::updater::release_update_install,
            commands::casework::case::case_create,
            commands::casework::case::case_open,
            commands::casework::case::case_close,
            commands::casework::case::case_active,
            commands::casework::case::case_list,
            commands::casework::case::case_remove,
            commands::casework::case::case_lock,
            commands::casework::case::case_unlock,
            commands::casework::case::case_rename,
            commands::casework::case::case_option_query,
            commands::casework::case::case_option_save,
            commands::casework::case::case_evidence_list,
            commands::casework::case::case_evidence_remove,
            commands::casework::case_snapshot::snapshot_build,
            commands::casework::case_snapshot::snapshot_list,
            commands::casework::case_snapshot::snapshot_drop,
        ])
}
