//! Native VMFS paths retain the partition and file descriptor identities.

use std::{cell::RefCell, io::Read, sync::Arc};

use anyhow::{ensure, Context, Result};
use vmfs_reader::{Error, FileInfo, FileKind, Filesystem, Volume};

use crate::core::common::error::Failure;
use crate::core::evidence::image::{find_partition, DiskImage};
use crate::core::evidence::partitions::{partition_cursor::PartitionCursor, vmfs::same_datastore};
use crate::core::filesystem::fs_dispatch::build_stat;
use crate::core::filesystem::tree::fs_tree::{FsNode, FsStat, Mactb, NodeKind};

pub struct VmfsVolume {
    partition: u8,
    filesystem: RefCell<Filesystem<PartitionCursor>>,
}

pub fn parse_vmfs_id(id: &str) -> Option<(u8, String)> {
    let (partition, path) = id.strip_prefix("vmfs:part:")?.split_once(':')?;
    let path = String::from_utf8(hex::decode(path).ok()?).ok()?;
    let valid = path.is_empty()
        || (path.starts_with('/')
            && !path.contains('\0')
            && path
                .split('/')
                .skip(1)
                .all(|part| !matches!(part, "" | "." | "..")));
    valid.then_some((partition.parse().ok()?, path))
}

pub fn open_vmfs_source(image: &Arc<DiskImage>, partition: u8) -> Result<VmfsVolume> {
    let primary = find_partition(image, &partition.to_string())?;
    let mut source = PartitionCursor::new(Arc::clone(image), primary)?;
    let header = vmfs_reader::probe_member(&mut source).map_err(vmfs_error)?;
    let mut sources = vec![source];
    if let Some(header) = header.filter(|member| member.member_count > 1) {
        for part in image
            .partitions
            .iter()
            .filter(|part| part.id != partition && part.kind == primary.kind)
        {
            let mut candidate = PartitionCursor::new(Arc::clone(image), part)?;
            if vmfs_reader::probe_member(&mut candidate).is_ok_and(|candidate| {
                candidate.is_some_and(|other| same_datastore(&header, &other))
            }) {
                sources.push(candidate);
            }
        }
    }
    let volume = Volume::open_members(sources).map_err(vmfs_error)?;
    let filesystem = Filesystem::open(volume).map_err(vmfs_error)?;
    Ok(VmfsVolume {
        partition,
        filesystem: RefCell::new(filesystem),
    })
}

impl VmfsVolume {
    fn node_id(&self, path: &str) -> String {
        format!("vmfs:part:{}:{}", self.partition, hex::encode(path))
    }

    pub fn root_children(&self) -> Result<Vec<FsNode>> {
        self.children("")
    }

    pub fn children(&self, path: &str) -> Result<Vec<FsNode>> {
        let mut filesystem = self.filesystem.borrow_mut();
        let address = filesystem.find(path).map_err(vmfs_error)?;
        let parent = self.node_id(path);
        let mut nodes = Vec::new();
        for entry in filesystem.entries(address).map_err(vmfs_error)? {
            let info = filesystem.file_info(entry.address).map_err(vmfs_error)?;
            ensure!(
                info.generation == entry.generation,
                Failure::corrupt("VMFS directory entry points to a reused file descriptor")
            );
            let kind = match info.kind {
                FileKind::Directory => NodeKind::Dir,
                FileKind::Symlink => NodeKind::Symlink,
                FileKind::Regular | FileKind::RawDeviceMapping => NodeKind::File,
            };
            let link_target = if info.kind == FileKind::Symlink {
                let bytes = read_range(&mut filesystem, &info, 0, 4096)?;
                ensure!(
                    bytes.len() as u64 == info.size,
                    Failure::corrupt("VMFS symlink target exceeds 4096 bytes")
                );
                Some(String::from_utf8_lossy(&bytes).into_owned())
            } else {
                None
            };
            nodes.push(FsNode {
                id: self.node_id(&format!("{path}/{}", entry.name)),
                name: entry.name,
                parent_id: Some(parent.clone()),
                kind,
                size: if info.kind == FileKind::Directory {
                    0
                } else {
                    info.size
                },
                deleted: false,
                inode: Some(u64::from(info.address)),
                timestamps: Mactb {
                    modified: timestamp(info.modified),
                    accessed: timestamp(info.accessed),
                    created: None,
                    mft_changed: timestamp(info.changed),
                    birth: None,
                },
                link_target,
                has_snapshot: false,
            });
        }
        nodes.sort_by(|a, b| {
            (a.kind != NodeKind::Dir)
                .cmp(&(b.kind != NodeKind::Dir))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(nodes)
    }

    pub fn read_file(&self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        self.read_file_range(path, 0, max_bytes)
    }

    pub fn read_file_range(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        let mut filesystem = self.filesystem.borrow_mut();
        let info = file_info(&mut filesystem, path)?;
        read_range(&mut filesystem, &info, offset, length)
    }

    pub fn stream_file(
        &self,
        path: &str,
        chunk_size: usize,
        mut visit: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<u64> {
        let mut filesystem = self.filesystem.borrow_mut();
        let info = file_info(&mut filesystem, path)?;
        ensure!(
            has_file_bytes(info.kind),
            Failure::unsupported("VMFS entry does not expose file bytes")
        );
        let mut reader = filesystem.open_file(info.address).map_err(vmfs_error)?;
        let mut buffer = vec![0; chunk_size.clamp(4096, 1024 * 1024)];
        let mut offset = 0;
        while offset < info.size {
            let count = (info.size - offset).min(buffer.len() as u64) as usize;
            reader
                .read_exact(&mut buffer[..count])
                .context("VMFS file stream")?;
            visit(&buffer[..count])?;
            offset += count as u64;
        }
        Ok(offset)
    }

    pub fn file_stat(&self, path: &str, id: &str) -> Result<FsStat> {
        let mut filesystem = self.filesystem.borrow_mut();
        let info = file_info(&mut filesystem, path)?;
        let sample = if has_file_bytes(info.kind) {
            read_range(&mut filesystem, &info, 0, 4096)?
        } else {
            Vec::new()
        };
        Ok(build_stat(
            id,
            info.size,
            Some(u64::from(info.address)),
            u16::try_from(info.mode).ok(),
            &sample,
            Vec::new(),
        ))
    }
}

fn file_info(filesystem: &mut Filesystem<PartitionCursor>, path: &str) -> Result<FileInfo> {
    let address = filesystem.find(path).map_err(vmfs_error)?;
    filesystem.file_info(address).map_err(vmfs_error)
}

fn read_range(
    filesystem: &mut Filesystem<PartitionCursor>,
    info: &FileInfo,
    offset: u64,
    length: usize,
) -> Result<Vec<u8>> {
    ensure!(
        has_file_bytes(info.kind),
        Failure::unsupported("VMFS entry does not expose file bytes")
    );
    let count =
        lamfold::checked_full_read_len(info.size.saturating_sub(offset).min(length as u64))?;
    let mut bytes = vec![0; count];
    let actual = filesystem
        .read_range(info.address, offset, &mut bytes)
        .map_err(vmfs_error)?;
    ensure!(actual == count, Failure::corrupt("VMFS short logical read"));
    Ok(bytes)
}

fn has_file_bytes(kind: FileKind) -> bool {
    matches!(kind, FileKind::Regular | FileKind::Symlink)
}

fn timestamp(seconds: u32) -> Option<String> {
    if seconds == 0 {
        return None;
    }
    chrono::DateTime::from_timestamp(i64::from(seconds), 0)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

fn vmfs_error(error: Error) -> anyhow::Error {
    match error {
        Error::NotFound => Failure::not_found(error.to_string()).into(),
        Error::NotDirectory => Failure::invalid_input(error.to_string()).into(),
        Error::MissingMembers | Error::Unsupported(_) => {
            Failure::unsupported(error.to_string()).into()
        }
        Error::Signature | Error::Corrupt(_) => Failure::corrupt(error.to_string()).into(),
        Error::Io(error) => error.into(),
    }
}
