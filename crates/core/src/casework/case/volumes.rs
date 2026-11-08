//! Snapshot/index publication, retirement and in-memory coverage.
use super::*;

impl CaseState {
    // ----- volumes ----------------------------------------------------------

    pub fn volumes(&self) -> &[VolumeRecord] {
        &self.volumes
    }

    pub fn volume(&self, image_id: &str, volume_key: &str) -> Option<&VolumeRecord> {
        self.volumes
            .iter()
            .find(|record| record.image_id == image_id && record.volume_key == volume_key)
    }

    pub fn snapshot_refs(&self) -> Vec<SnapshotRef> {
        self.volumes.iter().map(snapshot_ref).collect()
    }

    pub fn volume_dir(&self, image_id: &str, volume_key: &str) -> PathBuf {
        case_paths::volume_dir(&self.dir, image_id, volume_key)
    }

    /// 记录里快照文件的路径。
    pub fn snapshot_path(&self, record: &VolumeRecord) -> PathBuf {
        self.volume_dir(&record.image_id, &record.volume_key)
            .join(&record.snapshot_file)
    }

    /// 记录里当前服务的索引文件路径。
    pub fn index_path(&self, record: &VolumeRecord) -> Option<PathBuf> {
        record.index_file.as_ref().map(|index_file| {
            self.volume_dir(&record.image_id, &record.volume_key)
                .join(index_file)
        })
    }

    pub fn staging_dir(&self) -> PathBuf {
        case_paths::staging_dir(&self.dir)
    }

    pub(super) fn cache_volume(&mut self, record: VolumeRecord) {
        match self.volumes.iter_mut().find(|existing| {
            existing.image_id == record.image_id && existing.volume_key == record.volume_key
        }) {
            Some(existing) => *existing = record,
            None => self.volumes.push(record),
        }
    }

    pub(super) fn retire_volume_files(&self, image_id: &str, volume_key: &str, files: &[String]) {
        let directory = self.volume_dir(image_id, volume_key);
        for file in files {
            remove_file_quietly(&directory.join(file), "retired volume file");
        }
    }

    /// 发布一份走树完成的快照:暂存的 `.dirz` 移入卷目录,登记台账,回收被
    /// 替换的旧文件。`publication.snapshot_file` 由本方法填写。
    pub fn publish_snapshot(
        &mut self,
        staged: &Path,
        mut publication: SnapshotPublication,
    ) -> anyhow::Result<SnapshotPublish> {
        if !self.has_evidence(&publication.image_id) {
            remove_file_quietly(staged, "discarded snapshot");
            return Ok(SnapshotPublish::Discarded);
        }
        let directory = self.volume_dir(&publication.image_id, &publication.volume_key);
        std::fs::create_dir_all(&directory)
            .with_context(|| format!("create volume directory {}", directory.display()))?;
        let file_name = case_paths::snapshot_file_name(&case_paths::new_token());
        let target = directory.join(&file_name);
        std::fs::rename(staged, &target).with_context(|| {
            format!(
                "move snapshot {} into {}",
                staged.display(),
                target.display()
            )
        })?;
        publication.snapshot_file = file_name;
        let tx = case_ledger::write_transaction(&self.ledger)?;
        let outcome = ledger_volumes::publish_snapshot(&tx, &publication);
        let outcome = match outcome {
            Ok(outcome) => {
                tx.commit()?;
                outcome
            }
            Err(error) => {
                drop(tx);
                remove_file_quietly(&target, "unpublished snapshot");
                return Err(error);
            }
        };
        match outcome {
            SnapshotOutcome::Published { record, retired } => {
                self.retire_volume_files(&record.image_id, &record.volume_key, &retired);
                self.cache_volume((*record).clone());
                Ok(SnapshotPublish::Published(*record))
            }
            SnapshotOutcome::Superseded | SnapshotOutcome::EvidenceGone => {
                remove_file_quietly(&target, "superseded snapshot");
                Ok(SnapshotPublish::Discarded)
            }
        }
    }

    /// 发布由 `built_at` 那份快照导入的索引文件;依据的快照已不是当前快照时
    /// 删除暂存文件并返回 `false`。
    pub fn publish_index(
        &mut self,
        image_id: &str,
        volume_key: &str,
        built_at: &str,
        staged: &Path,
    ) -> anyhow::Result<bool> {
        let current = self
            .volume(image_id, volume_key)
            .is_some_and(|record| record.built_at == built_at);
        if !current {
            remove_file_quietly(staged, "stale index");
            return Ok(false);
        }
        let directory = self.volume_dir(image_id, volume_key);
        let file_name = case_paths::index_file_name(&case_paths::new_token());
        let target = directory.join(&file_name);
        std::fs::rename(staged, &target).with_context(|| {
            format!("move index {} into {}", staged.display(), target.display())
        })?;
        let tx = case_ledger::write_transaction(&self.ledger)?;
        let outcome =
            match ledger_volumes::publish_index(&tx, image_id, volume_key, built_at, &file_name) {
                Ok(outcome) => {
                    tx.commit()?;
                    outcome
                }
                Err(error) => {
                    drop(tx);
                    remove_file_quietly(&target, "unpublished index");
                    return Err(error);
                }
            };
        match outcome {
            IndexOutcome::Published { record, retired } => {
                self.retire_volume_files(image_id, volume_key, retired.as_slice());
                self.cache_volume(*record);
                // SQLite 现在是该卷的浏览来源,不再常驻内存快照体。
                self.remove_snapshot_body(image_id, volume_key);
                Ok(true)
            }
            IndexOutcome::Stale => {
                remove_file_quietly(&target, "stale index");
                Ok(false)
            }
        }
    }

    /// 删除一个卷的发布记录与它的全部文件。
    pub fn drop_volume(&mut self, image_id: &str, volume_key: &str) -> anyhow::Result<()> {
        let tx = case_ledger::write_transaction(&self.ledger)?;
        ledger_volumes::delete(&tx, image_id, volume_key)?;
        tx.commit()?;
        self.volumes
            .retain(|record| !(record.image_id == image_id && record.volume_key == volume_key));
        self.remove_snapshot_body(image_id, volume_key);
        let directory = self.volume_dir(image_id, volume_key);
        if let Err(error) = case_paths::remove_dir_if_exists(&directory) {
            tracing::debug!(
                "case: remove volume directory {} deferred: {error}",
                directory.display()
            );
        }
        Ok(())
    }

    // ----- in-memory snapshot bodies (fallback when no index) -------------

    /// 安装一份内存快照体(索引建立失败时的浏览回退)。
    pub fn install_snapshot(&mut self, snapshot: Arc<VolumeSnapshot>) {
        let header = &snapshot.header;
        let key = (header.image_id.clone(), header.volume_key.clone());
        self.snapshots.insert(key, snapshot);
    }

    pub fn remove_snapshot_body(&mut self, image_id: &str, volume_key: &str) {
        self.snapshots
            .remove(&(image_id.to_string(), volume_key.to_string()));
    }

    /// 覆盖 `parent_id` 的内存快照体。
    pub fn covering_snapshot(
        &self,
        image_id: &str,
        parent_id: &str,
    ) -> Option<Arc<VolumeSnapshot>> {
        self.snapshots
            .iter()
            .find(|((img, _), snapshot)| img == image_id && snapshot.covers(parent_id))
            .map(|(_, snapshot)| Arc::clone(snapshot))
    }

    /// 内存快照体记录的 `parent_id` 子项数;走过的空目录为 `Some(0)`。
    pub fn snapshot_child_count(&self, image_id: &str, parent_id: &str) -> Option<usize> {
        self.snapshots
            .iter()
            .filter(|((img, _), _)| img == image_id)
            .find_map(|(_, snapshot)| snapshot.child_count(parent_id))
    }

    /// `parent_id` 子树是否只有空目录;内存快照体无法回答时为 `None`。
    pub fn snapshot_subtree_empty(&self, image_id: &str, parent_id: &str) -> Option<bool> {
        self.snapshots
            .iter()
            .filter(|((img, _), _)| img == image_id)
            .find_map(|(_, snapshot)| snapshot.subtree_has_file(parent_id))
            .map(|has_file| !has_file)
    }
}
