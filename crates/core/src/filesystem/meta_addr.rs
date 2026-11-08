//! MetaAddr addressing model: `(image, volId, metaAddr, metaAddrHigh)`.
//!
//! Metadata filtering lives in
//! SQLite (`filedb`), content reads are addressed by a four-tuple that goes
//! straight to the filesystem parser:
//!
//! * `vol_id` is the index into the image's filesystem array ([`crate::filesystem::fs_all`]).
//! * `meta_addr` is the filesystem metadata address (NTFS MFT record, ext4 /
//!   btrfs inode). Deleted files keep a valid address.
//! * `meta_addr_high` is the extension word for sub-structures: an NTFS named
//!   stream hash, a btrfs subvolume id. `0` means a plain metadata address.
//! * Unallocated clusters, disk gaps, filesystem system files and deleted
//!   FAT/exFAT runs do **not** go through a metadata address; they are block
//!   addressed by `(offset, length)`, carried in the same two columns.
//! * Path-addressed filesystems (FAT, exFAT, ReFS, APFS, HFS+, UFS, XFS, ZFS,
//!   CDFS, SquashFS, archives) have no numeric address: `meta_addr = 0` and the
//!   `full_path` column resolves the entry.
//!
//! The node id string minted by each volume adapter remains the identity used
//! across IPC; this module classifies it into MetaAddr columns on the way into
//! the index and reconstructs it from a MetaAddr on the way out.

use serde::{Deserialize, Serialize};

use crate::filesystem::deleted::DELETED_RUN_PREFIX;
use crate::filesystem::unalloc::{DISK_GAP_PREFIX, SYSFILE_PREFIX, UNALLOC_PREFIX};

/// `is_delete` column semantics: `2` = live entry (default), `1` = deleted.
pub const IS_DELETE_LIVE: i64 = 2;
pub const IS_DELETE_DELETED: i64 = 1;

pub fn is_delete_value(deleted: bool) -> i64 {
    if deleted {
        IS_DELETE_DELETED
    } else {
        IS_DELETE_LIVE
    }
}

pub fn is_deleted_value(is_delete: i64) -> bool {
    is_delete == IS_DELETE_DELETED
}

/// The physical addressing class of a node id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaAddrKind {
    /// Filesystem metadata address. `fs` is the adapter prefix (`ntfs`,
    /// `ext4`, `btrfs`); `high` carries the sub-structure word.
    Meta {
        fs: &'static str,
        addr: u64,
        high: u64,
    },
    /// Block-level `(offset, length)` addressing on the raw image.
    Block { offset: u64, length: u64 },
    /// Path-addressed entry; resolved through `full_path`.
    Path,
}

impl MetaAddrKind {
    /// The `(meta_addr, meta_addr_high)` column pair.
    pub fn columns(&self) -> (i64, i64) {
        match self {
            Self::Meta { addr, high, .. } => (wrap_i64(*addr), wrap_i64(*high)),
            Self::Block { offset, length } => (wrap_i64(*offset), wrap_i64(*length)),
            Self::Path => (0, 0),
        }
    }
}

/// Resolved four-tuple address. `image_id` stands in for `imagePath`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaAddr {
    pub image_id: String,
    pub vol_id: i64,
    pub meta_addr: i64,
    pub meta_addr_high: i64,
}

/// `QueryFileSt` analog: everything a content read needs. `nid` is the
/// adapter node id when known; otherwise the address (plus `full_path` for
/// path-addressed filesystems) must resolve it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileQuery {
    pub image_id: String,
    pub vol_id: i64,
    pub meta_addr: i64,
    pub meta_addr_high: i64,
    #[serde(default)]
    pub nid: Option<String>,
    #[serde(default)]
    pub full_path: Option<String>,
    #[serde(default = "default_is_delete")]
    pub is_delete: i64,
}

fn default_is_delete() -> i64 {
    IS_DELETE_LIVE
}

/// Classify a node id into its addressing class.
pub fn classify(node_id: &str) -> MetaAddrKind {
    if let Some((offset, length)) = crate::filesystem::unalloc::parse_virtual_unalloc_id(node_id)
        .or_else(|| crate::filesystem::deleted::parse_run_id(node_id))
    {
        return MetaAddrKind::Block { offset, length };
    }
    if let Some((_, record, stream)) = crate::filesystem::volumes::ntfs::parse_ntfs_ads_id(node_id)
    {
        return MetaAddrKind::Meta {
            fs: "ntfs",
            addr: record,
            high: fnv1a64(stream.as_bytes()),
        };
    }
    if let Some((_, record)) = crate::filesystem::volumes::ntfs::parse_ntfs_id(node_id) {
        return MetaAddrKind::Meta {
            fs: "ntfs",
            addr: record,
            high: 0,
        };
    }
    let canonical = crate::filesystem::volumes::ext4::canonical_ext4_id(node_id);
    if let Some((_, inode)) = crate::filesystem::volumes::ext4::parse_ext4_id(&canonical) {
        return MetaAddrKind::Meta {
            fs: "ext4",
            addr: inode,
            high: 0,
        };
    }
    if let Some((subvol, inode)) = parse_btrfs_tail(node_id) {
        return MetaAddrKind::Meta {
            fs: "btrfs",
            addr: inode,
            high: subvol,
        };
    }
    MetaAddrKind::Path
}

/// `btrfs:{source}:{subvol}:{inode}` — only the numeric tail matters here; the
/// source is recovered from the volume key at reconstruction time.
fn parse_btrfs_tail(node_id: &str) -> Option<(u64, u64)> {
    let rest = node_id.strip_prefix("btrfs:")?;
    let mut parts = rest.rsplit(':');
    let inode = parts.next()?.parse().ok()?;
    let subvol = parts.next()?.parse().ok()?;
    Some((subvol, inode))
}

/// Reconstruct the adapter node id for a numeric or block address given the
/// volume's key (`part:N` / `lvm:N:M`) and filesystem prefix. Returns `None`
/// for path-addressed entries and for sub-structure addresses that cannot be
/// inverted (an NTFS stream hash); those resolve through `filedb`.
pub fn reconstruct_node_id(
    volume_key: &str,
    fs_prefix: &str,
    kind: &MetaAddrKind,
) -> Option<String> {
    match kind {
        MetaAddrKind::Block { offset, length } => Some(format!(
            "{UNALLOC_PREFIX}{volume_key}:{offset:#x}:{length:#x}"
        )),
        MetaAddrKind::Meta { addr, high, .. } => match fs_prefix {
            "ntfs" if *high == 0 => Some(format!("ntfs:{volume_key}:{addr}")),
            "ext4" if *high == 0 => Some(format!("ext4:{volume_key}:{addr}")),
            "btrfs" => Some(format!("btrfs:{volume_key}:{high}:{addr}")),
            _ => None,
        },
        MetaAddrKind::Path => None,
    }
}

/// Decode the `(meta_addr, meta_addr_high)` column pair back into an
/// addressing class for a given filesystem prefix. Block prefixes are decided
/// by the caller (a node whose id carries a block prefix is `Block`).
pub fn kind_from_columns(fs_prefix: &str, meta_addr: i64, meta_addr_high: i64) -> MetaAddrKind {
    match fs_prefix {
        "block" => MetaAddrKind::Block {
            offset: unwrap_u64(meta_addr),
            length: unwrap_u64(meta_addr_high),
        },
        "ntfs" | "ext4" | "btrfs" => MetaAddrKind::Meta {
            fs: match fs_prefix {
                "ntfs" => "ntfs",
                "ext4" => "ext4",
                _ => "btrfs",
            },
            addr: unwrap_u64(meta_addr),
            high: unwrap_u64(meta_addr_high),
        },
        _ => MetaAddrKind::Path,
    }
}

/// Whether `node_id` uses block addressing (any of the byte-range prefixes).
pub fn is_block_addressed(node_id: &str) -> bool {
    node_id.starts_with(UNALLOC_PREFIX)
        || node_id.starts_with(DISK_GAP_PREFIX)
        || node_id.starts_with(SYSFILE_PREFIX)
        || node_id.starts_with(DELETED_RUN_PREFIX)
}

fn wrap_i64(value: u64) -> i64 {
    i64::from_ne_bytes(value.to_ne_bytes())
}

fn unwrap_u64(value: i64) -> u64 {
    u64::from_ne_bytes(value.to_ne_bytes())
}

/// Stable 64-bit hash for stream names carried in `meta_addr_high`.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ntfs_record_is_meta_address() {
        assert_eq!(
            classify("ntfs:part:2:1234"),
            MetaAddrKind::Meta {
                fs: "ntfs",
                addr: 1234,
                high: 0
            }
        );
        assert_eq!(
            classify("ntfs:lvm:1:0:77"),
            MetaAddrKind::Meta {
                fs: "ntfs",
                addr: 77,
                high: 0
            }
        );
    }

    #[test]
    fn ntfs_stream_uses_high_word() {
        let id = format!("ntfs-ads:part:2:1234:{}", hex::encode("Zone.Identifier"));
        let kind = classify(&id);
        assert_eq!(
            kind,
            MetaAddrKind::Meta {
                fs: "ntfs",
                addr: 1234,
                high: fnv1a64(b"Zone.Identifier")
            }
        );
        assert_eq!(reconstruct_node_id("part:2", "ntfs", &kind), None);
    }

    #[test]
    fn ext4_and_recover_ids_share_inode_address() {
        assert_eq!(
            classify("ext4:part:1:2"),
            MetaAddrKind::Meta {
                fs: "ext4",
                addr: 2,
                high: 0
            }
        );
        assert_eq!(
            classify("recover:ext4:lvm:1:0:99:extra"),
            MetaAddrKind::Meta {
                fs: "ext4",
                addr: 99,
                high: 0
            }
        );
    }

    #[test]
    fn btrfs_subvolume_rides_in_high_word() {
        let kind = classify("btrfs:part:3:256:1024");
        assert_eq!(
            kind,
            MetaAddrKind::Meta {
                fs: "btrfs",
                addr: 1024,
                high: 256
            }
        );
        assert_eq!(
            reconstruct_node_id("part:3", "btrfs", &kind).as_deref(),
            Some("btrfs:part:3:256:1024")
        );
    }

    #[test]
    fn block_prefixes_are_offset_length_pairs() {
        for id in [
            "unalloc:part:1:0x1000:0x2000",
            "diskgap:0x1000:0x2000",
            "sysfile:part:1:0x1000:0x2000",
            "deleted:run:part:1:0x1000:0x2000",
        ] {
            assert_eq!(
                classify(id),
                MetaAddrKind::Block {
                    offset: 0x1000,
                    length: 0x2000
                },
                "{id}"
            );
            assert!(is_block_addressed(id));
        }
        let kind = MetaAddrKind::Block {
            offset: 0x1000,
            length: 0x2000,
        };
        assert_eq!(
            reconstruct_node_id("part:1", "block", &kind).as_deref(),
            Some("unalloc:part:1:0x1000:0x2000")
        );
    }

    #[test]
    fn path_filesystems_have_zero_address() {
        assert_eq!(classify("fat:part:0:/DIR/FILE.TXT"), MetaAddrKind::Path);
        assert_eq!(MetaAddrKind::Path.columns(), (0, 0));
        assert_eq!(
            reconstruct_node_id("part:0", "fat", &MetaAddrKind::Path),
            None
        );
    }

    #[test]
    fn columns_round_trip_through_i64() {
        let kind = MetaAddrKind::Meta {
            fs: "ntfs",
            addr: u64::MAX - 1,
            high: 0,
        };
        let (addr, high) = kind.columns();
        assert_eq!(kind_from_columns("ntfs", addr, high), kind);
        let block = MetaAddrKind::Block {
            offset: 7,
            length: 9,
        };
        let (addr, high) = block.columns();
        assert_eq!(kind_from_columns("block", addr, high), block);
    }

    #[test]
    fn is_delete_semantics() {
        assert_eq!(is_delete_value(false), 2);
        assert_eq!(is_delete_value(true), 1);
        assert!(is_deleted_value(1));
        assert!(!is_deleted_value(2));
    }
}
