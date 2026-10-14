//! Manual directory scans publish catalog snapshots without replacing live instances.
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::{PluginDefinition, PluginError, PluginManifest, Result};

pub const PLUGIN_CATALOG_CHANGED: &str = "plugin-catalog-changed";

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginScanEntry {
    pub directory: String,
    pub plugin_id: Option<String>,
    pub version: Option<String>,
    pub status: PluginScanStatus,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PluginScanStatus {
    Added,
    Unchanged,
    Removed,
    UpdatePending,
    Error,
    Duplicate,
    MissingRoot,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginScanReport {
    pub generation: u64,
    pub roots: Vec<String>,
    pub entries: Vec<PluginScanEntry>,
}

pub struct CatalogSnapshot {
    pub definitions: Arc<[PluginDefinition]>,
    pub report: PluginScanReport,
}

#[derive(Clone, Debug, PartialEq)]
struct FileStamp {
    length: u64,
    modified: SystemTime,
}

impl FileStamp {
    fn read(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path)?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(PluginError::Manifest(format!(
                "{} 不是非空文件",
                path.display()
            )));
        }
        Ok(Self {
            length: metadata.len(),
            modified: metadata.modified()?,
        })
    }
}

#[derive(Clone)]
struct Registration {
    root: PathBuf,
    definition: PluginDefinition,
    manifest_stamp: FileStamp,
    library_stamp: FileStamp,
}

/// Serializes disk inspection separately from the short snapshot publication lock.
/// History retains removed IDs so rediscovery cannot hot-swap a still-live library.
pub(super) struct PluginCatalog {
    roots: Vec<PathBuf>,
    history: Mutex<HashMap<String, Registration>>,
    snapshot: RwLock<Arc<CatalogSnapshot>>,
    closing: CancellationToken,
}

impl PluginCatalog {
    pub fn new(roots: &[PathBuf]) -> Self {
        let mut unique = HashSet::new();
        let roots = roots
            .iter()
            .map(|root| fs::canonicalize(root).unwrap_or_else(|_| root.clone()))
            .filter(|root| unique.insert(root.clone()))
            .collect();
        Self {
            roots,
            history: Mutex::default(),
            snapshot: RwLock::new(Arc::new(CatalogSnapshot {
                definitions: Arc::from([]),
                report: PluginScanReport::default(),
            })),
            closing: CancellationToken::new(),
        }
    }

    pub fn snapshot(&self) -> Arc<CatalogSnapshot> {
        Arc::clone(&self.snapshot.read())
    }

    /// Never load replacement bytes under the old definition or permissions.
    pub fn validate_for_load(&self, definition: &PluginDefinition) -> Result<()> {
        self.check_open()?;
        let registered = self
            .history
            .lock()
            .get(&definition.manifest.id)
            .cloned()
            .ok_or_else(|| PluginError::Manifest("插件未登记".to_owned()))?;
        let current = inspect(&registered.root, &definition.directory.join("plugin.toml"))?;
        if !same_installation(&registered, &current) {
            return Err(PluginError::Manifest(
                "插件文件已更新，请重启应用后使用".to_owned(),
            ));
        }
        Ok(())
    }

    /// Blocking: call on a blocking worker. Cancellation prevents late publication.
    pub fn rescan(&self) -> Result<PluginScanReport> {
        let mut history = self.history.lock();
        let previous = self.snapshot();
        let mut report = PluginScanReport {
            generation: previous
                .report
                .generation
                .checked_add(1)
                .ok_or_else(|| PluginError::Manifest("插件清单代际已达到上限".to_owned()))?,
            roots: self
                .roots
                .iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect(),
            entries: Vec::new(),
        };
        let mut selected = HashMap::<String, Registration>::new();
        for root in &self.roots {
            self.check_open()?;
            let paths = match manifest_paths(root) {
                Ok(Some(paths)) => paths,
                Ok(None) => {
                    report.entries.push(entry(
                        root,
                        None,
                        PluginScanStatus::MissingRoot,
                        "目录尚不存在",
                    ));
                    continue;
                }
                Err(error) => {
                    report.entries.push(entry(
                        root,
                        None,
                        PluginScanStatus::Error,
                        &error.to_string(),
                    ));
                    // A failed root is not proof that its installed plugins were removed.
                    for definition in previous.definitions.iter() {
                        if let Some(registered) = history
                            .get(&definition.manifest.id)
                            .filter(|registered| registered.root == *root)
                        {
                            selected
                                .entry(definition.manifest.id.clone())
                                .or_insert_with(|| registered.clone());
                        }
                    }
                    continue;
                }
            };
            for path in paths {
                self.check_open()?;
                let directory = path
                    .parent()
                    .ok_or_else(|| PluginError::Manifest("清单缺少父目录".to_owned()))?;
                let candidate = match inspect(root, &path) {
                    Ok(candidate) => candidate,
                    Err(error) => {
                        let registered = previous
                            .definitions
                            .iter()
                            .find(|definition| definition.directory == directory)
                            .and_then(|definition| history.get(&definition.manifest.id));
                        report.entries.push(entry(
                            directory,
                            registered.map(|registered| &registered.definition.manifest),
                            PluginScanStatus::Error,
                            &error.to_string(),
                        ));
                        if let Some(registered) = registered {
                            selected
                                .entry(registered.definition.manifest.id.clone())
                                .or_insert_with(|| registered.clone());
                        }
                        continue;
                    }
                };
                let id = candidate.definition.manifest.id.clone();
                if let Some(winner) = selected.get(&id) {
                    report.entries.push(entry(
                        directory,
                        Some(&candidate.definition.manifest),
                        PluginScanStatus::Duplicate,
                        &format!("同 ID 插件已采用 {}", winner.definition.directory.display()),
                    ));
                    continue;
                }
                let (registered, status, message) = match history.get(&id) {
                    Some(registered) if same_installation(registered, &candidate) => (
                        registered.clone(),
                        PluginScanStatus::Unchanged,
                        "插件未变化",
                    ),
                    Some(registered) => (
                        registered.clone(),
                        PluginScanStatus::UpdatePending,
                        "插件更新待重启生效，保留当前定义",
                    ),
                    None => (
                        candidate.clone(),
                        PluginScanStatus::Added,
                        "已发现插件，首次使用时加载",
                    ),
                };
                report.entries.push(entry(
                    directory,
                    Some(&candidate.definition.manifest),
                    status,
                    message,
                ));
                history
                    .entry(id.clone())
                    .or_insert_with(|| registered.clone());
                selected.insert(id, registered);
            }
        }
        for definition in previous.definitions.iter() {
            if !selected.contains_key(&definition.manifest.id) {
                report.entries.push(entry(
                    &definition.directory,
                    Some(&definition.manifest),
                    PluginScanStatus::Removed,
                    "插件已移除，不再接受新任务；现有任务和会话继续保留",
                ));
            }
        }
        let mut definitions = selected
            .into_values()
            .map(|registered| registered.definition)
            .collect::<Vec<_>>();
        definitions.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
        self.check_open()?;
        *self.snapshot.write() = Arc::new(CatalogSnapshot {
            definitions: definitions.into(),
            report: report.clone(),
        });
        Ok(report)
    }

    fn check_open(&self) -> Result<()> {
        if self.closing.is_cancelled() {
            Err(PluginError::Manifest("插件运行时正在关闭".to_owned()))
        } else {
            Ok(())
        }
    }

    /// Blocking: stops new scans and waits for any existing scan to finish.
    pub fn close(&self) {
        self.closing.cancel();
        drop(self.history.lock());
    }
}

fn same_installation(left: &Registration, right: &Registration) -> bool {
    left.definition.directory == right.definition.directory
        && left.definition.manifest == right.definition.manifest
        && left.manifest_stamp == right.manifest_stamp
        && left.library_stamp == right.library_stamp
}

fn entry(
    directory: &Path,
    manifest: Option<&PluginManifest>,
    status: PluginScanStatus,
    message: &str,
) -> PluginScanEntry {
    PluginScanEntry {
        directory: directory.to_string_lossy().into_owned(),
        plugin_id: manifest.map(|manifest| manifest.id.clone()),
        version: manifest.map(|manifest| manifest.version.clone()),
        status,
        message: message.to_owned(),
    }
}

fn inspect(root: &Path, path: &Path) -> Result<Registration> {
    let before = FileStamp::read(path)?;
    let manifest = PluginManifest::load(path)?;
    let directory = path
        .parent()
        .ok_or_else(|| PluginError::Manifest("清单缺少父目录".to_owned()))?;
    let library = manifest.library_path(directory);
    let library_before = FileStamp::read(&library).map_err(|error| {
        PluginError::Manifest(format!("动态库 {} 不可用: {error}", library.display()))
    })?;
    let manifest_stamp = FileStamp::read(path)?;
    let library_stamp = FileStamp::read(&library)?;
    if before != manifest_stamp || library_before != library_stamp {
        return Err(PluginError::Manifest(
            "文件仍在变化，请复制完成后重试".to_owned(),
        ));
    }
    Ok(Registration {
        root: root.to_owned(),
        definition: PluginDefinition::new(directory.to_owned(), manifest),
        manifest_stamp,
        library_stamp,
    })
}

fn manifest_paths(root: &Path) -> Result<Option<Vec<PathBuf>>> {
    match fs::metadata(root) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    }
    let direct = root.join("plugin.toml");
    match fs::metadata(&direct) {
        Ok(metadata) if metadata.is_file() => return Ok(Some(vec![direct])),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let candidate = entry.path().join("plugin.toml");
            match fs::metadata(&candidate) {
                Ok(_) => paths.push(candidate),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
    paths.sort();
    Ok(Some(paths))
}

#[cfg(test)]
mod tests;
