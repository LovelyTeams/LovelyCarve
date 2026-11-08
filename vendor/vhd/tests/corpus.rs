#![allow(clippy::unwrap_used, clippy::expect_used)]
use forensic_testgate::gated_file;
use std::io::{Read, Seek, SeekFrom};
use vhd::VhdReader;

#[test]
fn corpus_dynamic_vhd_opens_and_has_nonzero_size() {
    let Some(path) = gated_file("CORPUS_DIR", "dynamic.vhd") else {
        return;
    };
    let reader = VhdReader::open(&path).expect("open dynamic.vhd");
    assert!(
        reader.virtual_disk_size() > 0,
        "virtual_disk_size must be > 0"
    );
}

#[test]
fn corpus_dynamic_vhd_read_is_stable() {
    let Some(path) = gated_file("CORPUS_DIR", "dynamic.vhd") else {
        return;
    };
    let mut reader = VhdReader::open(&path).expect("open");
    let mut buf = [0u8; 512];
    reader.seek(SeekFrom::Start(0)).expect("seek");
    reader.read_exact(&mut buf).expect("read sector 0");
    assert_eq!(
        buf, [0u8; 512],
        "sector 0 of an empty dynamic VHD must be all zeros"
    );
}

#[test]
fn corpus_fixed_vhd_opens_and_has_nonzero_size() {
    let Some(path) = gated_file("CORPUS_DIR", "fixed.vhd") else {
        return;
    };
    let reader = VhdReader::open(&path).expect("open fixed.vhd");
    assert!(
        reader.virtual_disk_size() > 0,
        "virtual_disk_size must be > 0"
    );
}

#[test]
fn dynamic_fixture_reports_dynamic_disk_type() {
    let path = format!(
        "{}/minimal.vhd",
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data")
    );
    let reader = vhd::VhdReader::open(std::path::Path::new(&path)).expect("open");
    assert_eq!(reader.disk_type(), vhd::DiskType::Dynamic);
}
