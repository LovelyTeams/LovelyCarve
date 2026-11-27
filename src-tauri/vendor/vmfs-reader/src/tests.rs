use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;

use super::{Error, Filesystem, SourceSpan, Version, Volume, probe_member, read_exact_at};

fn fixture_directory() -> PathBuf {
    std::env::var_os("LOVELYCARVE_VMFS_ORACLE")
        .expect("VMFS fixture directory")
        .into()
}

#[test]
#[ignore = "requires the upstream VMFS filesystem fixtures"]
fn real_datastore_addresses_match_the_upstream_extent_map() {
    let fixtures = fixture_directory();
    for (name, version, local, wrapped) in [
        ("vmfs5", Version::Vmfs5, false, true),
        ("vmfs5d", Version::Vmfs5, false, false),
        ("vmfs5l", Version::Vmfs5, true, true),
        ("vmfs5ld", Version::Vmfs5, true, false),
        ("vmfs6", Version::Vmfs6, false, true),
        ("vmfs6d", Version::Vmfs6, false, false),
        ("vmfs6l", Version::Vmfs6, true, true),
        ("vmfs6ld", Version::Vmfs6, true, false),
        ("vmfs5-no-dense-sbpc", Version::Vmfs5, false, true),
        ("vmfs5-resource-optimized", Version::Vmfs5, false, true),
        ("vmfs6-resource-optimized", Version::Vmfs6, false, true),
    ] {
        let source = || File::open(fixtures.join(format!("{name}.bin"))).unwrap();
        let mut volume = Volume::open(source()).unwrap_or_else(|error| panic!("{name}: {error}"));
        let start = if wrapped { 0x1100000 } else { 0 };
        assert_eq!(
            probe_member(&mut source()).unwrap().is_some(),
            wrapped,
            "{name}"
        );
        assert_eq!(
            (
                &*volume.geometry().label,
                volume.geometry().version,
                volume.geometry().local
            ),
            (name, version, local)
        );
        assert_eq!(
            volume.source_spans(0x200000, 512).unwrap(),
            [SourceSpan {
                member: 0,
                offset: start + 0x200000,
                length: 512
            }]
        );
        assert!(
            volume.source_spans(volume.size() - 512, 513).is_err(),
            "{name}"
        );
        let mut expected = [0; 6147];
        let mut read = [0; 6147];
        read_exact_at(&mut source(), start + 0x200011, &mut expected).unwrap();
        volume.seek(SeekFrom::Start(0x200011)).unwrap();
        volume.read_exact(&mut read).unwrap();
        assert_eq!(read, expected, "{name}");
        let mut fs = Filesystem::open(volume).unwrap_or_else(|error| panic!("{name}: {error}"));
        let entries = fs
            .entries(crate::ROOT)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        for suffix in [
            ".fdc.sf", ".fbb.sf", ".pbc.sf", ".sbc.sf", ".pb2.sf", ".sdd.sf", ".vh.sf",
        ] {
            assert!(
                entries.iter().any(|entry| entry.name == suffix),
                "{name}: {suffix}"
            );
        }
    }
}

#[test]
#[ignore = "requires the upstream VMFS two-member fixtures"]
fn real_spanned_datastores_map_reversed_members_and_reject_missing_members() {
    let fixtures = fixture_directory().join("span");
    for version in [5, 6] {
        let source =
            |index| File::open(fixtures.join(format!("vmfs{version}-span-{index}.bin"))).unwrap();
        assert!(matches!(
            Volume::open(source(0)),
            Err(Error::MissingMembers)
        ));
        assert!(matches!(
            Volume::open(source(1)),
            Err(Error::MissingMembers)
        ));
        assert!(Volume::open_members(vec![source(0), source(0)]).is_err());
        let mut volume = Volume::open_members(vec![source(1), source(0)]).unwrap();
        let split = 2 * 1024 * 1024 * 1024;
        assert_eq!(volume.size(), 2 * split);
        assert_eq!(
            volume.source_spans(split - 17, 72).unwrap(),
            [
                SourceSpan {
                    member: 1,
                    offset: 0x1100000 + split - 17,
                    length: 17
                },
                SourceSpan {
                    member: 0,
                    offset: 0x1100000,
                    length: 55
                },
            ]
        );
        let mut expected = [0; 72];
        read_exact_at(&mut source(0), 0x1100000 + split - 17, &mut expected[..17]).unwrap();
        read_exact_at(&mut source(1), 0x1100000, &mut expected[17..]).unwrap();
        let mut read = [0; 72];
        read_exact_at(&mut volume, split - 17, &mut read).unwrap();
        assert_eq!(read, expected);
    }
}

#[test]
#[ignore = "requires the upstream VMFS filesystem fixtures"]
fn real_file_descriptors_read_resident_direct_and_indirect_data() {
    let fixtures = fixture_directory();
    for name in ["vmfs5", "vmfs6"] {
        let volume =
            Volume::open(File::open(fixtures.join(format!("{name}.bin"))).unwrap()).unwrap();
        let mut fs = Filesystem::open(volume).unwrap_or_else(|error| panic!("{name}: {error}"));
        let small = fs.find("/small").unwrap();
        let big = fs.find("/big").unwrap();
        let normal = fs.find("/normal-flat.vmdk").unwrap();
        let large = fs.find("/large-flat.vmdk").unwrap();
        let mut bytes = [0; 512];
        assert_eq!(fs.read_range(small, 0, &mut bytes).unwrap(), 5);
        assert_eq!(&bytes[..5], b"tiny\n");
        let dir = fs.find("/directory").unwrap();
        assert_eq!(fs.entries(dir).unwrap().len(), 100);
        let file100 = fs.find("/directory/file100").unwrap();
        let mut text = String::new();
        fs.open_file(file100)
            .unwrap()
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text, "File 100\n");
        let collisions = fs.find("/collision").unwrap();
        let entries = fs.entries(collisions).unwrap();
        assert!(!entries.is_empty());
        for entry in entries {
            assert_eq!(
                fs.find(&format!("/collision/{}", entry.name)).unwrap(),
                entry.address
            );
        }
        let pattern = b"Kusjes van SRT<3";
        assert_eq!(fs.read_range(big, 235, &mut bytes).unwrap(), bytes.len());
        assert!(
            bytes
                .iter()
                .enumerate()
                .all(|(index, byte)| *byte == pattern[(235 + index) % pattern.len()])
        );
        for (address, size, written) in [
            (normal, 69u64 << 30, 42u64 << 30),
            (large, 42u64 << 40, 117u64 << 30),
        ] {
            assert_eq!(fs.file_info(address).unwrap().size, size);
            assert_eq!(
                fs.read_range(address, written, &mut bytes).unwrap(),
                bytes.len()
            );
            assert_eq!(bytes, [0x42; 512]);
            assert_eq!(fs.read_range(address, 0, &mut bytes).unwrap(), bytes.len());
            assert_eq!(bytes, [0; 512]);
        }
    }
}

#[test]
#[ignore = "requires the upstream VMFS sparse disk fixtures"]
fn real_empty_disks_with_pointer_blocks_read_as_sparse() {
    for version in [5, 6] {
        let source = File::open(fixture_directory().join(format!("zero{version}.bin"))).unwrap();
        let mut fs = Filesystem::open(Volume::open(source).unwrap()).unwrap();
        let address = fs.find("/empty-flat.vmdk").unwrap();
        let size = fs.file_info(address).unwrap().size;
        for offset in [0, size / 2, size - 512] {
            let mut bytes = [0x42; 512];
            assert_eq!(fs.read_range(address, offset, &mut bytes).unwrap(), 512);
            assert_eq!(bytes, [0; 512]);
        }
    }
}
