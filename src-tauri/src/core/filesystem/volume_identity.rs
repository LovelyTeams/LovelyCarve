//! Volume UUIDs and labels as an operating system's mount table names
//! them: `UUID=` / `LABEL=` in Linux `/etc/fstab`.
//!
//! The values are read straight from each filesystem's superblock or boot
//! sector, in the spelling the system tools print: ext2/3/4, XFS and Btrfs
//! UUIDs as lowercase `8-4-4-4-12`, FAT and exFAT serials as `XXXX-XXXX`,
//! NTFS serials as 16 uppercase hex digits. Unreadable or unknown volumes
//! have no identity; matching is case-insensitive.

use crate::core::evidence::{image::DiskImage, partitions::PartitionKind};

/// A volume's identity as mount tables refer to it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VolumeIdentity {
    pub uuid: Option<String>,
    pub label: Option<String>,
}

impl VolumeIdentity {
    /// Whether `UUID=<value>` names this volume.
    pub fn has_uuid(&self, value: &str) -> bool {
        self.uuid
            .as_deref()
            .is_some_and(|uuid| uuid.eq_ignore_ascii_case(value.trim()))
    }

    /// Whether `LABEL=<value>` names this volume.
    pub fn has_label(&self, value: &str) -> bool {
        self.label.as_deref() == Some(value.trim())
    }
}

/// Reads the identity of the filesystem of `kind` at `offset`.
pub fn read_identity(image: &DiskImage, kind: PartitionKind, offset: u64) -> VolumeIdentity {
    let read = |at: u64, len: usize| -> Option<Vec<u8>> {
        let bytes = image.read_range(offset.checked_add(at)?, len).ok()?;
        (bytes.len() == len).then_some(bytes)
    };
    match kind {
        PartitionKind::Ext4 => read(1024, 0x88).and_then(|superblock| ext(&superblock)),
        PartitionKind::Xfs => read(0, 120).and_then(|superblock| xfs(&superblock)),
        PartitionKind::Btrfs => read(0x10000, 0x22B).and_then(|superblock| btrfs(&superblock)),
        PartitionKind::Fat12 | PartitionKind::Fat16 | PartitionKind::Fat32 => {
            read(0, 512).and_then(|boot| fat(&boot))
        }
        PartitionKind::ExFat => read(0, 512).and_then(|boot| exfat(&boot)),
        PartitionKind::Ntfs => read(0, 512).and_then(|boot| ntfs(&boot)),
        _ => None,
    }
    .unwrap_or_default()
}

fn ext(superblock: &[u8]) -> Option<VolumeIdentity> {
    if superblock.get(0x38..0x3A)? != [0x53, 0xEF] {
        return None;
    }
    Some(VolumeIdentity {
        uuid: uuid(superblock.get(0x68..0x78)?),
        label: text(superblock.get(0x78..0x88)?),
    })
}

fn xfs(superblock: &[u8]) -> Option<VolumeIdentity> {
    if superblock.get(0..4)? != b"XFSB" {
        return None;
    }
    Some(VolumeIdentity {
        uuid: uuid(superblock.get(32..48)?),
        label: text(superblock.get(108..120)?),
    })
}

fn btrfs(superblock: &[u8]) -> Option<VolumeIdentity> {
    if superblock.get(0x40..0x48)? != b"_BHRfS_M" {
        return None;
    }
    Some(VolumeIdentity {
        uuid: uuid(superblock.get(0x20..0x30)?),
        label: text(superblock.get(0x12B..0x22B)?),
    })
}

fn fat(boot: &[u8]) -> Option<VolumeIdentity> {
    let (serial, label) = if boot.get(0x52..0x57)? == b"FAT32" {
        (0x43, 0x47)
    } else if boot.get(0x36..0x39)? == b"FAT" {
        (0x27, 0x2B)
    } else {
        return None;
    };
    let label = text(boot.get(label..label + 11)?).filter(|label| label != "NO NAME");
    Some(VolumeIdentity {
        uuid: Some(serial_text(u32::from_le_bytes(
            boot.get(serial..serial + 4)?.try_into().ok()?,
        ))),
        label,
    })
}

fn exfat(boot: &[u8]) -> Option<VolumeIdentity> {
    if boot.get(3..11)? != b"EXFAT   " {
        return None;
    }
    Some(VolumeIdentity {
        uuid: Some(serial_text(u32::from_le_bytes(
            boot.get(0x64..0x68)?.try_into().ok()?,
        ))),
        // The exFAT label lives in a root directory entry, not the boot
        // sector; fstab entries of removable media use the serial.
        label: None,
    })
}

fn ntfs(boot: &[u8]) -> Option<VolumeIdentity> {
    if boot.get(3..11)? != b"NTFS    " {
        return None;
    }
    let serial = u64::from_le_bytes(boot.get(0x48..0x50)?.try_into().ok()?);
    Some(VolumeIdentity {
        uuid: Some(format!("{serial:016X}")),
        label: None,
    })
}

fn serial_text(serial: u32) -> String {
    format!("{:04X}-{:04X}", serial >> 16, serial & 0xFFFF)
}

fn uuid(bytes: &[u8]) -> Option<String> {
    let bytes: [u8; 16] = bytes.try_into().ok()?;
    (bytes != [0; 16]).then(|| uuid::Uuid::from_bytes(bytes).to_string())
}

fn text(bytes: &[u8]) -> Option<String> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let text = String::from_utf8_lossy(&bytes[..end]).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_xfs_and_btrfs_superblocks_name_their_volumes() {
        let mut superblock = vec![0_u8; 0x88];
        superblock[0x38..0x3A].copy_from_slice(&[0x53, 0xEF]);
        superblock[0x68..0x78].copy_from_slice(&[
            0x1b, 0x4e, 0x28, 0xba, 0x2f, 0xa1, 0x11, 0xd2, 0x88, 0x3f, 0x00, 0x16, 0xd3, 0xcc,
            0x42, 0x7e,
        ]);
        superblock[0x78..0x7C].copy_from_slice(b"root");
        let identity = ext(&superblock).expect("ext");
        assert!(identity.has_uuid("1B4E28BA-2FA1-11D2-883F-0016D3CC427E"));
        assert!(identity.has_label("root"));
        assert_eq!(ext(&[0; 0x88]), None);

        let mut xfs_block = vec![0_u8; 120];
        xfs_block[0..4].copy_from_slice(b"XFSB");
        xfs_block[32] = 1;
        xfs_block[108..112].copy_from_slice(b"boot");
        let identity = xfs(&xfs_block).expect("xfs");
        assert_eq!(
            identity.uuid.as_deref(),
            Some("01000000-0000-0000-0000-000000000000")
        );
        assert_eq!(identity.label.as_deref(), Some("boot"));

        let mut btrfs_block = vec![0_u8; 0x22B];
        btrfs_block[0x40..0x48].copy_from_slice(b"_BHRfS_M");
        btrfs_block[0x20] = 0xab;
        let identity = btrfs(&btrfs_block).expect("btrfs");
        assert!(identity.uuid.is_some_and(|uuid| uuid.starts_with("ab")));
    }

    #[test]
    fn fat_exfat_and_ntfs_serials_use_the_system_spelling() {
        let mut boot = vec![0_u8; 512];
        boot[0x52..0x57].copy_from_slice(b"FAT32");
        boot[0x43..0x47].copy_from_slice(&0x1234_ABCD_u32.to_le_bytes());
        boot[0x47..0x52].copy_from_slice(b"EFI        ");
        let identity = fat(&boot).expect("fat32");
        assert_eq!(identity.uuid.as_deref(), Some("1234-ABCD"));
        assert_eq!(identity.label.as_deref(), Some("EFI"));

        let mut boot = vec![0_u8; 512];
        boot[3..11].copy_from_slice(b"NTFS    ");
        boot[0x48..0x50].copy_from_slice(&0x0123_4567_89AB_CDEF_u64.to_le_bytes());
        assert_eq!(
            ntfs(&boot).expect("ntfs").uuid.as_deref(),
            Some("0123456789ABCDEF")
        );

        let mut boot = vec![0_u8; 512];
        boot[3..11].copy_from_slice(b"EXFAT   ");
        boot[0x64..0x68].copy_from_slice(&0xDEAD_BEEF_u32.to_le_bytes());
        assert_eq!(
            exfat(&boot).expect("exfat").uuid.as_deref(),
            Some("DEAD-BEEF")
        );
    }
}
