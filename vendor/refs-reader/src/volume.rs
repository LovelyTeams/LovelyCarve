use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use crate::bytes::{u16_at, u32_at, u64_at, utf16};
use crate::data::DataPlan;
use crate::metadata::{
    checkpoints, physical_block, read_node, read_physical_node, validate_header, BlockRange,
};
use crate::table::{Row, TablePage};
use crate::{
    corrupt, BootSector, Checkpoint, DirectoryRecord, FileEntry, NodeReference, ReadAt, Result,
};

pub struct Volume {
    source: Arc<dyn ReadAt>,
    boot: BootSector,
    checkpoint: Checkpoint,
    mappings: Vec<BlockRange>,
    objects: BTreeMap<u64, NodeReference>,
    directories: crate::directory_cache::DirectoryCache,
}

impl Volume {
    pub fn boot(&self) -> &BootSector {
        &self.boot
    }

    pub fn checkpoint(&self) -> &Checkpoint {
        &self.checkpoint
    }
    pub fn list_streams(&self, entry: &FileEntry) -> Result<Vec<crate::StreamEntry>> {
        crate::streams::list(entry, &self.boot, &self.mappings, self.source.as_ref())
    }

    /// Polls the caller between metadata reads, allowing cancellable tree scans.
    pub fn list_streams_checked(
        &self,
        entry: &FileEntry,
        check: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Vec<crate::StreamEntry>> {
        check()?;
        let source = CheckedSource {
            source: self.source.as_ref(),
            check,
        };
        crate::streams::list(entry, &self.boot, &self.mappings, &source)
    }

    pub fn open_named_stream(&self, mut entry: FileEntry, name: &str) -> Result<FileReader> {
        let (stream, plan) = crate::streams::open(
            &entry,
            name,
            &self.boot,
            &self.mappings,
            self.source.as_ref(),
        )?;
        entry.size = stream.size;
        entry.symlink_target = None;
        Ok(FileReader {
            source: Arc::clone(&self.source),
            entry,
            plan,
        })
    }

    pub fn open_file(&self, entry: FileEntry) -> Result<FileReader> {
        let plan = DataPlan::parse(&entry, &self.boot, &self.mappings, self.source.as_ref())?;
        Ok(FileReader {
            source: Arc::clone(&self.source),
            entry,
            plan,
        })
    }
    pub fn list_directory(&self, object_id: u64) -> Result<Vec<FileEntry>> {
        Ok(self.cached_directory(object_id)?.as_ref().clone())
    }

    /// Checks cancellation between B-tree pages and filename records.
    pub fn list_directory_checked(
        &self,
        object_id: u64,
        check: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Vec<FileEntry>> {
        Ok(self
            .cached_directory_checked(object_id, check)?
            .as_ref()
            .clone())
    }

    fn cached_directory(&self, object_id: u64) -> Result<Arc<Vec<FileEntry>>> {
        self.cached_directory_checked(object_id, &|| Ok(()))
    }

    fn cached_directory_checked(
        &self,
        object_id: u64,
        check: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Arc<Vec<FileEntry>>> {
        check()?;
        if let Some(entries) = self.directories.get(object_id)? {
            return Ok(entries);
        }
        let entries = self
            .directory_records_from(
                object_id,
                &CheckedSource {
                    source: self.source.as_ref(),
                    check,
                },
            )?
            .into_iter()
            .map(|record| {
                check()?;
                let mut entry = FileEntry::parse(record, &self.boot)?;
                if entry.entry_type == 1
                    && entry.flags & 0x400 != 0
                    && entry.symlink_target.is_none()
                {
                    entry.resolve_reparse(
                        &self.boot,
                        &self.mappings,
                        &CheckedSource {
                            source: self.source.as_ref(),
                            check,
                        },
                    )?;
                }
                Ok(entry)
            })
            .collect::<Result<Vec<_>>>()?;
        let entries = Arc::new(entries);
        self.directories.insert(object_id, Arc::clone(&entries))?;
        Ok(entries)
    }

    pub fn lookup(&self, path: &str) -> Result<FileEntry> {
        let components = path
            .split('/')
            .filter(|component| !component.is_empty())
            .collect::<Vec<_>>();
        if components.is_empty() {
            return Err(corrupt("root path has no filename entry"));
        }
        let mut object_id = 0x600;
        for (index, component) in components.iter().enumerate() {
            let entries = self.cached_directory(object_id)?;
            let entry = entries
                .iter()
                .find(|entry| entry.name == *component)
                .cloned()
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("ReFS path component {component} not found"),
                    )
                })?;
            if index == components.len() - 1 {
                return Ok(entry);
            }
            object_id = entry
                .directory_id
                .ok_or_else(|| corrupt("intermediate path component is a file"))?;
        }
        Err(corrupt("empty lookup path"))
    }
    pub fn open(source: Arc<dyn ReadAt>) -> Result<Self> {
        Self::open_checked(source, &|| Ok(()))
    }

    /// Mounts with a caller-supplied cancellation check between metadata reads.
    pub fn open_checked(
        source: Arc<dyn ReadAt>,
        check: &(dyn Fn() -> Result<()> + Sync),
    ) -> Result<Self> {
        check()?;
        let checked_source = CheckedSource {
            source: source.as_ref(),
            check,
        };
        let mut bytes = [0; 512];
        checked_source.read_exact_at(0, &mut bytes)?;
        let boot = BootSector::parse(&bytes, source.len())?;
        let superblock =
            read_physical_node(&checked_source, &boot, NodeReference::contiguous(30)?)?;
        let blocks = checkpoints(&superblock, &boot)?;
        let mut valid = Vec::new();
        let mut errors = Vec::new();
        for block in blocks {
            let result =
                read_physical_node(&checked_source, &boot, NodeReference::contiguous(block)?)
                    .and_then(|bytes| Checkpoint::parse(&bytes, block, &boot));
            match result {
                Ok(checkpoint) => valid.push(checkpoint),
                Err(error) => errors.push(format!("block {block}: {error}")),
            }
        }
        let checkpoint = valid
            .into_iter()
            .max_by_key(|checkpoint| checkpoint.sequence)
            .ok_or_else(|| corrupt(format!("no valid checkpoint: {}", errors.join("; "))))?;
        let mut mappings = Vec::new();
        let mut object_table = None;
        for reference in &checkpoint.roots {
            // The object and mapping roots must be physically accessible before
            // loading the virtual map; other checkpoint roots may be virtual.
            if physical_block(&boot, &[], reference.blocks[0]).is_err() {
                continue;
            }
            let bytes = read_node(&checked_source, &boot, &[], *reference)?;
            let object_id = validate_header(&bytes, reference.blocks[0], &boot, b"MSB+")?;
            match object_id {
                2 => object_table = Some(*reference),
                0xB => {
                    let rows = Self::walk_table(&checked_source, &boot, &[], *reference, 0xB)?;
                    for row in rows {
                        let offset = if boot.major >= 3 {
                            row.value
                                .len()
                                .checked_sub(16)
                                .ok_or_else(|| corrupt("truncated mapping value"))?
                        } else {
                            0x28
                        };
                        let range = BlockRange {
                            start: u64_at(&row.value, offset)?,
                            length: u64_at(&row.value, offset + 8)?,
                        };
                        if range.length == 0 {
                            return Err(corrupt("empty mapping range"));
                        }
                        mappings.push(range);
                    }
                }
                _ => {}
            }
        }
        if object_table.is_none() {
            for reference in &checkpoint.roots {
                let bytes = read_node(&checked_source, &boot, &mappings, *reference)?;
                if validate_header(&bytes, reference.blocks[0], &boot, b"MSB+")? == 2 {
                    object_table = Some(*reference);
                    break;
                }
            }
        }
        let object_table =
            object_table.ok_or_else(|| corrupt("checkpoint has no object table root"))?;
        let mut objects = BTreeMap::new();
        for row in Self::walk_table(&checked_source, &boot, &mappings, object_table, 2)? {
            let object_id = u64_at(&row.key, 8)?;
            let reference = NodeReference::parse(
                &row.value,
                if boot.modern() { 32 } else { 0 },
                boot.modern(),
            )?;
            if objects.insert(object_id, reference).is_some() {
                return Err(corrupt("duplicate object table identifier"));
            }
        }
        let volume = Self {
            source,
            boot,
            checkpoint,
            mappings,
            objects,
            directories: crate::directory_cache::DirectoryCache::default(),
        };
        // Mount success requires the live root directory to be resolvable.
        volume.directory_reference(0x600)?;
        Ok(volume)
    }

    fn walk_table(
        source: &dyn ReadAt,
        boot: &BootSector,
        mappings: &[BlockRange],
        root: NodeReference,
        object_id: u64,
    ) -> Result<Vec<Row>> {
        let mut stack = vec![root];
        let mut visited = HashSet::new();
        let mut rows = Vec::new();
        while let Some(reference) = stack.pop() {
            if !visited.insert(reference.blocks[0]) {
                return Err(corrupt("cycle or duplicate node in metadata tree"));
            }
            let bytes = read_node(source, boot, mappings, reference)?;
            let page = TablePage::parse(&bytes, reference.blocks[0], boot)?;
            if page.object_id != object_id {
                return Err(corrupt(format!(
                    "metadata object mismatch: expected {object_id:#x}, got {:#x}",
                    page.object_id
                )));
            }
            if page.index {
                stack.extend(page.children(boot)?.into_iter().rev());
            } else {
                rows.extend(page.rows);
            }
        }
        Ok(rows)
    }

    fn directory_reference(&self, object_id: u64) -> Result<NodeReference> {
        self.objects
            .get(&object_id)
            .copied()
            .ok_or_else(|| corrupt(format!("directory object {object_id:#x} not found")))
    }

    pub fn directory_records(&self, object_id: u64) -> Result<Vec<DirectoryRecord>> {
        self.directory_records_from(object_id, self.source.as_ref())
    }

    fn directory_records_from(
        &self,
        object_id: u64,
        source: &dyn ReadAt,
    ) -> Result<Vec<DirectoryRecord>> {
        let reference = self.directory_reference(object_id)?;
        let rows = Self::walk_table(source, &self.boot, &self.mappings, reference, object_id)?;
        let mut records = Vec::new();
        for row in rows {
            if row.key.len() < 4 || u16_at(&row.key, 0)? != 0x30 {
                continue;
            }
            let mut record = DirectoryRecord {
                name: utf16(&row.key[4..])?,
                entry_type: u16_at(&row.key, 2)?,
                value: row.value,
                inode: row.inode,
                filename_timestamps: None,
            };
            if self.boot.modern()
                && record.entry_type == 2
                && u32_at(&record.value, 64)? & 0x10000000 == 0
                && u64_at(&record.value, 0)? != 0
            {
                let parent_id = u64_at(&record.value, 8)?;
                let link_id = u64_at(&record.value, 0)?;
                record.filename_timestamps = Some([
                    u64_at(&record.value, 16)?,
                    u64_at(&record.value, 40)?,
                    u64_at(&record.value, 24)?,
                    u64_at(&record.value, 32)?,
                ]);
                let reference = self.directory_reference(parent_id)?;
                let target =
                    Self::walk_table(source, &self.boot, &self.mappings, reference, parent_id)?
                        .into_iter()
                        .find(|target| {
                            target.key.len() >= 24
                                && u16_at(&target.key, 0).is_ok_and(|kind| kind == 0x40)
                                && u64_at(&target.key, 8).is_ok_and(|id| id == link_id)
                                && u64_at(&target.key, 16).is_ok_and(|id| id == parent_id)
                        })
                        .ok_or_else(|| {
                            corrupt(format!(
                                "hardlink target {parent_id:#x}:{link_id:#x} missing"
                            ))
                        })?;
                record.entry_type = 1;
                record.value = target.value;
                record.inode = target.inode;
            }
            records.push(record);
        }
        Ok(records)
    }
}

pub struct FileReader {
    source: Arc<dyn ReadAt>,
    entry: FileEntry,
    plan: DataPlan,
}

struct CheckedSource<'a> {
    source: &'a dyn ReadAt,
    check: &'a (dyn Fn() -> Result<()> + Sync),
}

impl ReadAt for CheckedSource<'_> {
    fn len(&self) -> u64 {
        self.source.len()
    }

    fn read_exact_at(&self, offset: u64, buffer: &mut [u8]) -> Result<()> {
        (self.check)()?;
        self.source.read_exact_at(offset, buffer)
    }
}

impl FileReader {
    pub fn entry(&self) -> &FileEntry {
        &self.entry
    }
    /// Partition-relative byte spans; sparse holes have no physical span.
    pub fn extents(&self) -> &[crate::Extent] {
        match &self.plan {
            DataPlan::Resident(_) => &[],
            DataPlan::Extents(extents) => extents,
        }
    }

    pub fn read_range(&self, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        self.plan
            .read_range(self.source.as_ref(), &self.entry, offset, buffer)
    }
}
