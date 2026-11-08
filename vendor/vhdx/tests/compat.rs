#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::io::Read;
use vhdx::VhdxReader;

fn data(name: &str) -> Vec<u8> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/data")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|_| panic!("test data missing: {}", path.display()))
}

#[test]
fn ext2_vhdx_opens() {
    VhdxReader::from_bytes(data("ext2.vhdx")).expect("ext2.vhdx must open successfully");
}

#[test]
fn ext2_vhdx_virtual_disk_size() {
    let reader = VhdxReader::from_bytes(data("ext2.vhdx")).expect("must open");
    assert_eq!(
        reader.virtual_disk_size(),
        4 * 1024 * 1024,
        "virtual_disk_size must be 4 MiB"
    );
}

#[test]
fn ext2_vhdx_sector_0_readable() {
    let mut reader = VhdxReader::from_bytes(data("ext2.vhdx")).expect("must open");
    let mut buf = [0u8; 512];
    reader
        .read_exact(&mut buf)
        .expect("sector 0 must be readable");
}

#[test]
fn fat_parent_vhdx_opens() {
    VhdxReader::from_bytes(data("fat-parent.vhdx"))
        .expect("fat-parent.vhdx must open successfully");
}

#[test]
fn fat_parent_vhdx_virtual_disk_size() {
    let reader = VhdxReader::from_bytes(data("fat-parent.vhdx")).expect("must open");
    assert_eq!(
        reader.virtual_disk_size(),
        4 * 1024 * 1024,
        "virtual_disk_size must be 4 MiB"
    );
}

#[test]
fn fat_differential_vhdx_refused() {
    let result = VhdxReader::from_bytes(data("fat-differential.vhdx"));
    assert!(
        result.is_err(),
        "VhdxReader must refuse a differencing disk"
    );
    assert!(
        result.unwrap_err().to_string().contains("differencing"),
        "error must mention differencing disk"
    );
}

#[test]
fn ext2_vhd_rejected() {
    assert!(
        VhdxReader::from_bytes(data("ext2.vhd")).is_err(),
        "VHD file must be rejected — not a VHDX container"
    );
}

#[test]
fn qemu_empty_dynamic_opens() {
    VhdxReader::from_bytes(data("qemu_empty_dynamic.vhdx"))
        .expect("qemu_empty_dynamic.vhdx must open successfully");
}

#[test]
fn qemu_empty_dynamic_virtual_disk_size() {
    let reader = VhdxReader::from_bytes(data("qemu_empty_dynamic.vhdx")).expect("must open");
    assert_eq!(
        reader.virtual_disk_size(),
        16 * 1024 * 1024,
        "virtual_disk_size must be 16 MiB"
    );
}

#[test]
fn qemu_fixed_opens() {
    VhdxReader::from_bytes(data("qemu_fixed.vhdx"))
        .expect("qemu_fixed.vhdx must open successfully");
}

#[test]
fn qemu_fixed_virtual_disk_size() {
    let reader = VhdxReader::from_bytes(data("qemu_fixed.vhdx")).expect("must open");
    assert_eq!(
        reader.virtual_disk_size(),
        8 * 1024 * 1024,
        "virtual_disk_size must be 8 MiB"
    );
}
