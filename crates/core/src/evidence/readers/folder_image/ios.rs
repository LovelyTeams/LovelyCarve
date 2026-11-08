//! iOS logical tree projection over the upstream read-only backup reader.
use ios_backup_core::{Backup, Credentials, Error, FileKind, Password};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IosBackupInfo {
    pub locked: bool,
    pub declared_encrypted: bool,
    pub observed_encrypted: Option<bool>,
    pub device_name: Option<String>,
    pub product_type: Option<String>,
    pub product_version: Option<String>,
    pub serial_number: Option<String>,
    pub unique_identifier: Option<String>,
    pub snapshot_state: Option<String>,
    pub file_count: Option<usize>,
    pub unreadable_rows: Option<usize>,
    pub unresolved_sizes: usize,
}

#[derive(Debug)]
pub(super) struct IosBackup {
    pub info: IosBackupInfo,
    reader: Option<Backup>,
}

impl FolderImage {
    /// Detect only a selected backup root; ordinary folders keep their host tree.
    pub(super) fn detect_ios(root: &Path) -> Result<Option<Self>> {
        if !root.join("Manifest.plist").is_file() || !root.join("Manifest.db").is_file() {
            return Ok(None);
        }
        match Backup::open(root) {
            Ok(backup) => Self::from_ios(backup, &CancellationToken::new()).map(Some),
            Err(Error::PasswordRequired) => {
                let metadata = ios_backup_core::metadata::Manifest::read(root)?.metadata;
                let mut image = Self::open_host(root)?;
                image.ios_backup = Some(IosBackup {
                    info: backup_info(&metadata, true, None, None, None),
                    reader: None,
                });
                Ok(Some(image))
            }
            Err(error) => Err(error).context("无法读取 iOS 备份清单"),
        }
    }

    /// Passwords are consumed by the zeroizing upstream credential type.
    pub fn open_ios(root: &Path, password: Password, cancel: &CancellationToken) -> Result<Self> {
        if cancel.is_cancelled() {
            bail!("iOS backup unlock cancelled");
        }
        let manifest = ios_backup_core::metadata::Manifest::read(root)?;
        if let Some(keybag) = manifest.keybag {
            let keybag = ios_backup_core::keybag::KeyBag::parse(&keybag)?;
            // Bound attacker-controlled PBKDF2 work without changing the key.
            if keybag.iterations > 20_000_000
                || keybag
                    .double_protection_iterations
                    .is_some_and(|rounds| rounds > 20_000_000)
            {
                bail!("iOS 备份密钥派生参数超过支持的 2000 万轮预算");
            }
        }
        let credentials = Credentials::password(password);
        let backup = Backup::open_with(root, &credentials).context("iOS 备份解锁失败")?;
        Self::from_ios(backup, cancel)
    }

    fn from_ios(backup: Backup, cancel: &CancellationToken) -> Result<Self> {
        if cancel.is_cancelled() {
            bail!("iOS backup unlock cancelled");
        }
        let root = backup.root().to_path_buf();
        let mut info = backup_info(
            backup.metadata(),
            false,
            Some(backup.is_encrypted()),
            Some(backup.files().len()),
            Some(backup.unreadable_rows()),
        );
        let mut entries = BTreeMap::new();
        entries.insert(String::new(), directory(String::new()));
        // Keep control files available for independent review, without mixing
        // hashed content blobs into the reconstructed device tree.
        entries.insert("__backup".into(), directory("__backup".into()));
        for name in [
            "Manifest.plist",
            "Manifest.db",
            "Info.plist",
            "Status.plist",
        ] {
            let path = root.join(name);
            if let Ok(metadata) = fs::metadata(&path) {
                let rel = format!("__backup/{name}");
                entries.insert(
                    rel.clone(),
                    FolderEntry {
                        rel_path: rel,
                        kind: FolderEntryKind::File,
                        size: metadata.len(),
                        modified: metadata.modified().ok(),
                        created: metadata.created().ok(),
                        accessed: metadata.accessed().ok(),
                        status_changed: None,
                        inode: None,
                        mode: None,
                        link_target: None,
                        source: Some(FolderEntrySource::Host(path)),
                    },
                );
            }
        }
        let mut total_size = 0u64;
        for (index, file) in backup.files().iter().enumerate() {
            if cancel.is_cancelled() {
                bail!("iOS backup unlock cancelled");
            }
            ensure_safe_path(&file.domain, &file.relative_path)?;
            let rel = if file.relative_path.is_empty() {
                file.domain.clone()
            } else {
                format!("{}/{}", file.domain, file.relative_path)
            };
            let mut parent = rel.as_str();
            while let Some((prefix, _)) = parent.rsplit_once('/') {
                if let Some(existing) = entries.get(prefix) {
                    if existing.kind != FolderEntryKind::Dir {
                        bail!("iOS 清单路径的父节点不是目录：{prefix}");
                    }
                } else {
                    entries.insert(prefix.to_owned(), directory(prefix.to_owned()));
                }
                parent = prefix;
            }
            let kind = match file.kind {
                FileKind::Directory => FolderEntryKind::Dir,
                FileKind::Symlink => FolderEntryKind::Symlink,
                FileKind::File | FileKind::Unknown(_) => FolderEntryKind::File,
                _ => bail!("unsupported iOS manifest file kind"),
            };
            if entries
                .get(&rel)
                .is_some_and(|existing| existing.source.is_some() || kind != FolderEntryKind::Dir)
            {
                bail!("iOS 清单存在重复或冲突路径：{rel}");
            }
            let size = if file.kind.has_content() {
                match backup.content_size(file) {
                    Ok(size) => size,
                    Err(error) => {
                        info.unresolved_sizes += 1;
                        tracing::warn!("iOS file size unavailable for {rel}: {error}");
                        0
                    }
                }
            } else {
                0
            };
            total_size = total_size
                .checked_add(size)
                .context("iOS backup size overflow")?;
            entries.insert(
                rel.clone(),
                FolderEntry {
                    rel_path: rel,
                    kind,
                    size,
                    modified: unix_time(file.modified),
                    created: unix_time(file.created),
                    accessed: None,
                    status_changed: unix_time(file.status_changed),
                    inode: file.inode,
                    mode: file.mode,
                    link_target: None,
                    source: Some(FolderEntrySource::Ios(index)),
                },
            );
            if entries.len() > MAX_ENTRIES {
                bail!("iOS 备份目录超过 {MAX_ENTRIES} 个节点，无法完整加载");
            }
        }
        Ok(Self {
            label: info
                .device_name
                .clone()
                .unwrap_or_else(|| "iOS 备份".into()),
            root,
            entries,
            total_size,
            bak_archives: Vec::new(),
            ios_backup: Some(IosBackup {
                info,
                reader: Some(backup),
            }),
        })
    }

    pub fn ios_info(&self) -> Option<&IosBackupInfo> {
        self.ios_backup.as_ref().map(|backup| &backup.info)
    }

    /// Native upstream observations. Absence of an app domain remains unrated.
    pub fn audit_ios(&self, cancel: &CancellationToken) -> Result<serde_json::Value> {
        let reader = self.ios_reader()?;
        if cancel.is_cancelled() {
            bail!("iOS backup audit cancelled");
        }
        let findings = ios_backup_forensic::audit_with_cancel(reader, &|| cancel.is_cancelled())
            .context("iOS backup audit cancelled")?;
        Ok(serde_json::to_value(findings)?)
    }

    fn ios_reader(&self) -> Result<&Backup> {
        self.ios_backup
            .as_ref()
            .and_then(|backup| backup.reader.as_ref())
            .context("iOS 备份尚未解锁")
    }

    pub(super) fn read_ios(&self, index: usize, max_bytes: usize) -> Result<Vec<u8>> {
        let reader = self.ios_reader()?;
        let file = reader
            .files()
            .get(index)
            .context("iOS file index out of range")?;
        Ok(reader.read_range(file, 0, max_bytes)?)
    }

    /// Direct reads for reconstructed backup files. Other folder entries keep
    /// their existing stream fallback.
    pub fn read_ios_range(&self, rel: &str, offset: u64, length: usize) -> Result<Option<Vec<u8>>> {
        let entry = self.lookup(rel).context("folder entry not found")?;
        let Some(FolderEntrySource::Ios(index)) = entry.source.as_ref() else {
            return Ok(None);
        };
        let reader = self.ios_reader()?;
        let file = reader
            .files()
            .get(*index)
            .context("iOS file index out of range")?;
        Ok(Some(reader.read_range(file, offset, length)?))
    }

    pub(super) fn stream_ios<F>(
        &self,
        index: usize,
        chunk_size: usize,
        mut visitor: F,
    ) -> Result<u64>
    where
        F: FnMut(&[u8]) -> Result<()>,
    {
        let reader = self.ios_reader()?;
        let file = reader
            .files()
            .get(index)
            .context("iOS file index out of range")?;
        Ok(reader.stream(file, chunk_size, |chunk| {
            visitor(chunk).map_err(std::io::Error::other)?;
            Ok(true)
        })?)
    }
}

fn backup_info(
    metadata: &ios_backup_core::BackupMetadata,
    locked: bool,
    observed_encrypted: Option<bool>,
    file_count: Option<usize>,
    unreadable_rows: Option<usize>,
) -> IosBackupInfo {
    IosBackupInfo {
        locked,
        declared_encrypted: metadata.is_encrypted,
        observed_encrypted,
        device_name: metadata.device_name.clone(),
        product_type: metadata.product_type.clone(),
        product_version: metadata.product_version.clone(),
        serial_number: metadata.serial_number.clone(),
        unique_identifier: metadata.unique_identifier.clone(),
        snapshot_state: metadata.snapshot_state.clone(),
        file_count,
        unreadable_rows,
        unresolved_sizes: 0,
    }
}

pub(super) fn ensure_safe_path(domain: &str, relative: &str) -> Result<()> {
    if domain.is_empty()
        || domain == "__backup"
        || domain.contains(['/', '\\', ':'])
        || matches!(domain, "." | "..")
    {
        bail!("unsafe iOS domain: {domain}");
    }
    if !relative.is_empty()
        && relative.split('/').any(|part| {
            part.is_empty() || matches!(part, "." | "..") || part.contains(['\\', ':', '\0'])
        })
    {
        bail!("unsafe iOS relative path: {relative}");
    }
    Ok(())
}

fn directory(rel_path: String) -> FolderEntry {
    FolderEntry {
        rel_path,
        kind: FolderEntryKind::Dir,
        size: 0,
        modified: None,
        created: None,
        accessed: None,
        status_changed: None,
        inode: None,
        mode: None,
        link_target: None,
        source: None,
    }
}

fn unix_time(seconds: Option<i64>) -> Option<SystemTime> {
    let seconds = seconds?;
    if seconds >= 0 {
        UNIX_EPOCH.checked_add(Duration::from_secs(seconds as u64))
    } else {
        UNIX_EPOCH.checked_sub(Duration::from_secs(seconds.unsigned_abs()))
    }
}
