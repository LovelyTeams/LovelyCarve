//! Parallels containers must enter the ordinary block/partition pipeline.
use super::*;

fn phdi_fixture(directory: &Path) -> Vec<u8> {
    let mut bytes = vec![0; 4096];
    // MBR with a three-sector partition and independent recognizable bytes.
    bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
    bytes[450] = 0x83;
    bytes[454..458].copy_from_slice(&1u32.to_le_bytes());
    bytes[458..462].copy_from_slice(&3u32.to_le_bytes());
    bytes[512..2048].fill(0x62);
    std::fs::write(directory.join("data.hds"), &bytes).expect("write plain extent");
    std::fs::write(
        directory.join("DiskDescriptor.xml"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Parallels_disk_image Version="1.0"><Disk_Parameters><Disk_size>8</Disk_size></Disk_Parameters>
<StorageData><Storage><Start>0</Start><End>8</End><Blocksize>1</Blocksize><Image>
<GUID>{11111111-1111-1111-1111-111111111111}</GUID><Type>Plain</Type><File>data.hds</File>
</Image></Storage></StorageData></Parallels_disk_image>"#,
    )
    .expect("write descriptor");
    bytes
}

#[test]
fn phdi_bundle_and_descriptor_use_logical_disk_with_stability() {
    let temporary = tempfile::tempdir().expect("temporary");
    let bundle = temporary.path().join("disk.hdd");
    std::fs::create_dir(&bundle).expect("bundle");
    let expected = phdi_fixture(&bundle);
    for path in [&bundle, &bundle.join("DiskDescriptor.xml")] {
        let image = DiskImage::open(path).expect("open PHDI");
        assert_eq!(image.info().source_format, ImageFormat::Phdi);
        assert_eq!(image.len(), expected.len() as u64);
        assert_eq!(image.partitions.len(), 1);
        assert_eq!(image.partitions[0].start_lba, 1);
        assert_eq!(image.partitions[0].length_lba, 3);
        assert_eq!(
            image.read_range(0, expected.len()).expect("full read"),
            expected
        );
        let mut ranged = [0; 900];
        assert_eq!(image.read_into(490, &mut ranged).expect("ranged read"), 900);
        assert_eq!(&ranged, &expected[490..1390]);
        assert_eq!(image.source_file_stability_snapshots_at_open().len(), 2);
        image.validate_source_file_stable().expect("stable");
    }
    let image = DiskImage::open(&bundle).expect("open bundle");
    std::fs::OpenOptions::new()
        .write(true)
        .open(bundle.join("data.hds"))
        .expect("open to truncate")
        .set_len(512)
        .expect("truncate");
    assert!(image.validate_source_file_stable().is_err());
}

#[test]
fn phdi_signature_detection_and_corrupt_containers_do_not_fall_back() {
    let temporary = tempfile::tempdir().expect("temporary");
    let path = temporary.path().join("renamed.bin");
    let mut bytes = vec![0; 1024];
    bytes[..16].copy_from_slice(b"WithoutFreeSpace");
    for (offset, value) in [(16, 2u32), (28, 1), (32, 1), (48, 1), (64, 1)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[36..44].copy_from_slice(&1u64.to_le_bytes());
    bytes[512..].fill(0x71);
    std::fs::write(&path, &bytes).expect("write HDS");
    let source = ImageSource::open(&path).expect("signature detection");
    assert_eq!(source.format(), ImageFormat::Phdi);
    assert_eq!(
        source.read_range(500, 40).expect("EOF clipping"),
        vec![0x71; 12]
    );
    bytes[64..68].copy_from_slice(&99u32.to_le_bytes());
    std::fs::write(&path, &bytes).expect("corrupt BAT");
    assert!(ImageSource::open(&path).is_err());
    phdi_fixture(temporary.path());
    std::fs::write(temporary.path().join("DiskDescriptor.xml"), b"broken")
        .expect("corrupt descriptor");
    assert!(ImageSource::open(&temporary.path().join("DiskDescriptor.xml")).is_err());
}

#[test]
fn phdi_guest_signature_does_not_override_split_raw_identity() {
    let temporary = tempfile::tempdir().expect("temporary");
    let mut first = vec![0x47; 512];
    first[..16].copy_from_slice(b"WithoutFreeSpace");
    let path = temporary.path().join("evidence.001");
    std::fs::write(&path, &first).expect("first RAW member");
    std::fs::write(temporary.path().join("evidence.002"), vec![0x48; 512])
        .expect("second RAW member");
    let source = ImageSource::open(&path).expect("split RAW identity");
    assert_eq!(source.format(), ImageFormat::Raw);
    assert_eq!(
        source.read_range(0, 1024).expect("concatenated RAW"),
        [first, vec![0x48; 512]].concat()
    );
}
