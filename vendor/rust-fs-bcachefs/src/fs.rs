//! A read-only view of a single-device bcachefs filesystem: look up a
//! path, list a directory, read a file.
//!
//! Nothing is loaded at open but the superblock (and a journal replay when
//! one is needed): every lookup, listing and read seeks a btree cursor to
//! its own keys and reads only the nodes on their path.
//!
//! Snapshots and subvolumes (#12). Every key carries a snapshot id; a
//! snapshot sees the keys at its own id and at each of its ancestors', and
//! where several are at one position the nearest wins: a key at a
//! descendant replaces its ancestor's, and a whiteout there hides it
//! (docs/clean-room.md, "Snapshots and subvolumes"). The plain API
//! (`lookup`, `inode`, `read`, ...) reads the root subvolume; a path into
//! another subvolume resolves to a [`Node`], an inode number with the
//! snapshot it is read at, since the same number names a file in a
//! subvolume and in each of its snapshots.

use crate::bkey::{Bkey, Bpos};
use crate::btree::{btree_id, Cursor};
use crate::error::{Error, Result};
use crate::extent::{compression, DataExtent};
use crate::inode::{Dirent, Inode, InodeV3Raw, ROOT_INO};
use crate::journal::Replay;
use crate::superblock::Superblock;
use fs_core::BlockRead;

/// A file or directory as one snapshot sees it: its inode number and the
/// snapshot its keys are read at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Node {
    pub ino: u64,
    pub snapshot: u32,
}

/// The snapshots and subvolumes btrees (S1 11.3's order: subvolumes 8,
/// snapshots 9) and their key types (S1 11.5: subvolume 21, snapshot 22).
const SUBVOLUMES: u8 = 8;
const SNAPSHOTS: u8 = 9;
const KEY_SUBVOLUME: u8 = 21;
const KEY_SNAPSHOT: u8 = 22;
/// A dirent's type when it names a subvolume (the lister's `type subvol`).
const DT_SUBVOL: u8 = 16;
/// Deeper than any snapshot tree; a parent cycle stops here.
const MAX_SNAPSHOT_DEPTH: usize = 1024;
/// The freespace btree (id 11) and the key type of its runs of free
/// buckets (`set`, 25): docs/clean-room.md, "Allocating space" (S8).
const FREESPACE: u8 = 11;
const KEY_SET: u8 = 25;

/// A device's capacity and free space, from [`Filesystem::usage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    /// The size of one bucket, the unit space is allocated in.
    pub bucket_bytes: u64,
    /// Every bucket from the member's first usable one on.
    pub total_bytes: u64,
    /// The buckets the freespace btree holds.
    pub free_bytes: u64,
}

pub struct Filesystem<D: BlockRead> {
    dev: D,
    sb: Superblock,
    /// What a replay of the journal adds, when the filesystem was not shut
    /// down cleanly.
    replay: Option<Replay>,
    /// The nodes lookups pass through, read once.
    cache: crate::btree::NodeCache,
    /// The root subvolume's snapshot, the one the plain API reads at.
    snapshot: u32,
    /// Each snapshot's parent (0 for a tree's root), from the snapshots
    /// btree; empty on a filesystem without one.
    parents: std::collections::BTreeMap<u32, u32>,
    /// This device's index and bucket size (sectors), for judging pointers.
    dev_idx: u8,
    bucket_size: u64,
    device_sectors: u64,
    /// The master key of an encrypted filesystem, stored unsealed in its
    /// crypt field: every data extent is decrypted with it (#272).
    key: Option<crate::crypt::MasterKey>,
}

impl<D: BlockRead> Filesystem<D> {
    pub fn open(dev: D) -> Result<Self> {
        let sb = Superblock::read(&dev)?;
        // An encrypted filesystem is read when its master key is stored
        // unsealed: its btree nodes by crate::btree (#271), its data here
        // (#272). One whose key a passphrase seals is refused, saying so,
        // since that is what the person holding the image can act on.
        let key = match sb.crypt()? {
            Some(crypt) if crypt.is_sealed() => {
                return Err(Error::Unsupported(
                    "the filesystem is encrypted, its master key sealed by a passphrase: \
                     passphrase unlocking is not supported"
                        .into(),
                ))
            }
            Some(crypt) => crypt.master_key(),
            None => None,
        };
        if sb.nr_devices != 1 {
            return Err(Error::Unsupported(format!(
                "{} devices: only single-device filesystems are read",
                sb.nr_devices
            )));
        }
        // The roots in the superblock are as of the last clean shutdown;
        // after an unclean one everything since is in the journal, which
        // is replayed here, in memory. So is a clean one whose superblock
        // records a root that will not read (#166).
        let replay = crate::btree::journal_for(&dev, &sb)?;
        // The root inode must be there: a filesystem without it is not
        // one to read. Its snapshot is the one every other key must carry.
        let member = sb
            .members()?
            .into_iter()
            .nth(sb.dev_idx as usize)
            .ok_or_else(|| Error::Corrupt("no member entry for this device".into()))?;
        if member.bucket_size == 0 {
            return Err(Error::Corrupt("bucket size 0".into()));
        }
        let mut fs = Filesystem {
            dev_idx: sb.dev_idx,
            bucket_size: u64::from(member.bucket_size),
            device_sectors: member
                .nbuckets
                .checked_mul(u64::from(member.bucket_size))
                .ok_or_else(|| Error::Corrupt("member size overflow".into()))?,
            dev,
            sb,
            replay,
            cache: Default::default(),
            snapshot: u32::MAX,
            parents: Default::default(),
            key,
        };
        fs.parents = fs.load_snapshots()?;
        for &snapshot in fs.parents.keys() {
            let mut seen = std::collections::BTreeSet::new();
            let mut at = snapshot;
            while at != 0 {
                if !seen.insert(at) || seen.len() > MAX_SNAPSHOT_DEPTH {
                    return Err(Error::Corrupt(
                        "snapshot parent cycle or depth limit".into(),
                    ));
                }
                at = *fs
                    .parents
                    .get(&at)
                    .ok_or_else(|| Error::Corrupt("snapshot parent missing".into()))?;
            }
        }
        fs.snapshot = fs.root_snapshot()?;
        Ok(fs)
    }

    /// Each snapshot's parent: the high half of a snapshot key's first
    /// word (S3 + S4: the kernel oracle's snapshots, against the lister's
    /// `parent`).
    fn load_snapshots(&self) -> Result<std::collections::BTreeMap<u32, u32>> {
        let mut c = match self.cursor(SNAPSHOTS) {
            Err(Error::NotFound(_)) => return Ok(Default::default()),
            r => r?,
        };
        c.seek(Bpos {
            inode: 0,
            offset: 0,
            snapshot: 0,
        })?;
        let mut out = std::collections::BTreeMap::new();
        while let Some(k) = c.next_key()? {
            if k.key_type == KEY_SNAPSHOT && k.value.len() >= 8 {
                if out.len() >= 1_000_000 {
                    return Err(Error::Unsupported("snapshot count limit".into()));
                }
                let parent = (crate::util::le64(&k.value, 0) >> 32) as u32;
                out.insert(k.pos.offset as u32, parent);
            }
        }
        Ok(out)
    }

    /// The root subvolume's snapshot: subvolume 1's, or, without a
    /// subvolumes btree, the root inode's. A root inode at a snapshot the
    /// snapshots btree does not know is refused rather than misread.
    fn root_snapshot(&self) -> Result<u32> {
        let mut c = self.cursor(btree_id::INODES)?;
        c.seek(Bpos {
            inode: 0,
            offset: ROOT_INO,
            snapshot: 0,
        })?;
        let mut roots: Vec<Bkey> = Vec::new();
        while let Some(k) = c.next_key()? {
            if k.pos.inode != 0 || k.pos.offset != ROOT_INO {
                break;
            }
            if k.key_type == crate::bkey::key_type::INODE_V3 {
                roots.push(k);
            }
        }
        let first = roots
            .first()
            .ok_or_else(|| Error::NotFound(format!("inode {ROOT_INO}")))?;
        let snap = match self.subvolume(1) {
            Ok(n) => n.snapshot,
            Err(Error::NotFound(_)) => first.pos.snapshot,
            Err(e) => return Err(e),
        };
        for k in &roots {
            if k.pos.snapshot != snap && !self.parents.contains_key(&k.pos.snapshot) {
                return Err(snapshots_not_read(k, snap));
            }
        }
        Ok(snap)
    }

    /// Subvolume `id`'s root directory: its snapshot in the high half of
    /// the first word and its root inode in the second (S3 + S4: the
    /// kernel oracle's subvolumes, against the lister's `root` and
    /// `snapshot id`).
    pub fn subvolume(&self, id: u32) -> Result<Node> {
        let mut c = match self.cursor(SUBVOLUMES) {
            Err(Error::NotFound(_)) => return Err(Error::NotFound(format!("subvolume {id}"))),
            r => r?,
        };
        c.seek(Bpos {
            inode: 0,
            offset: u64::from(id),
            snapshot: 0,
        })?;
        while let Some(k) = c.next_key()? {
            if k.pos.inode != 0 || k.pos.offset != u64::from(id) {
                break;
            }
            if k.key_type == KEY_SUBVOLUME && k.value.len() >= 16 {
                return Ok(Node {
                    ino: crate::util::le64(&k.value, 8),
                    snapshot: (crate::util::le64(&k.value, 0) >> 32) as u32,
                });
            }
        }
        Err(Error::NotFound(format!("subvolume {id}")))
    }

    /// The snapshots `snap` sees, nearest first: itself, its parent, and so
    /// on to its tree's root.
    fn chain(&self, snap: u32) -> Vec<u32> {
        let mut out = vec![snap];
        let mut at = snap;
        while let Some(&p) = self.parents.get(&at) {
            if p == 0 || out.len() >= MAX_SNAPSHOT_DEPTH || out.contains(&p) {
                break;
            }
            out.push(p);
            at = p;
        }
        out
    }

    /// The node the root subvolume's root directory is.
    pub fn root(&self) -> Node {
        Node {
            ino: ROOT_INO,
            snapshot: self.snapshot,
        }
    }

    fn node(&self, ino: u64) -> Node {
        Node {
            ino,
            snapshot: self.snapshot,
        }
    }

    pub fn superblock(&self) -> &Superblock {
        &self.sb
    }

    /// The device's capacity and free space, in bytes. Capacity is the
    /// member's buckets from its first usable one on (S3: show-super's
    /// `Buckets` and `First bucket`); free space is the buckets the
    /// freespace btree holds, its runs being keys of type `set` at
    /// `dev:end`, `size` buckets long, as the writer takes them. A
    /// filesystem with no freespace btree has none free.
    pub fn usage(&self) -> Result<Usage> {
        let member = self
            .sb
            .members()?
            .into_iter()
            .nth(self.dev_idx as usize)
            .ok_or_else(|| Error::Corrupt("no member entry for this device".into()))?;
        let bucket_bytes = u64::from(member.bucket_size) * 512;
        let buckets = member
            .nbuckets
            .saturating_sub(u64::from(member.first_bucket));
        let mut free_buckets: u64 = 0;
        match self.cursor(FREESPACE) {
            Err(Error::NotFound(_)) => {}
            Err(e) => return Err(e),
            Ok(mut c) => {
                c.seek(Bpos {
                    inode: u64::from(self.dev_idx),
                    offset: 0,
                    snapshot: 0,
                })?;
                while let Some(k) = c.next_key()? {
                    if k.pos.inode != u64::from(self.dev_idx) {
                        break;
                    }
                    if k.key_type == KEY_SET {
                        free_buckets = free_buckets.saturating_add(u64::from(k.size));
                    }
                }
            }
        }
        Ok(Usage {
            bucket_bytes,
            total_bytes: buckets.saturating_mul(bucket_bytes),
            free_bytes: free_buckets.min(buckets).saturating_mul(bucket_bytes),
        })
    }

    fn cursor(&self, id: u8) -> Result<Cursor<'_>> {
        Ok(Cursor::new(&self.dev, &self.sb, id, self.replay.as_ref())?.with_cache(&self.cache))
    }

    /// The keys of btree `id` at `inode` that a snapshot whose chain is
    /// `chain` sees, in order: at each position the nearest, none where
    /// that is a whiteout.
    fn keys_of(&self, id: u8, inode: u64, chain: &[u32]) -> Result<Vec<Bkey>> {
        let mut c = self.cursor(id)?;
        c.seek(Bpos {
            inode,
            offset: 0,
            snapshot: 0,
        })?;
        let mut out = Vec::new();
        let mut group: Vec<Bkey> = Vec::new();
        while let Some(k) = c.next_key()? {
            if k.pos.inode != inode {
                break;
            }
            if out.len() + group.len() >= 1_000_000 {
                return Err(Error::Unsupported("inode key count limit".into()));
            }
            if group.last().is_some_and(|g| g.pos.offset != k.pos.offset) {
                out.extend(nearest(chain, std::mem::take(&mut group)));
            }
            group.push(k);
        }
        out.extend(nearest(chain, group));
        Ok(out)
    }

    /// The keys at exactly `inode:offset` of btree `id`, at every snapshot.
    fn keys_at(&self, id: u8, inode: u64, offset: u64) -> Result<Vec<Bkey>> {
        let mut c = self.cursor(id)?;
        c.seek(Bpos {
            inode,
            offset,
            snapshot: 0,
        })?;
        let mut out = Vec::new();
        while let Some(k) = c.next_key()? {
            if k.pos.inode != inode || k.pos.offset != offset {
                break;
            }
            if out.len() >= 1_000_000 {
                return Err(Error::Unsupported("position key count limit".into()));
            }
            out.push(k);
        }
        Ok(out)
    }

    /// An inode of the root subvolume.
    pub fn inode(&self, ino: u64) -> Result<Inode> {
        self.inode_at(self.node(ino))
    }

    /// An inode as the snapshot `n.snapshot` sees it.
    pub fn inode_at(&self, n: Node) -> Result<Inode> {
        Inode::from_key(&self.inode_key_at(n)?)
    }

    /// Every field of an inode as stored, as the snapshot `n.snapshot` sees
    /// it: what [`Inode`] leaves out, such as the creation time, the
    /// generation and a device node's number.
    pub fn inode_raw_at(&self, n: Node) -> Result<InodeV3Raw> {
        InodeV3Raw::parse(&self.inode_key_at(n)?.value)
    }

    /// The `inode_v3` key of an inode as the snapshot `n.snapshot` sees it.
    fn inode_key_at(&self, n: Node) -> Result<Bkey> {
        let ino = n.ino;
        let keys = self.keys_at(btree_id::INODES, 0, ino)?;
        let Some(k) = nearest(&self.chain(n.snapshot), keys) else {
            return Err(Error::NotFound(format!("inode {ino}")));
        };
        match k.key_type {
            crate::bkey::key_type::INODE_V3 => Ok(k),
            // The earlier encodings (S1 11.5, 11.6: `inode` is v1,
            // `inode_v2` is 0.18 to 0.22): a filesystem older than this
            // reader decodes, which is not the same as no inode.
            crate::bkey::key_type::INODE | crate::bkey::key_type::INODE_V2 => {
                Err(Error::Unsupported(format!(
                    "inode {ino} is stored in the {} encoding (key type {}), older than the \
                     inode_v3 this reader decodes",
                    if k.key_type == crate::bkey::key_type::INODE {
                        "inode (v1)"
                    } else {
                        "inode_v2"
                    },
                    k.key_type
                )))
            }
            _ => Err(Error::NotFound(format!("inode {ino}"))),
        }
    }

    /// The entries of a directory of the root subvolume, in btree (hash)
    /// order.
    pub fn readdir(&self, dir: u64) -> Result<Vec<Dirent>> {
        self.readdir_at(self.node(dir))
    }

    /// The entries of a directory as its snapshot sees them, in btree
    /// (hash) order. An entry of type `subvol` names a subvolume, whose
    /// root [`Filesystem::resolve`] enters.
    pub fn readdir_at(&self, dir: Node) -> Result<Vec<Dirent>> {
        if !self.inode_at(dir)?.is_dir() {
            return Err(Error::Corrupt(format!(
                "inode {} is not a directory",
                dir.ino
            )));
        }
        self.keys_of(btree_id::DIRENTS, dir.ino, &self.chain(dir.snapshot))?
            .iter()
            .filter(|k| k.key_type == crate::bkey::key_type::DIRENT)
            .map(Dirent::from_key)
            .collect()
    }

    /// Resolve one component while preserving its subvolume and snapshot.
    /// LovelyCarve uses this instead of reconstructing a path or dropping the
    /// snapshot from an inode number returned by the root-subvolume API.
    pub fn lookup_at(&self, parent: Node, name: &[u8]) -> Result<Option<Node>> {
        let directory = self.inode_at(parent)?;
        if !directory.is_dir() {
            return Err(Error::Corrupt("lookup parent is not a directory".into()));
        }
        self.find(&directory, parent.snapshot, name)?
            .map(|entry| self.dirent_node(parent, &entry))
            .transpose()
    }

    /// Convert a listed name to the same node that component lookup returns.
    pub fn dirent_node(&self, parent: Node, entry: &Dirent) -> Result<Node> {
        if entry.d_type == DT_SUBVOL {
            self.subvolume(entry.inum as u32)
        } else {
            Ok(Node {
                ino: entry.inum,
                snapshot: parent.snapshot,
            })
        }
    }

    /// The entry named `name` in directory `dir`: from its name's hash slot
    /// up, past slots holding other names or `hash_whiteout`s, until the
    /// name or an empty slot (S8: names that collide take the next free
    /// offset, and a removal inside a run leaves a whiteout). A directory
    /// whose hash is not known (crc64, never seen in a fixture) is scanned.
    ///
    /// A casefolded directory places and finds an entry by its folded name
    /// (S1 2.7; S4: the slot of every entry of the `casefold` set is the
    /// SipHash of its folded name), as the reference mount finds
    /// `HELLO.txt` as `Hello.TXT`. An ASCII name folds to its lowercase
    /// (S3: every ASCII name of the set), any other by Unicode's folding
    /// ([`crate::inode::casefold`], #111). A name that is not UTF-8 has no
    /// folded form and is matched as stored, by a scan.
    ///
    /// In a snapshot (#12) each slot is what the snapshot sees there: a
    /// whiteout at a nearer snapshot is a removed entry, passed over like a
    /// `hash_whiteout`; a slot with nothing the snapshot sees ends the run.
    fn find(&self, dir: &Inode, snapshot: u32, name: &[u8]) -> Result<Option<Dirent>> {
        let node = Node {
            ino: dir.ino,
            snapshot,
        };
        let chain = self.chain(snapshot);
        let casefolded = dir.is_casefolded();
        let key = if casefolded {
            match crate::inode::casefold(name) {
                Some(k) => k,
                None => {
                    return Ok(self.readdir_at(node)?.into_iter().find(|d| d.name == name));
                }
            }
        } else {
            name.to_vec()
        };
        let found = |k: &Bkey| -> Result<Option<Dirent>> {
            let d = Dirent::from_key(k)?;
            let hit = match crate::inode::dirent_folded_name(k)? {
                Some(f) if casefolded => f == key,
                _ => d.name == name,
            };
            Ok(hit.then_some(d))
        };
        let Some(mut at) = crate::inode::name_hash(dir.hash_type(), dir.hash_seed, &key) else {
            let keys = self.keys_of(btree_id::DIRENTS, dir.ino, &chain)?;
            for k in keys
                .iter()
                .filter(|k| k.key_type == crate::bkey::key_type::DIRENT)
            {
                if let Some(d) = found(k)? {
                    return Ok(Some(d));
                }
            }
            return Ok(None);
        };
        loop {
            let keys = self.keys_at(btree_id::DIRENTS, dir.ino, at)?;
            let Some(k) = nearest_or_whiteout(&chain, keys) else {
                return Ok(None);
            };
            match k.key_type {
                crate::bkey::key_type::DIRENT => {
                    if let Some(d) = found(&k)? {
                        return Ok(Some(d));
                    }
                }
                crate::bkey::key_type::HASH_WHITEOUT | crate::bkey::key_type::WHITEOUT => {}
                _ => return Ok(None),
            }
            at += 1;
        }
    }

    /// Resolve an absolute path to an inode of the root subvolume, without
    /// following a symlink in the last component. A path into another
    /// subvolume is refused: its inode numbers repeat in its snapshots, so
    /// it is read through [`Filesystem::resolve`] and the `_at` calls.
    pub fn lookup(&self, path: &str) -> Result<u64> {
        let n = self.resolve(path)?;
        if n.snapshot != self.snapshot {
            return Err(Error::Unsupported(format!(
                "{path} is in a subvolume other than the root one (snapshot {}): read it \
                 through Filesystem::resolve",
                n.snapshot
            )));
        }
        Ok(n.ino)
    }

    /// Resolve an absolute path to the node it names, entering each
    /// subvolume on the way at its root: a dirent of type `subvol` holds
    /// the subvolume's id in the low half of its first word and its
    /// parent's in the high half (S3 + S4: the kernel oracle's `sv` and
    /// `snap`, against the lister's `sv -> 1 -> 2`).
    pub fn resolve(&self, path: &str) -> Result<Node> {
        self.resolve_bytes(path.as_bytes())
    }

    /// [`Filesystem::resolve`] for a path whose names need not be UTF-8.
    pub fn resolve_bytes(&self, path: &[u8]) -> Result<Node> {
        let missing = || Error::NotFound(String::from_utf8_lossy(path).into_owned());
        let mut n = self.root();
        for part in path.split(|b| *b == b'/').filter(|p| !p.is_empty()) {
            let dir = self.inode_at(n)?;
            if !dir.is_dir() {
                return Err(missing());
            }
            let d = self.find(&dir, n.snapshot, part)?.ok_or_else(missing)?;
            n = if d.d_type == DT_SUBVOL {
                self.subvolume(d.inum as u32)?
            } else {
                Node {
                    ino: d.inum,
                    snapshot: n.snapshot,
                }
            };
        }
        Ok(n)
    }

    /// The extended attributes of an inode of the root subvolume, in btree
    /// order.
    pub fn xattrs(&self, ino: u64) -> Result<Vec<crate::xattr::Xattr>> {
        self.xattrs_at(self.node(ino))
    }

    /// The extended attributes of an inode as its snapshot sees them.
    pub fn xattrs_at(&self, n: Node) -> Result<Vec<crate::xattr::Xattr>> {
        self.inode_at(n)?;
        self.keys_of(btree_id::XATTRS, n.ino, &self.chain(n.snapshot))?
            .iter()
            .filter(|k| k.key_type == crate::bkey::key_type::XATTR)
            .map(crate::xattr::Xattr::from_key)
            .collect()
    }

    /// The whole contents of a file (or a symlink's target) of the root
    /// subvolume.
    pub fn read(&self, ino: u64) -> Result<Vec<u8>> {
        self.read_at(self.node(ino))
    }

    /// The whole contents of a file as its snapshot sees it.
    pub fn read_at(&self, n: Node) -> Result<Vec<u8>> {
        let size = self.inode_at(n)?.size;
        // Read whole, a file is held whole. One larger than its device can
        // only be mostly holes, so reading it whole is holding zeros, and a
        // crafted size makes it up to 2^64 bytes. Refusing the allocation
        // is not enough: Linux and macOS overcommit, and a process is killed
        // when the zeros are written (#149). Such a file is read in ranges.
        if size > self.dev.size_bytes() {
            return Err(Error::Unsupported(format!(
                "inode {}: {size} bytes is more than the device holds; read it with read_range",
                n.ino
            )));
        }
        let len = usize::try_from(size)
            .map_err(|_| Error::Unsupported("file larger than memory".into()))?;
        self.read_range_at(n, 0, len)
    }

    /// Up to `len` bytes of a file from byte `offset`: empty at or past the
    /// end, shorter than `len` across it. Only the extents covering the
    /// window are read and decoded, so a reader taking a file in pieces
    /// does work proportional to the pieces, not to the file.
    pub fn read_range(&self, ino: u64, offset: u64, len: usize) -> Result<Vec<u8>> {
        self.read_range_at(self.node(ino), offset, len)
    }

    /// [`Filesystem::read_range`] as the snapshot `n.snapshot` sees the
    /// file. Each sector is read from the nearest snapshot that has an
    /// extent over it (#12): the extents of farther snapshots are laid down
    /// first and nearer ones over them, a whiteout or a reservation as
    /// zeros.
    pub fn read_range_at(&self, n: Node, offset: u64, len: usize) -> Result<Vec<u8>> {
        let ino = n.ino;
        let chain = self.chain(n.snapshot);
        let size = self.inode_at(n)?.size;
        if offset >= size {
            return Ok(Vec::new());
        }
        let end = size.min(offset.saturating_add(len as u64));
        let n = usize::try_from(end - offset)
            .map_err(|_| Error::Unsupported(format!("inode {ino}: file larger than memory")))?;
        let mut out = crate::util::zeroed(n, format_args!("inode {ino}"))?;
        // An extent key sits at its END (S1 11.4), so the first key that can
        // cover byte `offset` is the first whose position is past its sector.
        let mut c = self.cursor(btree_id::EXTENTS)?;
        c.seek(Bpos {
            inode: ino,
            offset: offset / 512 + 1,
            snapshot: 0,
        })?;
        // Extents of one file at one snapshot must not overlap (S1's
        // check_extents: "no overlaps"): two keys claiming the same sectors
        // would be written into the buffer in position order, and the later
        // one, not the newer one, would win. Refused instead (issue #60).
        let mut prev_end: std::collections::BTreeMap<usize, u64> = Default::default();
        let mut seen: Vec<(usize, Bkey)> = Vec::new();
        while let Some(k) = c.next_key()? {
            if k.pos.inode != ino {
                break;
            }
            let Some(rank) = chain.iter().position(|&s| s == k.pos.snapshot) else {
                continue;
            };
            if u64::from(k.size) > k.pos.offset {
                return Err(Error::Corrupt("extent size exceeds its logical end".into()));
            }
            if seen.len() >= 1_000_000 {
                return Err(Error::Unsupported("range extent count limit".into()));
            }
            let key_start = k.start_offset().saturating_mul(512);
            if key_start >= end {
                // Starts only grow within one snapshot; an extent of another
                // may still start before this one, so with more than one
                // snapshot in view the scan goes on to the file's last key.
                if chain.len() == 1 {
                    break;
                }
                continue;
            }
            if matches!(
                k.key_type,
                crate::bkey::key_type::EXTENT
                    | crate::bkey::key_type::INLINE_DATA
                    | crate::bkey::key_type::RESERVATION
                    | crate::bkey::key_type::ERROR
                    | crate::bkey::key_type::REFLINK_P
            ) {
                if let Some(&prev) = prev_end.get(&rank) {
                    if k.start_offset() < prev {
                        return Err(Error::Corrupt(format!(
                            "extent {} starts at sector {} before the extent before it ends at \
                             {prev}: overlapping extents, check the filesystem",
                            k.pos,
                            k.start_offset()
                        )));
                    }
                }
                prev_end.insert(rank, k.pos.offset);
            }
            seen.push((rank, k));
        }
        // Farthest snapshot first, so a nearer one's extents land over it.
        seen.sort_by_key(|(rank, _)| std::cmp::Reverse(*rank));
        for (_, k) in seen {
            let key_start = k.start_offset().saturating_mul(512);
            match k.key_type {
                crate::bkey::key_type::EXTENT => {
                    // "Reads of poisoned extents return an error rather than
                    // silently serving corrupt data" (S1 9.1.2.1); the
                    // reference's mount fails such a read (S8, the `poison`
                    // image).
                    if crate::extent::poisoned(&k.value)? {
                        return Err(Error::Io(format!(
                            "inode {ino}, sectors {}..{}: the extent is poisoned (its data failed \
                             its checksum and no good copy was left)",
                            k.start_offset(),
                            k.pos.offset
                        )));
                    }
                    let e = DataExtent::from_key(&k)?;
                    let start = e
                        .file_start
                        .checked_mul(512)
                        .ok_or_else(|| Error::Corrupt("extent logical offset overflow".into()))?;
                    let bytes = e
                        .len
                        .checked_mul(512)
                        .ok_or_else(|| Error::Corrupt("extent logical size overflow".into()))?;
                    let from = offset.saturating_sub(start).min(bytes);
                    let to = end.saturating_sub(start).min(bytes);
                    let data = self.extent_window(&k, &e, from, to)?;
                    copy_window(&mut out, offset, end, start + from, &data);
                }
                crate::bkey::key_type::INLINE_DATA => {
                    // The value is the data itself, zero-padded to a whole
                    // u64, and the key covers `size` sectors ending at its
                    // position like any extent (S3: the lister prints the
                    // bytes and a `datalen` equal to the value length; S4:
                    // the bytes match the file the mount wrote).
                    let n = k.value.len().min(k.size as usize * 512);
                    copy_window(&mut out, offset, end, key_start, &k.value[..n]);
                }
                // A reservation is space with no data yet, a whiteout hides
                // what a farther snapshot had there: both read as zeros.
                crate::bkey::key_type::RESERVATION
                | crate::bkey::key_type::WHITEOUT
                | crate::bkey::key_type::EXTENT_WHITEOUT => {
                    // Only the part in the window is zeroed: the key's own
                    // size is up to 2 TiB, and a buffer of it would abort
                    // the process (#151).
                    zero_window(&mut out, offset, end, key_start, u64::from(k.size) * 512);
                }
                // "Reads to these ranges return IO errors" (S1 9.1.2.1).
                crate::bkey::key_type::ERROR => {
                    return Err(Error::Io(format!(
                        "inode {ino}, sectors {}..{}: the data was permanently lost (an `error` \
                         extent)",
                        k.start_offset(),
                        k.pos.offset
                    )))
                }
                // Reflinked data is in the reflink btree (#7): the key
                // covers `size` sectors there from the pointer's index.
                crate::bkey::key_type::REFLINK_P => {
                    let idx = crate::extent::reflink_p_idx(&k.value)?;
                    self.read_reflinked(ino, &k, idx, &mut out, offset, end)?;
                }
                t => return Err(Error::Unsupported(format!("extent key type {t}"))),
            }
        }
        Ok(out)
    }

    /// The generation the allocator records for the bucket holding
    /// `sector`: byte 4 of the second word of the `alloc_v4` key at
    /// `dev:bucket` (docs/clean-room.md, "Allocating space"), or `None`
    /// when the bucket has no alloc key.
    fn bucket_gen(&self, sector: u64) -> Result<Option<u8>> {
        let bucket = sector / self.bucket_size;
        let mut c = self.cursor(btree_id::ALLOC)?;
        c.seek(Bpos {
            inode: u64::from(self.dev_idx),
            offset: bucket,
            snapshot: 0,
        })?;
        while let Some(k) = c.next_key()? {
            if k.pos.inode != u64::from(self.dev_idx) || k.pos.offset != bucket {
                break;
            }
            if k.key_type == crate::bkey::key_type::ALLOC_V4 {
                return Ok(crate::extent::alloc_v4_gen(&k.value));
            }
        }
        Ok(None)
    }

    /// The generation extent `e`'s pointer is checked against: the
    /// bucket's alloc key's ([`Filesystem::bucket_gen`]). Allocation
    /// information is derived data, rebuilt from the extents by the
    /// reference checker (S1 1.3.1), and one damaged alloc node must not
    /// make every file unreadable (#167): when the alloc btree cannot be
    /// read, the bucket_gens btree's generation stands in for it, and when
    /// that has none either, the extent's own data checksum does, by taking
    /// the pointer's generation as it is. An extent with no checksum is
    /// then refused with what the alloc btree said.
    fn pointer_gen(&self, e: &DataExtent) -> Result<Option<u8>> {
        let alloc = match self.bucket_gen(e.ptr.offset) {
            Ok(g) => return Ok(g),
            Err(err) => err,
        };
        if let Ok(Some(g)) = self.bucket_gens_gen(e.ptr.offset) {
            return Ok(Some(g));
        }
        match e.crc {
            Some(c) if c.csum_type != 0 => Ok(Some(e.ptr.gen)),
            _ => Err(alloc),
        }
    }

    /// The generation the bucket_gens btree records for the bucket holding
    /// `sector`: byte `bucket % 256` of the key (type 30) at `dev:bucket /
    /// 256`, which holds 256 one-byte generations (S3 + S4: the lister
    /// prints them in order, and the alloc keys of reused buckets carry the
    /// same), or `None` when there is no such key.
    fn bucket_gens_gen(&self, sector: u64) -> Result<Option<u8>> {
        const BUCKET_GENS: u8 = 14;
        const BUCKET_GENS_KEY: u8 = 30;
        let bucket = sector / self.bucket_size;
        let at = Bpos {
            inode: u64::from(self.dev_idx),
            offset: bucket >> 8,
            snapshot: 0,
        };
        let mut c = self.cursor(BUCKET_GENS)?;
        c.seek(at)?;
        Ok(c.next_key()?
            .filter(|k| {
                k.key_type == BUCKET_GENS_KEY
                    && k.pos.inode == at.inode
                    && k.pos.offset == at.offset
            })
            .and_then(|k| k.value.get((bucket & 0xff) as usize).copied()))
    }

    /// The sectors `idx..idx + k.size` of the reflink btree, which the
    /// `reflink_p` `k` of inode `ino` shows at its own sectors, copied into
    /// the window `out` of the file's bytes `offset..end`. Each `reflink_v`
    /// there is a refcount, then an extent's entries (S1 9.1.6); a gap is
    /// shared data that is missing, an error rather than zeros.
    fn read_reflinked(
        &self,
        ino: u64,
        k: &Bkey,
        idx: u64,
        out: &mut [u8],
        offset: u64,
        end: u64,
    ) -> Result<()> {
        let hi = idx + k.size as u64;
        let missing = |at: u64| {
            Error::Corrupt(format!(
                "inode {ino}: the reflink btree has nothing at sector {at}, which the \
                 `reflink_p` at {} points into",
                k.pos
            ))
        };
        let mut c = match self.cursor(btree_id::REFLINK) {
            Err(Error::NotFound(_)) => return Err(missing(idx)),
            r => r?,
        };
        c.seek(Bpos {
            inode: 0,
            offset: idx + 1,
            snapshot: 0,
        })?;
        let mut at = idx;
        while at < hi {
            let Some(v) = c.next_key()? else { break };
            let from = v.start_offset();
            if v.pos.inode != 0 || from > at {
                break;
            }
            if v.key_type != crate::bkey::key_type::REFLINK_V || v.value.len() < 8 {
                return Err(Error::Unsupported(format!(
                    "inode {ino}: reflinked data in a key of type {} at {} of the reflink \
                     btree",
                    v.key_type, v.pos
                )));
            }
            let e = DataExtent::from_key(&Bkey {
                key_type: crate::bkey::key_type::EXTENT,
                value: v.value[8..].to_vec(),
                ..v.clone()
            })?;
            // The part of this shared extent the pointer covers, at the
            // file sector it shows up at.
            let to = v.pos.offset.min(hi);
            let file_start = (k.start_offset() + at - idx)
                .checked_mul(512)
                .ok_or_else(|| Error::Corrupt("reflink logical offset overflow".into()))?;
            let covered = (to - at) * 512;
            let window_start = offset.saturating_sub(file_start).min(covered);
            let window_end = end.saturating_sub(file_start).min(covered);
            let data = self.extent_window(
                &v,
                &e,
                (at - from) * 512 + window_start,
                (at - from) * 512 + window_end,
            )?;
            copy_window(out, offset, end, file_start + window_start, &data);
            at = to;
        }
        if at < hi {
            return Err(missing(at));
        }
        Ok(())
    }

    /// The `len` live sectors of one extent, decompressed and checked; an
    /// unwritten extent's are zeros, whatever its sectors hold (#158). `k`
    /// is the key it is in, whose version is part of an encrypted extent's
    /// nonce.
    fn extent_window(&self, k: &Bkey, e: &DataExtent, from: u64, to: u64) -> Result<Vec<u8>> {
        let length = e
            .len
            .checked_mul(512)
            .ok_or_else(|| Error::Corrupt("extent length overflow".into()))?;
        if from > to || to > length {
            return Err(Error::Corrupt("extent window exceeds logical data".into()));
        }
        e.ptr.check(self.dev_idx, self.pointer_gen(e)?)?;
        let stored_sectors = e.crc.map_or(e.len, |crc| u64::from(crc.compressed_size));
        if e.ptr
            .offset
            .checked_add(stored_sectors)
            .is_none_or(|end| end > self.device_sectors)
        {
            return Err(Error::Corrupt(
                "allocated extent exceeds filesystem member".into(),
            ));
        }
        if e.ptr.is_unwritten() {
            return crate::util::zeroed(
                usize::try_from(to - from)
                    .map_err(|_| Error::Unsupported("extent window exceeds memory".into()))?,
                "unwritten extent window",
            );
        }
        if e.crc.is_none() && self.key.is_none() {
            let start = e
                .ptr
                .offset
                .checked_mul(512)
                .ok_or_else(|| Error::Corrupt("extent physical offset overflow".into()))?;
            if start
                .checked_add(length)
                .is_none_or(|end| end > self.dev.size_bytes())
            {
                return Err(Error::Corrupt("allocated extent exceeds device".into()));
            }
            let mut data = crate::util::zeroed(
                usize::try_from(to - from)
                    .map_err(|_| Error::Unsupported("extent window exceeds memory".into()))?,
                "extent window",
            )?;
            self.dev.read_at(start + from, &mut data)?;
            return Ok(data);
        }
        let decrypt = self.key.as_ref().map(|key| Decrypt {
            key,
            version_hi: k.version_hi,
            version_lo: k.version_lo,
        });
        let data = extent_bytes(e, decrypt, self.dev.size_bytes(), |at, buf| {
            Ok(self.dev.read_at(at, buf)?)
        })?;
        let from = usize::try_from(from)
            .map_err(|_| Error::Corrupt("extent window offset exceeds memory".into()))?;
        let to = usize::try_from(to)
            .map_err(|_| Error::Corrupt("extent window end exceeds memory".into()))?;
        Ok(data
            .get(from..to)
            .ok_or_else(|| Error::Corrupt("extent window exceeds decoded data".into()))?
            .to_vec())
    }
}

/// What decrypts the data extents of an encrypted filesystem: its master
/// key, and the version of the key an extent is in.
#[derive(Clone, Copy)]
pub(crate) struct Decrypt<'a> {
    pub key: &'a crate::crypt::MasterKey,
    pub version_hi: u32,
    pub version_lo: u64,
}

/// The live bytes of a data extent, read with `read(byte offset, buffer)`:
/// the stored sectors, their checksum verified, decompressed, and the
/// extent's live range cut out. The writer reads a file's contents through
/// this too, for a write into part of it (#102). Stored sectors that would
/// run past `dev_bytes`, the device's size, are refused before anything is
/// allocated for them: an extent with no checksum entry is read by the
/// key's own size, up to 2 TiB, and a buffer that size aborts the process
/// (#151).
///
/// In an encrypted filesystem, `decrypt` carries its key: the stored bytes
/// are authenticated by their MAC and decrypted before anything else is
/// done with them (S1 9.1.4: every data pointer of an encrypted filesystem
/// carries a nonce and a MAC), and an extent without one is refused. Only
/// an extent whose whole data is live has been observed encrypted; one
/// partly overwritten is refused (#275).
pub(crate) fn extent_bytes(
    e: &DataExtent,
    decrypt: Option<Decrypt<'_>>,
    dev_bytes: u64,
    read: impl Fn(u64, &mut [u8]) -> Result<()>,
) -> Result<Vec<u8>> {
    // Its checksum is a MAC, of 80 bits or of 128 (S1 9.1.4.3).
    let encrypted = e.crc.is_some_and(|c| {
        c.csum_type == crate::crypt::CSUM_CHACHA20_POLY1305_80
            || c.csum_type == crate::crypt::CSUM_CHACHA20_POLY1305_128
    });
    match (encrypted, decrypt) {
        (false, None) | (true, Some(_)) => {}
        (false, Some(_)) => {
            return Err(Error::Corrupt(format!(
                "an extent at sector {} with no MAC in an encrypted filesystem",
                e.ptr.offset
            )))
        }
        (true, None) => {
            return Err(Error::Unsupported(format!(
                "an encrypted extent at sector {}, and no master key to decrypt it",
                e.ptr.offset
            )))
        }
    }
    if let (Some(c), Some(_)) = (e.crc, decrypt) {
        if e.skip != 0 || e.len != u64::from(c.uncompressed_size) {
            return Err(Error::Unsupported(format!(
                "an encrypted extent at sector {} of which {} of {} sectors from sector {} are \
                 live: a partly overwritten encrypted extent has not been observed",
                e.ptr.offset, e.len, c.uncompressed_size, e.skip
            )));
        }
    }
    let stored_sectors = match e.crc {
        Some(c) => c.compressed_size as u64,
        None => e.len,
    };
    let at = e.ptr.offset.saturating_mul(512);
    let stored_bytes = stored_sectors.saturating_mul(512);
    const MAX_VERIFIED_EXTENT_BYTES: u64 = 64 * 1024 * 1024;
    if stored_bytes > MAX_VERIFIED_EXTENT_BYTES
        || e.crc
            .is_some_and(|crc| u64::from(crc.uncompressed_size) * 512 > MAX_VERIFIED_EXTENT_BYTES)
    {
        return Err(Error::Unsupported(
            "checksum/compression region exceeds the 64 MiB verification limit".into(),
        ));
    }
    if at.saturating_add(stored_bytes) > dev_bytes {
        return Err(Error::Corrupt(format!(
            "an extent of {stored_sectors} sectors at sector {} runs past the end of the \
             device ({dev_bytes} bytes)",
            e.ptr.offset
        )));
    }
    let mut raw = crate::util::zeroed(stored_bytes as usize, "an extent")?;
    read(at, &mut raw)?;
    let Some(crc) = e.crc else {
        return Ok(raw);
    };
    if let Some(d) = decrypt {
        crate::crypt::decrypt_extent(d.key, &crc, d.version_hi, d.version_lo, &mut raw)?;
    } else {
        if !crate::csum::is_known(crc.csum_type) {
            return Err(Error::Unsupported(format!(
                "data checksum type {}",
                crc.csum_type
            )));
        }
        // 32- and 64-bit checksums live entirely in the low word; the
        // high bits are only used by MACs, which crate::crypt checks.
        let stored = crc.csum_lo;
        crate::csum::verify(crc.csum_type, &raw, stored).map_err(|computed| {
            Error::BadChecksum {
                what: "extent data",
                stored,
                computed,
            }
        })?;
    }
    let plain = match crc.compression_type {
        compression::NONE | compression::INCOMPRESSIBLE => raw,
        t => crate::compress::decompress(t, &raw, crc.uncompressed_size as usize * 512)?,
    };
    let from = e.skip as usize * 512;
    let to = from + e.len as usize * 512;
    if to > plain.len() {
        return Err(Error::Corrupt(
            "extent's live range exceeds its data".into(),
        ));
    }
    Ok(plain[from..to].to_vec())
}

/// The refusal for a key at a snapshot other than the root's.
/// Of the keys at one position, the one a snapshot whose chain is `chain`
/// sees: the one at the snapshot nearest it, whiteouts included.
fn nearest_or_whiteout(chain: &[u32], keys: Vec<Bkey>) -> Option<Bkey> {
    keys.into_iter()
        .filter_map(|k| Some((chain.iter().position(|&s| s == k.pos.snapshot)?, k)))
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, k)| k)
}

/// [`nearest_or_whiteout`], with a whiteout read as nothing there.
fn nearest(chain: &[u32], keys: Vec<Bkey>) -> Option<Bkey> {
    nearest_or_whiteout(chain, keys).filter(|k| k.key_type != crate::bkey::key_type::WHITEOUT)
}

fn snapshots_not_read(k: &Bkey, root_snapshot: u32) -> Error {
    Error::Unsupported(format!(
        "key {} is at snapshot {}, which the snapshots btree does not have (the root \
         subvolume is at {root_snapshot}): the snapshot cannot be placed, so it is not read",
        k.pos, k.pos.snapshot
    ))
}

/// Copy the part of `data` (which starts at file byte `data_start`) that
/// falls inside the window `[win_start, win_end)` into `out`, which holds
/// that window.
/// Zero the bytes `data_start..data_start + data_len` of a file in the
/// window `out` of its bytes `win_start..win_end`: [`copy_window`] for a
/// run of zeros, without a buffer of them.
fn zero_window(out: &mut [u8], win_start: u64, win_end: u64, data_start: u64, data_len: u64) {
    let from = data_start.max(win_start);
    let to = data_start.saturating_add(data_len).min(win_end);
    if from < to {
        out[(from - win_start) as usize..(to - win_start) as usize].fill(0);
    }
}

pub(crate) fn copy_window(
    out: &mut [u8],
    win_start: u64,
    win_end: u64,
    data_start: u64,
    data: &[u8],
) {
    let data_end = data_start.saturating_add(data.len() as u64);
    let from = data_start.max(win_start);
    let to = data_end.min(win_end);
    if from >= to {
        return;
    }
    out[(from - win_start) as usize..(to - win_start) as usize]
        .copy_from_slice(&data[(from - data_start) as usize..(to - data_start) as usize]);
}
