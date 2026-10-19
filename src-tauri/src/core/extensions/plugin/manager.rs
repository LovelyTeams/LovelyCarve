//! Discovered packages and their running instances.
//!
//! Each package with filesystem parsers has one shared instance for
//! browsing (frozen `fs.*`); every analyzer run gets a worker of its own
//! ([`super::TaskWorker`]). Packages are trusted on discovery. The package
//! list can be re-read at runtime (after installing or removing a
//! package); running instances keep the definition they started with.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use parking_lot::RwLock;
use rmpv::Value;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use super::{
    package::MANIFEST_FILE, PluginConnection, PluginDefinition, PluginError, PluginLogStore,
    RemoteVolume, Result, SourceHandle, SourceRegistry,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PluginState {
    Configured,
    Starting,
    Running,
    Crashed(String),
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct ProbeMatch {
    pub plugin_id: String,
    pub filesystem: String,
    pub confidence: f64,
    pub label: Option<String>,
    pub priority: i32,
}

#[derive(Deserialize)]
struct ProbeResponse {
    filesystem: String,
    confidence: f64,
    #[serde(default)]
    label: Option<String>,
}

#[derive(Deserialize)]
struct OpenResponse {
    session_id: String,
    root_node: String,
}

/// A package that could not be loaded, shown so the user can fix it.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RejectedPackage {
    pub directory: String,
    pub error: String,
}

pub struct PluginManager {
    roots: Vec<PathBuf>,
    install_root: Option<PathBuf>,
    definitions: RwLock<Arc<Vec<PluginDefinition>>>,
    rejected: RwLock<Vec<RejectedPackage>>,
    sources: SourceRegistry,
    clients: Mutex<HashMap<String, PluginConnection>>,
    states: RwLock<HashMap<String, PluginState>>,
    log_store: RwLock<Option<Arc<PluginLogStore>>>,
    /// Entry points served in this process instead of `lc-plugin-host`.
    #[cfg(test)]
    loopback: RwLock<HashMap<String, lovelycarve_plugin::abi::EntryFn>>,
}

impl PluginManager {
    /// Discovers the packages under `roots`: each root is a package
    /// directory itself or holds package directories. On duplicate ids the
    /// first root wins. `install_root` (also scanned) is where installed
    /// packages go; only those can be uninstalled.
    pub fn discover(roots: &[PathBuf], install_root: Option<PathBuf>) -> Result<Self> {
        let mut all_roots: Vec<PathBuf> = install_root.iter().cloned().collect();
        for root in roots {
            if !all_roots.contains(root) {
                all_roots.push(root.clone());
            }
        }
        let manager = Self {
            roots: all_roots,
            install_root,
            definitions: RwLock::default(),
            rejected: RwLock::default(),
            sources: SourceRegistry::default(),
            clients: Mutex::new(HashMap::new()),
            states: RwLock::default(),
            log_store: RwLock::new(None),
            #[cfg(test)]
            loopback: RwLock::default(),
        };
        manager.rediscover();
        Ok(manager)
    }

    /// Serves `plugin_id`'s workers from `entry` over in-memory LPP pipes
    /// (tests): the same frames, client and host services as a process.
    #[cfg(test)]
    pub(crate) fn serve_in_process(
        &self,
        plugin_id: &str,
        entry: lovelycarve_plugin::abi::EntryFn,
    ) {
        self.loopback.write().insert(plugin_id.to_owned(), entry);
    }

    #[cfg(test)]
    pub(super) fn loopback_entry(
        &self,
        plugin_id: &str,
    ) -> Option<lovelycarve_plugin::abi::EntryFn> {
        self.loopback.read().get(plugin_id).copied()
    }

    /// Re-reads every package root. Instances already running keep going.
    pub fn rediscover(&self) {
        let mut definitions = Vec::new();
        let mut rejected = Vec::new();
        let mut plugin_ids = HashSet::new();
        for root in &self.roots {
            let directories = match package_directories(root) {
                Ok(directories) => directories,
                Err(error) => {
                    tracing::warn!(root = %root.display(), "plugin root cannot be inspected: {error}");
                    continue;
                }
            };
            for directory in directories {
                let removable = self
                    .install_root
                    .as_ref()
                    .is_some_and(|install_root| directory.starts_with(install_root));
                match PluginDefinition::load(&directory, removable) {
                    Ok(definition) => {
                        if plugin_ids.insert(definition.manifest.id.clone()) {
                            definitions.push(definition);
                        } else {
                            tracing::warn!(plugin_id = %definition.manifest.id, directory = %directory.display(), "duplicate plugin ignored");
                        }
                    }
                    Err(error) => {
                        tracing::warn!(directory = %directory.display(), "plugin ignored: {error}");
                        rejected.push(RejectedPackage {
                            directory: directory.to_string_lossy().into_owned(),
                            error: error.to_string(),
                        });
                    }
                }
            }
        }
        definitions.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
        {
            let mut states = self.states.write();
            states.retain(|id, _| plugin_ids.contains(id));
            for definition in &definitions {
                states
                    .entry(definition.manifest.id.clone())
                    .or_insert(PluginState::Configured);
            }
        }
        *self.definitions.write() = Arc::new(definitions);
        *self.rejected.write() = rejected;
    }

    /// The packages as last discovered.
    pub fn definitions(&self) -> Arc<Vec<PluginDefinition>> {
        Arc::clone(&self.definitions.read())
    }

    /// Packages that failed to load.
    pub fn rejected(&self) -> Vec<RejectedPackage> {
        self.rejected.read().clone()
    }

    pub fn definition(&self, plugin_id: &str) -> Result<PluginDefinition> {
        self.definitions
            .read()
            .iter()
            .find(|definition| definition.manifest.id == plugin_id)
            .cloned()
            .ok_or_else(|| PluginError::Manifest(format!("plugin {plugin_id} is not installed")))
    }

    pub fn install_root(&self) -> Option<&Path> {
        self.install_root.as_deref()
    }

    pub fn sources(&self) -> &SourceRegistry {
        &self.sources
    }

    /// State of the package's shared instance (workers report through their
    /// tasks).
    pub fn state(&self, plugin_id: &str) -> Option<PluginState> {
        self.states.read().get(plugin_id).cloned()
    }

    /// Appends every plugin's log lines to its own file under the store's
    /// root. Without a store, plugin logs only reach `tracing`.
    pub fn set_log_store(&self, store: Arc<PluginLogStore>) {
        *self.log_store.write() = Some(store);
    }

    pub(super) fn log_store(&self) -> Option<Arc<PluginLogStore>> {
        self.log_store.read().clone()
    }

    /// Stops the package's shared instance; the next browse starts a fresh
    /// one. Task workers are separate instances and keep running.
    pub async fn restart(&self, plugin_id: &str) -> Result<()> {
        self.definition(plugin_id)?;
        self.stop_shared(plugin_id).await;
        Ok(())
    }

    /// Stops the shared instance of `plugin_id`, if any.
    pub(super) async fn stop_shared(&self, plugin_id: &str) {
        if let Some(client) = self.clients.lock().await.remove(plugin_id) {
            if let Err(error) = client.shutdown().await {
                tracing::debug!(%plugin_id, "plugin shutdown failed: {error}");
            }
        }
        self.states
            .write()
            .insert(plugin_id.to_owned(), PluginState::Configured);
    }

    pub async fn shutdown(&self) {
        let clients = std::mem::take(&mut *self.clients.lock().await);
        for (plugin_id, client) in clients {
            if let Err(error) = client.shutdown().await {
                tracing::warn!(%plugin_id, "plugin shutdown failed: {error}");
            }
            self.states
                .write()
                .insert(plugin_id, PluginState::Configured);
        }
        let store = self.log_store.write().take();
        if let Some(store) = store {
            if let Err(error) = tokio::task::spawn_blocking(move || store.shutdown()).await {
                tracing::warn!("plugin log shutdown failed: {error}");
            }
        }
    }

    pub async fn probe(&self, source: &SourceHandle) -> Vec<ProbeMatch> {
        let definitions = self.definitions();
        let mut matches = Vec::new();
        for definition in definitions
            .iter()
            .filter(|definition| !definition.manifest.filesystems.is_empty())
        {
            match self.probe_one(definition, source).await {
                Ok(Some(probe_match)) => matches.push(probe_match),
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(plugin_id = %definition.manifest.id, "plugin probe failed: {error}");
                    self.states.write().insert(
                        definition.manifest.id.clone(),
                        PluginState::Crashed(error.to_string()),
                    );
                    self.clients.lock().await.remove(&definition.manifest.id);
                }
            }
        }
        matches.sort_by(|left, right| {
            right
                .confidence
                .partial_cmp(&left.confidence)
                .unwrap_or(Ordering::Equal)
                .then_with(|| right.priority.cmp(&left.priority))
                .then_with(|| left.plugin_id.cmp(&right.plugin_id))
        });
        matches
    }

    pub(crate) async fn open_best(
        &self,
        source: &SourceHandle,
        host_root: String,
    ) -> Result<Option<RemoteVolume>> {
        let Some(best) = self.probe(source).await.into_iter().next() else {
            return Ok(None);
        };
        let definition = self.definition(&best.plugin_id)?;
        let client = self.client(&definition).await?;
        let params = Value::Map(vec![
            (
                Value::from("source_id"),
                Value::from(source.source_id.clone()),
            ),
            (Value::from("options"), Value::Map(Vec::new())),
        ]);
        let response = client
            .call_with_timeout("fs.open", params, Duration::from_secs(120))
            .await?;
        let response: OpenResponse = rmpv::ext::from_value(response)
            .map_err(|error| PluginError::InvalidFrame(error.to_string()))?;
        if response.session_id.is_empty() || response.root_node.is_empty() {
            return Err(PluginError::Rpc {
                code: "INVALID_OPEN_RESULT".to_owned(),
                message: "fs.open returned an empty session or root node".to_owned(),
            });
        }
        Ok(Some(RemoteVolume::new(
            client,
            response.session_id,
            response.root_node,
            host_root,
            source.source_id.clone(),
        )))
    }

    async fn probe_one(
        &self,
        definition: &PluginDefinition,
        source: &SourceHandle,
    ) -> Result<Option<ProbeMatch>> {
        let client = self.client(definition).await?;
        let params = Value::Map(vec![(
            Value::from("source_id"),
            Value::from(source.source_id.clone()),
        )]);
        let response = client
            .call_with_timeout("fs.probe", params, Duration::from_secs(30))
            .await?;
        let response: ProbeResponse = rmpv::ext::from_value(response)
            .map_err(|error| PluginError::InvalidFrame(error.to_string()))?;
        if !response.confidence.is_finite() || !(0.0..=1.0).contains(&response.confidence) {
            return Err(PluginError::Rpc {
                code: "INVALID_PROBE_RESULT".to_owned(),
                message: "confidence must be finite and between 0 and 1".to_owned(),
            });
        }
        // "No match" needs no declared filesystem name.
        if response.confidence == 0.0 {
            return Ok(None);
        }
        let priority = definition
            .manifest
            .filesystems
            .iter()
            .filter(|filesystem| filesystem.id == response.filesystem)
            .map(|filesystem| filesystem.priority)
            .max()
            .ok_or_else(|| PluginError::Rpc {
                code: "UNDECLARED_FILESYSTEM".to_owned(),
                message: format!(
                    "plugin returned undeclared filesystem {}",
                    response.filesystem
                ),
            })?;
        Ok(Some(ProbeMatch {
            plugin_id: definition.manifest.id.clone(),
            filesystem: response.filesystem,
            confidence: response.confidence,
            label: response.label,
            priority,
        }))
    }

    /// The package's shared instance (filesystem browsing), started on
    /// first use and restarted after it died.
    async fn client(&self, definition: &PluginDefinition) -> Result<PluginConnection> {
        let mut clients = self.clients.lock().await;
        if let Some(client) = clients.get(&definition.manifest.id) {
            if client.is_alive() {
                return Ok(client.clone());
            }
            clients.remove(&definition.manifest.id);
            self.states.write().insert(
                definition.manifest.id.clone(),
                PluginState::Crashed("plugin connection closed".to_owned()),
            );
        }
        self.states
            .write()
            .insert(definition.manifest.id.clone(), PluginState::Starting);
        let started =
            PluginConnection::open(definition, self.sources.clone(), self.log_store()).await;
        let state = match &started {
            Ok(_) => PluginState::Running,
            Err(error) => PluginState::Crashed(error.to_string()),
        };
        self.states
            .write()
            .insert(definition.manifest.id.clone(), state);
        let client = started?;
        clients.insert(definition.manifest.id.clone(), client.clone());
        Ok(client)
    }
}

/// Package directories under `root`: `root` itself when it holds a
/// manifest, else its immediate subdirectories that do. Hidden directories
/// (an interrupted installation's `.staging-*` / `.old-*`) are skipped.
fn package_directories(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    if root.join(MANIFEST_FILE).is_file() {
        return Ok(vec![root.to_owned()]);
    }
    let mut directories = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if entry.file_type()?.is_dir() && entry.path().join(MANIFEST_FILE).is_file() {
            directories.push(entry.path());
        }
    }
    directories.sort();
    Ok(directories)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_package(directory: &Path, id: &str) {
        fs::create_dir_all(directory).expect("package directory");
        fs::write(
            directory.join(MANIFEST_FILE),
            format!(
                "id = \"{id}\"\nname = \"Test\"\nversion = \"1\"\nprotocol = 2\nlibrary = \"test\"\n"
            ),
        )
        .expect("manifest");
    }

    #[test]
    fn discovers_sorted_packages_and_reports_broken_ones() {
        let temporary = tempfile::tempdir().expect("roots");
        write_package(&temporary.path().join("z-last"), "org.test.z");
        write_package(&temporary.path().join("a-first"), "org.test.a");
        let broken = temporary.path().join("broken");
        fs::create_dir(&broken).expect("broken");
        fs::write(
            broken.join(MANIFEST_FILE),
            "id = \"org.test.broken\"\nprotocol_min = 1\n",
        )
        .expect("broken manifest");
        let manager =
            PluginManager::discover(&[temporary.path().to_owned()], None).expect("discover");
        assert_eq!(
            manager
                .definitions()
                .iter()
                .map(|definition| definition.manifest.id.as_str())
                .collect::<Vec<_>>(),
            vec!["org.test.a", "org.test.z"]
        );
        assert_eq!(manager.state("org.test.a"), Some(PluginState::Configured));
        let rejected = manager.rejected();
        assert_eq!(rejected.len(), 1);
        assert!(
            rejected[0].error.contains("LPP v1"),
            "{}",
            rejected[0].error
        );
    }

    #[test]
    fn installed_packages_are_removable_and_rediscovered() {
        let install = tempfile::tempdir().expect("install root");
        let bundled = tempfile::tempdir().expect("bundled root");
        write_package(&bundled.path().join("bundled"), "org.test.bundled");
        let manager = PluginManager::discover(
            &[bundled.path().to_owned()],
            Some(install.path().to_owned()),
        )
        .expect("discover");
        assert_eq!(manager.definitions().len(), 1);
        assert!(
            !manager
                .definition("org.test.bundled")
                .expect("bundled")
                .removable
        );

        write_package(&install.path().join("org.test.added"), "org.test.added");
        manager.rediscover();
        assert!(
            manager
                .definition("org.test.added")
                .expect("added")
                .removable
        );
        assert_eq!(
            manager.state("org.test.added"),
            Some(PluginState::Configured)
        );
    }
}
