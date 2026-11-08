use super::*;
use crate::evidence::image::{DiskImage, ImageFormat};
use sha2::{Digest, Sha256};

fn fixtures(kind: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../vendor")
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
    let error = VhdHandle::open_vhdx(&fixtures("vhdx").join("fat-differential.vhdx")).unwrap_err();
    assert!(format!("{error:#}").contains("parent"));
}

#[test]
fn vhd_resized_capacity_uses_current_size() {
    let reader = VhdHandle::open_vhd(&fixtures("vhd").join("original-size-mismatch.vhd")).unwrap();
    assert_eq!(reader.media_size(), 2_123_776);
}
