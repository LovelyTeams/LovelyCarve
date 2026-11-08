extern crate positioned_io;
extern crate qcow2;

use positioned_io::ReadAt;
use qcow2::Qcow2;
use std::fs::File;

#[test]
fn basic_read() {
    let file = File::open("tests/test.qcow2").unwrap();
    let qcow = Qcow2::open(file).unwrap();
    let reader = qcow.reader().unwrap();
    let mut buf = [0; 11];
    reader.read_exact_at(1024 * 1024 * 200, &mut buf).unwrap();
    let s = std::str::from_utf8(&buf).unwrap();
    assert_eq!(s, "Lorem ipsum");
}

#[test]
fn reads_version_2_and_sparse_clusters_directly() {
    const CLUSTER_SIZE: usize = 512;
    let mut image = vec![0u8; CLUSTER_SIZE * 4];

    put_u32(&mut image, 0, 0x5146_49fb);
    put_u32(&mut image, 4, 2);
    put_u32(&mut image, 20, 9);
    put_u64(&mut image, 24, (CLUSTER_SIZE * 2) as u64);
    put_u32(&mut image, 36, 1);
    put_u64(&mut image, 40, CLUSTER_SIZE as u64);

    put_u64(&mut image, CLUSTER_SIZE, (CLUSTER_SIZE * 2) as u64);
    put_u64(&mut image, CLUSTER_SIZE * 2, (CLUSTER_SIZE * 3) as u64);
    image[CLUSTER_SIZE * 3..CLUSTER_SIZE * 3 + 9].copy_from_slice(b"v2 qcow2!");

    let qcow = Qcow2::open(image).expect("open synthetic v2 image");
    assert_eq!(qcow.guest_size(), (CLUSTER_SIZE * 2) as u64);

    let mut allocated = [0u8; 9];
    assert_eq!(qcow.read_at(0, &mut allocated).expect("read allocated"), 9);
    assert_eq!(&allocated, b"v2 qcow2!");

    let mut sparse = [0xffu8; 16];
    assert_eq!(
        qcow.read_at(CLUSTER_SIZE as u64, &mut sparse)
            .expect("read sparse"),
        sparse.len()
    );
    assert_eq!(sparse, [0u8; 16]);
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}
