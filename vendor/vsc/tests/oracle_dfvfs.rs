//! Independent oracle: dfVFS `test_data/vss.raw` reconstructed by pyvshadow.
//!
//! Env-gated on `VSC_ORACLE_IMAGE`, the path to `vss.raw` from
//! <https://github.com/log2timeline/dfvfs> at commit
//! `0778d3fb20728cbf5c60e14b702527eab0b1c227` (an 82,771,968-byte NTFS volume
//! with two shadow copies). The expected digests are the SHA-256 of each
//! store's full volume as read by pyvshadow (libvshadow-python 20260714).
//! Without the variable the test returns early, so a passing run does not by
//! itself mean the dataset was exercised.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Write as _;
use std::fs::File;
use std::io::Read;

use sha2::{Digest, Sha256};
use vsc::VssVolume;

const IMAGE_SHA256: &str = "e633f0be5fb9ee9a07d44ba5223b786012ce89e9f0024f93d743568e6d052f16";

/// (store identifier, creation FILETIME, reconstructed-volume SHA-256), oldest
/// store first.
const STORES: [(&str, u64, &str); 2] = [
    (
        "de81cc22-aa8b-11eb-9339-8cdcd4557abc",
        132_643_644_032_230_304,
        "d830fe9e2f75e66edfe0f9b0730c0ec8148233ed9c2ab5601195c293d593d9a9",
    ),
    (
        "de81cc2b-aa8b-11eb-9339-8cdcd4557abc",
        132_643_644_882_249_863,
        "d47ca0497f4aa3098a7d0b5407a6406a4f8d747e6207fdbc197e2224fb4838ae",
    ),
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        write!(out, "{byte:02x}").expect("writing to a String cannot fail");
        out
    })
}

#[test]
fn reconstructs_every_dfvfs_store_like_pyvshadow() {
    let Some(path) = std::env::var_os("VSC_ORACLE_IMAGE") else {
        eprintln!("VSC_ORACLE_IMAGE not set; skipping the dfVFS vss.raw oracle");
        return;
    };
    let mut image = Vec::new();
    File::open(&path)
        .expect("open VSC_ORACLE_IMAGE")
        .read_to_end(&mut image)
        .expect("read VSC_ORACLE_IMAGE");
    assert_eq!(
        hex(&Sha256::digest(&image)),
        IMAGE_SHA256,
        "not dfVFS vss.raw"
    );

    let mut volume = VssVolume::open(std::io::Cursor::new(image)).unwrap();
    let stores: Vec<(String, u64)> = volume
        .stores()
        .iter()
        .map(|store| (store.store_id_string(), store.creation_time))
        .collect();
    let expected: Vec<(String, u64)> = STORES
        .iter()
        .map(|(id, time, _)| ((*id).to_string(), *time))
        .collect();
    assert_eq!(stores, expected);

    for (index, (_, _, digest)) in STORES.iter().enumerate() {
        let mut snapshot = volume.snapshot(index).unwrap();
        let size = usize::try_from(snapshot.layout().volume_size()).unwrap();
        let mut reconstructed = vec![0u8; size];
        snapshot.read_at(0, &mut reconstructed).unwrap();
        assert_eq!(
            hex(&Sha256::digest(&reconstructed)),
            *digest,
            "store {index}"
        );
    }
}
