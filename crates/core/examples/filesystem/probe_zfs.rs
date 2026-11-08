//! ZFS end-to-end probe against a raw disk image.
//!
//! Drives the vendored `zfs-rs` crate directly — no lovelycarve dispatcher
//! plumbing yet. Auto-detects the freebsd-zfs partition (or accepts an
//! explicit `--offset <bytes>`) and prints uberblocks, MOS root, and ZPL root
//! listing. Used to verify the upstream library can actually parse the
//! FreeBSD installer image before wiring it into `core::evidence::image` /
//! `core::evidence::partitions` / `core::filesystem::tree::fs_tree`.
//!
//! Usage:
//!   cargo run --example probe_zfs -- <image>
//!   cargo run --example probe_zfs -- <image> --offset 0x420b2600
//!   cargo run --example probe_zfs -- <image> --dataset zroot/ROOT/default --path /etc/rc.conf

use std::env;
use std::fs::File;
use std::io::{Read, Result as IoResult, Seek, SeekFrom};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Mutex;

use zfs_rs::dsl::DirPhys;
use zfs_rs::zap::ZapString;
use zfs_rs::{
    DefaultDMU, DefaultDSL, DefaultSPA, DefaultZAP, DefaultZPL, Device, RawDevice, ZfsDirEntry,
    ZfsDrive, ZfsDslDir, ZfsError, ZfsErrorKind,
};

use lovelycarve_core::evidence::image::{DiskImage, PartitionKind};

/// FreeBSD's freebsd-zfs GPT partition type GUID.
const FREEBSD_ZFS_GUID: &str = "516e7cba-6ecf-11d6-8ff8-00022d09712b";

/// Adapter that satisfies the upstream `zfs_rs::DefaultDrive` shape but
/// reads from a base offset inside an arbitrary file. Mirrors the upstream
/// `DefaultDrive` implementation byte-for-byte except for the pre-applied
/// `base` offset. The mutex matches `(&self.file).seek` access in the
/// upstream copy, where the inner file is borrowed shared (`&File`); we use
/// it instead because we want to seek without exclusive borrow either.
struct PartitionDrive {
    file: Mutex<File>,
    base: u64,
}

impl PartitionDrive {
    fn open<P: AsRef<Path>>(path: P, base: u64) -> IoResult<Self> {
        Ok(Self {
            file: Mutex::new(File::open(path)?),
            base,
        })
    }
}

impl RawDevice for PartitionDrive {
    type Block = Vec<u8>;
    fn read_raw(&self, addr: u64, size: u64) -> IoResult<Self::Block> {
        let mut buf = vec![0u8; size as usize];
        let mut file = self.file.lock().expect("PartitionDrive mutex poisoned");
        file.seek(SeekFrom::Start(self.base + addr))?;
        file.read_exact(&mut buf)?;
        Ok(buf)
    }
}

impl DefaultSPA for PartitionDrive {}
impl DefaultDMU for PartitionDrive {}
impl DefaultZAP for PartitionDrive {}
impl DefaultDSL for PartitionDrive {}
impl DefaultZPL for PartitionDrive {}

fn parse_offset(s: &str) -> Option<u64> {
    let trimmed = s.trim();
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
    {
        u64::from_str_radix(hex, 16).ok()
    } else {
        trimmed.parse::<u64>().ok()
    }
}

fn detect_zfs_offset(path: &Path) -> anyhow::Result<u64> {
    let image = DiskImage::open(path)?;
    let sector_size = image.sector_size as u64;
    // Prefer the freebsd-zfs GUID; fall back to whichever GPT entry is the
    // largest non-Fat32/non-Swap partition. The bare PartitionKind enum
    // already classifies Swap / Fat32 / BiosBoot, so an Unknown-kind 5 GiB
    // entry on a FreeBSD installer is the rootfs.
    let from_guid = image
        .partitions
        .iter()
        .find(|p| {
            p.kind == PartitionKind::Unknown
                && p.label
                    .as_deref()
                    .map(str::to_lowercase)
                    .unwrap_or_default()
                    == "rootfs"
        })
        .or_else(|| {
            image
                .partitions
                .iter()
                .filter(|p| {
                    !matches!(
                        p.kind,
                        PartitionKind::Fat32
                            | PartitionKind::Fat12
                            | PartitionKind::Fat16
                            | PartitionKind::Swap
                            | PartitionKind::BiosBoot
                    )
                })
                .max_by_key(|p| p.length_lba)
        })
        .ok_or_else(|| anyhow::anyhow!("no ZFS-shaped partition found"))?;
    Ok(from_guid.start_lba * sector_size)
}

fn main() -> anyhow::Result<()> {
    let mut args = env::args().skip(1);
    let path = args.next().map(PathBuf::from).ok_or_else(|| {
        anyhow::anyhow!("usage: probe_zfs <image> [--offset N] [--dataset D] [--path P]")
    })?;
    let mut offset_override: Option<u64> = None;
    let mut dataset_path = String::from("/");
    let mut zfs_fs_path: Option<String> = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--offset" => {
                let v = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--offset needs a value"))?;
                offset_override =
                    Some(parse_offset(&v).ok_or_else(|| anyhow::anyhow!("bad --offset {v}"))?);
            }
            "--dataset" => {
                dataset_path = args
                    .next()
                    .ok_or_else(|| anyhow::anyhow!("--dataset needs a value"))?;
            }
            "--path" => {
                zfs_fs_path = Some(
                    args.next()
                        .ok_or_else(|| anyhow::anyhow!("--path needs a value"))?,
                );
            }
            other => anyhow::bail!("unknown argument {other}"),
        }
    }

    let offset = match offset_override {
        Some(o) => o,
        None => detect_zfs_offset(&path)?,
    };
    println!("image:  {}", path.display());
    println!("offset: {offset} (0x{offset:x})");
    println!("zfs partition GUID lookup: {FREEBSD_ZFS_GUID} (informational)");

    let drive = PartitionDrive::open(&path, offset)
        .map_err(|err| anyhow::anyhow!("open partition drive: {err}"))?;
    let label = drive
        .get_label(0)
        .map_err(|err| anyhow::anyhow!("get_label(0): {err}"))?
        .ok_or_else(|| anyhow::anyhow!("label 0 absent"))?;
    let mut ubers = label.uberblocks;
    ubers.sort_unstable_by_key(|u| u.txg);
    let valid_count = ubers.iter().filter(|u| u.magic == 0x00BAB10C).count();
    println!(
        "uberblocks: {} total, {} valid (magic 0x00BAB10C)",
        ubers.len(),
        valid_count
    );
    if let Some(latest) = ubers.iter().rev().find(|u| u.magic == 0x00BAB10C) {
        println!(
            "latest uberblock: txg={} timestamp={} version={}",
            latest.txg, latest.timestamp, latest.version
        );
    } else {
        anyhow::bail!("no uberblock with valid magic — wrong offset?");
    }

    let zfs_drive = ZfsDrive::new(drive).map_err(|err| anyhow::anyhow!("ZfsDrive::new: {err}"))?;
    let mos = zfs_drive
        .get_most_recent_mos()
        .map_err(|err| anyhow::anyhow!("get_most_recent_mos: {err}"))?;

    let root_dir = match mos.get_root_dir() {
        Ok(dir) => dir,
        Err(_) => {
            println!("MOS root_dataset ZAP missing — scanning MOS for orphan root DSL dir");
            let os = mos.as_os();
            let header = &os.os.metadnode.header;
            let max = header.max_block_id * header.datablkszsec as u64;
            let mut found: Option<ZfsDslDir<_>> = None;
            for i in 0..max {
                if let Ok(dnode) = os.get_dnode(i) {
                    if dnode.header.kind == 12 {
                        if let Ok((_, dir)) = DirPhys::parse(&dnode.bonus) {
                            if dir.parent_obj == 0 {
                                println!("  found candidate root DSL dir at obj_id {i}");
                                found = Some(ZfsDslDir::new(&mos, dir));
                                break;
                            }
                        }
                    }
                }
            }
            found.ok_or_else(|| anyhow::anyhow!("could not locate root DSL dir"))?
        }
    };

    println!("\n== DSL children of root ==");
    let dataset_components: Vec<String> = dataset_path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let mut walked = root_dir;
    for component in &dataset_components {
        let key = ZapString::from_byte_slice(component.as_bytes())
            .ok_or_else(|| anyhow::anyhow!("dataset component {component} has NUL byte"))?;
        walked = match walked.get_child(&key) {
            Ok(Some(child)) => child,
            Ok(None) => anyhow::bail!("dataset component {component} not found"),
            Err(err) => anyhow::bail!("get_child({component}): {err}"),
        };
    }
    println!("walked dataset path: /{}", dataset_components.join("/"));

    let children = walked
        .children()
        .map_err(|err| anyhow::anyhow!("children: {err}"))?;
    if children.is_empty() {
        println!("  (no DSL children — this is a leaf dataset directory)");
    } else {
        for (name, _) in &children {
            println!("  {name:?}");
        }
    }

    println!("\n== ZPL root listing (head dataset) ==");
    let dataset = walked
        .get_head_dataset()
        .map_err(|err| anyhow::anyhow!("get_head_dataset: {err}"))?;
    let zpl = dataset
        .get_object_set()
        .map_err(|err| anyhow::anyhow!("get_object_set: {err}"))?
        .as_zpl()
        .ok_or_else(|| anyhow::anyhow!("dataset object set is not ZPL"))?;
    let zpl_root = zpl
        .get_root()
        .map_err(|err| anyhow::anyhow!("zpl get_root: {err}"))?;

    let entries = zpl_root
        .children()
        .map_err(|err| anyhow::anyhow!("zpl children: {err}"))?;
    let mut shown = 0usize;
    for (name, entry) in &entries {
        let kind = match entry {
            ZfsDirEntry::Dir(_) => "dir",
            ZfsDirEntry::File(_) => "file",
        };
        println!("  {name:?}  ({kind})");
        shown += 1;
        if shown >= 40 {
            println!("  ... ({} more)", entries.len() - shown);
            break;
        }
    }
    println!("zpl root entry count: {}", entries.len());

    if let Some(zfs_fs_path) = zfs_fs_path {
        println!("\n== Reading file: {zfs_fs_path} ==");
        let mut cursor = ZfsDirEntry::Dir(zpl_root);
        for component in zfs_fs_path.split('/').filter(|s| !s.is_empty()) {
            let dir = match cursor {
                ZfsDirEntry::Dir(d) => d,
                ZfsDirEntry::File(_) => {
                    anyhow::bail!("path component {component} expected dir, got file")
                }
            };
            let key = ZapString::from_byte_slice(component.as_bytes())
                .ok_or_else(|| anyhow::anyhow!("path {component} contains NUL"))?;
            cursor = match dir
                .get_child(&key)
                .map_err(|e: ZfsError| anyhow::anyhow!("get_child({component}): {e}"))?
            {
                Some(c) => c,
                None => {
                    let _ = ZfsErrorKind::NotFound;
                    anyhow::bail!("not found: {component}")
                }
            };
        }
        match cursor {
            ZfsDirEntry::Dir(_) => println!("(target is a directory; pass --path /file to read)"),
            ZfsDirEntry::File(file) => {
                let mut reader = file.reader().map_err(|e| anyhow::anyhow!("reader: {e}"))?;
                let mut buf = Vec::with_capacity(8192);
                std::io::Read::take(&mut reader, 4096)
                    .read_to_end(&mut buf)
                    .map_err(|e| anyhow::anyhow!("file read: {e}"))?;
                println!("  read {} bytes", buf.len());
                let preview = buf.iter().take(256).copied().collect::<Vec<_>>();
                let mut as_text = String::new();
                let printable = preview
                    .iter()
                    .all(|b| (0x20..=0x7e).contains(b) || matches!(b, b'\n' | b'\r' | b'\t'));
                if printable {
                    as_text.push_str(&String::from_utf8_lossy(&preview));
                    println!("  preview (text): {as_text}");
                } else {
                    let hex = preview
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ");
                    println!("  preview (hex): {hex}");
                }
            }
        }
    }

    Ok(())
}
