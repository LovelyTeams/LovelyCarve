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
    client::{HostRpcServices, PluginProgressSink, PluginSinks},
    fingerprint::fingerprint_directory,
    EvidenceRegistry, FileRegistry, PluginConnection, PluginError, PluginLogStore, PluginManifest,
    PluginProgressEvent, RemoteVolume, Result, SourceHandle, SourceRegistry,
};

#[derive(Clone, Debug)]
pub struct PluginDefinition {
    pub directory: PathBuf,
    pub manifest: PluginManifest,
    /// Display-only content hash, computed on first request instead of during
    /// app startup (hashing a plugin with a bundled venv can read hundreds of
    /// MiB). Shared across clones so it is computed at most once.
    fingerprint: Arc<std::sync::OnceLock<String>>,
}

impl PluginDefinition {
    pub fn new(directory: PathBuf, manifest: PluginManifest) -> Self {
        Self {
            directory,
            manifest,
            fingerprint: Arc::default(),
        }
    }

    /// Directory content fingerprint for display. Blocking (walks and hashes
    /// the plugin directory on first call) — call from a blocking context.
    /// A directory that cannot be fingerprinted (too large, symlinks, I/O
    /// error) reports `unavailable: <reason>` rather than hiding the plugin.
    pub fn fingerprint(&self) -> String {
        self.fingerprint
            .get_or_init(|| match fingerprint_directory(&self.directory) {
                Ok(fingerprint) => fingerprint,
                Err(error) => {
                    tracing::debug!(
                        plugin_id = %self.manifest.id,
                        "plugin fingerprint unavailable: {error}"
                    );
                    format!("unavailable: {error}")
                }
            })
            .clone()
    }
}

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

pub struct PluginManager {
    definitions: Vec<PluginDefinition>,
    sources: SourceRegistry,
    files: FileRegistry,
    evidences: EvidenceRegistry,
    clients: Mutex<HashMap<String, PluginConnection>>,
    states: RwLock<HashMap<String, PluginState>>,
    progress_sink: RwLock<Option<PluginProgressSink>>,
    log_store: RwLock<Option<Arc<PluginLogStore>>>,
}

impl PluginManager {
    /// 发现即信任:所有清单合法的插件初始为 `Configured`(可派发/可探测)。
    /// 指纹仅作信息展示,不构成启用门槛。
    pub fn discover(roots: &[PathBuf]) -> Result<Self> {
        let mut definitions = Vec::new();
        let mut plugin_ids = HashSet::new();
        for root in roots {
            let paths = match manifest_paths(root) {
                Ok(paths) => paths,
                Err(error) => {
                    tracing::warn!(root = %root.display(), "plugin root cannot be inspected: {error}");
                    continue;
                }
            };
            for manifest_path in paths {
                let manifest = match PluginManifest::load(&manifest_path) {
                    Ok(manifest) => manifest,
                    Err(error) => {
                        tracing::warn!(manifest = %manifest_path.display(), "plugin ignored: {error}");
                        continue;
                    }
                };
                if !plugin_ids.insert(manifest.id.clone()) {
                    tracing::warn!(plugin_id = %manifest.id, "duplicate plugin ignored");
                    continue;
                }
                let directory = manifest_path.parent().ok_or_else(|| {
                    PluginError::Manifest(format!(
                        "manifest {} has no parent directory",
                        manifest_path.display()
                    ))
                })?;
                definitions.push(PluginDefinition::new(directory.to_owned(), manifest));
            }
        }
        definitions.sort_by(|left, right| left.manifest.id.cmp(&right.manifest.id));
        let states = definitions
            .iter()
            .map(|definition| (definition.manifest.id.clone(), PluginState::Configured))
            .collect();
        Ok(Self {
            definitions,
            sources: SourceRegistry::default(),
            files: FileRegistry::default(),
            evidences: EvidenceRegistry::default(),
            clients: Mutex::new(HashMap::new()),
            states: RwLock::new(states),
            progress_sink: RwLock::new(None),
            log_store: RwLock::new(None),
        })
    }

    pub fn definitions(&self) -> &[PluginDefinition] {
        &self.definitions
    }

    pub fn sources(&self) -> &SourceRegistry {
        &self.sources
    }

    pub fn files(&self) -> &FileRegistry {
        &self.files
    }

    pub fn evidences(&self) -> &EvidenceRegistry {
        &self.evidences
    }

    /// State of the plugin's shared instance (task workers are not tracked
    /// here; their state is the task's).
    pub fn state(&self, plugin_id: &str) -> Option<PluginState> {
        self.states.read().get(plugin_id).cloned()
    }

    /// Receives `host.progress` of the shared instances (direct
    /// `plugin_analyze_file` calls); a task worker reports to its task.
    pub fn set_progress_sink(&self, sink: impl Fn(PluginProgressEvent) + Send + Sync + 'static) {
        *self.progress_sink.write() = Some(Arc::new(sink));
    }

    /// Appends every plugin's `Context::log` lines to its own file under the
    /// store's root. Without a store, plugin logs only reach `tracing`.
    pub fn set_log_store(&self, store: Arc<PluginLogStore>) {
        *self.log_store.write() = Some(store);
    }

    /// Stops the plugin's shared instance; the next browse or direct analysis
    /// starts a fresh one. Task workers are separate instances and keep
    /// running, and a call cut short by the restart releases its handles as
    /// it fails.
    pub async fn restart(&self, plugin_id: &str) -> Result<()> {
        self.definition(plugin_id)?;
        if let Some(client) = self.clients.lock().await.remove(plugin_id) {
            if let Err(error) = client.shutdown().await {
                tracing::debug!(%plugin_id, "plugin restart shutdown failed: {error}");
            }
        }
        self.states
            .write()
            .insert(plugin_id.to_owned(), PluginState::Configured);
        Ok(())
    }

    pub(super) fn definition(&self, plugin_id: &str) -> Result<&PluginDefinition> {
        self.definitions
            .iter()
            .find(|definition| definition.manifest.id == plugin_id)
            .ok_or_else(|| PluginError::Manifest(format!("plugin {plugin_id} is not configured")))
    }

    pub async fn shutdown(&self) {
        let clients = std::mem::take(&mut *self.clients.lock().await);
        for (plugin_id, client) in clients {
            if let Err(error) = client.shutdown().await {
                tracing::warn!(%plugin_id, "plugin shutdown failed: {error}");
            }
            self.files.cleanup_plugin(&plugin_id);
            self.evidences.cleanup_plugin(&plugin_id);
            self.states
                .write()
                .insert(plugin_id, PluginState::Configured);
        }
    }

    pub async fn probe(&self, source: &SourceHandle) -> Vec<ProbeMatch> {
        let mut matches = Vec::new();
        for definition in self.definitions.iter().filter(|definition| {
            definition
                .manifest
                .filesystem_capabilities()
                .next()
                .is_some()
        }) {
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
        let definition = self
            .definitions
            .iter()
            .find(|definition| definition.manifest.id == best.plugin_id)
            .ok_or_else(|| {
                PluginError::Manifest(format!(
                    "probed plugin {} is no longer configured",
                    best.plugin_id
                ))
            })?;
        let client = self.client(definition).await?;
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
            .filesystem_capabilities()
            .filter(|capability| capability.filesystem == response.filesystem)
            .map(|capability| capability.priority)
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

    /// The plugin's shared instance (filesystem browsing, direct analysis),
    /// started on first use and restarted after it died.
    pub(super) async fn client(&self, definition: &PluginDefinition) -> Result<PluginConnection> {
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
        let started = self.open_connection(definition, self.sinks()).await;
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

    /// Where a new instance reports: the progress sink of the manager's owner
    /// and the shared log store. Read once, before the connection starts, so
    /// no lock is held across the await.
    pub(super) fn sinks(&self) -> PluginSinks {
        PluginSinks {
            progress: self.progress_sink.read().clone(),
            logs: self.log_store.read().clone(),
        }
    }

    /// Starts an instance of `definition` and checks that it is the plugin
    /// its manifest declares. `sinks` decides where its `host.progress`
    /// reports and `Context::log` lines go.
    pub(super) async fn open_connection(
        &self,
        definition: &PluginDefinition,
        sinks: PluginSinks,
    ) -> Result<PluginConnection> {
        let client = PluginConnection::open(
            definition,
            HostRpcServices::new(
                self.sources.clone(),
                self.files.clone(),
                self.evidences.clone(),
                definition.manifest.permissions.iter().copied(),
            ),
            sinks,
        )
        .await?;
        let mismatch = if client.identity().id != definition.manifest.id {
            Some(format!(
                "manifest id {} does not match hello id {}",
                definition.manifest.id,
                client.identity().id
            ))
        } else {
            definition
                .manifest
                .capability_names()
                .find(|capability| !client.capabilities().iter().any(|item| item == capability))
                .map(|capability| {
                    format!("plugin did not advertise declared {capability} capability")
                })
        };
        if let Some(mismatch) = mismatch {
            if let Err(error) = client.shutdown().await {
                tracing::debug!(plugin_id = %definition.manifest.id, "rejected plugin shutdown failed: {error}");
            }
            return Err(PluginError::Handshake(mismatch));
        }
        Ok(client)
    }
}

fn manifest_paths(root: &Path) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let direct = root.join("plugin.toml");
    if direct.is_file() {
        return Ok(vec![direct]);
    }
    let mut paths = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let candidate = entry.path().join("plugin.toml");
            if candidate.is_file() {
                paths.push(candidate);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{
        extensions::plugin::EvidenceAnalysisRequest, filesystem::evidence_session::EvidenceSession,
    };

    #[test]
    fn discovers_sorted_immediate_plugin_directories() {
        let temporary = tempfile::tempdir().expect("temp directory");
        for (directory, id) in [("z-last", "org.test.z"), ("a-first", "org.test.a")] {
            let plugin_directory = temporary.path().join(directory);
            fs::create_dir(&plugin_directory).expect("create plugin directory");
            fs::write(
                plugin_directory.join("plugin.toml"),
                format!(
                    r#"id = "{id}"
name = "Test"
version = "1"
protocol_min = 1
protocol_max = 1
[library]
path = "test"
"#
                ),
            )
            .expect("write manifest");
        }
        let manager =
            PluginManager::discover(&[temporary.path().to_owned()]).expect("discover plugins");
        assert_eq!(
            manager
                .definitions()
                .iter()
                .map(|definition| definition.manifest.id.as_str())
                .collect::<Vec<_>>(),
            vec!["org.test.a", "org.test.z"]
        );
        // 发现即信任:无需批准,插件初始即可派发。
        assert_eq!(manager.state("org.test.a"), Some(PluginState::Configured));
    }

    #[test]
    fn rediscovery_keeps_plugins_trusted_after_content_changes() {
        let temporary = tempfile::tempdir().expect("temp directory");
        let plugin_directory = temporary.path().join("plugin");
        fs::create_dir(&plugin_directory).expect("plugin directory");
        fs::write(
            plugin_directory.join("plugin.toml"),
            r#"id = "org.test.plugin"
name = "Test"
version = "1"
protocol_min = 1
protocol_max = 1
[library]
path = "test"
"#,
        )
        .expect("manifest");
        fs::write(plugin_directory.join("test.dll"), b"v1").expect("plugin library");
        let manager =
            PluginManager::discover(std::slice::from_ref(&plugin_directory)).expect("discover");
        assert_eq!(
            manager.state("org.test.plugin"),
            Some(PluginState::Configured)
        );

        // 插件内容变化只改指纹,不撤销可用状态。
        fs::write(plugin_directory.join("test.dll"), b"v2").expect("change library");
        let rediscovered =
            PluginManager::discover(std::slice::from_ref(&plugin_directory)).expect("rediscover");
        assert_eq!(
            rediscovered.state("org.test.plugin"),
            Some(PluginState::Configured)
        );
    }

    /// 端到端集成:经 lc-plugin-host 真实加载 linuxinfo 插件库,对真 Linux
    /// 镜像跑一次 `analyzer.analyze_evidence`(需要 `just build-plugins`
    /// 的构建产物与 `LOVELYCARVE_TEST_LINUX_IMAGE` 指向的磁盘镜像)。
    #[tokio::test]
    #[ignore = "set LOVELYCARVE_TEST_LINUX_IMAGE to a Linux disk image to run"]
    async fn linuxinfo_plugin_analyzes_real_image() {
        let plugins_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("plugins");
        let manager = PluginManager::discover(&[plugins_root]).expect("discover plugins");
        let image_path = std::env::var("LOVELYCARVE_TEST_LINUX_IMAGE").expect("test image env");
        let image = Arc::new(EvidenceSession::open(Path::new(&image_path)).expect("open image"));
        let result = manager
            .analyze_evidence(EvidenceAnalysisRequest {
                plugin_id: "org.lovelycarve.linuxinfo".to_owned(),
                analyzer: "linuxbaseinfo".to_owned(),
                image_id: "linux-test".to_owned(),
                image,
                params: serde_json::json!({"platform": "linux", "subItems": []}),
            })
            .await
            .expect("analyze evidence");
        println!("summary: {}", result.summary.as_deref().unwrap_or(""));
        println!(
            "artifacts={} timeline={}",
            result.artifacts.len(),
            result.timeline.len()
        );
        assert!(
            result
                .artifacts
                .iter()
                .any(|artifact| artifact.artifact_type == "linux.systeminfo"),
            "expected a linux.systeminfo artifact"
        );
        assert!(
            result
                .artifacts
                .iter()
                .any(|artifact| artifact.artifact_type == "linux.user"),
            "expected linux.user artifacts"
        );
        assert!(
            result
                .nodes
                .iter()
                .any(|node| node.node_type == "linux.user" && node.display_name == "用户列表"),
            "expected a declared linux.user result node"
        );
        assert!(
            result.nodes.iter().all(|node| !node.columns.is_empty()),
            "declared result nodes should carry table columns"
        );
        for node in &result.nodes {
            println!("{} columns={:?}", node.display_name, node.columns);
        }
        for artifact in &result.artifacts {
            if matches!(
                artifact.artifact_type.as_str(),
                "linux.autostart"
                    | "linux.user"
                    | "linux.group"
                    | "linux.service"
                    | "linux.software"
            ) {
                assert_eq!(
                    artifact.value.as_ref().and_then(serde_json::Value::as_str),
                    Some(artifact.title.as_str()),
                    "the title should identify the record, not repeat its category"
                );
            }
        }
        if let Some(node) = result
            .nodes
            .iter()
            .find(|node| node.node_type == "linux.autostart")
        {
            assert!(node.columns.iter().all(|column| column.key != "linux.name"));
        }
        manager.shutdown().await;
    }
}
