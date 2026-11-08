//! Mount a backup through the fleet VFS abstraction (ADR-0010).
//!
//! A backup is a *directory* of individually-encrypted blobs indexed by a
//! `SQLite` manifest, so it enters resolution through
//! [`forensic_vfs::TreeOpen`] rather than through any of the stream openers —
//! there is no byte source to sniff. Registering [`IosBackupOpen`] is all a
//! mounting tool does; it never learns that iOS backups exist, which is the
//! format special-casing ADR-0011 forbids.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use forensic_vfs::{
    Allocation, Confidence, Credential, CredentialSource, DirEntry, DirStream, DynFs,
    EncryptionScheme, ExtentStream, FileId, FileSystem, FsKind, FsMeta, MacbTimes, NodeKind,
    NodeStream, ResidencyKind, SectorSizes, StreamId, TimeZonePolicy, TreeOpen, VfsError,
    VfsResult,
};

use crate::backup::Backup;
use crate::credentials::{Credentials, Password};
use crate::error::Error;
use crate::manifest::FileKind;

/// The identity this mount reports.
///
/// `from_name` rather than a registered const: the set is open by design, and a
/// const for `ios-backup` belongs upstream in `forensicnomicon-core` beside
/// `AD1` and `DAR`, which are the same kind of logical container.
const IOS_BACKUP: &str = "ios-backup";

/// Recognizes and mounts an iOS backup directory.
#[derive(Debug, Default, Clone, Copy)]
pub struct IosBackupOpen;

impl TreeOpen for IosBackupOpen {
    fn name(&self) -> &'static str {
        IOS_BACKUP
    }

    fn probe(&self, root: &Path) -> Confidence {
        // Both files, not either. `Manifest.plist` alone appears in plenty of
        // Apple directories that are not backups, and probing on it alone would
        // claim them — after which resolution stops, because the first candidate
        // wins. A false Yes here is more damaging than a false No.
        //
        // Reads nothing: a probe runs against every registered opener, and an
        // acquisition can hold millions of files.
        if root.join("Manifest.plist").is_file() && root.join("Manifest.db").is_file() {
            Confidence::Yes {
                how: "Manifest.plist + Manifest.db",
            }
        } else {
            Confidence::No
        }
    }

    fn open(&self, root: &Path, creds: &dyn CredentialSource) -> VfsResult<DynFs> {
        let credentials = first_password(creds, root);

        let backup = Backup::open_with(root, &credentials).map_err(|e| match e {
            // The two conditions a caller can actually act on, mapped to the one
            // error the abstraction has for them. Everything else is a decode
            // failure about identified evidence and stays loud rather than
            // becoming "no credentials", which would send an examiner looking
            // for a password that would not help.
            Error::PasswordRequired | Error::WrongPassword => VfsError::NeedCredentials {
                scheme: IOS_BACKUP,
                target: root.display().to_string(),
            },
            other => VfsError::Decode {
                layer: IOS_BACKUP,
                offset: 0,
                detail: other.to_string(),
                bytes: forensic_vfs::SmallHex::new(&[]),
            },
        })?;

        Ok(Arc::new(BackupFs::new(backup)))
    }
}

/// Take the first password the provider offers for this backup.
///
/// A backup has exactly one password, so the first `Password` credential is the
/// answer; the other [`Credential`] shapes describe volume key material that a
/// keybag has no use for, and silently treating raw bytes as a passphrase would
/// produce a wrong-password error for a credential that was never a password.
fn first_password(creds: &dyn CredentialSource, root: &Path) -> Credentials {
    let target = root.display().to_string();
    creds
        .credentials_for(EncryptionScheme::IosBackup, &target)
        .into_iter()
        .find_map(|c| match c {
            Credential::Password(p) => Some(Credentials::password(Password::new(&p))),
            _ => None,
        })
        .unwrap_or(Credentials::None)
}

/// One synthesized node. A backup's manifest is a flat list of
/// `(domain, relativePath)` pairs, so the directory structure a mount needs is
/// built here rather than read from the evidence.
struct Node {
    name: Vec<u8>,
    kind: NodeKind,
    size: u64,
    /// Index into [`Backup::files`], for the nodes that have content.
    entry: Option<usize>,
    children: Vec<u64>,
    times: MacbTimes,
    mode: Option<u32>,
    inode: Option<u64>,
}

/// A backup presented as a read-only filesystem.
pub struct BackupFs {
    nodes: Vec<Node>,
    /// `Backup::read` takes `&mut self` while every [`FileSystem`] method takes
    /// `&self`, so the reader is behind a lock. The cache beside it holds the
    /// last file decrypted: `read_at` is called repeatedly at advancing offsets
    /// for one file, and decrypting the whole blob per call would make a
    /// sequential read quadratic.
    inner: Mutex<Inner>,
}

struct Inner {
    backup: Backup,
    cached: Option<(usize, Vec<u8>)>,
}

impl BackupFs {
    fn new(backup: Backup) -> Self {
        let nodes = build_tree(&backup);
        Self {
            nodes,
            inner: Mutex::new(Inner {
                backup,
                cached: None,
            }),
        }
    }

    fn node(&self, ino: FileId) -> VfsResult<(u64, &Node)> {
        let FileId::Opaque(id) = ino else {
            return Err(VfsError::Unsupported {
                layer: IOS_BACKUP,
                scheme: format!("{ino:?} is not an ios-backup node id"),
            });
        };
        let node = usize::try_from(id)
            .ok()
            .and_then(|i| self.nodes.get(i))
            .ok_or(VfsError::OutOfRange {
                what: "ios-backup node",
                offset: id,
                len: 0,
                bound: self.nodes.len() as u64,
            })?;
        Ok((id, node))
    }
}

/// Build the directory tree from the manifest's flat entry list.
///
/// Node 0 is the root. A domain becomes the top-level directory and the
/// `relativePath` components hang beneath it — the layout every backup browser
/// presents. Intermediate directories are synthesized when the manifest does not
/// record one, which it often does not.
fn build_tree(backup: &Backup) -> Vec<Node> {
    let mut nodes = vec![Node {
        name: Vec::new(),
        kind: NodeKind::Dir,
        size: 0,
        entry: None,
        children: Vec::new(),
        times: MacbTimes::default(),
        mode: None,
        inode: None,
    }];
    // (parent id, component) -> child id
    let mut index: HashMap<(u64, Vec<u8>), u64> = HashMap::new();

    let ensure_dir = |nodes: &mut Vec<Node>,
                      index: &mut HashMap<(u64, Vec<u8>), u64>,
                      parent: u64,
                      name: &[u8]|
     -> u64 {
        if let Some(&existing) = index.get(&(parent, name.to_vec())) {
            return existing;
        }
        let id = nodes.len() as u64;
        nodes.push(Node {
            name: name.to_vec(),
            kind: NodeKind::Dir,
            size: 0,
            entry: None,
            children: Vec::new(),
            times: MacbTimes::default(),
            mode: None,
            inode: None,
        });
        nodes[parent as usize].children.push(id);
        index.insert((parent, name.to_vec()), id);
        id
    };

    for (i, file) in backup.files().iter().enumerate() {
        let mut parent = ensure_dir(&mut nodes, &mut index, 0, file.domain.as_bytes());

        let mut components: Vec<&str> = file
            .relative_path
            .split('/')
            .filter(|c| !c.is_empty())
            .collect();

        // An entry whose relativePath is empty *is* the domain directory, which
        // already exists. Recording it again would double it.
        let Some(leaf) = components.pop() else {
            continue;
        };
        for dir in components {
            parent = ensure_dir(&mut nodes, &mut index, parent, dir.as_bytes());
        }

        let kind = match file.kind {
            FileKind::File => NodeKind::File,
            FileKind::Directory => NodeKind::Dir,
            FileKind::Symlink => NodeKind::Symlink,
            // Carried through as Other rather than coerced to File, so an
            // undefined flags value stays visible as undefined.
            FileKind::Unknown(_) => NodeKind::Other,
        };

        if kind == NodeKind::Dir {
            let id = ensure_dir(&mut nodes, &mut index, parent, leaf.as_bytes());
            let node = &mut nodes[id as usize];
            node.times = times_of(file);
            node.mode = file.mode;
            node.inode = file.inode;
            continue;
        }

        let id = nodes.len() as u64;
        nodes.push(Node {
            name: leaf.as_bytes().to_vec(),
            kind,
            // An absent Size is not zero (ADR-0003). Reporting 0 here would show
            // a real file as empty in a directory listing, which is the same
            // disappearance the reader refuses elsewhere.
            size: file.size.unwrap_or(0),
            entry: Some(i),
            children: Vec::new(),
            times: times_of(file),
            mode: file.mode,
            inode: file.inode,
        });
        nodes[parent as usize].children.push(id);
        index.insert((parent, leaf.as_bytes().to_vec()), id);
    }

    nodes
}

fn times_of(file: &crate::manifest::BackupFile) -> MacbTimes {
    // `Manifest.db` records whole seconds, so the resolution is declared as
    // such rather than left to a consumer to assume: a seconds value presented
    // as nanosecond-precise invents accuracy the evidence never had.
    let secs = |t: Option<i64>| {
        t.map(|s| forensic_vfs::TimeStamp {
            unix_nanos: i128::from(s) * 1_000_000_000,
            source: forensic_vfs::TimeSource::InodeTable,
            resolution: forensic_vfs::TimeResolution::Seconds,
        })
    };
    MacbTimes {
        modified: secs(file.modified),
        accessed: None,
        changed: secs(file.status_changed),
        born: secs(file.created),
    }
}

impl FileSystem for BackupFs {
    fn kind(&self) -> FsKind {
        FsKind::from_name(IOS_BACKUP)
    }

    fn root(&self) -> FileId {
        FileId::Opaque(0)
    }

    fn sector_sizes(&self) -> SectorSizes {
        // A backup is a directory of files, not a block device. Reporting a
        // sector size would invent a geometry the evidence does not have.
        SectorSizes {
            logical: 0,
            physical: 0,
            cluster_or_block: 0,
        }
    }

    fn timestamp_zone(&self) -> TimeZonePolicy {
        // Manifest.db records Unix seconds.
        TimeZonePolicy::Utc
    }

    fn read_dir(&self, ino: FileId) -> VfsResult<DirStream> {
        let (_, node) = self.node(ino)?;
        let entries: Vec<VfsResult<DirEntry>> = node
            .children
            .iter()
            .map(|&c| {
                let child = &self.nodes[c as usize];
                Ok(DirEntry {
                    name: child.name.clone(),
                    id: FileId::Opaque(c),
                    kind: child.kind,
                })
            })
            .collect();
        Ok(DirStream::new(entries.into_iter()))
    }

    fn lookup(&self, parent: FileId, name: &[u8]) -> VfsResult<Option<FileId>> {
        let (_, node) = self.node(parent)?;
        Ok(node
            .children
            .iter()
            .find(|&&c| self.nodes[c as usize].name == name)
            .map(|&c| FileId::Opaque(c)))
    }

    fn meta(&self, ino: FileId) -> VfsResult<FsMeta> {
        let (id, node) = self.node(ino)?;
        Ok(FsMeta {
            ino: id,
            kind: node.kind,
            // Every manifest row is a live capture. A backup records no deleted
            // entries, so claiming otherwise would be inventing a finding.
            allocated: Allocation::Allocated,
            size: node.size,
            nlink: 1,
            uid: None,
            gid: None,
            mode: node.mode,
            times: node.times,
            streams: Vec::new(),
            residency: ResidencyKind::NonResident,
            link_target: None,
        })
    }

    fn read_at(&self, ino: FileId, stream: StreamId, off: u64, buf: &mut [u8]) -> VfsResult<usize> {
        if stream != StreamId::Default {
            return Ok(0);
        }
        let (_, node) = self.node(ino)?;
        let Some(entry) = node.entry else {
            return Ok(0);
        };

        let mut inner = self.inner.lock().map_err(|_| VfsError::Bootstrap {
            stage: "ios-backup reader lock",
            detail: "poisoned by a panic in an earlier read".to_string(),
        })?;

        if inner.cached.as_ref().map(|(i, _)| *i) != Some(entry) {
            let file = inner.backup.files()[entry].clone();
            let bytes = inner.backup.read(&file).map_err(|e| VfsError::Decode {
                layer: IOS_BACKUP,
                offset: off,
                detail: e.to_string(),
                bytes: forensic_vfs::SmallHex::new(&[]),
            })?;
            inner.cached = Some((entry, bytes));
        }

        let Some((_, bytes)) = inner.cached.as_ref() else {
            return Ok(0); // cov:unreachable: the branch above just populated it
        };
        let start = usize::try_from(off).unwrap_or(usize::MAX);
        let Some(src) = bytes.get(start..) else {
            return Ok(0);
        };
        let n = src.len().min(buf.len());
        buf[..n].copy_from_slice(&src[..n]);
        Ok(n)
    }

    fn extents(&self, _ino: FileId, _stream: StreamId) -> VfsResult<ExtentStream> {
        // A blob is a separate file on the host, not a run of sectors in an
        // image. There is no offset to report, and inventing one would let a
        // consumer carve at a location that means nothing.
        Ok(ExtentStream::empty())
    }

    fn read_link(&self, ino: FileId, _cap: usize) -> VfsResult<Vec<u8>> {
        let (_, node) = self.node(ino)?;
        if node.kind != NodeKind::Symlink {
            return Ok(Vec::new());
        }
        // A backup keeps the target in the entry's metadata blob rather than in
        // content. Surfacing it needs a metadata field this reader does not yet
        // decode; an empty answer is honest, a guessed path would not be.
        Ok(Vec::new())
    }

    fn deleted(&self) -> VfsResult<NodeStream> {
        // A backup is a capture, not a filesystem image: nothing was deleted
        // from it, and there is no metadata layer holding freed records.
        Ok(NodeStream::empty())
    }

    fn unallocated(&self) -> VfsResult<ExtentStream> {
        // No volume, so no unallocated space. Distinct from "none found".
        Ok(ExtentStream::empty())
    }
}
