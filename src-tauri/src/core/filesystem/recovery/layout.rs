//! Output layout of a recovery export. Each recovered volume becomes a folder and
//! files keep the recovered directory tree, matching how the scan is browsed:
//! directory identity follows the record, so same-name live and deleted folders
//! stay apart, and paths only supply ancestors whose records are missing.
use super::*;
use crate::core::filesystem::tree::fs_tree_export::sanitize_export_component;
use std::collections::HashMap;

/// Parent chains longer than this hang off their volume folder, as in the browser tree.
const MAX_DEPTH: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Folder<'a> {
    Volume(usize),
    /// A recovered directory record: volume index and node ID.
    Record(usize, &'a str),
    /// A path component without a recovered record.
    Path(usize, String),
}

impl Folder<'_> {
    fn volume(&self) -> usize {
        match self {
            Self::Volume(volume) | Self::Record(volume, _) | Self::Path(volume, _) => *volume,
        }
    }
}

pub(super) struct ExportLayout<'a> {
    scan: &'a RecoveryScan,
    records: HashMap<(usize, &'a str), &'a RecoveryDirectory>,
    /// First directory record at each path, used for ancestors without a parent record.
    paths: HashMap<(usize, &'a str), &'a str>,
    parents: HashMap<Folder<'a>, Folder<'a>>,
    placed: HashMap<Folder<'a>, Vec<String>>,
    /// Lower-cased names already used in each output folder; files and folders share it.
    taken: HashMap<Vec<String>, HashSet<String>>,
}

impl<'a> ExportLayout<'a> {
    pub(super) fn new(scan: &'a RecoveryScan) -> Self {
        let mut layout = Self {
            scan,
            records: HashMap::new(),
            paths: HashMap::new(),
            parents: HashMap::new(),
            placed: HashMap::new(),
            taken: HashMap::new(),
        };
        for directory in &scan.directories {
            let key = (directory.volume_index, directory.node.id.as_str());
            layout.records.entry(key).or_insert(directory);
            layout
                .paths
                .entry((directory.volume_index, directory.path.as_str()))
                .or_insert(directory.node.id.as_str());
        }
        let mut order = Vec::new();
        for directory in &scan.directories {
            let folder = Folder::Record(directory.volume_index, directory.node.id.as_str());
            if layout.parents.contains_key(&folder) {
                continue;
            }
            let parent = match directory.node.parent_id.as_deref() {
                Some(parent)
                    if parent != directory.node.id
                        && layout
                            .records
                            .contains_key(&(directory.volume_index, parent)) =>
                {
                    Folder::Record(directory.volume_index, parent)
                }
                _ => layout.path_folder(directory.volume_index, parent_path(&directory.path)),
            };
            layout.parents.insert(folder.clone(), parent);
            order.push(folder);
        }
        // Break malformed parent cycles in record order, as the browser tree does.
        for folder in order {
            let mut seen = HashSet::from([folder.clone()]);
            let mut current = layout.parents.get(&folder).cloned();
            let mut depth = 0;
            while let Some(parent) = current {
                if matches!(parent, Folder::Volume(_)) {
                    break;
                }
                depth += 1;
                if !seen.insert(parent.clone()) || depth >= MAX_DEPTH {
                    layout
                        .parents
                        .insert(folder.clone(), Folder::Volume(folder.volume()));
                    break;
                }
                current = layout.parents.get(&parent).cloned();
            }
        }
        layout
    }

    /// Path of the candidate's file relative to the export directory, unique within it.
    pub(super) fn file_path(&mut self, candidate: &'a RecoveryCandidate) -> Vec<String> {
        let volume = candidate.volume_index;
        let parent = match candidate.node.parent_id.as_deref() {
            Some(parent) if self.records.contains_key(&(volume, parent)) => {
                Folder::Record(volume, parent)
            }
            _ => self.path_folder(volume, parent_path(&candidate.path)),
        };
        let mut path = self.folder_path(parent);
        let name = self.claim(&path, &candidate.node.name);
        path.push(name);
        path
    }

    /// Folder for a recovered path, adding parent links for components without records.
    fn path_folder(&mut self, volume: usize, path: &str) -> Folder<'a> {
        let mut parent = Folder::Volume(volume);
        let mut prefix = String::new();
        for component in path
            .split('/')
            .filter(|component| !component.is_empty())
            .take(MAX_DEPTH)
        {
            prefix.push('/');
            prefix.push_str(component);
            let folder = match self.paths.get(&(volume, prefix.as_str())) {
                Some(id) => Folder::Record(volume, id),
                None => {
                    let folder = Folder::Path(volume, prefix.clone());
                    self.parents.entry(folder.clone()).or_insert(parent);
                    folder
                }
            };
            parent = folder;
        }
        parent
    }

    fn folder_path(&mut self, folder: Folder<'a>) -> Vec<String> {
        let mut chain = Vec::new();
        let mut current = folder;
        let base = loop {
            if let Some(path) = self.placed.get(&current) {
                break path.clone();
            }
            match current {
                Folder::Volume(volume) => {
                    let name = self.volume_folder_name(volume);
                    let path = vec![self.claim(&[], &name)];
                    self.placed.insert(current, path.clone());
                    break path;
                }
                Folder::Record(..) | Folder::Path(..) => {
                    let parent = match self.parents.get(&current) {
                        Some(parent) if chain.len() < MAX_DEPTH => parent.clone(),
                        _ => Folder::Volume(current.volume()),
                    };
                    chain.push(current);
                    current = parent;
                }
            }
        };
        let mut path = base;
        for folder in chain.into_iter().rev() {
            let name = self.folder_name(&folder);
            path.push(self.claim(&path, &name));
            self.placed.insert(folder, path.clone());
        }
        path
    }

    fn folder_name(&self, folder: &Folder<'a>) -> String {
        match folder {
            Folder::Volume(volume) => self.volume_folder_name(*volume),
            Folder::Record(volume, id) => self.records.get(&(*volume, *id)).map_or_else(
                || (*id).to_string(),
                |directory| directory.node.name.clone(),
            ),
            Folder::Path(_, path) => path.rsplit('/').next().unwrap_or(path).to_string(),
        }
    }

    /// `v{index}-{label}`; volumes without a label use their filesystem family.
    fn volume_folder_name(&self, volume: usize) -> String {
        let label = self
            .scan
            .volumes
            .get(volume)
            .map(|recovered| {
                recovered
                    .partition
                    .label
                    .as_deref()
                    .map(str::trim)
                    .filter(|label| !label.is_empty())
                    .map_or_else(
                        || family(recovered.partition.kind).to_string(),
                        str::to_string,
                    )
            })
            .unwrap_or_default();
        format!("v{volume}-{label}")
    }

    /// Sanitized name that is unique in `folder`, ignoring case; repeats get `-1`, `-2`…
    fn claim(&mut self, folder: &[String], name: &str) -> String {
        let safe = sanitize_export_component(name);
        let (stem, extension) = match safe.rsplit_once('.') {
            Some((stem, extension)) if !stem.is_empty() => {
                (stem.to_string(), format!(".{extension}"))
            }
            _ => (safe.clone(), String::new()),
        };
        let taken = self.taken.entry(folder.to_vec()).or_default();
        let mut claimed = safe;
        let mut counter = 0u64;
        while !taken.insert(claimed.to_lowercase()) {
            counter += 1;
            claimed = format!("{stem}-{counter}{extension}");
        }
        claimed
    }
}

fn parent_path(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::filesystem::tree::fs_tree::Mactb;

    fn node(id: &str, name: &str, parent: &str, kind: NodeKind, deleted: bool) -> FsNode {
        FsNode {
            id: id.to_string(),
            name: name.to_string(),
            parent_id: Some(parent.to_string()),
            kind,
            size: 1,
            deleted,
            inode: None,
            timestamps: Mactb::default(),
            link_target: None,
            has_snapshot: false,
        }
    }
    fn directory(
        volume: usize,
        path: &str,
        id: &str,
        parent: &str,
        deleted: bool,
    ) -> RecoveryDirectory {
        RecoveryDirectory {
            source_path: None,
            source_node: None,
            volume_index: volume,
            path: path.to_string(),
            node: node(
                id,
                path.rsplit('/').next().unwrap_or(path),
                parent,
                NodeKind::Dir,
                deleted,
            ),
        }
    }
    fn file(id: &str, volume: usize, path: &str, parent: &str) -> RecoveryCandidate {
        RecoveryCandidate {
            id: id.to_string(),
            volume_index: volume,
            path: path.to_string(),
            node: node(
                &format!("file:{id}"),
                path.rsplit('/').next().unwrap_or(path),
                parent,
                NodeKind::File,
                true,
            ),
            readable: true,
            initialized_size: None,
            source_path: None,
            source_node: None,
        }
    }
    fn volume(kind: PartitionKind, label: Option<&str>) -> RecoveryVolume {
        RecoveryVolume {
            partition: Partition {
                id: 0,
                kind,
                start_lba: 2048,
                length_lba: 1024,
                label: label.map(str::to_string),
                unique_guid: None,
            },
            lost: true,
            reconstruction: None,
            error: None,
        }
    }
    fn scan(
        directories: Vec<RecoveryDirectory>,
        candidates: Vec<RecoveryCandidate>,
    ) -> RecoveryScan {
        RecoveryScan {
            scan_id: "scan".into(),
            image_id: "image".into(),
            options: RecoveryOptions::default(),
            ranges: Vec::new(),
            volumes: vec![
                volume(PartitionKind::Ntfs, Some("新加卷")),
                volume(PartitionKind::Fat32, Some("  ")),
            ],
            candidates,
            directories,
            truncated: false,
            warnings: Vec::new(),
            read_faults: ReadFaultReport::default(),
        }
    }
    fn layout_of(scan: &RecoveryScan) -> Vec<String> {
        let mut layout = ExportLayout::new(scan);
        scan.candidates
            .iter()
            .map(|candidate| layout.file_path(candidate).join("/"))
            .collect()
    }

    #[test]
    fn same_name_live_and_deleted_folders_stay_apart_and_names_are_unique() {
        let result = scan(
            vec![
                directory(0, "/temp", "live", "part:0", false),
                directory(0, "/temp", "gone", "part:0", true),
                directory(0, "/temp/sub", "sub", "gone", true),
            ],
            vec![
                file("0", 0, "/temp/a.txt", "live"),
                file("1", 0, "/temp/a.txt", "gone"),
                file("2", 0, "/temp/A.TXT", "gone"),
                file("3", 0, "/temp/sub/b.bin", "sub"),
                file("4", 1, "/DCIM/IMG:1.JPG", "missing"),
                file("5", 0, "/Temp", "part:0"),
            ],
        );
        assert_eq!(
            layout_of(&result),
            vec![
                "v0-新加卷/temp/a.txt",
                "v0-新加卷/temp-1/a.txt",
                "v0-新加卷/temp-1/A-1.TXT",
                "v0-新加卷/temp-1/sub/b.bin",
                "v1-FAT32/DCIM/IMG_1.JPG",
                "v0-新加卷/Temp-2",
            ]
        );
    }

    #[test]
    fn missing_parents_follow_the_path_and_cycles_hang_off_the_volume() {
        let result = scan(
            vec![
                directory(0, "/x", "x", "y", true),
                directory(0, "/x/y", "y", "x", true),
                directory(0, "/..", "dots", "part:0", true),
            ],
            vec![
                file("0", 0, "/lost/deep/orphan.txt", "missing"),
                file("1", 0, "/x/y/inside.txt", "y"),
                file("2", 0, "/../evil.txt", "dots"),
            ],
        );
        assert_eq!(
            layout_of(&result),
            vec![
                "v0-新加卷/lost/deep/orphan.txt",
                // `x` is visited first, so its parent link to `y` is the one cut.
                "v0-新加卷/x/y/inside.txt",
                "v0-新加卷/_/evil.txt",
            ]
        );
    }
}
