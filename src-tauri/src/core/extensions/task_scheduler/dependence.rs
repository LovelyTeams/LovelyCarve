//! 自动任务:检材上线时的首任务,与画像事实发布后的后续任务。
//!
//! - 检材上线:为每个自动触发、不等待事实的 function 建任务——检材作用域
//!   一条,系统作用域为每个适用的已识别系统各一条;
//! - 任务成功:它的 function 声明发布(provides)的事实唤醒等待(requires)
//!   这些事实的自动 function,在同一检材、同一系统上建任务。所需事实都要
//!   有成功的发布者(同一系统,或整个检材);同一作业已有过任务(任何状态)
//!   就不再自动建,自动链因此不会循环,重跑由用户发起;
//! - 失败/取消:不建后续任务。
//!
//! 自动建的任务 `trigger = auto`,离线时重排队;用户手动建的任务
//! `trigger = manually`,离线时判失败。

use super::{
    functions::{FunctionRegistry, SchedFunction},
    model::{NewTask, SchedTaskRecord, SchedTaskStatus, TaskParams},
    store::{InsertOutcome, TaskStore},
};
use crate::core::extensions::plugin::{AnalyzerScope, TaskTrigger};

/// A detected system a system-scope task can target.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemTarget {
    /// `linux@part:2`.
    pub id: String,
    pub family: String,
    pub label: String,
}

/// 建任务的归属上下文:台账检材主键与目标参数。
#[derive(Clone, Debug, PartialEq)]
pub struct TaskTarget {
    pub evidence_id: i64,
    pub params: TaskParams,
}

pub fn new_task_for_function(
    function: &SchedFunction,
    target: &TaskTarget,
    trigger: TaskTrigger,
    parent_tid: Option<i64>,
) -> NewTask {
    NewTask {
        name: function.function.clone(),
        display_name: function.display_name.clone(),
        type_name: function.type_name.clone(),
        evidence_id: target.evidence_id,
        plugin_id: function.plugin_id.clone(),
        analyzer: function.analyzer.clone(),
        trigger,
        parent_tid,
        params: target.params.clone(),
        single_instance: function.single_instance,
        internal: function.internal,
    }
}

/// The params of a system-scope task on `system`.
pub fn system_params(system: &SystemTarget) -> TaskParams {
    TaskParams {
        system_id: Some(system.id.clone()),
        target_label: Some(system.label.clone()),
        ..TaskParams::default()
    }
}

/// The OS family in a system id (`linux@part:2` → `linux`).
fn family_of(system_id: &str) -> &str {
    system_id.split('@').next().unwrap_or_default()
}

/// 检材上线自动建首任务。
pub fn create_first_tasks(
    store: &TaskStore,
    registry: &FunctionRegistry,
    evidence_id: i64,
    systems: &[SystemTarget],
) -> anyhow::Result<Vec<InsertOutcome>> {
    let mut outcomes = Vec::new();
    for function in registry.auto_roots() {
        let targets: Vec<TaskParams> = match function.scope {
            AnalyzerScope::Evidence => vec![TaskParams::default()],
            AnalyzerScope::System => systems
                .iter()
                .filter(|system| function.applies_to_family(&system.family))
                .map(system_params)
                .collect(),
            // Files runs start only when the user selects files.
            AnalyzerScope::Files => Vec::new(),
        };
        for params in targets {
            let target = TaskTarget {
                evidence_id,
                params,
            };
            let new_task = new_task_for_function(function, &target, TaskTrigger::Auto, None);
            let outcome = store.insert_task(&new_task)?;
            tracing::info!(
                "evidence {evidence_id} auto task {} on {:?}: {:?}",
                function.function,
                target.params.target_key(),
                outcome.record().status
            );
            outcomes.push(outcome);
        }
    }
    Ok(outcomes)
}

/// 任务成功后,按它发布的事实建后续任务(见模块说明)。
pub fn check_follow_ups(
    store: &TaskStore,
    registry: &FunctionRegistry,
    finished: &SchedTaskRecord,
) -> anyhow::Result<Vec<InsertOutcome>> {
    if finished.status != SchedTaskStatus::Success {
        return Ok(Vec::new());
    }
    let Some(function) = registry.get(&finished.name) else {
        return Ok(Vec::new());
    };
    if function.provides.is_empty() {
        return Ok(Vec::new());
    }
    let mut outcomes = Vec::new();
    for dependent in registry.waiting_for(&function.provides) {
        let params = match dependent.scope {
            AnalyzerScope::System => match &finished.params.system_id {
                Some(system_id) if dependent.applies_to_family(family_of(system_id)) => {
                    TaskParams {
                        system_id: Some(system_id.clone()),
                        target_label: finished.params.target_label.clone(),
                        ..TaskParams::default()
                    }
                }
                // The facts belong to no system the dependent applies to.
                Some(_) | None => continue,
            },
            AnalyzerScope::Evidence => TaskParams::default(),
            AnalyzerScope::Files => continue,
        };
        let target_key = params.target_key().to_owned();
        if !requirements_met(
            store,
            registry,
            dependent,
            finished.evidence_id,
            &target_key,
        )? {
            continue;
        }
        if store.has_task(&dependent.function, finished.evidence_id, &target_key)? {
            continue;
        }
        let target = TaskTarget {
            evidence_id: finished.evidence_id,
            params,
        };
        let new_task =
            new_task_for_function(dependent, &target, TaskTrigger::Auto, Some(finished.id));
        let outcome = store.insert_task(&new_task)?;
        if let InsertOutcome::Created(record) = &outcome {
            tracing::info!(
                "task {} published {:?} -> follow-up {} (tid {})",
                finished.name,
                function.provides,
                dependent.function,
                record.id
            );
        }
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

/// Every fact `dependent` requires has a provider that succeeded on the
/// same target or on the whole evidence.
fn requirements_met(
    store: &TaskStore,
    registry: &FunctionRegistry,
    dependent: &SchedFunction,
    evidence_id: i64,
    target_key: &str,
) -> anyhow::Result<bool> {
    for fact in &dependent.requires {
        let mut provided = false;
        for provider in registry.providers_of(fact) {
            let targets = store.succeeded_targets(&provider.function, evidence_id)?;
            if targets
                .iter()
                .any(|target| target == target_key || target.is_empty())
            {
                provided = true;
                break;
            }
        }
        if !provided {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::extensions::task_scheduler::{
        functions::tests::{chain_definitions, definition},
        store::tests::{insert_evidence, temp_store},
    };

    #[test]
    fn evidence_online_creates_auto_roots_per_target() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let mut definitions = chain_definitions();
        definitions.push(definition(
            r#"
id = "org.test.system"
name = "System"
version = "1"
protocol = 2
library = "test"
[[analyzer]]
id = "base"
name = "Base"
scope = "system"
platforms = ["linux"]
trigger = "auto"
"#,
        ));
        let registry = FunctionRegistry::from_definitions(&definitions);
        let systems = [
            SystemTarget {
                id: "linux@part:2".to_owned(),
                family: "linux".to_owned(),
                label: "Kali · part:2".to_owned(),
            },
            SystemTarget {
                id: "windows@part:3".to_owned(),
                family: "windows".to_owned(),
                label: "Windows · part:3".to_owned(),
            },
        ];
        let outcomes =
            create_first_tasks(&store, &registry, evidence_id, &systems).expect("first tasks");
        let created: Vec<(String, String)> = outcomes
            .iter()
            .map(|outcome| {
                let record = outcome.record();
                (record.name.clone(), record.params.target_key().to_owned())
            })
            .collect();
        assert_eq!(
            created,
            vec![
                ("org.test.first/stage_one".to_owned(), String::new()),
                ("org.test.system/base".to_owned(), "linux@part:2".to_owned()),
            ]
        );
        assert!(outcomes
            .iter()
            .all(|outcome| outcome.record().info.trigger == TaskTrigger::Auto));

        // Coming online again hits the single-instance tasks.
        let again =
            create_first_tasks(&store, &registry, evidence_id, &systems).expect("first tasks");
        assert!(again
            .iter()
            .all(|outcome| matches!(outcome, InsertOutcome::Existing(_))));
    }

    #[test]
    fn published_facts_create_follow_ups_once() {
        let (_dir, store) = temp_store();
        let evidence_id = insert_evidence(&store, "image-1");
        let registry = FunctionRegistry::from_definitions(&chain_definitions());
        let mut first = create_first_tasks(&store, &registry, evidence_id, &[])
            .expect("first")
            .remove(0)
            .into_record();
        first = store
            .update_status(first.id, SchedTaskStatus::Failed, Some("x"))
            .expect("fail")
            .expect("row");
        assert!(check_follow_ups(&store, &registry, &first)
            .expect("follow-ups")
            .is_empty());

        first = store
            .update_status(first.id, SchedTaskStatus::Success, None)
            .expect("success")
            .expect("row");
        let next = check_follow_ups(&store, &registry, &first).expect("follow-ups");
        assert_eq!(next.len(), 1);
        let InsertOutcome::Created(second) = &next[0] else {
            panic!("the follow-up must be created");
        };
        assert_eq!(second.name, "org.test.second/stage_two");
        assert_eq!(second.info.parent_tid, Some(first.id));
        assert_eq!(second.info.trigger, TaskTrigger::Auto);
        assert!(second.internal);
        assert_eq!(second.evidence_id, evidence_id);

        // The follow-up is not created twice, even once it has finished.
        store
            .update_status(second.id, SchedTaskStatus::Success, None)
            .expect("second done");
        assert!(check_follow_ups(&store, &registry, &first)
            .expect("again")
            .is_empty());
    }
}
