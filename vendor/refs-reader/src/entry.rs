// Ported from refsprogs parse_level3_short_value / parse_level3_long_value.
use crate::bytes::{u16_at, u32_at, u64_at};
use crate::table::TablePage;
use crate::{corrupt, BootSector, DirectoryRecord, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub directory_id: Option<u64>,
    pub size: u64,
    pub allocated_size: u64,
    pub flags: u32,
    /// Windows FILETIME values, in 100 ns ticks since 1601-01-01 UTC.
    pub created: u64,
    pub accessed: u64,
    pub modified: u64,
    pub changed: u64,
    pub symlink_target: Option<String>,
    pub inode: Option<u64>,
    pub(crate) entry_type: u16,
    pub(crate) value: Vec<u8>,
}

impl FileEntry {
    pub(crate) fn resolve_reparse(
        &mut self,
        boot: &BootSector,
        mappings: &[crate::metadata::BlockRange],
        source: &dyn crate::ReadAt,
    ) -> Result<()> {
        for attribute in crate::attributes::read_attributes(self, boot, mappings, source)? {
            if crate::attributes::attribute_type(&attribute, boot)? == Some(0xC0) {
                self.symlink_target = crate::reparse::target(&attribute.value)?;
                if let Some(target) = &self.symlink_target {
                    self.size = target.len() as u64;
                }
                break;
            }
        }
        Ok(())
    }
    pub fn is_directory(&self) -> bool {
        self.directory_id.is_some()
    }

    pub(crate) fn parse(record: DirectoryRecord, boot: &BootSector) -> Result<Self> {
        let value = &record.value;
        let (directory_id, mut size, allocated_size, flags, created, accessed, modified, changed) =
            match record.entry_type {
                2 => {
                    let flags = u32_at(value, 64)?;
                    let directory_id = if flags & 0x10000000 != 0 {
                        Some(u64_at(value, if boot.modern() { 8 } else { 0 })?)
                    } else {
                        None
                    };
                    (
                        directory_id,
                        u64_at(value, 56)?,
                        u64_at(value, 48)?,
                        flags,
                        u64_at(value, 16)?,
                        u64_at(value, 40)?,
                        u64_at(value, 24)?,
                        u64_at(value, 32)?,
                    )
                }
                1 => (
                    None,
                    u64_at(value, if boot.modern() { 88 } else { 104 })?,
                    u64_at(value, if boot.modern() { 96 } else { 112 })?,
                    u32_at(value, 72)?,
                    u64_at(value, 40)?,
                    u64_at(value, 64)?,
                    u64_at(value, 48)?,
                    u64_at(value, 56)?,
                ),
                value => return Err(corrupt(format!("unsupported filename entry type {value}"))),
            };
        let [created, accessed, modified, changed] = record
            .filename_timestamps
            .unwrap_or([created, accessed, modified, changed]);
        let mut symlink_target = None;
        if record.entry_type == 1 && flags & 0x400 != 0 {
            let standard_size = u16_at(value, 0)? as usize;
            let attributes = TablePage::parse_allocation(value, standard_size, 0, boot)?;
            let type_offset = if (boot.major, boot.minor) >= (3, 5) {
                12
            } else {
                8
            };
            for attribute in attributes.rows {
                if attribute.key.len() >= type_offset + 2
                    && u16_at(&attribute.key, type_offset)? == 0xC0
                {
                    symlink_target = crate::reparse::target(&attribute.value)?;
                    if let Some(target) = &symlink_target {
                        size = target.len() as u64;
                    }
                    break;
                }
            }
        }
        Ok(Self {
            name: record.name,
            directory_id,
            size,
            allocated_size,
            flags,
            created,
            accessed,
            modified,
            changed,
            symlink_target,
            inode: record.inode,
            entry_type: record.entry_type,
            value: record.value,
        })
    }
}
