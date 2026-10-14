use std::{
    collections::HashSet,
    env::consts::{DLL_PREFIX, DLL_SUFFIX},
    ffi::OsStr,
    fs,
    path::{Component, Path, PathBuf},
};

use lovelycarve_plugin::wire::PROTOCOL_VERSION;
use serde::{Deserialize, Serialize};

use super::{PluginError, Result};

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub protocol_min: u16,
    pub protocol_max: u16,
    pub library: LibraryManifest,
    #[serde(default)]
    pub permissions: Vec<PluginPermission>,
    #[serde(default)]
    pub capabilities: Vec<Capability>,
    /// 任务调度元数据:依赖、触发方式、可见性、实例限制、暂停能力与任务类型。
    #[serde(default)]
    pub task: TaskManifest,
}

/// The plugin library and where it runs.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibraryManifest {
    /// Library path relative to the plugin directory. A name without a
    /// library extension (`linuxinfo`) resolves to the platform file name:
    /// `linuxinfo.dll`, `liblinuxinfo.so` or `liblinuxinfo.dylib`.
    pub path: String,
    #[serde(default)]
    pub isolation: PluginIsolation,
    /// Additional runtime files to include in plugin packages, relative to this directory.
    #[serde(default)]
    pub resources: Vec<String>,
}

/// Where a plugin library runs.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginIsolation {
    /// Inside `lc-plugin-host`: crashes, hangs and memory use stay outside
    /// LovelyCarve, and cancelling a task kills the process.
    #[default]
    Process,
    /// Loaded into LovelyCarve: no IPC, but a crash ends the application,
    /// cancellation is cooperative and no memory limit applies.
    InProcess,
}

/// 任务触发方式:`auto` 任务在证据上线或依赖链前驱
/// 成功后由调度器自动创建;`manually` 任务只由用户显式创建。
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskTrigger {
    Auto,
    Manually,
}

/// 插件级任务调度声明。所有字段可省略,默认值为:
/// 手动触发、非内部、单实例、不可暂停。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskManifest {
    /// 前驱 function 名,空表示依赖链第一环。
    #[serde(default)]
    pub dependence: Option<String>,
    #[serde(default = "TaskManifest::default_trigger")]
    pub trigger: TaskTrigger,
    /// 内部任务:前端任务列表默认隐藏。
    #[serde(default)]
    pub internal: bool,
    /// 同一 (function, evidence) 同时最多一个未终结任务。
    #[serde(default = "TaskManifest::default_single_instance")]
    pub single_instance: bool,
    /// 允许暂停/恢复。
    #[serde(default)]
    pub can_pause: bool,
    /// 任务类型名,如 `longtime`/`analysis`;缺省按能力推导。
    #[serde(default, rename = "type")]
    pub type_name: Option<String>,
    /// 适用平台,如 `linux`/`windows`;缺省 `all` 表示全平台。
    #[serde(default)]
    pub platform: Option<String>,
    /// 功能子项,按数组声明;快速分析对话框据此渲染子项勾选。
    #[serde(default)]
    pub support: Vec<String>,
    /// 适用检材类型;空数组 = 全部。
    #[serde(default)]
    pub evidence_types: Vec<String>,
}

impl TaskManifest {
    fn default_trigger() -> TaskTrigger {
        TaskTrigger::Manually
    }

    fn default_single_instance() -> bool {
        true
    }
}

impl Default for TaskManifest {
    fn default() -> Self {
        Self {
            dependence: None,
            trigger: Self::default_trigger(),
            internal: false,
            single_instance: Self::default_single_instance(),
            can_pause: false,
            type_name: None,
            platform: None,
            support: Vec::new(),
            evidence_types: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginPermission {
    FileMaterialize,
    EmitArtifacts,
    Network,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Capability {
    Filesystem(FilesystemCapability),
    FileAnalyzer(FileAnalyzerCapability),
    EvidenceAnalyzer(EvidenceAnalyzerCapability),
    Test,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FilesystemCapability {
    pub filesystem: String,
    #[serde(default)]
    pub priority: i32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileAnalyzerCapability {
    pub analyzer: String,
    #[serde(default)]
    pub extensions: Vec<String>,
    #[serde(default)]
    pub mime_types: Vec<String>,
    #[serde(default)]
    pub priority: i32,
}

/// 检材级分析能力:任务不指向单个文件,
/// 插件拿到整个检材的只读文件视图自行定位数据。
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EvidenceAnalyzerCapability {
    pub analyzer: String,
    /// function 显示名,如「Linux基本信息」;
    /// 缺省用 `<插件名> · <analyzer>`。
    #[serde(default)]
    pub display: Option<String>,
    #[serde(default)]
    pub priority: i32,
}

impl PluginManifest {
    pub fn load(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        let manifest: Self = toml::from_str(&text).map_err(|error| {
            let legacy_process = toml::from_str::<toml::Table>(&text)
                .is_ok_and(|table| table.contains_key("process"));
            let message = if legacy_process {
                "[process] plugins are no longer supported; build the plugin as a library and \
                 declare [library] (docs/PLUGIN_PROTOCOL.md)"
                    .to_owned()
            } else {
                error.to_string()
            };
            PluginError::Manifest(format!("{}: {message}", path.display()))
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty()
            || !self
                .id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return Err(PluginError::Manifest(
                "id must contain only ASCII letters, digits, '.', '-' or '_'".to_owned(),
            ));
        }
        if self.name.trim().is_empty() || self.version.trim().is_empty() {
            return Err(PluginError::Manifest(
                "name and version must not be empty".to_owned(),
            ));
        }
        if self.protocol_min == 0 || self.protocol_min > self.protocol_max {
            return Err(PluginError::Manifest(
                "protocol range is invalid".to_owned(),
            ));
        }
        if self.protocol_min > PROTOCOL_VERSION || self.protocol_max < PROTOCOL_VERSION {
            return Err(PluginError::Manifest(format!(
                "plugin protocol range {}..{} does not include host protocol {PROTOCOL_VERSION}",
                self.protocol_min, self.protocol_max
            )));
        }
        let library = Path::new(&self.library.path);
        if self.library.path.trim().is_empty()
            || library
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(PluginError::Manifest(
                "library.path must be a relative path inside the plugin directory".to_owned(),
            ));
        }
        for resource in &self.library.resources {
            if resource.trim().is_empty()
                || Path::new(resource)
                    .components()
                    .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(PluginError::Manifest(
                    "library.resources 必须是插件目录内的相对文件路径".to_owned(),
                ));
            }
        }
        for capability in self.filesystem_capabilities() {
            if capability.filesystem.trim().is_empty()
                || !capability.filesystem.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'-' | b'_' | b'.')
                })
            {
                return Err(PluginError::Manifest(
                    "filesystem capability names must be non-empty lowercase ASCII identifiers"
                        .to_owned(),
                ));
            }
        }
        for capability in self.file_analyzer_capabilities() {
            validate_identifier("analyzer", &capability.analyzer)?;
            for extension in &capability.extensions {
                if extension.is_empty()
                    || !extension.bytes().all(|byte| {
                        byte.is_ascii_lowercase()
                            || byte.is_ascii_digit()
                            || matches!(byte, b'-' | b'_')
                    })
                {
                    return Err(PluginError::Manifest(
                        "analyzer extensions must be lowercase without a leading dot".to_owned(),
                    ));
                }
            }
            for mime in &capability.mime_types {
                if !mime.contains('/') || !mime.is_ascii() || mime.contains(char::is_whitespace) {
                    return Err(PluginError::Manifest(
                        "analyzer MIME types must be ASCII type/subtype values".to_owned(),
                    ));
                }
            }
        }
        for capability in self.evidence_analyzer_capabilities() {
            validate_identifier("analyzer", &capability.analyzer)?;
        }
        let mut permissions = HashSet::new();
        if let Some(permission) = self
            .permissions
            .iter()
            .find(|permission| !permissions.insert(**permission))
        {
            return Err(PluginError::Manifest(format!(
                "permission {permission:?} is declared more than once"
            )));
        }
        if let Some(dependence) = &self.task.dependence {
            validate_function_name("task.dependence", dependence)?;
        }
        if let Some(type_name) = &self.task.type_name {
            validate_identifier("task.type", type_name)?;
        }
        if let Some(platform) = &self.task.platform {
            validate_identifier("task.platform", platform)?;
        }
        if self.task.support.iter().any(|item| item.trim().is_empty()) {
            return Err(PluginError::Manifest(
                "task.support entries must not be empty".to_owned(),
            ));
        }
        if self
            .task
            .evidence_types
            .iter()
            .any(|item| item.trim().is_empty())
        {
            return Err(PluginError::Manifest(
                "task.evidence_types entries must not be empty".to_owned(),
            ));
        }
        Ok(())
    }

    /// The plugin library inside `plugin_directory`.
    pub fn library_path(&self, plugin_directory: &Path) -> PathBuf {
        let relative = Path::new(&self.library.path);
        let has_library_extension =
            relative
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|extension| {
                    ["dll", "so", "dylib"]
                        .iter()
                        .any(|known| extension.eq_ignore_ascii_case(known))
                });
        if has_library_extension {
            return plugin_directory.join(relative);
        }
        let file_name = relative
            .file_name()
            .map(|name| format!("{DLL_PREFIX}{}{DLL_SUFFIX}", name.to_string_lossy()))
            .unwrap_or_default();
        plugin_directory.join(relative.with_file_name(file_name))
    }

    pub fn filesystem_capabilities(&self) -> impl Iterator<Item = &FilesystemCapability> {
        self.capabilities
            .iter()
            .filter_map(|capability| match capability {
                Capability::Filesystem(filesystem) => Some(filesystem),
                Capability::FileAnalyzer(_) => None,
                Capability::EvidenceAnalyzer(_) => None,
                Capability::Test => None,
            })
    }

    pub fn file_analyzer_capabilities(&self) -> impl Iterator<Item = &FileAnalyzerCapability> {
        self.capabilities
            .iter()
            .filter_map(|capability| match capability {
                Capability::FileAnalyzer(analyzer) => Some(analyzer),
                Capability::Filesystem(_) | Capability::EvidenceAnalyzer(_) | Capability::Test => {
                    None
                }
            })
    }

    pub fn evidence_analyzer_capabilities(
        &self,
    ) -> impl Iterator<Item = &EvidenceAnalyzerCapability> {
        self.capabilities
            .iter()
            .filter_map(|capability| match capability {
                Capability::EvidenceAnalyzer(analyzer) => Some(analyzer),
                Capability::Filesystem(_) | Capability::FileAnalyzer(_) | Capability::Test => None,
            })
    }

    pub fn has_permission(&self, permission: PluginPermission) -> bool {
        self.permissions.contains(&permission)
    }

    pub(crate) fn capability_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.capabilities.iter().map(|capability| match capability {
            Capability::Filesystem(_) => "filesystem",
            Capability::FileAnalyzer(_) => "file_analyzer",
            Capability::EvidenceAnalyzer(_) => "evidence_analyzer",
            Capability::Test => "test",
        })
    }
}

fn validate_identifier(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(PluginError::Manifest(format!(
            "{label} must be a non-empty lowercase ASCII identifier"
        )));
    }
    Ok(())
}

/// Function 名形如 `<plugin id>/<analyzer>`;plugin id 允许大小写字母,
/// analyzer 段沿用小写标识符规则。
fn validate_function_name(label: &str, value: &str) -> Result<()> {
    let Some((plugin_id, analyzer)) = value.split_once('/') else {
        return Err(PluginError::Manifest(format!(
            "{label} must be written as <plugin id>/<analyzer>"
        )));
    };
    if plugin_id.is_empty()
        || !plugin_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(PluginError::Manifest(format!(
            "{label} plugin id segment contains unsupported characters"
        )));
    }
    validate_identifier(label, analyzer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_filesystem_manifest() {
        let manifest: PluginManifest = toml::from_str(
            r#"
id = "org.lovelycarve.f2fs"
name = "F2FS"
version = "0.1.0"
protocol_min = 1
protocol_max = 1
[library]
path = "plugin"
[[capabilities]]
type = "filesystem"
filesystem = "f2fs"
priority = 100
"#,
        )
        .expect("parse manifest");
        manifest.validate().expect("validate manifest");
        assert_eq!(
            manifest.filesystem_capabilities().collect::<Vec<_>>(),
            vec![&FilesystemCapability {
                filesystem: "f2fs".to_owned(),
                priority: 100,
            }]
        );
    }

    #[test]
    fn rejects_unsafe_plugin_id() {
        let mut manifest: PluginManifest = toml::from_str(
            r#"
id = "valid"
name = "Test"
version = "1"
protocol_min = 1
protocol_max = 1
[library]
path = "test"
"#,
        )
        .expect("parse manifest");
        manifest.id = "../escape".to_owned();
        assert!(matches!(manifest.validate(), Err(PluginError::Manifest(_))));
    }

    #[test]
    fn rejects_duplicate_permissions() {
        let manifest: PluginManifest = toml::from_str(
            r#"
id = "valid"
name = "Test"
version = "1"
protocol_min = 1
protocol_max = 1
permissions = ["network", "network"]
[library]
path = "test"
"#,
        )
        .expect("parse manifest");
        assert!(matches!(manifest.validate(), Err(PluginError::Manifest(_))));
    }

    #[test]
    fn parses_file_analyzer_capability_and_permissions() {
        let manifest: PluginManifest = toml::from_str(
            r#"
id = "org.test.analyzer"
name = "Analyzer"
version = "1"
protocol_min = 1
protocol_max = 1
permissions = ["file_materialize", "emit_artifacts"]
[library]
path = "plugin"
[[capabilities]]
type = "file_analyzer"
analyzer = "browser.sqlite"
extensions = ["db", "sqlite"]
mime_types = ["application/vnd.sqlite3"]
priority = 10
"#,
        )
        .expect("parse manifest");
        manifest.validate().expect("validate manifest");
        assert!(manifest.has_permission(PluginPermission::FileMaterialize));
        assert_eq!(
            manifest
                .file_analyzer_capabilities()
                .next()
                .expect("analyzer")
                .analyzer,
            "browser.sqlite"
        );
    }

    #[test]
    fn repository_plugin_manifests_stay_valid() {
        let plugins = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("plugins");
        let linuxinfo = PluginManifest::load(&plugins.join("linuxinfo").join("plugin.toml"))
            .expect("load linuxinfo manifest");
        assert_eq!(linuxinfo.id, "org.lovelycarve.linuxinfo");
        assert!(linuxinfo.has_permission(PluginPermission::EmitArtifacts));
        assert_eq!(linuxinfo.library.isolation, PluginIsolation::Process);
        assert_eq!(linuxinfo.evidence_analyzer_capabilities().count(), 1);
    }

    #[test]
    fn library_paths_resolve_to_platform_file_names() {
        let manifest = |path: &str| {
            toml::from_str::<PluginManifest>(&format!(
                r#"
id = "org.test.library"
name = "Library"
version = "1"
protocol_min = 1
protocol_max = 1
[library]
path = "{path}"
"#
            ))
            .expect("parse manifest")
        };
        let directory = Path::new("plugins").join("demo");
        assert_eq!(
            manifest("demo").library_path(&directory),
            directory.join(format!("{DLL_PREFIX}demo{DLL_SUFFIX}"))
        );
        assert_eq!(
            manifest("bin/demo").library_path(&directory),
            directory
                .join("bin")
                .join(format!("{DLL_PREFIX}demo{DLL_SUFFIX}"))
        );
        assert_eq!(
            manifest("custom.DLL").library_path(&directory),
            directory.join("custom.DLL")
        );
        assert_eq!(manifest("demo").library.isolation, PluginIsolation::Process);
        for escaping in ["../demo", "", "/abs/demo"] {
            assert!(
                matches!(manifest(escaping).validate(), Err(PluginError::Manifest(_))),
                "{escaping:?} must be rejected"
            );
        }
    }

    #[test]
    fn legacy_process_manifests_explain_the_library_requirement() {
        let temporary = tempfile::tempdir().expect("temp directory");
        let path = temporary.path().join("plugin.toml");
        fs::write(
            &path,
            r#"id = "org.test.legacy"
name = "Legacy"
version = "1"
protocol_min = 1
protocol_max = 1
[process]
command = "python"
"#,
        )
        .expect("write manifest");
        let Err(PluginError::Manifest(message)) = PluginManifest::load(&path) else {
            panic!("legacy manifest must be rejected");
        };
        assert!(message.contains("[library]"), "{message}");
    }

    #[test]
    fn task_section_defaults_match_reference_plugins() {
        let manifest: PluginManifest = toml::from_str(
            r#"
id = "org.test.plain"
name = "Plain"
version = "1"
protocol_min = 1
protocol_max = 1
[library]
path = "test"
"#,
        )
        .expect("parse manifest");
        assert_eq!(
            manifest.task,
            TaskManifest {
                dependence: None,
                trigger: TaskTrigger::Manually,
                internal: false,
                single_instance: true,
                can_pause: false,
                type_name: None,
                platform: None,
                support: Vec::new(),
                evidence_types: Vec::new(),
            }
        );
    }

    #[test]
    fn task_section_parses_dependence_chain_fields() {
        let manifest: PluginManifest = toml::from_str(
            r#"
id = "org.test.chained"
name = "Chained"
version = "1"
protocol_min = 1
protocol_max = 1
[library]
path = "test"
[task]
dependence = "org.test.first/stage.one"
trigger = "auto"
internal = true
single_instance = false
can_pause = true
type = "longtime"
"#,
        )
        .expect("parse manifest");
        manifest.validate().expect("validate manifest");
        assert_eq!(
            manifest.task,
            TaskManifest {
                dependence: Some("org.test.first/stage.one".to_owned()),
                trigger: TaskTrigger::Auto,
                internal: true,
                single_instance: false,
                can_pause: true,
                type_name: Some("longtime".to_owned()),
                platform: None,
                support: Vec::new(),
                evidence_types: Vec::new(),
            }
        );
    }

    #[test]
    fn task_dependence_must_name_a_function() {
        let manifest: PluginManifest = toml::from_str(
            r#"
id = "org.test.bad"
name = "Bad"
version = "1"
protocol_min = 1
protocol_max = 1
[library]
path = "test"
[task]
dependence = "not-a-function"
"#,
        )
        .expect("parse manifest");
        assert!(matches!(manifest.validate(), Err(PluginError::Manifest(_))));
    }

    #[test]
    fn parses_evidence_analyzer_with_task_metadata() {
        let manifest: PluginManifest = toml::from_str(
            r#"
id = "org.lovelycarve.linuxinfo"
name = "Linux信息"
version = "2.0.6"
protocol_min = 1
protocol_max = 1
permissions = ["emit_artifacts"]
[library]
path = "plugin"
[task]
type = "baseinfo"
platform = "linux"
support = ["系统信息", "用户列表"]
[[capabilities]]
type = "evidence_analyzer"
analyzer = "linuxbaseinfo"
display = "Linux基本信息"
"#,
        )
        .expect("parse manifest");
        manifest.validate().expect("validate manifest");
        let capability = manifest
            .evidence_analyzer_capabilities()
            .next()
            .expect("evidence analyzer");
        assert_eq!(capability.analyzer, "linuxbaseinfo");
        assert_eq!(capability.display.as_deref(), Some("Linux基本信息"));
        assert_eq!(manifest.task.platform.as_deref(), Some("linux"));
        assert_eq!(
            manifest.task.support,
            vec!["系统信息".to_owned(), "用户列表".to_owned()]
        );
        assert!(manifest.file_analyzer_capabilities().next().is_none());
    }

    #[test]
    fn task_platform_and_support_are_validated() {
        for task_section in ["platform = \"Linux!\"", "support = [\"\", \"用户\"]"] {
            let manifest: PluginManifest = toml::from_str(&format!(
                r#"
id = "org.test.bad"
name = "Bad"
version = "1"
protocol_min = 1
protocol_max = 1
[library]
path = "test"
[task]
{task_section}
"#
            ))
            .expect("parse manifest");
            assert!(matches!(manifest.validate(), Err(PluginError::Manifest(_))));
        }
    }
}
