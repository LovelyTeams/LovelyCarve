//! Pure Rust VHD/VHDX readers sharing the decoded-image random-read interface.
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path};

use anyhow::{ensure, Context};

use crate::core::common::{error::Failure, evidence_path::evidence_path_key};
use crate::core::evidence::image::SourceFileSnapshot;

const MAX_VHDX_LAYERS: usize = 32;

enum Reader {
    Vhd(vhd::VhdReader),
    Vhdx(vhdx::VhdxReader),
}

pub struct VhdHandle {
    reader: Reader,
    media_size: u64,
    bytes_per_sector: usize,
    file_snapshots: Vec<SourceFileSnapshot>,
}

impl std::fmt::Debug for VhdHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VhdHandle")
            .field("media_size", &self.media_size)
            .field("bytes_per_sector", &self.bytes_per_sector)
            .finish_non_exhaustive()
    }
}

impl VhdHandle {
    pub fn open_vhd(path: &Path) -> anyhow::Result<Self> {
        let reader = vhd::VhdReader::open(path)
            .with_context(|| format!("failed to open VHD image: {}", path.display()))?;
        Ok(Self {
            media_size: reader.virtual_disk_size(),
            bytes_per_sector: 512,
            reader: Reader::Vhd(reader),
            file_snapshots: Vec::new(),
        })
    }

    pub fn open_vhdx(path: &Path) -> anyhow::Result<Self> {
        let canonical = path
            .canonicalize()
            .with_context(|| format!("locate VHDX image {}", path.display()))?;
        let root = canonical.parent().context("VHDX image has no directory")?;
        let mut file_snapshots = Vec::new();
        let reader = open_vhdx_layer(
            path,
            &canonical,
            root,
            &mut HashSet::new(),
            &mut file_snapshots,
        )
        .with_context(|| format!("failed to open VHDX image: {}", path.display()))?;
        Ok(Self {
            media_size: reader.virtual_disk_size(),
            bytes_per_sector: reader.logical_sector_size() as usize,
            reader: Reader::Vhdx(reader),
            file_snapshots,
        })
    }

    pub fn media_size(&self) -> u64 {
        self.media_size
    }
    pub fn bytes_per_sector(&self) -> usize {
        self.bytes_per_sector
    }

    pub(in crate::core::evidence) fn file_snapshots(&self) -> &[SourceFileSnapshot] {
        &self.file_snapshots
    }

    pub(in crate::core::evidence) fn files_on_disk_bytes(&self) -> Option<u64> {
        self.file_snapshots
            .iter()
            .try_fold(0u64, |total, snapshot| total.checked_add(snapshot.len))
    }

    pub fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> anyhow::Result<usize> {
        let length = self
            .media_size
            .saturating_sub(offset)
            .min(buffer.len() as u64) as usize;
        if length == 0 {
            return Ok(0);
        }
        match &mut self.reader {
            Reader::Vhd(reader) => {
                reader.seek(SeekFrom::Start(offset))?;
                reader.read_exact(&mut buffer[..length])?;
            }
            Reader::Vhdx(reader) => {
                reader.seek(SeekFrom::Start(offset))?;
                reader.read_exact(&mut buffer[..length])?;
            }
        }
        Ok(length)
    }
}

fn open_vhdx_layer(
    path: &Path,
    canonical: &Path,
    root: &Path,
    visited: &mut HashSet<String>,
    snapshots: &mut Vec<SourceFileSnapshot>,
) -> anyhow::Result<vhdx::VhdxReader> {
    ensure!(
        snapshots.len() < MAX_VHDX_LAYERS,
        Failure::resource_limit(format!(
            "VHDX parent chain exceeds {MAX_VHDX_LAYERS} layers"
        ))
    );
    ensure!(
        visited.insert(evidence_path_key(&canonical.to_string_lossy())),
        Failure::corrupt("VHDX parent chain contains a cycle")
    );
    let file = File::open(canonical)?;
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file(),
        "VHDX layer is not a file: {}",
        path.display()
    );
    snapshots.push(SourceFileSnapshot::from_metadata(path, &metadata));
    vhdx::VhdxReader::from_backing_resolving_parent(vhdx::Backing::File(file), |relative| {
        resolve_vhdx_parent(canonical, root, relative, visited, snapshots)
            .map_err(|error| vhdx::VhdxError::ParentResolution(error.into_boxed_dyn_error()))
    })
    .map_err(|error| match error {
        vhdx::VhdxError::ParentResolution(source) => anyhow::Error::from_boxed(source),
        vhdx::VhdxError::ParentIdMismatch | vhdx::VhdxError::ParentGeometryMismatch => {
            anyhow::Error::new(error)
                .context(Failure::corrupt("VHDX parent does not match the child"))
        }
        error => error.into(),
    })
    .with_context(|| format!("open VHDX layer {}", path.display()))
}

fn resolve_vhdx_parent(
    child: &Path,
    root: &Path,
    relative: Option<&str>,
    visited: &mut HashSet<String>,
    snapshots: &mut Vec<SourceFileSnapshot>,
) -> anyhow::Result<vhdx::VhdxReader> {
    let relative = relative.context(Failure::unsupported(
        "VHDX parent requires a relative_path locator",
    ))?;
    let relative = relative.replace('\\', "/");
    let path = Path::new(&relative);
    ensure!(
        !relative.contains(':')
            && path
                .components()
                .all(|part| matches!(part, Component::Normal(_) | Component::CurDir)),
        Failure::unsupported(format!(
            "VHDX parent must be inside the image directory: {relative}"
        ))
    );
    let path = child
        .parent()
        .context("VHDX layer has no directory")?
        .join(path);
    let canonical = path
        .canonicalize()
        .with_context(|| format!("locate VHDX parent {}", path.display()))?;
    ensure!(
        canonical.starts_with(root),
        Failure::unsupported(format!(
            "VHDX parent is outside the image directory: {}",
            path.display()
        ))
    );
    ensure!(
        canonical.is_file(),
        "VHDX parent is not a file: {}",
        path.display()
    );
    open_vhdx_layer(&path, &canonical, root, visited, snapshots)
}

#[cfg(test)]
#[path = "vhd_image_tests.rs"]
mod tests;
