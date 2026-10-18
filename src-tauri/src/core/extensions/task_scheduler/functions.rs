//! 可调度 Function 注册表。
//!
//! 任务按 function 名创建、依赖链按 function 名匹配。可调度单元是
//! 插件清单里的每个 `file_analyzer` 能力,function 名固定为
//! `<plugin id>/<analyzer>`;任务级元数据(`dependence`/`trigger`/`internal`/
//! `single_instance`/`can_pause`/`type`)取自清单 `[task]` 段。文件系统能力
//! 走懒探测路径,不进入任务调度。

use serde::Serialize;

use crate::core::extensions::plugin::{PluginDefinition, TaskTrigger};

/// 默认任务类型名。
pub const DEFAULT_TYPE_NAME: &str = "fileanalyzer";

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedFunction {
    /// `<plugin id>/<analyzer>`。
    pub function: String,
    pub plugin_id: String,
    pub plugin_name: String,
    pub plugin_version: String,
    pub analyzer: String,
    pub display_name: String,
    pub type_name: String,
    pub dependence: Option<String>,
    pub trigger: TaskTrigger,
    pub internal: bool,
    pub single_instance: bool,
    pub can_pause: bool,
    pub extensions: Vec<String>,
    pub mime_types: Vec<String>,
    pub priority: i32,
    /// 适用平台;`None` = `all`。
    pub platform: Option<String>,
    /// 功能子项,供快速分析对话框渲染勾选。
    pub support: Vec<String>,
    /// 适用检材类型;空 = 全部。
    pub evidence_types: Vec<String>,
    /// 检材级 function:任务不指向文件,插件经 `evidence.*` 遍历检材。
    pub evidence_level: bool,
}

impl SchedFunction {
    pub fn is_auto_root(&self) -> bool {
        self.trigger == TaskTrigger::Auto && self.dependence.is_none()
    }
}

pub fn function_name(plugin_id: &str, analyzer: &str) -> String {
    format!("{plugin_id}/{analyzer}")
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FunctionRegistry {
    functions: Vec<SchedFunction>,
}

impl FunctionRegistry {
    pub fn from_definitions(definitions: &[PluginDefinition]) -> Self {
        let mut functions = Vec::new();
        for definition in definitions {
            let manifest = &definition.manifest;
            let type_name = manifest
                .task
                .type_name
                .clone()
                .unwrap_or_else(|| DEFAULT_TYPE_NAME.to_owned());
            let dependence = manifest
                .task
                .dependence
                .clone()
                .filter(|dependence| !dependence.is_empty());
            for capability in manifest.file_analyzer_capabilities() {
                functions.push(SchedFunction {
                    function: function_name(&manifest.id, &capability.analyzer),
                    plugin_id: manifest.id.clone(),
                    plugin_name: manifest.name.clone(),
                    plugin_version: manifest.version.clone(),
                    analyzer: capability.analyzer.clone(),
                    display_name: format!("{} · {}", manifest.name, capability.analyzer),
                    type_name: type_name.clone(),
                    dependence: dependence.clone(),
                    trigger: manifest.task.trigger,
                    internal: manifest.task.internal,
                    single_instance: manifest.task.single_instance,
                    can_pause: manifest.task.can_pause,
                    extensions: capability.extensions.clone(),
                    mime_types: capability.mime_types.clone(),
                    priority: capability.priority,
                    platform: manifest.task.platform.clone(),
                    support: manifest.task.support.clone(),
                    evidence_types: manifest.task.evidence_types.clone(),
                    evidence_level: false,
                });
            }
            for capability in manifest.evidence_analyzer_capabilities() {
                functions.push(SchedFunction {
                    function: function_name(&manifest.id, &capability.analyzer),
                    plugin_id: manifest.id.clone(),
                    plugin_name: manifest.name.clone(),
                    plugin_version: manifest.version.clone(),
                    analyzer: capability.analyzer.clone(),
                    display_name: capability
                        .display
                        .clone()
                        .unwrap_or_else(|| format!("{} · {}", manifest.name, capability.analyzer)),
                    type_name: type_name.clone(),
                    dependence: dependence.clone(),
                    trigger: manifest.task.trigger,
                    internal: manifest.task.internal,
                    single_instance: manifest.task.single_instance,
                    can_pause: manifest.task.can_pause,
                    extensions: Vec::new(),
                    mime_types: Vec::new(),
                    priority: capability.priority,
                    platform: manifest.task.platform.clone(),
                    support: manifest.task.support.clone(),
                    evidence_types: manifest.task.evidence_types.clone(),
                    evidence_level: true,
                });
            }
        }
        functions.sort_by(|left, right| left.function.cmp(&right.function));
        // Validate the entire graph before exposing functions: single-instance
        // only limits active rows and cannot stop a cycle of successful tasks.
        let by_name: std::collections::HashMap<_, _> = functions
            .iter()
            .map(|function| (function.function.as_str(), function))
            .collect();
        let invalid: std::collections::HashSet<String> = functions.iter().filter_map(|function| {
            let mut visited = std::collections::HashSet::new();
            let mut current = function;
            loop {
                if !visited.insert(current.function.as_str()) {
                    tracing::warn!(function = %function.function, "task function ignored: dependency cycle");
                    return Some(function.function.clone());
                }
                let dependency = current.dependence.as_deref()?;
                let Some(next) = by_name.get(dependency).copied() else {
                    tracing::warn!(function = %function.function, %dependency, "task function ignored: missing dependency");
                    return Some(function.function.clone());
                };
                current = next;
            }
        }).collect();
        functions.retain(|function| !invalid.contains(&function.function));
        Self { functions }
    }

    pub fn all(&self) -> &[SchedFunction] {
        &self.functions
    }

    pub fn get(&self, function: &str) -> Option<&SchedFunction> {
        self.functions
            .iter()
            .find(|entry| entry.function == function)
    }

    /// 依赖当前 function 的下一级 function。
    pub fn dependents_of<'a>(
        &'a self,
        function: &'a str,
    ) -> impl Iterator<Item = &'a SchedFunction> + 'a {
        self.functions
            .iter()
            .filter(move |entry| entry.dependence.as_deref() == Some(function))
    }

    /// 依赖链第一环的自动任务。
    pub fn auto_roots(&self) -> impl Iterator<Item = &SchedFunction> {
        self.functions.iter().filter(|entry| entry.is_auto_root())
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::core::extensions::plugin::PluginManifest;

    pub(in crate::core::extensions::task_scheduler) fn definition(
        toml_text: &str,
    ) -> PluginDefinition {
        let manifest: PluginManifest = toml::from_str(toml_text).expect("manifest");
        manifest.validate().expect("valid manifest");
        PluginDefinition::new(std::path::PathBuf::from("."), manifest)
    }

    pub(in crate::core::extensions::task_scheduler) fn chain_definitions() -> Vec<PluginDefinition>
    {
        vec![
            definition(
                r#"
id = "org.test.first"
name = "First"
version = "1"
protocol_min = 1
protocol_max = 1
[library]
path = "test"
[task]
trigger = "auto"
type = "filesystem"
[[capabilities]]
type = "file_analyzer"
analyzer = "stage.one"
"#,
            ),
            definition(
                r#"
id = "org.test.second"
name = "Second"
version = "2"
protocol_min = 1
protocol_max = 1
[library]
path = "test"
[task]
dependence = "org.test.first/stage.one"
trigger = "auto"
internal = true
[[capabilities]]
type = "file_analyzer"
analyzer = "stage.two"
extensions = ["db"]
"#,
            ),
            definition(
                r#"
id = "org.test.manual"
name = "Manual"
version = "3"
protocol_min = 1
protocol_max = 1
[library]
path = "test"
[[capabilities]]
type = "file_analyzer"
analyzer = "manual.scan"
[[capabilities]]
type = "filesystem"
filesystem = "demo-fs"
"#,
            ),
        ]
    }

    #[test]
    fn registry_lists_only_file_analyzers_sorted_by_function() {
        let registry = FunctionRegistry::from_definitions(&chain_definitions());
        assert_eq!(
            registry
                .all()
                .iter()
                .map(|function| function.function.as_str())
                .collect::<Vec<_>>(),
            vec![
                "org.test.first/stage.one",
                "org.test.manual/manual.scan",
                "org.test.second/stage.two",
            ]
        );
        let first = registry.get("org.test.first/stage.one").expect("first");
        assert_eq!(
            first,
            &SchedFunction {
                function: "org.test.first/stage.one".to_owned(),
                plugin_id: "org.test.first".to_owned(),
                plugin_name: "First".to_owned(),
                plugin_version: "1".to_owned(),
                analyzer: "stage.one".to_owned(),
                display_name: "First · stage.one".to_owned(),
                type_name: "filesystem".to_owned(),
                dependence: None,
                trigger: TaskTrigger::Auto,
                internal: false,
                single_instance: true,
                can_pause: false,
                extensions: Vec::new(),
                mime_types: Vec::new(),
                priority: 0,
                platform: None,
                support: Vec::new(),
                evidence_types: Vec::new(),
                evidence_level: false,
            }
        );
    }

    #[test]
    fn evidence_analyzers_surface_platform_and_support() {
        let definitions = vec![definition(
            r#"
id = "org.lovelycarve.linuxinfo"
name = "Linux信息"
version = "2.0.6"
protocol_min = 1
protocol_max = 1
[library]
path = "linuxinfo"
[task]
type = "baseinfo"
platform = "linux"
support = ["系统信息", "用户列表"]
[[capabilities]]
type = "evidence_analyzer"
analyzer = "linuxbaseinfo"
display = "Linux基本信息"
"#,
        )];
        let registry = FunctionRegistry::from_definitions(&definitions);
        let function = registry
            .get("org.lovelycarve.linuxinfo/linuxbaseinfo")
            .expect("function");
        assert_eq!(function.display_name, "Linux基本信息");
        assert_eq!(function.type_name, "baseinfo");
        assert_eq!(function.platform.as_deref(), Some("linux"));
        assert_eq!(
            function.support,
            vec!["系统信息".to_owned(), "用户列表".to_owned()]
        );
        assert!(function.evidence_level);
        assert!(function.extensions.is_empty());
    }

    #[test]
    fn dependents_and_auto_roots_follow_manifest_chain() {
        let registry = FunctionRegistry::from_definitions(&chain_definitions());
        assert_eq!(
            registry
                .auto_roots()
                .map(|function| function.function.as_str())
                .collect::<Vec<_>>(),
            vec!["org.test.first/stage.one"]
        );
        assert_eq!(
            registry
                .dependents_of("org.test.first/stage.one")
                .map(|function| function.function.as_str())
                .collect::<Vec<_>>(),
            vec!["org.test.second/stage.two"]
        );
        assert!(registry
            .dependents_of("org.test.second/stage.two")
            .next()
            .is_none());
        assert_eq!(
            registry
                .get("org.test.manual/manual.scan")
                .map(|function| function.type_name.as_str()),
            Some(DEFAULT_TYPE_NAME)
        );
    }
    #[test]
    fn dependency_cycles_and_missing_predecessors_are_not_dispatchable() {
        let mut definitions = chain_definitions();
        let mut self_dependent = definitions[2].clone();
        self_dependent.manifest.task.dependence = Some("org.test.manual/manual.scan".into());
        definitions[2] = self_dependent;
        let mut cycle = definitions.clone();
        cycle[0].manifest.task.dependence = Some("org.test.second/stage.two".into());
        assert!(FunctionRegistry::from_definitions(&cycle).all().is_empty());
        let registry = FunctionRegistry::from_definitions(&definitions);
        assert!(registry.get("org.test.manual/manual.scan").is_none());
        assert_eq!(registry.all().len(), 2);
        definitions[0].manifest.task.dependence = Some("missing/function".into());
        assert!(FunctionRegistry::from_definitions(&definitions)
            .all()
            .is_empty());
    }
}
