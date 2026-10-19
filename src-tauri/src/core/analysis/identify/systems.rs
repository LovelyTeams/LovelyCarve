//! Operating systems installed on an evidence item, each with the volumes it
//! mounts in one tree: the system views that system-scope analyzers run on.
//!
//! Every filesystem of the evidence ([`FsAll`]) is probed with the OS
//! detectors; Docker container roots count as systems of their own. A
//! volume whose OS lives below its root — Btrfs subvolumes (`@`), or a
//! folder / archive evidence with one top-level directory — is probed one
//! level down. The mounts then come from the system itself:
//!
//! - Linux: `/etc/fstab`, matching `UUID=` / `LABEL=` against the volumes'
//!   superblocks, `PARTUUID=` against GPT partition GUIDs and
//!   `/dev/mapper/<vg>-<lv>` / `/dev/<vg>/<lv>` against LVM names;
//!   `subvol=` mounts a Btrfs subvolume. Device paths such as `/dev/sda2`
//!   depend on the boot-time disk order and are not mapped.
//! - Windows: the drive letters of `MountedDevices`; the system volume is the
//!   view root and is also reachable under its own letter.

use std::{collections::HashMap, sync::Arc};

use serde::Serialize;

use crate::core::{
    analysis::identify::{
        fstab::{self, DeviceSpec},
        os_detect::{self, DetectedOs, OsFamily},
        os_detect_adapters::{supports_os_detect, Fingerprint},
        win_drive_letters,
    },
    evidence::partitions::PartitionKind,
    filesystem::{
        evidence_session::EvidenceSession,
        fs_all::{directory_root, FsAll, FsEntry, ROOT_VOLUME_KEY},
        tree::fs_tree::{self, NodeKind},
        volume_identity::{self, VolumeIdentity},
        volumes::lvm::LvmVolume,
    },
};

/// Largest `/etc/fstab` read.
const FSTAB_CAP: usize = 1024 * 1024;
/// Most child directories probed below a volume root.
const NESTED_PROBES: usize = 32;

/// One detected system.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedSystem {
    /// Stable within the evidence: `linux@part:2`, `linux@part:3/@`.
    pub id: String,
    /// `linux`, `windows`, `macos`, `esxi`, `mikrotik`, `openwrt`, `dingtalk`.
    pub family: String,
    pub product: String,
    pub version: Option<String>,
    /// `Kali GNU/Linux 2024.1 · part:2`.
    pub label: String,
    /// Volume key of the system volume.
    pub root: String,
    pub mounts: Vec<SystemMount>,
}

/// A view path showing a node of a volume.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMount {
    /// `""` for the view root, `home`, `boot/efi`, `D:`.
    pub path: String,
    /// Volume key.
    pub root: String,
    /// File-tree node shown at `path`.
    pub node: String,
    /// That node's path inside the volume (`""` for the volume root).
    pub node_path: String,
    /// `root`, `fstab` or `drive_letter`.
    pub source: &'static str,
    /// Listed in its parent directory; aliases (`C:` of the system volume)
    /// resolve but are not listed twice.
    pub listed: bool,
}

/// Discovered systems by image id. Discovery reads registry hives and
/// fstab, so each loaded image is discovered once; unloading an image
/// forgets its entry (`remove_image_runtime`).
#[derive(Default)]
pub struct SystemCache {
    entries: parking_lot::Mutex<HashMap<String, Arc<Vec<DetectedSystem>>>>,
}

impl SystemCache {
    /// The systems of `image`, discovered on first use. Blocking.
    pub fn get_or_discover(&self, image: &Arc<EvidenceSession>) -> Arc<Vec<DetectedSystem>> {
        if let Some(systems) = self.entries.lock().get(&image.id) {
            return Arc::clone(systems);
        }
        let systems = Arc::new(discover(image));
        self.entries
            .lock()
            .entry(image.id.clone())
            .or_insert_with(|| Arc::clone(&systems))
            .clone()
    }

    pub fn forget(&self, image_id: &str) {
        self.entries.lock().remove(image_id);
    }
}

/// The systems of `image`, in volume order.
pub fn discover(image: &Arc<EvidenceSession>) -> Vec<DetectedSystem> {
    let volumes = FsAll::for_image(image);
    let mut found = Vec::new();
    for entry in volumes.entries() {
        if !probed(entry) {
            continue;
        }
        for (node, node_path, detected) in detect_volume(image, entry) {
            found.push((entry.clone(), node, node_path, detected));
        }
    }
    if found.is_empty() {
        return Vec::new();
    }
    let mut context = MountContext::new(image, &volumes);
    found
        .into_iter()
        .map(|(entry, node, node_path, detected)| {
            let family = family_name(detected.family);
            let mut mounts = vec![SystemMount {
                path: String::new(),
                root: entry.volume_key.clone(),
                node: node.clone(),
                node_path: node_path.clone(),
                source: "root",
                listed: true,
            }];
            match detected.family {
                OsFamily::Linux => context.linux_mounts(&entry, &node, &mut mounts),
                OsFamily::Windows => context.windows_mounts(&entry, &mut mounts),
                OsFamily::MacOs
                | OsFamily::Esxi
                | OsFamily::MikroTik
                | OsFamily::OpenWrt
                | OsFamily::DingTalk => {}
            }
            let place = if node_path.is_empty() {
                entry.volume_key.clone()
            } else {
                format!("{}/{node_path}", entry.volume_key)
            };
            let product = detected.product_name.clone();
            let label = match &detected.version {
                Some(version) if !product.contains(version.as_str()) => {
                    format!("{product} {version} · {place}")
                }
                Some(_) | None => format!("{product} · {place}"),
            };
            DetectedSystem {
                id: format!("{family}@{place}"),
                family: family.to_owned(),
                product,
                version: detected.version.clone(),
                label,
                root: entry.volume_key.clone(),
                mounts,
            }
        })
        .collect()
}

fn family_name(family: OsFamily) -> &'static str {
    match family {
        OsFamily::Windows => "windows",
        OsFamily::Linux => "linux",
        OsFamily::MacOs => "macos",
        OsFamily::Esxi => "esxi",
        OsFamily::MikroTik => "mikrotik",
        OsFamily::OpenWrt => "openwrt",
        OsFamily::DingTalk => "dingtalk",
    }
}

fn probed(entry: &FsEntry) -> bool {
    supports_os_detect(entry.kind)
        || entry.volume_key == ROOT_VOLUME_KEY
        || entry.volume_key.starts_with("host-docker:")
}

/// `(root node, its path inside the volume, detection)` of every system on
/// one volume.
fn detect_volume(
    image: &Arc<EvidenceSession>,
    entry: &FsEntry,
) -> Vec<(String, String, DetectedOs)> {
    let root = match directory_root(image, &entry.volume_key) {
        Ok(root) => root,
        Err(error) => {
            tracing::debug!(volume = %entry.volume_key, "system probe: {error:#}");
            return Vec::new();
        }
    };
    let at_root = Fingerprint::new(Arc::clone(image), root.clone());
    if let Some(detected) = os_detect::detect(&at_root).into_iter().next() {
        return vec![(root, String::new(), detected)];
    }
    let nested = entry.volume_key == ROOT_VOLUME_KEY || entry.kind == PartitionKind::Btrfs;
    if !nested {
        return Vec::new();
    }
    let children = match fs_tree::children_for_image(image, &root) {
        Ok(children) => children,
        Err(error) => {
            tracing::debug!(volume = %entry.volume_key, "system probe: {error:#}");
            return Vec::new();
        }
    };
    children
        .into_iter()
        .filter(|child| child.kind == NodeKind::Dir && !child.deleted)
        .take(NESTED_PROBES)
        .filter_map(|child| {
            let fingerprint = Fingerprint::new(Arc::clone(image), child.id.clone());
            os_detect::detect(&fingerprint)
                .into_iter()
                .next()
                .map(|detected| (child.id, child.name, detected))
        })
        .collect()
}

/// Volume identities and names, read once per discovery.
struct MountContext<'a> {
    image: &'a Arc<EvidenceSession>,
    volumes: &'a FsAll,
    identities: Option<Vec<(String, VolumeIdentity)>>,
    logical_volumes: Option<HashMap<(String, String), String>>,
    drive_letters: Option<win_drive_letters::DriveLetterReport>,
}

impl<'a> MountContext<'a> {
    fn new(image: &'a Arc<EvidenceSession>, volumes: &'a FsAll) -> Self {
        Self {
            image,
            volumes,
            identities: None,
            logical_volumes: None,
            drive_letters: None,
        }
    }

    fn linux_mounts(&mut self, entry: &FsEntry, node: &str, mounts: &mut Vec<SystemMount>) {
        let fingerprint = Fingerprint::new(Arc::clone(self.image), node.to_owned());
        let text = match fingerprint.read_limited("/etc/fstab", FSTAB_CAP) {
            Ok(Some(bytes)) => String::from_utf8_lossy(&bytes).into_owned(),
            Ok(None) => return,
            Err(error) => {
                tracing::debug!(volume = %entry.volume_key, "fstab unreadable: {error:#}");
                return;
            }
        };
        for line in fstab::parse(&text) {
            let path = line.view_path();
            if path.is_empty() || mounts.iter().any(|mount| mount.path == path) {
                continue;
            }
            let Some(volume) = self.resolve(&line.device()) else {
                tracing::debug!(spec = %line.spec, target = %line.target, "fstab volume not in the evidence");
                continue;
            };
            let (node, node_path) = match line.subvolume() {
                Some(subvolume) => match self.lookup(&volume, &subvolume) {
                    Some(node) => (node, subvolume),
                    None => continue,
                },
                None => (volume.clone(), String::new()),
            };
            mounts.push(SystemMount {
                path,
                root: volume,
                node,
                node_path,
                source: "fstab",
                listed: true,
            });
        }
    }

    fn windows_mounts(&mut self, entry: &FsEntry, mounts: &mut Vec<SystemMount>) {
        let report = self
            .drive_letters
            .get_or_insert_with(|| win_drive_letters::read_image_drive_letters(self.image));
        for assignment in &report.assignments {
            let root = format!("part:{}", assignment.partition_id);
            if mounts
                .iter()
                .any(|mount| mount.path == assignment.drive_letter)
            {
                continue;
            }
            mounts.push(SystemMount {
                path: assignment.drive_letter.clone(),
                listed: root != entry.volume_key,
                node: root.clone(),
                root,
                node_path: String::new(),
                source: "drive_letter",
            });
        }
    }

    /// The volume key a mount table line names.
    fn resolve(&mut self, device: &DeviceSpec) -> Option<String> {
        match device {
            DeviceSpec::Uuid(uuid) => self
                .identities()
                .iter()
                .find(|(_, identity)| identity.has_uuid(uuid))
                .map(|(key, _)| key.clone()),
            DeviceSpec::Label(label) => self
                .identities()
                .iter()
                .find(|(_, identity)| identity.has_label(label))
                .map(|(key, _)| key.clone()),
            DeviceSpec::PartUuid(guid) => self.image.partitions.iter().find_map(|partition| {
                let unique = uuid::Uuid::from_bytes(partition.unique_guid?).to_string();
                unique
                    .eq_ignore_ascii_case(guid)
                    .then(|| format!("part:{}", partition.id))
            }),
            DeviceSpec::LogicalVolume { group, volume } => self
                .logical_volumes()
                .get(&(group.clone(), volume.clone()))
                .cloned(),
            DeviceSpec::Device(_) => None,
        }
    }

    fn identities(&mut self) -> &[(String, VolumeIdentity)] {
        let (image, volumes) = (self.image, self.volumes);
        self.identities.get_or_insert_with(|| {
            volumes
                .entries()
                .iter()
                .filter(|entry| {
                    entry.volume_key.starts_with("part:") || entry.volume_key.starts_with("lvm:")
                })
                .map(|entry| {
                    let reader = image.filesystem_session(&entry.volume_key);
                    (
                        entry.volume_key.clone(),
                        volume_identity::read_identity(reader, entry.kind, entry.offset),
                    )
                })
                .collect()
        })
    }

    fn logical_volumes(&mut self) -> &HashMap<(String, String), String> {
        let image = self.image;
        self.logical_volumes.get_or_insert_with(|| {
            let mut names = HashMap::new();
            for partition in image
                .partitions
                .iter()
                .filter(|partition| partition.kind == PartitionKind::Lvm)
            {
                let reader = image.filesystem_session(&format!("part:{}", partition.id));
                match LvmVolume::open(reader, partition) {
                    Ok(lvm) => {
                        for (index, volume) in lvm.logical_volumes.iter().enumerate() {
                            names.insert(
                                (lvm.metadata.name.clone(), volume.name.clone()),
                                format!("lvm:{}:{index}", partition.id),
                            );
                        }
                    }
                    Err(error) => {
                        tracing::debug!(partition = partition.id, "LVM names: {error:#}")
                    }
                }
            }
            names
        })
    }

    /// The node at `path` inside volume `root`.
    fn lookup(&self, root: &str, path: &str) -> Option<String> {
        let mut current = root.to_owned();
        for component in path.split('/').filter(|component| !component.is_empty()) {
            let children = fs_tree::children_for_image(self.image, &current).ok()?;
            current = children
                .into_iter()
                .find(|child| child.kind == NodeKind::Dir && child.name == component)?
                .id;
        }
        Some(current)
    }
}
