//! 可调度功能(function)注册表。
//!
//! 每个插件包的每个分析器是一个 function,名字固定为 `<plugin id>/<analyzer>`;
//! 作用域(系统 / 检材 / 文件)、触发方式、平台、内部标志、单实例、选项与
//! 画像事实(provides / requires)都来自包清单。文件系统能力走懒探测路径,
//! 不进入任务调度。

use serde::Serialize;

use super::model::TaskParams;
use crate::core::extensions::plugin::{AnalyzerScope, OptionKind, PluginDefinition, TaskTrigger};

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
    /// 结果树分类(`baseinfo`、`vehicle`……)。
    pub type_name: String,
    pub scope: AnalyzerScope,
    /// 系统作用域适用的操作系统族;空 = 全部。
    pub platforms: Vec<String>,
    pub trigger: TaskTrigger,
    pub internal: bool,
    pub single_instance: bool,
    /// 成功运行发布的画像事实。
    pub provides: Vec<String>,
    /// 自动运行等待的画像事实。
    pub requires: Vec<String>,
    /// 文件作用域接受的扩展名与 MIME。
    pub extensions: Vec<String>,
    pub mime_types: Vec<String>,
    /// 系统 / 检材作用域声明的输入路径模式。
    pub inputs: Vec<String>,
    pub options: Vec<FunctionOption>,
}

/// 运行前可选的一个选项(默认值为 JSON)。
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionOption {
    pub id: String,
    pub kind: OptionKind,
    pub label: String,
    pub choices: Vec<FunctionChoice>,
    pub default: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FunctionChoice {
    pub id: String,
    pub label: String,
}

impl SchedFunction {
    /// 检材上线即自动建任务:自动触发且不等待任何事实。
    pub fn is_auto_root(&self) -> bool {
        self.trigger == TaskTrigger::Auto && self.requires.is_empty()
    }

    /// 是否适用于某个操作系统族。
    pub fn applies_to_family(&self, family: &str) -> bool {
        self.platforms.is_empty()
            || self
                .platforms
                .iter()
                .any(|platform| platform.eq_ignore_ascii_case(family))
    }

    /// 校验任务的目标与选项。
    pub fn check_params(&self, params: &TaskParams) -> Result<(), String> {
        match self.scope {
            AnalyzerScope::System if params.system_id.is_none() => {
                return Err(format!("{} 需要选择一个系统", self.display_name));
            }
            AnalyzerScope::Files if params.node_id.is_none() => {
                return Err(format!("{} 需要选择一个文件", self.display_name));
            }
            AnalyzerScope::System | AnalyzerScope::Files | AnalyzerScope::Evidence => {}
        }
        for (id, value) in &params.options {
            let option = self
                .options
                .iter()
                .find(|option| option.id == *id)
                .ok_or_else(|| format!("{} 没有选项 {id}", self.display_name))?;
            option.check(value)?;
        }
        Ok(())
    }
}

impl FunctionOption {
    fn check(&self, value: &serde_json::Value) -> Result<(), String> {
        let known = |choice: &str| self.choices.iter().any(|item| item.id == choice);
        let valid = match (self.kind, value) {
            (OptionKind::Flag, serde_json::Value::Bool(_)) => true,
            (OptionKind::Text, serde_json::Value::String(text)) => text.len() <= 4096,
            (OptionKind::Choice, serde_json::Value::String(choice)) => known(choice),
            (OptionKind::Multi, serde_json::Value::Array(items)) => {
                items.iter().all(|item| item.as_str().is_some_and(known))
            }
            _ => false,
        };
        if valid {
            Ok(())
        } else {
            Err(format!("选项 {} 的值无效", self.id))
        }
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
            for analyzer in &manifest.analyzers {
                functions.push(SchedFunction {
                    function: function_name(&manifest.id, &analyzer.id),
                    plugin_id: manifest.id.clone(),
                    plugin_name: manifest.name.clone(),
                    plugin_version: manifest.version.clone(),
                    analyzer: analyzer.id.clone(),
                    display_name: analyzer.name.clone(),
                    type_name: analyzer.category.clone(),
                    scope: analyzer.scope,
                    platforms: analyzer.platforms.clone(),
                    trigger: analyzer.trigger,
                    internal: analyzer.internal,
                    single_instance: analyzer.single_instance,
                    provides: analyzer.provides.clone(),
                    requires: analyzer.requires.clone(),
                    extensions: analyzer.extensions.clone(),
                    mime_types: analyzer.mime_types.clone(),
                    inputs: analyzer.inputs.clone(),
                    options: analyzer
                        .options
                        .iter()
                        .map(|option| FunctionOption {
                            id: option.id.clone(),
                            kind: option.kind,
                            label: option.label.clone(),
                            choices: option
                                .choices
                                .iter()
                                .map(|choice| FunctionChoice {
                                    id: choice.id.clone(),
                                    label: choice.label.clone(),
                                })
                                .collect(),
                            default: option.default_json(),
                        })
                        .collect(),
                });
            }
        }
        functions.sort_by(|left, right| left.function.cmp(&right.function));
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

    /// 检材上线时自动建任务的 function。
    pub fn auto_roots(&self) -> impl Iterator<Item = &SchedFunction> {
        self.functions.iter().filter(|entry| entry.is_auto_root())
    }

    /// 等待 `facts` 中至少一个事实的自动 function。
    pub fn waiting_for<'a>(
        &'a self,
        facts: &'a [String],
    ) -> impl Iterator<Item = &'a SchedFunction> + 'a {
        self.functions.iter().filter(move |entry| {
            entry.trigger == TaskTrigger::Auto
                && entry
                    .requires
                    .iter()
                    .any(|required| facts.contains(required))
        })
    }

    /// 发布 `fact` 的 function。
    pub fn providers_of<'a>(
        &'a self,
        fact: &'a str,
    ) -> impl Iterator<Item = &'a SchedFunction> + 'a {
        self.functions
            .iter()
            .filter(move |entry| entry.provides.iter().any(|provided| provided == fact))
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::core::extensions::plugin::PluginManifest;

    pub(in crate::core::extensions::task_scheduler) fn definition(
        toml_text: &str,
    ) -> PluginDefinition {
        let manifest = PluginManifest::parse(toml_text).expect("manifest");
        PluginDefinition::from_manifest(std::path::PathBuf::from("."), manifest)
    }

    /// `first` runs when evidence comes online and provides a fact that
    /// `second` waits for; `manual` only runs on request.
    pub(in crate::core::extensions::task_scheduler) fn chain_definitions() -> Vec<PluginDefinition>
    {
        vec![
            definition(
                r#"
id = "org.test.first"
name = "First"
version = "1"
protocol = 2
library = "test"
[[analyzer]]
id = "stage_one"
name = "First · stage_one"
scope = "evidence"
trigger = "auto"
category = "filesystem"
provides = ["stage_one_done"]
"#,
            ),
            definition(
                r#"
id = "org.test.second"
name = "Second"
version = "2"
protocol = 2
library = "test"
[[analyzer]]
id = "stage_two"
name = "Second · stage_two"
scope = "evidence"
trigger = "auto"
internal = true
requires = ["stage_one_done"]
"#,
            ),
            definition(
                r#"
id = "org.test.manual"
name = "Manual"
version = "3"
protocol = 2
library = "test"
[[analyzer]]
id = "manual_scan"
name = "Manual · manual_scan"
scope = "files"
extensions = ["db"]
[[filesystem]]
id = "demo-fs"
"#,
            ),
        ]
    }

    #[test]
    fn registry_lists_analyzers_sorted_by_function() {
        let registry = FunctionRegistry::from_definitions(&chain_definitions());
        assert_eq!(
            registry
                .all()
                .iter()
                .map(|function| function.function.as_str())
                .collect::<Vec<_>>(),
            vec![
                "org.test.first/stage_one",
                "org.test.manual/manual_scan",
                "org.test.second/stage_two",
            ]
        );
        let first = registry.get("org.test.first/stage_one").expect("first");
        assert_eq!(
            first,
            &SchedFunction {
                function: "org.test.first/stage_one".to_owned(),
                plugin_id: "org.test.first".to_owned(),
                plugin_name: "First".to_owned(),
                plugin_version: "1".to_owned(),
                analyzer: "stage_one".to_owned(),
                display_name: "First · stage_one".to_owned(),
                type_name: "filesystem".to_owned(),
                scope: AnalyzerScope::Evidence,
                platforms: Vec::new(),
                trigger: TaskTrigger::Auto,
                internal: false,
                single_instance: true,
                provides: vec!["stage_one_done".to_owned()],
                requires: Vec::new(),
                extensions: Vec::new(),
                mime_types: Vec::new(),
                inputs: Vec::new(),
                options: Vec::new(),
            }
        );
        assert_eq!(
            registry
                .get("org.test.manual/manual_scan")
                .map(|function| function.type_name.as_str()),
            Some("analysis")
        );
    }

    #[test]
    fn facts_link_providers_to_the_analyzers_waiting_for_them() {
        let registry = FunctionRegistry::from_definitions(&chain_definitions());
        assert_eq!(
            registry
                .auto_roots()
                .map(|function| function.function.as_str())
                .collect::<Vec<_>>(),
            vec!["org.test.first/stage_one"]
        );
        let facts = vec!["stage_one_done".to_owned()];
        assert_eq!(
            registry
                .waiting_for(&facts)
                .map(|function| function.function.as_str())
                .collect::<Vec<_>>(),
            vec!["org.test.second/stage_two"]
        );
        assert_eq!(
            registry
                .providers_of("stage_one_done")
                .map(|function| function.function.as_str())
                .collect::<Vec<_>>(),
            vec!["org.test.first/stage_one"]
        );
    }

    #[test]
    fn system_analyzers_carry_platforms_and_option_defaults() {
        let registry = FunctionRegistry::from_definitions(&[definition(
            r#"
id = "org.lovelycarve.linuxinfo"
name = "Linux信息"
version = "2.0.0"
protocol = 2
library = "linuxinfo"
[[analyzer]]
id = "linuxbaseinfo"
name = "Linux基本信息"
scope = "system"
platforms = ["linux"]
category = "baseinfo"
[[analyzer.option]]
id = "items"
type = "multi"
label = "子项"
choices = [{ id = "users", label = "用户列表" }]
"#,
        )]);
        let function = registry
            .get("org.lovelycarve.linuxinfo/linuxbaseinfo")
            .expect("function");
        assert!(function.applies_to_family("linux"));
        assert!(!function.applies_to_family("windows"));
        assert_eq!(function.options[0].default, serde_json::json!([]));
        assert_eq!(function.trigger, TaskTrigger::Manually);
    }
}
