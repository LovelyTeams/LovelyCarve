//! Enumerate VMFS directories in allocation order.
use std::collections::HashSet;
use std::io::{Read, Seek};

use crate::descriptor::{Descriptor, FileKind};
use crate::{Error, Filesystem, Result, Version, le32};

const V5_ENTRY_SIZE: usize = 140;
const V6_ENTRY_SIZE: usize = 288;
const V6_HEADER_SIZE: u64 = 0x10000;
const V6_BLOCK_HEADER: usize = 64;

#[derive(Clone, Debug)]
pub struct Entry {
    pub address: u32,
    pub generation: u32,
    pub name: String,
}

impl<R: Read + Seek> Filesystem<R> {
    pub fn entries(&mut self, address: u32) -> Result<Vec<Entry>> {
        let descriptor = self.descriptor(address)?;
        if descriptor.info.kind != FileKind::Directory {
            return Err(Error::NotDirectory);
        }
        let mut entries = Vec::new();
        match self.geometry().version {
            Version::Vmfs5 => self.directory_v5(&descriptor, &mut entries)?,
            Version::Vmfs6 => self.directory_v6(&descriptor, &mut entries)?,
        }
        let mut names = HashSet::with_capacity(entries.len());
        if entries.iter().any(|entry| !names.insert(&entry.name)) {
            return Err(Error::Corrupt("duplicate live directory entry name"));
        }
        entries.retain(|entry| !matches!(entry.name.as_str(), "." | ".."));
        Ok(entries)
    }

    pub fn find(&mut self, path: &str) -> Result<u32> {
        let mut address = crate::ROOT;
        let mut ancestors = HashSet::from([address]);
        for component in path.split('/').filter(|component| !component.is_empty()) {
            let entry = self
                .entries(address)?
                .into_iter()
                .find(|entry| entry.name == component)
                .ok_or(Error::NotFound)?;
            let info = self.file_info(entry.address)?;
            if info.generation != entry.generation {
                return Err(Error::Corrupt(
                    "directory entry points to a reused file descriptor",
                ));
            }
            if info.kind == FileKind::Directory && !ancestors.insert(entry.address) {
                return Err(Error::Corrupt("VMFS directory ancestry cycle"));
            }
            address = entry.address;
        }
        Ok(address)
    }

    fn directory_v5(&mut self, descriptor: &Descriptor, entries: &mut Vec<Entry>) -> Result<()> {
        let size = descriptor.info.size;
        if !size.is_multiple_of(V5_ENTRY_SIZE as u64) {
            return Err(Error::Corrupt("partial VMFS5 directory record"));
        }
        let mut offset = 0;
        let mut bytes = [0; V5_ENTRY_SIZE * 32];
        while offset < size {
            let count = (size - offset).min(bytes.len() as u64) as usize;
            self.read_descriptor(descriptor, offset, &mut bytes[..count])?;
            for record in bytes[..count].as_chunks::<V5_ENTRY_SIZE>().0 {
                if le32(record, 4) != 0 {
                    entries.push(directory_entry(record, &record[12..])?);
                }
            }
            offset += count as u64;
        }
        Ok(())
    }

    fn directory_v6(&mut self, descriptor: &Descriptor, entries: &mut Vec<Entry>) -> Result<()> {
        let mut header = [0; 0x5f8];
        if descriptor.info.size < V6_HEADER_SIZE {
            return Err(Error::Corrupt("short VMFS6 directory"));
        }
        self.read_descriptor(descriptor, 0, &mut header)?;
        if !matches!(le32(&header, 0), 0xf50001 | 0xfdc001) {
            return Err(Error::Corrupt("VMFS6 directory header version"));
        }
        let entry_count = le32(&header, 4) as usize;
        let map_count = le32(&header, 12) as usize;
        if map_count > 128 {
            return Err(Error::Corrupt("VMFS6 directory allocation map count"));
        }
        for offset in [0x3b8, 0x4d8] {
            let record = &header[offset..offset + V6_ENTRY_SIZE];
            if le32(record, 4) != 0 {
                entries.push(directory_entry(record, &record[24..280])?);
            }
        }
        let block_size = self.geometry().metadata_alignment as usize;
        let block_count = (descriptor.info.size - V6_HEADER_SIZE) / block_size as u64;
        let mut map = vec![0; block_size];
        let mut block = vec![0; block_size];
        let mut seen_maps = HashSet::new();
        for (map_index, address) in header[16..16 + map_count * 4]
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
        {
            let pointer = u64::from(u32::from_le_bytes(*address));
            if pointer >= block_count || !seen_maps.insert(pointer) {
                return Err(Error::Corrupt(
                    "duplicate or out-of-range directory allocation map",
                ));
            }
            self.read_descriptor(
                descriptor,
                V6_HEADER_SIZE + pointer * block_size as u64,
                &mut map,
            )?;
            if le16(&map, 2) != 3 {
                return Err(Error::Corrupt("directory allocation map block type"));
            }
            for (index, state) in map[V6_BLOCK_HEADER..]
                .iter()
                .flat_map(|byte| [byte >> 4, byte & 15])
                .enumerate()
            {
                if state & 0x0b != 1 {
                    continue;
                }
                let number = (map_index * (block_size - V6_BLOCK_HEADER) * 2 + index) as u64;
                if number >= block_count {
                    return Err(Error::Corrupt("mapped directory block exceeds the file"));
                }
                self.read_descriptor(
                    descriptor,
                    V6_HEADER_SIZE + number * block_size as u64,
                    &mut block,
                )?;
                let slots = usize::from(le16(&block, 4));
                if le16(&block, 2) != 1
                    || slots > 448
                    || slots > (block_size - V6_BLOCK_HEADER) / V6_ENTRY_SIZE
                {
                    return Err(Error::Corrupt("directory entry block type or slot count"));
                }
                for slot in 0..slots {
                    if block[8 + slot / 8] & (1 << (slot % 8)) == 0 {
                        let start = V6_BLOCK_HEADER + slot * V6_ENTRY_SIZE;
                        let record = &block[start..start + V6_ENTRY_SIZE];
                        entries.push(directory_entry(record, &record[24..280])?);
                    }
                }
            }
        }
        if entries.len() != entry_count {
            return Err(Error::Corrupt(
                "directory entry count differs from allocation map",
            ));
        }
        Ok(())
    }
}

fn le16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn directory_entry(bytes: &[u8], name_bytes: &[u8]) -> Result<Entry> {
    let address = le32(bytes, 4);
    if address & 7 != 4 || !matches!(le32(bytes, 0), 2..=6) {
        return Err(Error::Corrupt("directory entry type or descriptor address"));
    }
    let name = std::str::from_utf8(
        name_bytes
            .split(|&byte| byte == 0)
            .next()
            .unwrap_or_default(),
    )
    .map_err(|_| Error::Corrupt("invalid UTF-8 directory entry name"))?;
    if name.is_empty() || name.contains('/') {
        return Err(Error::Corrupt("invalid directory entry name"));
    }
    Ok(Entry {
        address,
        generation: le32(bytes, 8),
        name: name.into(),
    })
}
