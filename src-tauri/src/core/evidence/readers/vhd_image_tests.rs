use super::*;
use crate::core::evidence::image::{BadRegion, DiskImage, ImageFormat};
use sha2::{Digest, Sha256};
use std::io::Write;

fn fixtures(kind: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("vendor")
        .join(kind)
        .join("tests/data")
}

#[test]
fn vhd_family_logical_streams_match_independent_libvhdi_oracle() {
    for (kind, format) in [("vhd", ImageFormat::Vhd), ("vhdx", ImageFormat::Vhdx)] {
        let directory = fixtures(kind);
        let oracle: serde_json::Value = serde_json::from_slice(
            &std::fs::read(directory.join("LOVELYCARVE_ORACLE.json")).unwrap(),
        )
        .unwrap();
        for sample in oracle["images"].as_array().unwrap() {
            let path = directory.join(sample["name"].as_str().unwrap());
            let image = DiskImage::open(&path).unwrap();
            let length = sample["size"].as_u64().unwrap();
            assert_eq!((image.len(), image.info().source_format), (length, format));
            let bytes = image.read_range(0, length as usize).unwrap();
            assert_eq!(
                hex::encode(Sha256::digest(&bytes)),
                sample["sha256"].as_str().unwrap()
            );
            let mut reader = if kind == "vhd" {
                VhdHandle::open_vhd(&path)
            } else {
                VhdHandle::open_vhdx(&path)
            }
            .unwrap();
            assert_eq!(reader.bytes_per_sector(), 512);
            let mut unaligned = [0; 1234];
            reader.read_at(499, &mut unaligned).unwrap();
            assert_eq!(&unaligned, &bytes[499..1733]);
            assert_eq!(reader.read_at(length - 10, &mut unaligned).unwrap(), 10);
            assert_eq!(&unaligned[..10], &bytes[bytes.len() - 10..]);
            assert_eq!(reader.read_at(u64::MAX, &mut unaligned).unwrap(), 0);
        }
    }
}

#[test]
fn vhd_family_signature_detection_without_extension_and_corruption_rejection() {
    let directory = tempfile::tempdir().unwrap();
    for (kind, filename, format) in [
        ("vhd", "ntfs_dynamic.vhd", ImageFormat::Vhd),
        ("vhdx", "ext2.vhdx", ImageFormat::Vhdx),
    ] {
        let path = directory.path().join(format!("{kind}.bin"));
        std::fs::copy(fixtures(kind).join(filename), &path).unwrap();
        assert_eq!(DiskImage::open(&path).unwrap().info().source_format, format);
    }
    let path = directory.path().join("broken.bin");
    let mut bytes = std::fs::read(fixtures("vhd").join("ntfs_fixed.vhd")).unwrap();
    let checksum = bytes.len() - 512 + 64;
    bytes[checksum] ^= 1;
    std::fs::write(&path, bytes).unwrap();
    assert!(format!("{:#}", DiskImage::open(&path).unwrap_err()).contains("checksum"));
}

#[test]
fn vhdx_missing_parent_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("child.vhdx");
    std::fs::copy(fixtures("vhdx").join("fat-differential.vhdx"), &path).unwrap();
    let error = VhdHandle::open_vhdx(&path).unwrap_err();
    assert!(format!("{error:#}").contains("parent"));
}

#[test]
fn vhdx_differencing_reads_child_and_parent_sectors() {
    const START: u64 = 133 * 512;
    let directory = fixtures("vhdx");
    let image = DiskImage::open(&directory.join("fat-differential.vhdx")).unwrap();
    let parent = DiskImage::open(&directory.join("fat-parent.vhdx")).unwrap();
    let (bytes, faults) = image.read_range_traced(START, 3 * 512).unwrap();
    let parent_bytes = parent.read_range(START, 3 * 512).unwrap();
    assert_eq!(
        (image.len(), image.info().source_format),
        (parent.len(), ImageFormat::Vhdx)
    );
    assert_eq!(&bytes[..512], &parent_bytes[..512]);
    assert_ne!(&bytes[512..1024], &parent_bytes[512..1024]);
    assert_eq!(bytes[512 + 6], 0xff);
    assert_eq!(&bytes[1024..], &parent_bytes[1024..]);
    assert!(faults.is_empty());
}

#[test]
fn vhdx_rejects_wrong_parent_and_checks_parent_stability() {
    let directory = tempfile::tempdir().unwrap();
    let child = directory.path().join("child.vhdx");
    let parent = directory.path().join("fat-parent.vhdx");
    let samples = fixtures("vhdx");
    std::fs::copy(samples.join("fat-differential.vhdx"), &child).unwrap();
    std::fs::copy(samples.join("ext2.vhdx"), &parent).unwrap();
    let error = DiskImage::open(&child).unwrap_err();
    assert!(format!("{error:#}").contains("parent DataWriteGuid mismatch"));

    std::fs::copy(samples.join("fat-parent.vhdx"), &parent).unwrap();
    let image = DiskImage::open(&child).unwrap();
    image.validate_source_file_stable().unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&parent)
        .unwrap();
    file.write_all(&[0]).unwrap();
    assert!(image.validate_source_file_stable().is_err());
}

#[test]
fn vhdx_parent_cycle_is_an_error() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fat-parent.vhdx");
    std::fs::copy(fixtures("vhdx").join("fat-differential.vhdx"), &path).unwrap();
    let error = DiskImage::open(&path).unwrap_err();
    assert!(format!("{error:#}").contains("VHDX parent chain contains a cycle"));
}

#[test]
fn vhdx_parent_read_faults_are_reported_at_child_offsets() {
    const START: u64 = 133 * 512;
    let directory = tempfile::tempdir().unwrap();
    let child = directory.path().join("child.vhdx");
    let parent = directory.path().join("fat-parent.vhdx");
    let samples = fixtures("vhdx");
    std::fs::copy(samples.join("fat-differential.vhdx"), &child).unwrap();
    std::fs::copy(samples.join("fat-parent.vhdx"), &parent).unwrap();
    // Preserve the parent's metadata and BAT, but remove its allocated payload.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&parent)
        .unwrap()
        .set_len(4 * 1024 * 1024)
        .unwrap();

    let image = DiskImage::open(&child).unwrap();
    let (bytes, faults) = image.read_range_traced(START, 3 * 512).unwrap();
    assert_eq!(
        faults,
        [
            BadRegion {
                offset: START,
                len: 512
            },
            BadRegion {
                offset: START + 1024,
                len: 512
            },
        ]
    );
    assert_eq!(&bytes[..512], &[0; 512]);
    assert_eq!(bytes[512 + 6], 0xff);
    assert_eq!(&bytes[1024..], &[0; 512]);
}

#[test]
fn vhd_resized_capacity_uses_current_size() {
    let reader = VhdHandle::open_vhd(&fixtures("vhd").join("original-size-mismatch.vhd")).unwrap();
    assert_eq!(reader.media_size(), 2_123_776);
}
