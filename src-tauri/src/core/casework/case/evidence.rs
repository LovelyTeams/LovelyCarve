//! Journaled evidence registration, removal and runtime lookup.
use super::*;

impl CaseState {
    // ----- evidence ---------------------------------------------------------

    pub fn evidence_refs(&self) -> Vec<EvidenceRef> {
        self.evidence
            .iter()
            .filter_map(evidence_ref_from_record)
            .collect()
    }

    pub fn has_evidence(&self, image_id: &str) -> bool {
        self.evidence.iter().any(|record| record.uid == image_id)
    }

    pub fn evidence_ref(&self, image_id: &str) -> Option<EvidenceRef> {
        self.evidence
            .iter()
            .find(|record| record.uid == image_id)
            .and_then(evidence_ref_from_record)
    }

    /// 某个来源路径登记成的检材 id。
    pub fn evidence_image_id_for_path(&self, path: &str) -> Option<String> {
        let path_key = evidence_path_key(path);
        self.evidence
            .iter()
            .find(|record| evidence_path_key(&record.path) == path_key)
            .map(|record| record.uid.clone())
    }

    /// 登记(或刷新)一个检材,以 `image_id` 幂等;同一来源已是另一个检材时
    /// 不登记。
    pub fn register_evidence(
        &mut self,
        evidence: &EvidenceRef,
    ) -> anyhow::Result<EvidenceRegistration> {
        let format = source_format_name(evidence.source_format);
        let path_key = evidence_path_key(&evidence.path);
        let previous = self
            .evidence
            .iter()
            .find(|record| record.uid == evidence.image_id)
            .cloned();
        let detail = serde_json::json!({
            "path": evidence.path,
            "format": format,
            "sizeBytes": evidence.size_bytes,
            "previousPath": previous.as_ref().map(|record| record.path.clone()),
        })
        .to_string();
        let tx = case_ledger::write_transaction(&self.ledger)?;
        let registration = ledger_evidence::register(
            &tx,
            ledger_evidence::NewEvidence {
                uid: &evidence.image_id,
                path: &evidence.path,
                path_key: &path_key,
                format: &format,
                size_bytes: evidence.size_bytes,
                added_at: &evidence.added_at,
            },
        )?;
        let (outcome, record) = match registration {
            Registration::Added(record) => (EvidenceRegistration::Added, Some(record)),
            Registration::Refreshed(record) => (EvidenceRegistration::Known, Some(record)),
            Registration::Unchanged(record) => {
                tx.commit()?;
                self.cache_evidence(record);
                return Ok(EvidenceRegistration::Known);
            }
            Registration::SourceTaken { existing_uid } => {
                tx.commit()?;
                return Ok(EvidenceRegistration::SourceTaken {
                    existing_image_id: existing_uid,
                });
            }
        };
        let action = match outcome {
            EvidenceRegistration::Added => "evidence.add",
            EvidenceRegistration::Known | EvidenceRegistration::SourceTaken { .. } => {
                "evidence.relink"
            }
        };
        case_ledger::touch(&tx)?;
        case_journal::append(
            &tx,
            &self.profile.case_uid,
            Event {
                action,
                evidence: Some(&evidence.image_id),
                detail: &detail,
            },
        )?;
        tx.commit()?;
        self.profile = case_ledger::load_profile(&self.ledger)?;
        if let Some(record) = record {
            std::fs::create_dir_all(case_paths::evidence_dir(&self.dir, &record.uid))
                .with_context(|| format!("create evidence directory for {}", record.uid))?;
            self.cache_evidence(record);
        }
        Ok(outcome)
    }

    pub(super) fn cache_evidence(&mut self, record: EvidenceRecord) {
        match self
            .evidence
            .iter_mut()
            .find(|existing| existing.uid == record.uid)
        {
            Some(existing) => *existing = record,
            None => self.evidence.push(record),
        }
    }

    /// 从案件移除检材并立即删除它的整个检材目录(阻塞)。
    pub fn remove_evidence(&mut self, image_id: &str) -> anyhow::Result<()> {
        self.detach_evidence(image_id)?.run();
        Ok(())
    }

    /// 删除检材的台账行(级联带走卷记录、标签、盘符、读故障与任务)并刷新
    /// 缓存;返回需要在案件锁外执行的目录清理。
    pub fn detach_evidence(&mut self, image_id: &str) -> anyhow::Result<EvidenceRemovalCleanup> {
        let detail = self
            .evidence
            .iter()
            .find(|record| record.uid == image_id)
            .map(|record| {
                serde_json::json!({
                    "path": record.path,
                    "format": record.format,
                    "sizeBytes": record.size_bytes,
                    "addedAt": record.added_at,
                })
                .to_string()
            });
        if let Some(detail) = detail {
            self.journaled(
                Event {
                    action: "evidence.remove",
                    evidence: Some(image_id),
                    detail: &detail,
                },
                |tx| {
                    ledger_evidence::delete(tx, image_id)?;
                    case_ledger::touch(tx)
                },
            )?;
        }
        self.evidence.retain(|record| record.uid != image_id);
        self.volumes.retain(|record| record.image_id != image_id);
        self.snapshots.retain(|(id, _), _| id != image_id);
        Ok(EvidenceRemovalCleanup {
            evidence_dir: case_paths::evidence_dir(&self.dir, image_id),
        })
    }

    /// 自动识别结论回写检材行。检材不在案件中时返回 `Ok(false)`。
    pub fn record_evidence_detection(
        &mut self,
        image_id: &str,
        platform: Option<&str>,
        kind: &str,
        detection_json: &str,
    ) -> anyhow::Result<bool> {
        let tx = case_ledger::write_transaction(&self.ledger)?;
        let updated =
            ledger_evidence::record_detection(&tx, image_id, platform, kind, detection_json)?;
        tx.commit()?;
        match updated {
            Some(record) => {
                self.cache_evidence(record);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// 检验人手动指定检材平台(`None` 清除手动选择,恢复自动识别结论),与审计
    /// 记录同一事务提交。检材不在案件中时返回 `Ok(None)`。
    pub fn set_evidence_platform(
        &mut self,
        image_id: &str,
        platform: Option<&str>,
    ) -> anyhow::Result<Option<EvidenceRef>> {
        let Some(current) = self.evidence.iter().find(|record| record.uid == image_id) else {
            return Ok(None);
        };
        let detail = serde_json::json!({
            "platform": platform,
            "previous": current.manual_platform,
            "detected": current.platform,
        })
        .to_string();
        let record = self.journaled(
            Event {
                action: "evidence.platform",
                evidence: Some(image_id),
                detail: &detail,
            },
            |tx| {
                let record = ledger_evidence::set_manual_platform(tx, image_id, platform)?
                    .context("evidence row vanished")?;
                case_ledger::touch(tx)?;
                Ok(record)
            },
        )?;
        let evidence = evidence_ref_from_record(&record);
        self.cache_evidence(record);
        Ok(evidence)
    }
}
