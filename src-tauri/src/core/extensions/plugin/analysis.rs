//! Analyzer runs (`analyzer.analyze` / `analyzer.analyze_evidence`).
//!
//! A direct `plugin_analyze_file` call runs on the plugin's shared instance,
//! the one filesystem browsing also uses. A scheduled task runs on a
//! [`TaskWorker`] instead: an instance started for that task alone, in its
//! own `lc-plugin-host` process (or its own in-process instance). Tasks of
//! one plugin therefore run side by side, each with its own memory limit, a
//! worker's `host.progress` reports belong to its task, and stopping a task
//! ends only that task's worker.
//!
//! The file or evidence handle a run registers is released when the run
//! ends, also when the caller stops waiting (a cancelled task).

use std::{sync::Arc, time::Duration};

use rmpv::Value;

use super::analysis_result::parse_analysis_result;
use super::{
    client::PluginSinks, manager::PluginDefinition, EvidenceRegistry, FileRegistry,
    PluginConnection, PluginError, PluginManager, PluginManifest, PluginPermission,
    PluginProgressEvent, Result,
};
use crate::core::analysis::results::models::PluginAnalysisResult;
use crate::core::filesystem::evidence_session::EvidenceSession;

const FILE_ANALYSIS_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const EVIDENCE_ANALYSIS_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub struct FileAnalysisRequest {
    pub plugin_id: String,
    pub analyzer: String,
    pub image_id: String,
    pub image: Arc<EvidenceSession>,
    pub node_id: String,
    pub name: String,
}

/// 检材级分析请求:不指向单个
/// 文件,插件经 `evidence.*` 自行遍历该检材的文件系统。`params` 是任务
/// 参数 JSON(含子项勾选),原样转交插件。
pub struct EvidenceAnalysisRequest {
    pub plugin_id: String,
    pub analyzer: String,
    pub image_id: String,
    pub image: Arc<EvidenceSession>,
    pub params: serde_json::Value,
}

/// A plugin instance started for one scheduled task (see the module docs).
/// When the last reference goes, a process worker is killed with it.
pub struct TaskWorker {
    plugin_id: String,
    connection: PluginConnection,
}

impl TaskWorker {
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Whether the worker can still serve its run (process up, instance open).
    pub fn is_alive(&self) -> bool {
        self.connection.is_alive()
    }

    /// Lets the worker exit after its run returned.
    pub async fn shutdown(&self) -> Result<()> {
        self.connection.shutdown().await
    }

    /// Ends the worker now (task cancelled, paused or timed out): kills its
    /// `lc-plugin-host` process, or cancels the in-process calls.
    pub async fn terminate(&self) {
        self.connection.terminate().await;
    }

    fn connection_for(&self, plugin_id: &str) -> Result<&PluginConnection> {
        if self.plugin_id == plugin_id {
            Ok(&self.connection)
        } else {
            Err(PluginError::Rpc {
                code: "WORKER_MISMATCH".to_owned(),
                message: format!("a {} worker cannot run {plugin_id}", self.plugin_id),
            })
        }
    }
}

impl PluginManager {
    /// Starts a dedicated instance of `plugin_id` for one task. Its
    /// `host.progress` reports go to `progress` and nowhere else.
    pub async fn start_task_worker(
        &self,
        plugin_id: &str,
        progress: impl Fn(PluginProgressEvent) + Send + Sync + 'static,
    ) -> Result<TaskWorker> {
        let definition = self.definition(plugin_id)?;
        let connection = self
            .open_connection(
                definition,
                PluginSinks {
                    progress: Some(Arc::new(progress)),
                    ..self.sinks()
                },
            )
            .await?;
        Ok(TaskWorker {
            plugin_id: plugin_id.to_owned(),
            connection,
        })
    }

    /// Runs a file analyzer on the plugin's shared instance.
    pub async fn analyze_file(&self, request: FileAnalysisRequest) -> Result<PluginAnalysisResult> {
        let definition = self.file_analyzer(&request)?;
        let connection = self.client(definition).await?;
        self.run_file_analysis(&connection, definition, request)
            .await
    }

    /// Runs a file analyzer on a task's worker.
    pub async fn analyze_file_on(
        &self,
        worker: &TaskWorker,
        request: FileAnalysisRequest,
    ) -> Result<PluginAnalysisResult> {
        let connection = worker.connection_for(&request.plugin_id)?;
        let definition = self.file_analyzer(&request)?;
        self.run_file_analysis(connection, definition, request)
            .await
    }

    /// 检材级分析:登记证据会话后调用插件的 `analyzer.analyze_evidence`,
    /// 期间插件可经 `evidence.*` 只读访问该检材的全部可浏览卷。
    pub async fn analyze_evidence(
        &self,
        request: EvidenceAnalysisRequest,
    ) -> Result<PluginAnalysisResult> {
        let definition = self.evidence_analyzer(&request)?;
        let connection = self.client(definition).await?;
        self.run_evidence_analysis(&connection, definition, request)
            .await
    }

    /// [`Self::analyze_evidence`] on a task's worker.
    pub async fn analyze_evidence_on(
        &self,
        worker: &TaskWorker,
        request: EvidenceAnalysisRequest,
    ) -> Result<PluginAnalysisResult> {
        let connection = worker.connection_for(&request.plugin_id)?;
        let definition = self.evidence_analyzer(&request)?;
        self.run_evidence_analysis(connection, definition, request)
            .await
    }

    fn file_analyzer(&self, request: &FileAnalysisRequest) -> Result<&PluginDefinition> {
        let definition = self.definition(&request.plugin_id)?;
        if definition
            .manifest
            .file_analyzer_capabilities()
            .any(|capability| capability.analyzer == request.analyzer)
        {
            Ok(definition)
        } else {
            Err(PluginError::Rpc {
                code: "ANALYZER_NOT_DECLARED".to_owned(),
                message: format!(
                    "plugin {} did not declare analyzer {}",
                    request.plugin_id, request.analyzer
                ),
            })
        }
    }

    fn evidence_analyzer(&self, request: &EvidenceAnalysisRequest) -> Result<&PluginDefinition> {
        let definition = self.definition(&request.plugin_id)?;
        if definition
            .manifest
            .evidence_analyzer_capabilities()
            .any(|capability| capability.analyzer == request.analyzer)
        {
            Ok(definition)
        } else {
            Err(PluginError::Rpc {
                code: "ANALYZER_NOT_DECLARED".to_owned(),
                message: format!(
                    "plugin {} did not declare evidence analyzer {}",
                    request.plugin_id, request.analyzer
                ),
            })
        }
    }

    async fn run_file_analysis(
        &self,
        connection: &PluginConnection,
        definition: &PluginDefinition,
        request: FileAnalysisRequest,
    ) -> Result<PluginAnalysisResult> {
        let FileAnalysisRequest {
            plugin_id,
            analyzer,
            image_id,
            image,
            node_id,
            name,
        } = request;
        let handle = self
            .files()
            .register(&plugin_id, image_id, image, node_id, name)?;
        let _registration = FileRegistration {
            files: self.files(),
            plugin_id: &plugin_id,
            file_id: handle.file_id.clone(),
        };
        let params = Value::Map(vec![
            (Value::from("analyzer"), Value::from(analyzer)),
            (
                Value::from("file"),
                Value::Map(vec![
                    (Value::from("file_id"), Value::from(handle.file_id)),
                    (Value::from("name"), Value::from(handle.name)),
                    (Value::from("size"), Value::from(handle.size)),
                    (Value::from("mime"), Value::from(handle.mime)),
                    (Value::from("read_only"), Value::Boolean(true)),
                ]),
            ),
        ]);
        let response = connection
            .call_with_timeout("analyzer.analyze", params, FILE_ANALYSIS_TIMEOUT)
            .await?;
        let result = parse_analysis_result(response)?;
        validate_analysis_permissions(&definition.manifest, &result)?;
        Ok(result)
    }

    async fn run_evidence_analysis(
        &self,
        connection: &PluginConnection,
        definition: &PluginDefinition,
        request: EvidenceAnalysisRequest,
    ) -> Result<PluginAnalysisResult> {
        let EvidenceAnalysisRequest {
            plugin_id,
            analyzer,
            image_id,
            image,
            params,
        } = request;
        let handle = self.evidences().register(&plugin_id, image_id, image);
        let _registration = EvidenceRegistration {
            evidences: self.evidences(),
            plugin_id: &plugin_id,
            evidence_id: handle.evidence_id.clone(),
        };
        let evidence_params = match &params {
            serde_json::Value::Null => Value::Map(Vec::new()),
            value => serde_json_to_value(value),
        };
        let params = Value::Map(vec![
            (Value::from("analyzer"), Value::from(analyzer)),
            (
                Value::from("evidence"),
                Value::Map(vec![
                    (Value::from("evidence_id"), Value::from(handle.evidence_id)),
                    (Value::from("image_id"), Value::from(handle.image_id)),
                    (Value::from("params"), evidence_params),
                ]),
            ),
        ]);
        let response = connection
            .call_with_timeout(
                "analyzer.analyze_evidence",
                params,
                EVIDENCE_ANALYSIS_TIMEOUT,
            )
            .await?;
        let result = parse_analysis_result(response)?;
        validate_analysis_permissions(&definition.manifest, &result)?;
        Ok(result)
    }
}

/// Releases a run's file handle, with its materialized copies, when the run
/// ends or is dropped.
struct FileRegistration<'a> {
    files: &'a FileRegistry,
    plugin_id: &'a str,
    file_id: String,
}

impl Drop for FileRegistration<'_> {
    fn drop(&mut self) {
        self.files.cleanup_file(self.plugin_id, &self.file_id);
    }
}

/// Releases a run's evidence session when the run ends or is dropped.
struct EvidenceRegistration<'a> {
    evidences: &'a EvidenceRegistry,
    plugin_id: &'a str,
    evidence_id: String,
}

impl Drop for EvidenceRegistration<'_> {
    fn drop(&mut self) {
        self.evidences
            .remove_evidence(self.plugin_id, &self.evidence_id);
    }
}

fn validate_analysis_permissions(
    manifest: &PluginManifest,
    result: &PluginAnalysisResult,
) -> Result<()> {
    if (!result.artifacts.is_empty() || !result.timeline.is_empty() || !result.findings.is_empty())
        && !manifest.has_permission(PluginPermission::EmitArtifacts)
    {
        return Err(PluginError::Rpc {
            code: "PERMISSION_DENIED".to_owned(),
            message: "plugin returned structured results without emit_artifacts permission"
                .to_owned(),
        });
    }
    Ok(())
}

/// 任务参数 JSON(`task.params`)→ MessagePack 值。子项勾选等原始字段
/// 原样转交插件,宿主不做语义解释。
fn serde_json_to_value(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Nil,
        serde_json::Value::Bool(flag) => Value::Boolean(*flag),
        serde_json::Value::Number(number) => {
            if let Some(unsigned) = number.as_u64() {
                Value::from(unsigned)
            } else if let Some(signed) = number.as_i64() {
                Value::from(signed)
            } else {
                Value::from(number.as_f64().unwrap_or_default())
            }
        }
        serde_json::Value::String(text) => Value::from(text.as_str()),
        serde_json::Value::Array(items) => {
            Value::Array(items.iter().map(serde_json_to_value).collect())
        }
        serde_json::Value::Object(fields) => Value::Map(
            fields
                .iter()
                .map(|(key, item)| (Value::from(key.as_str()), serde_json_to_value(item)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{Condvar, Mutex, PoisonError},
    };

    use lovelycarve_plugin::{
        abi::{HostApiV1, PluginApiV1},
        runtime, AnalysisResult, File, FileAnalyzer, LogLevel, Plugin,
    };

    use super::*;
    use crate::core::analysis::results::models::PluginAnalysisArtifact;
    use crate::core::extensions::plugin::{
        client::HostRpcServices, in_process::InProcessClient, PluginLogStore,
    };

    const PLUGIN_ID: &str = "org.test.workers";
    const ANALYZER: &str = "test.workers";

    /// Lets two plugin calls wait for each other; a call that finds itself
    /// alone gives up instead of blocking the test forever.
    struct Rendezvous {
        arrived: Mutex<usize>,
        changed: Condvar,
    }

    impl Rendezvous {
        fn meet(&self, parties: usize) -> bool {
            let mut arrived = self.arrived.lock().unwrap_or_else(PoisonError::into_inner);
            *arrived += 1;
            self.changed.notify_all();
            let (_arrived, wait) = self
                .changed
                .wait_timeout_while(arrived, Duration::from_secs(5), |arrived| {
                    *arrived < parties
                })
                .unwrap_or_else(PoisonError::into_inner);
            !wait.timed_out()
        }
    }

    static BOTH_RUNNING: Rendezvous = Rendezvous {
        arrived: Mutex::new(0),
        changed: Condvar::new(),
    };
    static WAITING_FILE: Mutex<Option<String>> = Mutex::new(None);

    /// `wait` runs until cancelled, `log` writes a line without waiting, and
    /// other names rendezvous to verify concurrent task workers.
    struct Workers;

    impl FileAnalyzer for Workers {
        fn analyzer(&self) -> &str {
            ANALYZER
        }

        fn analyze(&self, file: &File) -> lovelycarve_plugin::Result<AnalysisResult> {
            if file.name() == "wait" {
                *WAITING_FILE.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(file.id().to_owned());
                while !file.context().is_cancelled() {
                    std::thread::sleep(Duration::from_millis(2));
                }
                return Err(lovelycarve_plugin::Error::cancelled());
            }
            file.context().progress(1, Some(2), Some(file.name()))?;
            file.context().log(LogLevel::Info, "workers run");
            if file.name() != "log" && !BOTH_RUNNING.meet(2) {
                return Err(lovelycarve_plugin::Error::new(
                    "NOT_CONCURRENT",
                    "the other run never started",
                ));
            }
            Ok(AnalysisResult {
                summary: Some(file.name().to_owned()),
                ..AnalysisResult::default()
            })
        }
    }

    fn workers_plugin() -> Plugin {
        Plugin::new(PLUGIN_ID, "Workers", "1").file_analyzer(Workers)
    }

    unsafe extern "C" fn workers_entry(host: *const HostApiV1, plugin: *mut PluginApiV1) -> i32 {
        unsafe { runtime::entry(host, plugin, workers_plugin) }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        manager: PluginManager,
        image: Arc<EvidenceSession>,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("temp directory");
            let plugin_directory = directory.path().join("workers");
            std::fs::create_dir(&plugin_directory).expect("plugin directory");
            std::fs::write(
                plugin_directory.join("plugin.toml"),
                format!(
                    r#"id = "{PLUGIN_ID}"
name = "Workers"
version = "1"
protocol_min = 1
protocol_max = 1
[library]
path = "workers"
isolation = "in_process"
[[capabilities]]
type = "file_analyzer"
analyzer = "{ANALYZER}"
"#
                ),
            )
            .expect("manifest");
            let manager = PluginManager::discover(&[plugin_directory]).expect("discover");
            let image_path = directory.path().join("image.raw");
            std::fs::write(&image_path, b"hello world!").expect("raw image");
            let image = Arc::new(EvidenceSession::open(&image_path).expect("open image"));
            Self {
                _directory: directory,
                manager,
                image,
            }
        }

        /// A worker as `start_task_worker` builds it, from the test entry
        /// point instead of a library on disk.
        fn worker(
            &self,
            progress: impl Fn(PluginProgressEvent) + Send + Sync + 'static,
        ) -> TaskWorker {
            self.worker_with_logs(progress, None)
        }

        /// The same worker, with the log store the manager would hand it.
        fn worker_with_logs(
            &self,
            progress: impl Fn(PluginProgressEvent) + Send + Sync + 'static,
            logs: Option<Arc<PluginLogStore>>,
        ) -> TaskWorker {
            let services = HostRpcServices::new(
                self.manager.sources().clone(),
                self.manager.files().clone(),
                self.manager.evidences().clone(),
                [],
            );
            let client = InProcessClient::create(
                workers_entry,
                PLUGIN_ID,
                services,
                PluginSinks {
                    progress: Some(Arc::new(progress)),
                    logs,
                },
            )
            .expect("in-process worker");
            TaskWorker {
                plugin_id: PLUGIN_ID.to_owned(),
                connection: PluginConnection::InProcess(client),
            }
        }

        fn request(&self, name: &str) -> FileAnalysisRequest {
            FileAnalysisRequest {
                plugin_id: PLUGIN_ID.to_owned(),
                analyzer: ANALYZER.to_owned(),
                image_id: "image".to_owned(),
                image: Arc::clone(&self.image),
                node_id: "diskgap:0x0:0xc".to_owned(),
                name: name.to_owned(),
            }
        }
    }

    fn recorder() -> (
        Arc<parking_lot::Mutex<Vec<String>>>,
        impl Fn(PluginProgressEvent) + Send + Sync + 'static,
    ) {
        let messages = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let sink = Arc::clone(&messages);
        (messages, move |event: PluginProgressEvent| {
            sink.lock().push(event.message.unwrap_or_default());
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn task_workers_run_side_by_side_and_keep_their_progress_apart() {
        let fixture = Fixture::new();
        let (first_messages, first_sink) = recorder();
        let (second_messages, second_sink) = recorder();
        let first = fixture.worker(first_sink);
        let second = fixture.worker(second_sink);

        let (alpha, beta) = tokio::join!(
            fixture
                .manager
                .analyze_file_on(&first, fixture.request("alpha")),
            fixture
                .manager
                .analyze_file_on(&second, fixture.request("beta")),
        );

        assert_eq!(alpha.expect("alpha").summary.as_deref(), Some("alpha"));
        assert_eq!(beta.expect("beta").summary.as_deref(), Some("beta"));
        assert_eq!(*first_messages.lock(), vec!["alpha".to_owned()]);
        assert_eq!(*second_messages.lock(), vec!["beta".to_owned()]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dropped_run_releases_its_file_handle() {
        let fixture = Fixture::new();
        let worker = fixture.worker(|_| {});
        let waiting_file = async {
            for _ in 0..1000 {
                let file_id = WAITING_FILE
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                if let Some(file_id) = file_id {
                    return file_id;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            panic!("the waiting run never started");
        };

        // Leaving the `select!` drops the run, as a cancelled task does.
        let file_id = tokio::select! {
            _ = fixture.manager.analyze_file_on(&worker, fixture.request("wait")) => {
                panic!("the waiting run must not finish on its own")
            }
            file_id = waiting_file => file_id,
        };

        assert!(fixture.manager.files().info(PLUGIN_ID, &file_id).is_err());
    }

    /// End to end: a log line a plugin writes during a run is appended to
    /// that plugin's own file.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_plugin_log_line_lands_in_its_own_file() {
        let fixture = Fixture::new();
        let log_root = fixture._directory.path().join("plugin-logs");
        let store = PluginLogStore::open(&log_root).expect("log store");
        let worker = fixture.worker_with_logs(|_| {}, Some(Arc::clone(&store)));

        fixture
            .manager
            .analyze_file_on(&worker, fixture.request("log"))
            .await
            .expect("run");

        worker.shutdown().await.expect("shutdown worker");
        store.shutdown();
        let path = store.file_path(PLUGIN_ID);
        let content = std::fs::read_to_string(&path).expect("plugin log file");
        assert!(content.contains(" INFO workers run"), "{content}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn workers_only_run_their_own_plugin() {
        let fixture = Fixture::new();
        let worker = fixture.worker(|_| {});
        let mut request = fixture.request("alpha");
        request.plugin_id = "org.test.other".to_owned();
        assert!(matches!(
            fixture.manager.analyze_file_on(&worker, request).await,
            Err(PluginError::Rpc { code, .. }) if code == "WORKER_MISMATCH"
        ));
    }

    /// 端到端:真实的 lc-plugin-host 任务实例同时分析同一个 Linux 镜像,
    /// 其中两条任务属于同一个插件(需要 `just build-plugins` 的构建产物与
    /// `LOVELYCARVE_TEST_LINUX_IMAGE` 指向的磁盘镜像)。
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "set LOVELYCARVE_TEST_LINUX_IMAGE to a Linux disk image to run"]
    async fn task_workers_analyze_a_real_image_side_by_side() {
        let plugins_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("plugins");
        let manager = PluginManager::discover(&[plugins_root]).expect("discover plugins");
        let image_path = std::env::var("LOVELYCARVE_TEST_LINUX_IMAGE").expect("test image env");
        let image =
            Arc::new(EvidenceSession::open(std::path::Path::new(&image_path)).expect("open image"));
        let runs = [
            ("org.lovelycarve.linuxinfo", "linuxbaseinfo"),
            ("org.lovelycarve.linuxlogs", "dpkg.actions"),
            ("org.lovelycarve.linuxlogs", "alternatives"),
        ];
        let mut workers = Vec::new();
        let mut reports = Vec::new();
        for (plugin_id, _) in runs {
            let (messages, sink) = recorder();
            workers.push(
                manager
                    .start_task_worker(plugin_id, sink)
                    .await
                    .expect("start task worker"),
            );
            reports.push(messages);
        }
        let request = |index: usize| EvidenceAnalysisRequest {
            plugin_id: runs[index].0.to_owned(),
            analyzer: runs[index].1.to_owned(),
            image_id: "linux-test".to_owned(),
            image: Arc::clone(&image),
            params: serde_json::json!({"platform": "linux", "subItems": []}),
        };
        let started = std::time::Instant::now();
        let (first, second, third) = tokio::join!(
            manager.analyze_evidence_on(&workers[0], request(0)),
            manager.analyze_evidence_on(&workers[1], request(1)),
            manager.analyze_evidence_on(&workers[2], request(2)),
        );
        println!("three concurrent runs took {:?}", started.elapsed());
        for (index, result) in [first, second, third].into_iter().enumerate() {
            let result = result.expect("analysis");
            println!(
                "{} {}: {} artifacts, {} timeline events, {} progress reports",
                runs[index].0,
                runs[index].1,
                result.artifacts.len(),
                result.timeline.len(),
                reports[index].lock().len()
            );
            assert!(
                !result.artifacts.is_empty(),
                "{} returned nothing",
                runs[index].1
            );
        }
        for worker in &workers {
            worker.shutdown().await.expect("worker shutdown");
        }
    }

    #[test]
    fn structured_results_require_emit_artifacts_permission() {
        let manifest = |permissions: &str| {
            toml::from_str::<PluginManifest>(&format!(
                r#"id = "org.test.analyzer"
name = "Test"
version = "1"
protocol_min = 1
protocol_max = 1
permissions = [{permissions}]
[library]
path = "test"
"#
            ))
            .expect("manifest")
        };
        let result = PluginAnalysisResult {
            summary: Some("done".to_owned()),
            nodes: Vec::new(),
            artifacts: vec![PluginAnalysisArtifact {
                artifact_type: "test.value".to_owned(),
                title: "value".to_owned(),
                value: None,
                attributes: HashMap::new(),
            }],
            timeline: Vec::new(),
            findings: Vec::new(),
        };

        assert!(matches!(
            validate_analysis_permissions(&manifest(""), &result),
            Err(PluginError::Rpc { code, .. }) if code == "PERMISSION_DENIED"
        ));
        validate_analysis_permissions(&manifest("\"emit_artifacts\""), &result)
            .expect("permission accepted");
    }
}
