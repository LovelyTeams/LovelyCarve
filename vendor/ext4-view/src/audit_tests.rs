//! Regression coverage for the libfsext capability audit.

use crate::{Ext4, ExtendedAttribute};
use alloc::{boxed::Box, vec, vec::Vec};

const FIXTURE: &[u8] = include_bytes!("../test_data/audit_ext.bin");

fn inode_offset(number: usize) -> usize {
    4096 + (number - 1) * 256
}

#[test]
fn inline_files_seek_across_both_resident_regions() {
    let fs = Ext4::load(Box::new(FIXTURE.to_vec())).unwrap();
    assert_eq!(fs.read("/inline").unwrap(), (0..80).collect::<Vec<u8>>());
    let mut file = fs.open_inode_by_index(12).unwrap();
    file.seek_to(57).unwrap();
    let mut bytes = [0; 10];
    assert_eq!(file.read_bytes(&mut bytes).unwrap(), 10);
    assert_eq!(bytes, [57, 58, 59, 60, 61, 62, 63, 64, 65, 66]);
    file.seek_to(u64::MAX).unwrap();
    assert_eq!(file.read_bytes(&mut bytes).unwrap(), 0);
    assert_eq!(
        fs.extended_attributes(12).unwrap(),
        vec![
            ExtendedAttribute {
                name: "system.data".into(),
                value: (60..80).collect()
            },
            ExtendedAttribute {
                name: "trusted.flag".into(),
                value: b"y".to_vec()
            },
        ]
    );
}

#[test]
fn inline_directories_enumerate_and_resolve_the_xattr_region() {
    let fs = Ext4::load(Box::new(FIXTURE.to_vec())).unwrap();
    let names = fs
        .read_dir("/compact")
        .unwrap()
        .map(|entry| entry.unwrap().file_name().as_ref().to_vec())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            b".".to_vec(),
            b"..".to_vec(),
            b"base".to_vec(),
            b"tail".to_vec()
        ]
    );
    assert_eq!(
        fs.read("/compact/base").unwrap(),
        (0..80).collect::<Vec<u8>>()
    );
    assert_eq!(fs.metadata("/compact/tail").unwrap().len(), 0);
    assert_eq!(
        fs.read_link_inode(18).unwrap().as_ref(),
        [b"/".as_slice(), &[b'p'; 79]].concat()
    );
}

#[test]
fn ea_inode_attributes_return_exact_binary_values() {
    let fs = Ext4::load(Box::new(FIXTURE.to_vec())).unwrap();
    assert_eq!(
        fs.extended_attributes(16).unwrap(),
        vec![ExtendedAttribute {
            name: "user.large".into(),
            value: (0..=255).cycle().take(1024).collect(),
        }]
    );
}

#[test]
fn unwritten_extents_and_missing_indirect_subtrees_are_zero() {
    let fs = Ext4::load(Box::new(FIXTURE.to_vec())).unwrap();
    assert_eq!(
        fs.read("/unwritten").unwrap(),
        [vec![0; 1024], vec![b'W'; 1024]].concat()
    );
    let expected = (1..=12)
        .flat_map(|byte| vec![byte; 1024])
        .chain(vec![b'L'; 1024])
        .chain(vec![0; 1024])
        .collect::<Vec<_>>();
    assert_eq!(fs.read("/legacy").unwrap(), expected);
    let mut bytes = FIXTURE.to_vec();
    // Missing single-indirect root: the entire final region is a hole.
    bytes[inode_offset(15) + 88..inode_offset(15) + 92].fill(0);
    let fs = Ext4::load(Box::new(bytes)).unwrap();
    assert_eq!(&fs.read("/legacy").unwrap()[12 * 1024..], vec![0; 2048]);
}

#[test]
fn corrupt_inline_tables_and_ea_references_fail() {
    for (offset, replacement) in [
        (inode_offset(12) + 166, 4u16.to_le_bytes().to_vec()), // value overlaps entries
        (inode_offset(12) + 172, 99u32.to_le_bytes().to_vec()), // past inode end
        (inode_offset(12) + 160, 0u32.to_le_bytes().to_vec()), // missing system.data
        (inode_offset(12) + 128, 256u16.to_le_bytes().to_vec()), // extra fields out of bounds
    ] {
        let mut bytes = FIXTURE.to_vec();
        bytes[offset..offset + replacement.len()].copy_from_slice(&replacement);
        let fs = Ext4::load(Box::new(bytes)).unwrap();
        assert!(fs.read("/inline").is_err());
    }
    for value_inode in [16u32, 12, u32::MAX] {
        let mut bytes = FIXTURE.to_vec();
        bytes[30 * 1024 + 36..30 * 1024 + 40].copy_from_slice(&value_inode.to_le_bytes());
        let fs = Ext4::load(Box::new(bytes)).unwrap();
        assert!(fs.extended_attributes(16).is_err());
    }
}

#[test]
fn legacy_directory_entries_and_revision_zero_are_readable() {
    let mut bytes = FIXTURE.to_vec();
    bytes[1024 + 96..1024 + 100].fill(0); // no FILETYPE, inline or EA feature
    // Convert root to the ext2 16-bit name-length layout.
    let mut offset = 20 * 1024;
    while offset < 21 * 1024 {
        bytes[offset + 7] = 0;
        offset += usize::from(u16::from_le_bytes([bytes[offset + 4], bytes[offset + 5]]));
    }
    let fs = Ext4::load(Box::new(bytes.clone())).unwrap();
    assert_eq!(fs.read_dir("/").unwrap().count(), 8);
    assert_eq!(fs.read("/legacy").unwrap().len(), 14 * 1024);
    // Static revision records omit dynamic inode geometry; their size is 128.
    bytes[1024 + 76..1024 + 80].fill(0);
    bytes[1024 + 88..1024 + 90].fill(0);
    let fs = Ext4::load(Box::new(bytes)).unwrap();
    assert_eq!(fs.0.superblock.inode_size, 128);
}

#[test]
fn timestamps_use_signed_epoch_bits_nanoseconds_and_extra_isize() {
    let mut bytes = FIXTURE.to_vec();
    let start = inode_offset(12);
    bytes[start + 16..start + 20].copy_from_slice(&0x8000_0000u32.to_le_bytes());
    bytes[start + 136..start + 140].copy_from_slice(&((123_456_789u32 << 2) | 1).to_le_bytes());
    bytes[start + 144..start + 148].copy_from_slice(&10u32.to_le_bytes());
    let fs = Ext4::load(Box::new(bytes.clone())).unwrap();
    let metadata = fs.inode_metadata(12).unwrap();
    assert_eq!(
        (
            metadata.modified().seconds(),
            metadata.modified().nanoseconds()
        ),
        (2_147_483_648, 123_456_789)
    );
    assert_eq!(metadata.created().unwrap().seconds(), 10);
    bytes[start + 128..start + 130].fill(0); // trailing bytes are xattrs/reserved, not time fields
    let fs = Ext4::load(Box::new(bytes)).unwrap();
    let metadata = fs.inode_metadata(12).unwrap();
    assert_eq!(
        (
            metadata.modified().seconds(),
            metadata.modified().nanoseconds(),
            metadata.created()
        ),
        (-2_147_483_648, 0, None)
    );
}

#[test]
fn metadata_checksum_covers_the_external_attribute_block() {
    use crate::checksum::Checksum;
    let mut bytes = FIXTURE.to_vec();
    bytes[1024 + 100..1024 + 104].copy_from_slice(&0x400u32.to_le_bytes());
    let mut seed = Checksum::new();
    seed.update(&[0; 16]);
    let seed = seed.finalize();
    for number in [16, 17] {
        let start = inode_offset(number);
        let mut checksum = Checksum::with_seed(seed);
        checksum.update_u32_le(number as u32);
        checksum.update_u32_le(0); // generation
        checksum.update(&bytes[start..start + 256]);
        let value = checksum.finalize();
        bytes[start + 124..start + 126].copy_from_slice(&(value as u16).to_le_bytes());
        bytes[start + 130..start + 132].copy_from_slice(&((value >> 16) as u16).to_le_bytes());
    }
    let mut checksum = Checksum::with_seed(seed);
    checksum.update(&30u64.to_le_bytes());
    checksum.update(&bytes[30 * 1024..31 * 1024]);
    bytes[30 * 1024 + 16..30 * 1024 + 20].copy_from_slice(&checksum.finalize().to_le_bytes());
    let mut checksum = Checksum::with_seed(seed);
    checksum.update_u32_le(0);
    checksum.update(&bytes[2048..2080]);
    bytes[2078..2080].copy_from_slice(&(checksum.finalize() as u16).to_le_bytes());
    let mut checksum = Checksum::new();
    checksum.update(&bytes[1024..1024 + 1020]);
    bytes[1024 + 1020..2048].copy_from_slice(&checksum.finalize().to_le_bytes());
    let fs = Ext4::load(Box::new(bytes.clone())).unwrap();
    assert_eq!(fs.extended_attributes(16).unwrap()[0].value.len(), 1024);
    bytes[30 * 1024 + 400] ^= 1;
    let fs = Ext4::load(Box::new(bytes)).unwrap();
    assert!(fs.extended_attributes(16).is_err());
}

#[test]
fn meta_bg_descriptors_follow_sparse_and_sparse_super2_layouts() {
    for (compatible, read_only, backup_group, has_super) in [
        (0u32, 0u32, 0u32, true),
        (0, 1, 0, false),
        (0x200, 0, 32, true),
    ] {
        let mut bytes = FIXTURE.to_vec();
        bytes.resize(2113 * 1024, 0);
        for (offset, value) in [
            (0, 1056),
            (4, 2113),
            (92, compatible),
            (96, 0x8452),
            (100, read_only),
            (260, 1),
            (588, backup_group),
        ] {
            bytes[1024 + offset..1024 + offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        let descriptor = (2049 + usize::from(has_super)) * 1024;
        bytes[descriptor + 8..descriptor + 12].copy_from_slice(&2060u32.to_le_bytes());
        let original = bytes[inode_offset(12)..inode_offset(12) + 256].to_vec();
        bytes[2060 * 1024..2060 * 1024 + 256].copy_from_slice(&original);
        let fs = Ext4::load(Box::new(bytes)).unwrap();
        let mut file = fs.open_inode_by_index(1025).unwrap();
        let mut payload = [0; 80];
        assert_eq!(file.read_bytes(&mut payload).unwrap(), 80);
        assert_eq!(payload.to_vec(), (0..80).collect::<Vec<u8>>());
    }
}

#[test]
fn existing_ext2_ext3_and_4k_images_remain_readable() {
    for fixture in [
        "test_disk_ext2.bin.zst",
        "test_disk_ext3.bin.zst",
        "test_disk_4k_block_journal.bin.zst",
    ] {
        let fs = crate::test_util::load_compressed_filesystem(fixture);
        let entries = fs
            .read_dir("/")
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(!entries.is_empty());
    }
}

#[test]
fn volume_and_special_inode_metadata_preserve_raw_identity() {
    let mut bytes = FIXTURE.to_vec();
    bytes[1024 + 136..1024 + 140].copy_from_slice(b"/mnt");
    bytes[1024 + 12..1024 + 16].copy_from_slice(&5u32.to_le_bytes());
    bytes[1024 + 344..1024 + 348].copy_from_slice(&99u32.to_le_bytes()); // no 64bit: ignore high count
    let start = inode_offset(12);
    bytes[start..start + 2].copy_from_slice(&0x21b6u16.to_le_bytes());
    bytes[start + 2..start + 4].copy_from_slice(&2u16.to_le_bytes());
    bytes[start + 120..start + 122].copy_from_slice(&1u16.to_le_bytes());
    bytes[start + 20..start + 24].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes[start + 40..start + 44].copy_from_slice(&0u32.to_le_bytes());
    let device = (259u32 << 8) | 7 | (0x123u32 << 20);
    bytes[start + 44..start + 48].copy_from_slice(&device.to_le_bytes());
    let fs = Ext4::load(Box::new(bytes)).unwrap();
    let info = fs.volume_info().unwrap();
    assert_eq!(
        (
            info.format_version,
            info.revision,
            info.inode_count,
            info.free_block_count,
            info.last_mount_path
        ),
        (4, 1, 32, 5, b"/mnt".to_vec())
    );
    let metadata = fs.inode_metadata(12).unwrap();
    assert_eq!(
        (
            metadata.raw_mode(),
            metadata.uid(),
            metadata.links(),
            metadata.deletion_time().unwrap().seconds(),
            metadata.device()
        ),
        (0x21b6, 65538, 1, -1, Some((259, 0x12307)))
    );
}

#[test]
fn triple_indirect_seek_skips_entire_missing_subtrees() {
    let mut bytes = FIXTURE.to_vec();
    let start = inode_offset(15);
    let logical = 12 + 256 + 256 * 256;
    bytes[start + 4..start + 8].copy_from_slice(&((logical + 1) * 1024u32).to_le_bytes());
    bytes[start + 40..start + 100].fill(0);
    bytes[start + 96..start + 100].copy_from_slice(&57u32.to_le_bytes());
    for (block, pointer) in [(57, 58u32), (58, 59), (59, 60)] {
        bytes[block * 1024..block * 1024 + 4].copy_from_slice(&pointer.to_le_bytes());
    }
    bytes[60 * 1024..61 * 1024].fill(b'T');
    let fs = Ext4::load(Box::new(bytes)).unwrap();
    let mut file = fs.open_inode_by_index(15).unwrap();
    file.seek_to(u64::from(logical) * 1024 - 2).unwrap();
    let mut tail = [0; 2];
    assert_eq!(file.read_bytes(&mut tail).unwrap(), 2);
    assert_eq!(tail, [0, 0]);
    assert_eq!(file.read_bytes(&mut tail).unwrap(), 2);
    assert_eq!(tail, [b'T', b'T']);
}

#[test]
fn full_64k_directory_record_uses_the_sentinel_length() {
    use crate::dir_entry::DirEntry;
    use crate::inode::InodeIndex;
    use alloc::rc::Rc;
    let mut bytes = FIXTURE.to_vec();
    // Only directory-entry parsing is exercised here; no disk lookup is needed.
    bytes[1024 + 24..1024 + 28].copy_from_slice(&6u32.to_le_bytes());
    bytes[1024 + 20..1024 + 24].fill(0);
    bytes[1024 + 4..1024 + 8].copy_from_slice(&64u32.to_le_bytes());
    bytes.resize(64 * 65536, 0);
    bytes[65536 + 8..65536 + 12].copy_from_slice(&4u32.to_le_bytes());
    let fs = Ext4::load(Box::new(bytes)).unwrap();
    for length in [0u16, u16::MAX] {
        let mut record = vec![0; 65536];
        record[..4].copy_from_slice(&2u32.to_le_bytes());
        record[4..6].copy_from_slice(&length.to_le_bytes());
        record[6] = 1;
        record[7] = 2;
        record[8] = b'.';
        let (entry, size) = DirEntry::from_bytes(
            fs.clone(),
            &record,
            InodeIndex::new(2).unwrap(),
            Rc::new(crate::PathBuf::empty()),
        )
        .unwrap();
        assert_eq!(
            (entry.unwrap().file_name().as_ref().to_vec(), size.get()),
            (b".".to_vec(), 65536)
        );
    }
}
