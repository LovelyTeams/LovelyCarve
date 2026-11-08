//! 本机最近案件登记。
//!
//! 登记已创建/打开过的案件供案件列表枚举。登记簿是应用数据目录下的
//! `case_index.json`,不属于任何案件;每个条目记录案件目录、编号、名称与
//! 最近打开时间。写入走临时文件 + 原子改名。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 登记簿文件名(位于应用数据目录)。
pub const REGISTRY_FILE: &str = "case_index.json";

/// 一条案件登记。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryCase {
    /// 案件目录。
    pub dir: String,
    /// 案件编号(台账档案)。
    pub case_number: String,
    /// 案件名称(台账档案)。
    pub name: String,
    /// 首次登记时间。
    pub created_at: String,
    /// 最近打开时间(列表按此倒序)。
    pub opened_at: String,
}

pub fn registry_path(app_data: &Path) -> PathBuf {
    app_data.join(REGISTRY_FILE)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    schema_version: u32,
    cases: Vec<RegistryCase>,
}

const REGISTRY_SCHEMA: u32 = 1;

fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn load(app_data: &Path) -> RegistryFile {
    match std::fs::read(registry_path(app_data)) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|err| {
            tracing::warn!("case registry: unreadable, starting fresh: {err}");
            RegistryFile::default()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => RegistryFile {
            schema_version: REGISTRY_SCHEMA,
            cases: Vec::new(),
        },
        Err(err) => {
            tracing::warn!("case registry: read failed, starting fresh: {err}");
            RegistryFile::default()
        }
    }
}

fn store(app_data: &Path, registry: &RegistryFile) -> anyhow::Result<()> {
    std::fs::create_dir_all(app_data)?;
    let path = registry_path(app_data);
    let new_path = app_data.join(format!("{REGISTRY_FILE}.new"));
    let json = serde_json::to_string_pretty(registry)?;
    std::fs::write(&new_path, json.as_bytes())?;
    std::fs::rename(&new_path, &path)?;
    Ok(())
}

/// 列出登记的案件,最近打开在前。
pub fn list(app_data: &Path) -> Vec<RegistryCase> {
    let mut cases = load(app_data).cases;
    cases.sort_by(|a, b| b.opened_at.cmp(&a.opened_at));
    cases
}

/// 登记(或刷新)一个案件目录。编号/名称取台账当前值;打开时间刷新。
pub fn record(app_data: &Path, dir: &str, case_number: &str, name: &str) {
    let mut registry = load(app_data);
    let now = now_rfc3339();
    match registry.cases.iter_mut().find(|entry| entry.dir == dir) {
        Some(entry) => {
            entry.case_number = case_number.to_string();
            entry.name = name.to_string();
            entry.opened_at = now;
        }
        None => registry.cases.push(RegistryCase {
            dir: dir.to_string(),
            case_number: case_number.to_string(),
            name: name.to_string(),
            created_at: now.clone(),
            opened_at: now,
        }),
    }
    if let Err(err) = store(app_data, &registry) {
        tracing::warn!("case registry: record {} failed: {err}", dir);
    }
}

/// 从登记簿移除一个案件目录(案件删除级联的一部分)。
pub fn remove(app_data: &Path, dir: &str) {
    let mut registry = load(app_data);
    let before = registry.cases.len();
    registry.cases.retain(|entry| entry.dir != dir);
    if registry.cases.len() == before {
        return;
    }
    if let Err(err) = store(app_data, &registry) {
        tracing::warn!("case registry: remove {} failed: {err}", dir);
    }
}

/// 案件改名后同步登记簿。
pub fn rename(app_data: &Path, dir: &str, name: &str) {
    let mut registry = load(app_data);
    if let Some(entry) = registry.cases.iter_mut().find(|entry| entry.dir == dir) {
        entry.name = name.to_string();
        if let Err(err) = store(app_data, &registry) {
            tracing::warn!("case registry: rename {} failed: {err}", dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_lists_renames_and_removes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app_data = dir.path();

        record(app_data, "/c/a", "AW-1", "案件一");
        record(app_data, "/c/b", "AW-2", "案件二");
        record(app_data, "/c/a", "AW-1", "案件一改");

        let cases = list(app_data);
        assert_eq!(cases.len(), 2);
        // Most recently opened/refreshed first.
        assert_eq!(cases[0].dir, "/c/a");
        assert_eq!(cases[0].name, "案件一改");
        assert_eq!(cases[1].case_number, "AW-2");

        rename(app_data, "/c/b", "案件二改");
        assert_eq!(list(app_data)[1].name, "案件二改");

        remove(app_data, "/c/b");
        let cases = list(app_data);
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].dir, "/c/a");
        // Removing an unknown dir is a no-op.
        remove(app_data, "/c/missing");
        assert_eq!(list(app_data).len(), 1);
    }
}
