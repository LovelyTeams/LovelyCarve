//! Evidence identity and single-flight regressions.

use super::{image_load_key, image_load_lock};
use crate::state::AppState;
use std::sync::Arc;

#[tokio::test]
async fn concurrent_image_loads_share_one_path_lock() {
    let state = AppState::new();
    let first = image_load_lock(&state, "D:/evidence/disk.E01").await;
    let second = image_load_lock(&state, "D:/evidence/disk.E01").await;

    assert!(Arc::ptr_eq(&first, &second));
}

#[cfg(target_os = "windows")]
#[test]
fn image_load_key_normalizes_windows_case_and_separators() {
    assert_eq!(
        image_load_key(r"D:\Evidence\Disk.E01"),
        image_load_key("d:/evidence/disk.e01")
    );
}
