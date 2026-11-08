//! Read-only VMDK adapter using SecurityRonin/vmdk-forensic's Rust reader.
//! Source snapshots include every opened descriptor, extent and parent layer.

use std::collections::HashSet;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};

use anyhow::{ensure, Context};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VmdkFileSnapshot {
    pub path: PathBuf,
    pub len: u64,
    pub modified_unix_ms: Option<i64>,
    pub created_unix_ms: Option<i64>,
}

pub struct VmdkHandle {
    layers: Vec<vmdk::VmdkFileReader>,
    media_size: u64,
    grain_boundary: u64,
    file_snapshots: Vec<VmdkFileSnapshot>,
}

impl std::fmt::Debug for VmdkHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VmdkHandle")
            .field("media_size", &self.media_size)
            .field("layers", &self.layers.len())
            .field("file_snapshots", &self.file_snapshots)
            .finish()
    }
}

impl VmdkHandle {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let mut layers: Vec<vmdk::VmdkFileReader> = Vec::new();
        let mut file_snapshots = Vec::new();
        let mut visited_layers = HashSet::new();
        let mut visited_files = HashSet::new();
        let mut current = path.to_path_buf();
        loop {
            ensure!(layers.len() < 64, "VMDK parent chain exceeds 64 layers");
            let canonical = current
                .canonicalize()
                .with_context(|| format!("locate VMDK layer {}", current.display()))?;
            ensure!(
                visited_layers.insert(canonical.clone()),
                "VMDK parent chain contains a cycle"
            );
            // Enumerate before opening to capture the same evidence set's stat facts.
            let mut paths = vec![canonical.clone()];
            paths.extend(
                vmdk::VmdkFileReader::extent_dependencies(&canonical)
                    .context("enumerate VMDK extent dependencies")?,
            );
            for path in paths {
                let path = path
                    .canonicalize()
                    .with_context(|| format!("locate VMDK extent {}", path.display()))?;
                if !visited_files.insert(path.clone()) {
                    continue;
                }
                let metadata = path
                    .metadata()
                    .with_context(|| format!("stat VMDK source {}", path.display()))?;
                file_snapshots.push(VmdkFileSnapshot {
                    path,
                    len: metadata.len(),
                    modified_unix_ms: metadata.modified().ok().and_then(unix_ms),
                    created_unix_ms: metadata.created().ok().and_then(unix_ms),
                });
            }
            let reader = vmdk::VmdkFileReader::open_path(&canonical)
                .with_context(|| format!("open VMDK layer {}", canonical.display()))?;
            ensure!(
                reader.virtual_disk_size() > 0,
                "VMDK has no addressable media"
            );
            if let Some(child) = layers.last() {
                ensure!(
                    child.parent_cid() == reader.cid(),
                    "VMDK parent CID mismatch"
                );
                ensure!(
                    child.virtual_disk_size() == reader.virtual_disk_size(),
                    "VMDK parent capacity mismatch"
                );
            }
            let parent_cid = reader.parent_cid();
            let parent = if parent_cid == u32::MAX {
                None
            } else {
                // The upstream multi-sparse path erases allocation provenance.
                // Refuse such delta layers instead of hiding parent data with zeroes.
                let info = reader.info();
                ensure!(
                    !matches!(
                        info.disk_type.as_str(),
                        "twoGbMaxExtentSparse" | "vmfsSparse" | "vmfsThin" | "custom"
                    ),
                    "VMDK multi-extent delta layers are not supported; use a consolidated image"
                );
                let hint = reader
                    .descriptor_text()
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once('=')?;
                        (name.trim() == "parentFileNameHint")
                            .then(|| value.trim().trim_matches('"'))
                    })
                    .filter(|hint| !hint.is_empty())
                    .context("VMDK parentCID requires parentFileNameHint")?;
                let relative = Path::new(hint);
                ensure!(
                    !relative.components().any(|component| matches!(
                        component,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )),
                    "VMDK parent must be inside the image directory: {hint}"
                );
                Some(
                    canonical
                        .parent()
                        .context("VMDK layer has no directory")?
                        .join(relative),
                )
            };
            layers.push(reader);
            match parent {
                Some(parent) => current = parent,
                None => break,
            }
        }
        let media_size = layers
            .first()
            .context("VMDK has no layers")?
            .virtual_disk_size();
        let grain_boundary = layers
            .iter()
            .map(|layer| layer.info().grain_size_bytes)
            .filter(|size| *size > 0)
            .reduce(|mut left, mut right| {
                while right != 0 {
                    (left, right) = (right, left % right);
                }
                left
            })
            .unwrap_or(media_size);
        Ok(Self {
            layers,
            media_size,
            grain_boundary,
            file_snapshots,
        })
    }

    pub fn media_size(&self) -> u64 {
        self.media_size
    }
    pub fn file_snapshots(&self) -> &[VmdkFileSnapshot] {
        &self.file_snapshots
    }
    pub fn files_on_disk_bytes(&self) -> Option<u64> {
        self.file_snapshots
            .iter()
            .try_fold(0u64, |sum, entry| sum.checked_add(entry.len))
    }

    pub fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> anyhow::Result<usize> {
        let available = self
            .media_size
            .saturating_sub(offset)
            .min(buffer.len() as u64) as usize;
        let last = self.layers.len().saturating_sub(1);
        let mut filled = 0;
        while filled < available {
            let position = offset + filled as u64;
            let count = (self.grain_boundary - position % self.grain_boundary)
                .min((available - filled) as u64) as usize;
            for (index, layer) in self.layers.iter_mut().enumerate() {
                if index == last || layer.has_layer_data(position / 512)? {
                    layer.seek(SeekFrom::Start(position))?;
                    // Unexpected EOF must reach the application's read-fault collector.
                    layer
                        .read_exact(&mut buffer[filled..filled + count])
                        .with_context(|| {
                            format!("VMDK read at byte {position} in layer {index}")
                        })?;
                    break;
                }
            }
            filled += count;
        }
        Ok(filled)
    }
}

fn unix_ms(time: std::time::SystemTime) -> Option<i64> {
    time.duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
}

#[cfg(test)]
#[path = "vmdk_image_tests.rs"]
mod tests;
