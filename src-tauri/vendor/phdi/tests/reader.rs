//! Invented byte fixtures; expected logical bytes never use the reader's mapper.
use phdi::{Descriptor, Guid, OpenOptions, PhdiImage};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const BASE: &str = "{11111111-1111-1111-1111-111111111111}";
const CHILD: &str = "{22222222-2222-2222-2222-222222222222}";
const ZERO: &str = "{00000000-0000-0000-0000-000000000000}";
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Directory(PathBuf);
impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "phdi-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) {
        fs::write(self.path(name), bytes).unwrap();
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn sparse(entries: &[u32], block_sectors: u32, sectors: u64) -> Vec<u8> {
    let end = entries
        .iter()
        .filter(|entry| **entry != 0)
        .map(|entry| u64::from(*entry) * 512 + u64::from(block_sectors) * 512)
        .max()
        .unwrap_or(512);
    let mut bytes = vec![0; end as usize];
    bytes[..16].copy_from_slice(b"WithoutFreeSpace");
    for (offset, value) in [
        (16, 2),
        (20, 16),
        (24, 1),
        (28, block_sectors),
        (32, entries.len() as u32),
        (48, 1),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[36..44].copy_from_slice(&sectors.to_le_bytes());
    for (index, entry) in entries.iter().enumerate() {
        bytes[64 + index * 4..68 + index * 4].copy_from_slice(&entry.to_le_bytes());
        if *entry != 0 {
            let start = *entry as usize * 512;
            bytes[start..start + block_sectors as usize * 512].fill(0x40 + index as u8);
        }
    }
    bytes
}
fn image(identifier: &str, kind: &str, filename: &str) -> String {
    format!("<Image><GUID>{identifier}</GUID><Type>{kind}</Type><File>{filename}</File></Image>")
}
fn extent(start: u64, end: u64, block: u64, images: &str) -> String {
    format!("<Storage><Start>{start}</Start><End>{end}</End><Blocksize>{block}</Blocksize>{images}</Storage>")
}
fn shot(identifier: &str, parent: &str) -> String {
    format!("<Shot><GUID>{identifier}</GUID><ParentGUID>{parent}</ParentGUID></Shot>")
}
fn xml(sectors: u64, extents: &str, shots: &str) -> String {
    format!("<?xml version='1.0' encoding='UTF-8'?><Parallels_disk_image Version='1.0'><Disk_Parameters><Disk_size>{sectors}</Disk_size><LogicSectorSize>512</LogicSectorSize><Name>磁盘 &amp; 数据 &#x41;</Name></Disk_Parameters><StorageData>{extents}</StorageData><Snapshots>{shots}</Snapshots></Parallels_disk_image>")
}
fn plain_directory() -> Directory {
    let directory = Directory::new();
    directory.write("base.hds", vec![0x17; 1024]);
    directory.write(
        "DiskDescriptor.xml",
        xml(
            2,
            &extent(0, 2, 1, &image(BASE, "Plain", "base.hds")),
            &shot(BASE, ZERO),
        ),
    );
    directory
}
fn read_all(path: &Path) -> Vec<u8> {
    let mut reader = PhdiImage::open(path).unwrap();
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).unwrap();
    bytes
}

#[test]
fn plain_metadata_seek_unicode_and_eof() {
    let directory = plain_directory();
    let mut reader = PhdiImage::open(&directory.0).unwrap();
    assert_eq!(reader.media_size(), 1024);
    assert_eq!(reader.descriptor().name.as_deref(), Some("磁盘 & 数据 A"));
    assert_eq!(reader.descriptor().snapshots[0].parent, None);
    assert_eq!(reader.source_files().len(), 2);
    reader.seek(SeekFrom::End(-3)).unwrap();
    let mut bytes = [0xcc; 8];
    assert_eq!(reader.read(&mut bytes).unwrap(), 3);
    assert_eq!(&bytes, &[0x17, 0x17, 0x17, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc]);
    assert_eq!(reader.position(), 1024);
    assert_eq!(reader.read_at(u64::MAX, &mut bytes).unwrap(), 0);
    assert!(reader.seek(SeekFrom::Current(-2048)).is_err());
}

#[test]
fn standalone_sparse_random_and_sequential_reads_agree() {
    let directory = Directory::new();
    directory.write("disk.hds", sparse(&[1, 0, 3, 5], 2, 7));
    let mut expected = vec![0x40; 1024];
    expected.extend(vec![0; 1024]);
    expected.extend(vec![0x42; 1024]);
    expected.extend(vec![0x43; 512]);
    let reader = PhdiImage::open(directory.path("disk.hds")).unwrap();
    assert_eq!(read_all(&directory.path("disk.hds")), expected);
    for offset in (0..expected.len() + 64).step_by(37) {
        let mut bytes = [0xab; 777];
        let count = reader.read_at(offset as u64, &mut bytes).unwrap();
        let end = (offset + 777).min(expected.len());
        assert_eq!(count, end.saturating_sub(offset));
        if offset < expected.len() {
            assert_eq!(&bytes[..count], &expected[offset..end]);
        }
        assert!(bytes[count..].iter().all(|byte| *byte == 0xab));
    }
}

#[test]
fn split_plain_sparse_and_differential_parent_chain() {
    let directory = Directory::new();
    directory.write("left-base.hds", vec![0x11; 2048]);
    directory.write("left-child.hds", sparse(&[0, 1], 2, 4));
    directory.write("right-base.hds", sparse(&[1, 0], 2, 4));
    directory.write("right-child.hds", sparse(&[0, 3], 2, 4));
    let left = extent(
        0,
        4,
        2,
        &(image(BASE, "Plain", "left-base.hds") + &image(CHILD, "Compressed", "left-child.hds")),
    );
    let right = extent(
        4,
        8,
        2,
        &(image(BASE, "Compressed", "right-base.hds")
            + &image(CHILD, "Compressed", "right-child.hds")),
    );
    directory.write(
        "DiskDescriptor.xml",
        xml(
            8,
            &(left + &right),
            &(shot(CHILD, BASE) + &shot(BASE, ZERO)),
        ),
    );
    let options = OpenOptions {
        maximum_open_handles: 1,
        ..OpenOptions::default()
    };
    let reader = PhdiImage::open_with_options(&directory.0, &options, &|| false).unwrap();
    let mut expected = vec![0x11; 1024];
    expected.extend(vec![0x41; 1024]);
    expected.extend(vec![0x40; 1024]);
    expected.extend(vec![0x41; 1024]);
    let mut bytes = vec![0; 4096];
    assert_eq!(reader.read_at(0, &mut bytes).unwrap(), 4096);
    assert_eq!(bytes, expected);
    let options = OpenOptions {
        snapshot: Some(BASE.parse().unwrap()),
        ..options
    };
    let parent = PhdiImage::open_with_options(&directory.0, &options, &|| false).unwrap();
    parent.read_at(0, &mut bytes).unwrap();
    assert_eq!(&bytes[..2048], vec![0x11; 2048]);
    assert_eq!(&bytes[2048..3072], vec![0x40; 1024]);
    assert_eq!(&bytes[3072..], vec![0; 1024]);
}

#[test]
fn top_guid_selects_parent_and_missing_parent_fails() {
    let extents = extent(
        0,
        2,
        1,
        &(image(BASE, "Plain", "base") + &image(CHILD, "Compressed", "child")),
    );
    let descriptor = Descriptor::parse(
        xml(
            2,
            &extents,
            &(format!("<TopGUID>{BASE}</TopGUID>") + &shot(CHILD, BASE) + &shot(BASE, ZERO)),
        )
        .as_bytes(),
    )
    .unwrap();
    assert_eq!(descriptor.top_identifier, Some(BASE.parse().unwrap()));
    assert!(Descriptor::parse(xml(2, &extents, &shot(CHILD, BASE)).as_bytes()).is_err());
}

#[test]
fn unchanged_child_extent_inherits_root_extent() {
    let directory = Directory::new();
    directory.write("left", [0x21; 512]);
    directory.write("right", [0x31; 512]);
    directory.write("child", sparse(&[1], 1, 1));
    let left = extent(
        0,
        1,
        1,
        &(image(BASE, "Plain", "left") + &image(CHILD, "Compressed", "child")),
    );
    let right = extent(1, 2, 1, &image(BASE, "Plain", "right"));
    directory.write(
        "DiskDescriptor.xml",
        xml(
            2,
            &(left + &right),
            &(shot(CHILD, BASE) + &shot(BASE, ZERO)),
        ),
    );
    assert_eq!(
        read_all(&directory.0),
        [vec![0x40; 512], vec![0x31; 512]].concat()
    );
}

#[test]
fn logical_sector_geometry_does_not_change_xml_units_and_override_directory() {
    let directory = plain_directory();
    let descriptor_directory = Directory::new();
    let descriptor = fs::read_to_string(directory.path("DiskDescriptor.xml"))
        .unwrap()
        .replace("<LogicSectorSize>512", "<LogicSectorSize>4096");
    descriptor_directory.write("DiskDescriptor.xml", &descriptor);
    let options = OpenOptions {
        extent_directory: Some(directory.0.clone()),
        ..OpenOptions::default()
    };
    let reader =
        PhdiImage::open_with_options(&descriptor_directory.0, &options, &|| false).unwrap();
    assert_eq!(reader.media_size(), 1024);
    assert_eq!(reader.sector_size(), 4096);
    assert_eq!(reader.descriptor().extents[0].size, 1024);
    directory.write("base.hds", sparse(&[1, 2], 1, 2));
    descriptor_directory.write(
        "DiskDescriptor.xml",
        descriptor
            .replace("Plain", "Compressed")
            .replace("<Blocksize>1", "<Blocksize>2"),
    );
    assert!(PhdiImage::open_with_options(&descriptor_directory.0, &options, &|| false).is_err());
}

#[test]
fn cyclic_and_duplicate_guids_fail() {
    let extents = extent(0, 2, 1, &image(BASE, "Plain", "base"));
    for shots in [
        shot(BASE, BASE),
        shot(CHILD, BASE) + &shot(BASE, CHILD),
        shot(BASE, ZERO) + &shot(BASE, ZERO),
    ] {
        assert!(Descriptor::parse(xml(2, &extents, &shots).as_bytes()).is_err());
    }
    assert!(Descriptor::parse(
        xml(
            2,
            &extent(
                0,
                2,
                1,
                &(image(BASE, "Plain", "one") + &image(BASE, "Plain", "two"))
            ),
            ""
        )
        .as_bytes()
    )
    .is_err());
}

#[test]
fn gaps_overlap_zero_and_overflow_fail() {
    for extents in [
        extent(1, 3, 1, &image(BASE, "Plain", "base")),
        extent(0, 3, 1, &image(BASE, "Plain", "base"))
            + &extent(2, 4, 1, &image(BASE, "Plain", "base")),
        extent(0, 0, 1, &image(BASE, "Plain", "base")),
        extent(0, 2, 0, &image(BASE, "Plain", "base")),
    ] {
        assert!(Descriptor::parse(xml(4, &extents, "").as_bytes()).is_err());
    }
    assert!(Descriptor::parse(xml(u64::MAX, "", "").as_bytes()).is_err());
}

#[test]
fn malformed_xml_duplicate_fields_entities_and_encoding_fail() {
    let valid = xml(2, &extent(0, 2, 1, &image(BASE, "Plain", "base")), "");
    for broken in [
        valid.replace(
            "<Disk_size>2</Disk_size>",
            "<Disk_size>2</Disk_size><Disk_size>2</Disk_size>",
        ),
        valid.replace("&amp;", "&unknown;"),
        valid.replace("UTF-8", "UTF-16"),
        valid.replace("Version='1.0'", "Version='2.0'"),
        valid.replace("</Storage>", "</Image>"),
        valid.replace(
            "<?xml version='1.0' encoding='UTF-8'?>",
            "<!DOCTYPE external SYSTEM 'file:///secret'>",
        ),
        valid.replace("<LogicSectorSize>512", "<LogicSectorSize>999"),
    ] {
        assert!(Descriptor::parse(broken.as_bytes()).is_err(), "{broken}");
    }
    assert!(Descriptor::parse(&[0xff]).is_err());
    assert!(Descriptor::parse(&valid.as_bytes()[..valid.len() - 8]).is_err());
}

#[test]
fn missing_and_truncated_extents_fail_without_zero_fallback() {
    let directory = plain_directory();
    fs::remove_file(directory.path("base.hds")).unwrap();
    assert!(PhdiImage::open(&directory.0).is_err());
    directory.write("base.hds", [1; 512]);
    assert!(PhdiImage::open(&directory.0).is_err());
    directory.write("base.hds", sparse(&[1, 2], 1, 2));
    directory.write(
        "DiskDescriptor.xml",
        xml(
            2,
            &extent(0, 2, 1, &image(BASE, "Compressed", "base.hds")),
            "",
        ),
    );
    fs::OpenOptions::new()
        .write(true)
        .open(directory.path("base.hds"))
        .unwrap()
        .set_len(1024)
        .unwrap();
    assert!(PhdiImage::open(&directory.0).is_err());
}

#[test]
fn source_truncation_and_descriptor_changes_fail_after_open() {
    let directory = plain_directory();
    let reader = PhdiImage::open(&directory.0).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(directory.path("base.hds"))
        .unwrap()
        .set_len(512)
        .unwrap();
    assert!(reader.read_at(0, &mut [0; 512]).is_err());
    directory.write("base.hds", [0; 1024]);
    let reader = PhdiImage::open(&directory.0).unwrap();
    directory.write("DiskDescriptor.xml", b"broken");
    assert!(reader.read_at(0, &mut [0; 512]).is_err());
}

#[test]
fn sparse_metadata_geometry_alias_and_truncation_checks() {
    let directory = Directory::new();
    let original = sparse(&[1, 2], 1, 2);
    for (offset, value) in [(16, 1), (28, 0), (32, 1), (48, 99), (64, 99), (68, 1)] {
        let mut bytes = original.clone();
        bytes[offset..offset + 4].copy_from_slice(&u32::to_le_bytes(value));
        directory.write("disk.hds", bytes);
        assert!(
            PhdiImage::open(directory.path("disk.hds")).is_err(),
            "offset={offset}"
        );
    }
    directory.write("disk.hds", &original[..70]);
    assert!(PhdiImage::open(directory.path("disk.hds")).is_err());
    let mut bytes = original;
    bytes[..16].copy_from_slice(b"WithouFreSpacExt");
    directory.write("disk.hds", bytes);
    assert!(PhdiImage::open(directory.path("disk.hds"))
        .unwrap_err()
        .to_string()
        .contains("extended"));
}

#[test]
fn zero_data_start_legacy_and_unallocated_root_read_zero() {
    let directory = Directory::new();
    let mut bytes = sparse(&[0, 0], 1, 2);
    bytes[48..52].fill(0);
    directory.write("disk.hds", bytes);
    assert_eq!(read_all(&directory.path("disk.hds")), vec![0; 1024]);
}

#[test]
fn resource_limits_and_cancellation() {
    let directory = plain_directory();
    let mut options = OpenOptions {
        maximum_descriptor_bytes: 10,
        ..OpenOptions::default()
    };
    assert!(PhdiImage::open_with_options(&directory.0, &options, &|| false).is_err());
    options = OpenOptions::default();
    assert_eq!(
        PhdiImage::open_with_options(&directory.0, &options, &|| true)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Interrupted
    );
    let reader = PhdiImage::open(&directory.0).unwrap();
    assert_eq!(
        reader
            .read_at_with_cancel(0, &mut [0; 1], &|| true)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Interrupted
    );
    directory.write("disk.hds", sparse(&[0, 0], 1, 2));
    options.maximum_bat_entries = 1;
    assert!(PhdiImage::open_with_options(directory.path("disk.hds"), &options, &|| false).is_err());
    options.maximum_open_handles = 0;
    assert!(PhdiImage::open_with_options(&directory.0, &options, &|| false).is_err());
}

#[test]
fn traversal_absolute_and_foreign_paths_fail() {
    let directory = plain_directory();
    for filename in ["../outside.hds", "/tmp/outside", "C:\\outside.hds"] {
        directory.write(
            "DiskDescriptor.xml",
            xml(2, &extent(0, 2, 1, &image(BASE, "Plain", filename)), ""),
        );
        assert!(PhdiImage::open(&directory.0).is_err());
    }
}

#[test]
fn encrypted_images_and_unknown_types_fail() {
    let valid = xml(2, &extent(0, 2, 1, &image(BASE, "Plain", "base")), "");
    let encrypted = valid.replace(
        "</Disk_Parameters>",
        &format!("<Encryption><Engine>{BASE}</Engine><Data/></Encryption></Disk_Parameters>"),
    );
    assert!(Descriptor::parse(encrypted.as_bytes()).is_err());
    assert!(Descriptor::parse(valid.replace("Plain", "Encrypted").as_bytes()).is_err());
    assert_eq!(Guid::ZERO.0, [0; 16]);
}

#[test]
fn concurrent_positioned_reads_are_independent() {
    let directory = plain_directory();
    let reader = PhdiImage::open(&directory.0).unwrap();
    std::thread::scope(|scope| {
        for offset in [0, 17, 511, 777] {
            let reader = &reader;
            scope.spawn(move || {
                for _ in 0..20 {
                    let mut bytes = [0; 200];
                    assert_eq!(reader.read_at(offset, &mut bytes).unwrap(), 200);
                    assert_eq!(bytes, [0x17; 200]);
                }
            });
        }
    });
    assert_eq!(reader.position(), 0);
}
