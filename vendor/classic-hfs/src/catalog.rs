use crate::{
    be16, be32,
    btree::{self, TreeKind},
    fork::Fork,
    metadata::extents,
    names, require,
};
use lamfold::{FoldError, Result};
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
pub(crate) struct Entry {
    pub parent: u32,
    pub name: Vec<u8>,
    pub directory: bool,
    pub created: u32,
    pub modified: u32,
    pub backup: u32,
    pub data: Option<Fork>,
    pub resource: Option<Fork>,
    valence: u16,
}
pub(crate) struct Catalog {
    pub entries: HashMap<u32, Entry>,
    pub children: HashMap<u32, Vec<u32>>,
}
impl Catalog {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut entries = HashMap::new();
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        let mut threads = Vec::new();
        for (key, data) in btree::leaves(bytes, TreeKind::Catalog)? {
            require(
                data.len() >= 2 && data[1] == 0,
                "hfs: catalog record type/reserved byte",
            )?;
            let parent = be32(key, 1)?;
            let name = key[6..6 + usize::from(key[5])].to_vec();
            match data[0] {
                1 | 2 => {
                    require(
                        !name.is_empty() && !name.contains(&0) && !name.contains(&b':'),
                        "hfs: catalog filename",
                    )?;
                    let directory = data[0] == 1;
                    require(
                        data.len() == if directory { 70 } else { 102 },
                        "hfs: catalog file/directory record length",
                    )?;
                    let id = be32(data, if directory { 6 } else { 20 })?;
                    require(id == 2 || id >= 16, "hfs: reserved/invalid catalog CNID")?;
                    let time = if directory { 10 } else { 44 };
                    let entry = Entry {
                        parent,
                        name,
                        directory,
                        created: be32(data, time)?,
                        modified: be32(data, time + 4)?,
                        backup: be32(data, time + 8)?,
                        valence: if directory { be16(data, 4)? } else { 0 },
                        data: if directory {
                            None
                        } else {
                            Some(Fork {
                                logical: be32(data, 26)?,
                                physical: be32(data, 30)?,
                                extents: extents(data, 74)?,
                            })
                        },
                        resource: if directory {
                            None
                        } else {
                            Some(Fork {
                                logical: be32(data, 36)?,
                                physical: be32(data, 40)?,
                                extents: extents(data, 86)?,
                            })
                        },
                    };
                    require(entries.insert(id, entry).is_none(), "hfs: duplicate CNID")?;
                    children.entry(parent).or_default().push(id);
                }
                3 | 4 => {
                    require(
                        name.is_empty() && data.len() == 46 && data[14] <= 31,
                        "hfs: catalog thread key/length",
                    )?;
                    threads.push((
                        parent,
                        data[0] == 3,
                        be32(data, 10)?,
                        data[15..15 + usize::from(data[14])].to_vec(),
                    ));
                }
                _ => return Err(FoldError::Corrupt("hfs: unknown catalog record type")),
            }
        }
        let root = entries
            .get(&2)
            .ok_or(FoldError::Corrupt("hfs: missing root CNID"))?;
        require(
            root.directory && root.parent == 1,
            "hfs: invalid root directory",
        )?;
        for (id, entry) in &entries {
            if entry.directory {
                require(
                    children.get(id).map_or(0, Vec::len) == usize::from(entry.valence),
                    "hfs: directory valence mismatch",
                )?;
            }
            let mut current = *id;
            let mut seen = HashSet::new();
            while current != 2 {
                require(
                    seen.len() < 256 && seen.insert(current),
                    "hfs: catalog parent cycle/depth limit",
                )?;
                let child = entries
                    .get(&current)
                    .ok_or(FoldError::Corrupt("hfs: missing parent entry"))?;
                current = child.parent;
                require(
                    entries.get(&current).is_some_and(|parent| parent.directory),
                    "hfs: orphan/file as parent",
                )?;
            }
        }
        for (id, directory, parent, name) in threads {
            let entry = entries
                .get(&id)
                .ok_or(FoldError::Corrupt("hfs: orphan thread"))?;
            require(
                entry.directory == directory
                    && entry.parent == parent
                    && names::compare(&entry.name, &name).is_eq(),
                "hfs: catalog/thread mismatch",
            )?;
        }
        Ok(Self { entries, children })
    }
    pub fn get(&self, node: u64) -> Result<&Entry> {
        u32::try_from(node)
            .ok()
            .and_then(|id| self.entries.get(&id))
            .ok_or(FoldError::Corrupt("hfs: unknown CNID"))
    }
}
