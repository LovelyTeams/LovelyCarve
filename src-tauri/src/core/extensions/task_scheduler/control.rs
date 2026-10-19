//! 调度器控制面:任务创建、取消、启动、重启、删除与证据上线/下线钩子。
//!
//! 每个操作先校验状态机(`model.rs` 的 `can_*`),再改写台账,最后广播;
//! 对运行中的任务,取消会撤销执行 token 并终止该任务的插件 worker,同一
//! 插件的其他任务不受影响。状态改写只从读到的状态迁出:与同时到达的
//! commit 竞争时,晚到的一方得到"状态已变"的错误,而不是覆盖对方。
//!
//! 同步方法(查询、建任务、证据上线)直接读写台账,调用方应在阻塞线程上
//! 调用;异步方法自行把台账步骤放到阻塞线程池。

use std::sync::Arc;

use super::{
    config::{SchedError, SchedResult},
    dependence::{self, TaskTarget},
    model::{NewTask, SchedTaskRecord, SchedTaskStatus, TaskParams},
    scheduler::{BoundCase, TaskScheduler},
    store::{InsertOutcome, TaskFilter},
};
use crate::core::extensions::plugin::{AnalyzerScope, TaskTrigger};

/// 检材移除前的任务收尾([`TaskScheduler::withdraw_evidence`] 的结果)。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvidenceWithdrawal {
    /// 台账检材主键;检材不在案件中(或没有绑定案件)时为 `None`。
    pub evidence_id: Option<i64>,
    /// 检材的全部任务,删除检材行时随之级联删除。
    pub task_ids: Vec<i64>,
    /// 本次取消的未终结任务数。
    pub cancelled: usize,
}

/// 改写单条任务状态的控制操作。
#[derive(Clone, Copy, Debug)]
enum Control {
    Cancel,
    Start,
}

impl Control {
    fn action(self) -> &'static str {
        match self {
            Self::Cancel => "取消",
            Self::Start => "启动",
        }
    }

    fn target(self) -> SchedTaskStatus {
        match self {
            Self::Cancel => SchedTaskStatus::Cancel,
            Self::Start => SchedTaskStatus::Created,
        }
    }

    fn allows(self, task: &SchedTaskRecord) -> bool {
        match self {
            Self::Cancel => task.status.can_cancel(),
            Self::Start => task.status.can_start(),
        }
    }
}

impl TaskScheduler {
    // ----- queries ------------------------------------------------------------

    pub fn list(&self, filter: &TaskFilter) -> SchedResult<Vec<SchedTaskRecord>> {
        match self.bound() {
            Some(bound) => Ok(bound.store.list(filter)?),
            None => Ok(Vec::new()),
        }
    }

    /// 按 image id 过滤任务;未知检材返回空。
    pub fn list_for_image(
        &self,
        image_id: Option<&str>,
        include_internal: bool,
    ) -> SchedResult<Vec<SchedTaskRecord>> {
        let Some(bound) = self.bound() else {
            return Ok(Vec::new());
        };
        let evidence_id = match image_id {
            Some(image_id) => match bound.store.evidence_id_for_image(image_id)? {
                Some(evidence_id) => Some(evidence_id),
                None => return Ok(Vec::new()),
            },
            None => None,
        };
        Ok(bound.store.list(&TaskFilter {
            evidence_id,
            include_internal,
        })?)
    }

    pub fn get(&self, task_id: i64) -> SchedResult<SchedTaskRecord> {
        let bound = self.require_bound()?;
        bound
            .store
            .get(task_id)?
            .ok_or(SchedError::TaskNotFound(task_id))
    }

    // ----- create -------------------------------------------------------------

    /// 手动建任务。目标与选项按分析器的作用域和选项声明校验;系统作用域的
    /// 系统必须是该检材识别出的、分析器适用的系统。单实例约束命中时返回
    /// 已有任务。阻塞(可能识别检材上的系统),在阻塞线程上调用。
    pub fn create_task(
        &self,
        function: &str,
        image_id: &str,
        mut params: TaskParams,
    ) -> SchedResult<InsertOutcome> {
        let bound = self.require_bound()?;
        let registry = self.registry();
        let function = registry
            .get(function)
            .ok_or_else(|| SchedError::FunctionUnknown(function.to_owned()))?;
        function
            .check_params(&params)
            .map_err(SchedError::InvalidParams)?;
        if function.scope == AnalyzerScope::System {
            let system_id = params.system_id.clone().unwrap_or_default();
            let system = self
                .systems_for(image_id)
                .into_iter()
                .find(|system| system.id == system_id)
                .ok_or_else(|| {
                    SchedError::InvalidParams(format!("检材上没有识别出系统 {system_id}"))
                })?;
            if !function.applies_to_family(&system.family) {
                return Err(SchedError::InvalidParams(format!(
                    "{} 不适用于 {}",
                    function.display_name, system.label
                )));
            }
            params.target_label = Some(system.label);
        }
        if function.scope != AnalyzerScope::System {
            params.system_id = None;
        }
        if function.scope != AnalyzerScope::Files {
            params.node_id = None;
            params.file_name = None;
        }
        let evidence_id = bound
            .store
            .evidence_id_for_image(image_id)?
            .ok_or_else(|| SchedError::EvidenceUnknown(image_id.to_owned()))?;
        let target = TaskTarget {
            evidence_id,
            params,
        };
        let new_task =
            dependence::new_task_for_function(function, &target, TaskTrigger::Manually, None);
        let outcome = self.ordered(|| {
            let outcome = bound.store.insert_task(&new_task);
            if let Ok(InsertOutcome::Created(task)) = &outcome {
                self.emit_task(task);
            }
            outcome
        })?;
        match &outcome {
            InsertOutcome::Created(task) => {
                self.inner.post_actions.on_task_created(task.evidence_id);
            }
            InsertOutcome::Existing(task) => tracing::info!(
                "task {} for evidence {} already exists (tid {})",
                task.name,
                task.evidence_id,
                task.id
            ),
        }
        Ok(outcome)
    }

    // ----- evidence hooks -----------------------------------------------------

    /// 证据上线:为自动触发、不等待事实的 function 建首任务(系统作用域按
    /// 识别出的系统各一条)。阻塞,在阻塞线程上调用。
    pub fn on_evidence_online(&self, image_id: &str) -> SchedResult<Vec<InsertOutcome>> {
        let bound = self.require_bound()?;
        let evidence_id = bound
            .store
            .evidence_id_for_image(image_id)?
            .ok_or_else(|| SchedError::EvidenceUnknown(image_id.to_owned()))?;
        let registry = self.registry();
        // Identifying systems reads the evidence: only when an analyzer needs it.
        let systems = if registry
            .auto_roots()
            .any(|function| function.scope == AnalyzerScope::System)
        {
            self.systems_for(image_id)
        } else {
            Vec::new()
        };
        let outcomes = self.ordered(|| {
            let outcomes =
                dependence::create_first_tasks(&bound.store, &registry, evidence_id, &systems);
            for outcome in outcomes.iter().flatten() {
                if let InsertOutcome::Created(task) = outcome {
                    self.emit_task(task);
                }
            }
            outcomes
        })?;
        for outcome in &outcomes {
            if let InsertOutcome::Created(task) = outcome {
                self.inner.post_actions.on_task_created(task.evidence_id);
            }
        }
        Ok(outcomes)
    }

    /// 检材移除前:取消该检材全部未终结任务,返回它的全部任务。删除检材行
    /// 时台账级联删除这些任务,随后调用 [`Self::forget_evidence`] 广播移除。
    pub async fn withdraw_evidence(&self, image_id: &str) -> SchedResult<EvidenceWithdrawal> {
        let Some(bound) = self.bound() else {
            return Ok(EvidenceWithdrawal::default());
        };
        let image_id = image_id.to_owned();
        let (evidence_id, tasks) = self
            .on_blocking(
                move |_| -> SchedResult<(Option<i64>, Vec<SchedTaskRecord>)> {
                    match bound.store.evidence_id_for_image(&image_id)? {
                        Some(evidence_id) => Ok((
                            Some(evidence_id),
                            bound.store.list_for_evidence(evidence_id)?,
                        )),
                        None => Ok((None, Vec::new())),
                    }
                },
            )
            .await??;
        let mut cancelled = 0;
        for task in &tasks {
            if !task.status.can_cancel() {
                continue;
            }
            match self.cancel(task.id).await {
                Ok(_) => cancelled += 1,
                // Finished or removed meanwhile: nothing left to cancel.
                Err(SchedError::InvalidState { .. } | SchedError::TaskNotFound(_)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(EvidenceWithdrawal {
            evidence_id,
            task_ids: tasks.iter().map(|task| task.id).collect(),
            cancelled,
        })
    }

    /// 检材行已删除(任务随之级联删除):广播任务移除,丢弃收尾动作登记。
    pub fn forget_evidence(&self, withdrawal: &EvidenceWithdrawal) {
        let Some(evidence_id) = withdrawal.evidence_id else {
            return;
        };
        self.inner.post_actions.forget(evidence_id);
        for task_id in &withdrawal.task_ids {
            self.emit(super::model::SchedEvent::TaskRemoved {
                task_id: *task_id,
                evidence_id,
            });
        }
    }

    // ----- control ------------------------------------------------------------

    /// 取消任务:校验状态 → 置 `cancel` → 终止 worker → 完成处理。
    pub async fn cancel(&self, task_id: i64) -> SchedResult<SchedTaskRecord> {
        let bound = self.require_bound()?;
        let _guard = self.inner.tick_lock.lock().await;
        let (task, cancelled) = self
            .on_blocking({
                let bound = Arc::clone(&bound);
                move |scheduler| scheduler.apply_control(&bound, task_id, Control::Cancel)
            })
            .await??;
        self.stop_execution(&bound, &task).await;
        self.on_blocking({
            let bound = Arc::clone(&bound);
            let cancelled = cancelled.clone();
            move |scheduler| scheduler.after_finish(&bound, &cancelled)
        })
        .await?;
        Ok(cancelled)
    }

    /// 启动任务:离线任务(以及旧案件里暂停的任务)重新排队(`created`)。
    pub async fn start(&self, task_id: i64) -> SchedResult<SchedTaskRecord> {
        let bound = self.require_bound()?;
        let _guard = self.inner.tick_lock.lock().await;
        self.on_blocking(move |scheduler| scheduler.start_task(&bound, task_id))
            .await?
    }

    /// 重启任务:终态任务重建一条新任务(`parent_tid` 指向原任务);离线
    /// 任务直接重排队。
    pub async fn restart(&self, task_id: i64) -> SchedResult<SchedTaskRecord> {
        let bound = self.require_bound()?;
        let _guard = self.inner.tick_lock.lock().await;
        self.on_blocking(move |scheduler| scheduler.restart_task(&bound, task_id))
            .await?
    }

    /// 删除终态任务并广播。
    pub async fn remove(&self, task_id: i64) -> SchedResult<SchedTaskRecord> {
        let bound = self.require_bound()?;
        let _guard = self.inner.tick_lock.lock().await;
        self.on_blocking(move |scheduler| scheduler.remove_task(&bound, task_id))
            .await?
    }

    /// 设置检材的收尾动作(台账 `task_followup`),重置执行标记。
    pub fn set_evidence_post_actions(
        &self,
        image_id: &str,
        actions_json: Option<&str>,
    ) -> SchedResult<()> {
        let bound = self.require_bound()?;
        let evidence_id = bound
            .store
            .evidence_id_for_image(image_id)?
            .ok_or_else(|| SchedError::EvidenceUnknown(image_id.to_owned()))?;
        if let Some(text) = actions_json {
            serde_json::from_str::<Vec<super::post_action::PostAction>>(text)
                .map_err(|error| anyhow::anyhow!("post actions JSON invalid: {error}"))?;
        }
        bound
            .store
            .set_evidence_post_actions(evidence_id, actions_json)?;
        Ok(())
    }

    /// 校验并改写状态,返回 `(改写前, 改写后)`。读与写之间若有 commit 先
    /// 改了状态,按新状态报 `InvalidState`。
    fn apply_control(
        &self,
        bound: &BoundCase,
        task_id: i64,
        control: Control,
    ) -> SchedResult<(SchedTaskRecord, SchedTaskRecord)> {
        let publication = self.publication_gate(task_id);
        let _publication = publication.as_ref().map(|gate| gate.lock());
        let task = bound
            .store
            .get(task_id)?
            .ok_or(SchedError::TaskNotFound(task_id))?;
        if !control.allows(&task) {
            return Err(SchedError::InvalidState {
                task_id,
                status: task.status.as_str(),
                action: control.action(),
            });
        }
        let Some(updated) = self.write_task(|| {
            bound
                .store
                .transition(task_id, &[task.status], control.target(), None)
        })?
        else {
            let current = bound
                .store
                .get(task_id)?
                .ok_or(SchedError::TaskNotFound(task_id))?;
            return Err(SchedError::InvalidState {
                task_id,
                status: current.status.as_str(),
                action: control.action(),
            });
        };
        Ok((task, updated))
    }

    fn start_task(&self, bound: &BoundCase, task_id: i64) -> SchedResult<SchedTaskRecord> {
        let (_, started) = self.apply_control(bound, task_id, Control::Start)?;
        Ok(started)
    }

    fn restart_task(&self, bound: &BoundCase, task_id: i64) -> SchedResult<SchedTaskRecord> {
        let task = bound
            .store
            .get(task_id)?
            .ok_or(SchedError::TaskNotFound(task_id))?;
        if !task.status.can_restart() {
            return Err(SchedError::InvalidState {
                task_id,
                status: task.status.as_str(),
                action: "重启",
            });
        }
        if task.status == SchedTaskStatus::Offline {
            return self.start_task(bound, task_id);
        }
        let registry = self.registry();
        let function = registry
            .get(&task.name)
            .ok_or_else(|| SchedError::FunctionUnknown(task.name.clone()))?;
        let new_task = NewTask {
            name: task.name.clone(),
            display_name: function.display_name.clone(),
            type_name: function.type_name.clone(),
            evidence_id: task.evidence_id,
            plugin_id: task.info.plugin_id.clone(),
            analyzer: task.info.analyzer.clone(),
            trigger: task.info.trigger,
            parent_tid: Some(task.id),
            params: task.params.clone(),
            single_instance: function.single_instance,
            internal: function.internal,
        };
        tracing::info!("restarting task {} ({})", task.id, task.name);
        let outcome = self.ordered(|| {
            let outcome = bound.store.insert_task(&new_task);
            if let Ok(InsertOutcome::Created(created)) = &outcome {
                self.emit_task(created);
            }
            outcome
        })?;
        if let InsertOutcome::Created(created) = &outcome {
            self.inner.post_actions.on_task_created(created.evidence_id);
        }
        Ok(outcome.into_record())
    }

    fn remove_task(&self, bound: &BoundCase, task_id: i64) -> SchedResult<SchedTaskRecord> {
        let publication = self.publication_gate(task_id);
        let _publication = publication.as_ref().map(|gate| gate.lock());
        let task = bound
            .store
            .get(task_id)?
            .ok_or(SchedError::TaskNotFound(task_id))?;
        if !task.status.is_terminal() {
            return Err(SchedError::InvalidState {
                task_id,
                status: task.status.as_str(),
                action: "删除",
            });
        }
        let removed = bound
            .store
            .remove(task_id)?
            .ok_or(SchedError::TaskNotFound(task_id))?;
        // 删除任务时同时清理已发布的结果与时间线投影。尽力而为:失败只
        // 留下孤儿运行,不影响任务删除本身。
        if let Err(error) = bound.runner.remove_results(&removed) {
            tracing::warn!(
                "remove analysis results for task {} ({}): {error:#}",
                removed.id,
                removed.name
            );
        }
        if let Err(error) = self
            .inner
            .post_actions
            .on_task_removed(&bound.store, removed.evidence_id)
        {
            tracing::warn!("PostActionWatcher on remove tid {task_id}: {error:#}");
        }
        self.emit(super::model::SchedEvent::TaskRemoved {
            task_id,
            evidence_id: removed.evidence_id,
        });
        Ok(removed)
    }

    /// 撤销执行:撤销 token、移除运行登记、终止该任务的插件实例。没有进程内
    /// 执行(未派发、已离线)时什么也不做。
    async fn stop_execution(&self, bound: &Arc<BoundCase>, task: &SchedTaskRecord) {
        let entry = self.inner.running.lock().remove(&task.id);
        let Some(entry) = entry else {
            return;
        };
        entry.token.cancel();
        bound.runner.terminate_worker(task.id, entry.seq).await;
    }
}
