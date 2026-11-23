//! Real VeraCrypt volumes from the upstream oracle set, each gated on an
//! environment variable naming the image. The expected sector hashes are the
//! ground truth VeraCrypt 1.26.20 and cryptsetup 2.7.0 agreed on (upstream
//! `veracrypt-forensic` validation); an unset variable skips the test.
//!
//! ```text
//! VC_ORACLE=<dir>/vc_1-sha512-xts-aes cargo test --release --test oracle_veracrypt
//! ```

mod common;

use common::sha256_hex;
use veracrypt::{unlock, Control, FileSource, HeaderSelection, UnlockRequest, Volume};

const PASSWORD: &[u8] = b"aaaaaaaaaaaa";

fn open(variable: &str, password: &[u8], headers: HeaderSelection) -> Option<Volume<FileSource>> {
    let Ok(path) = std::env::var(variable) else {
        eprintln!("{variable} unset: skipping");
        return None;
    };
    let source = FileSource::open(&path).unwrap_or_else(|error| panic!("open {path}: {error}"));
    let request = UnlockRequest {
        password,
        pim: 0,
        prf: None,
        headers,
    };
    let control = Control {
        cancelled: &|| false,
        progress: None,
        threads: None,
    };
    Some(
        unlock(source, &request, &control).unwrap_or_else(|error| panic!("unlock {path}: {error}")),
    )
}

fn sector_hash(volume: &Volume<FileSource>, lba: u64) -> String {
    let mut sector = [0u8; 512];
    volume.read_at(lba * 512, &mut sector).unwrap();
    sha256_hex(&sector)
}

#[test]
fn sha512_aes_matches_veracrypt_and_cryptsetup() {
    let Some(volume) = open("VC_ORACLE", PASSWORD, HeaderSelection::Auto) else {
        return;
    };
    assert_eq!(volume.info().kdf.prf.id(), "sha512");
    assert_eq!(volume.info().chain.name(), "AES");
    assert_eq!(volume.info().encrypted_area_start, 131_072);
    for (lba, expected) in [
        (
            0,
            "76a9e8419a1e688732c03236e01e564c6b3660c0bcdc4561eb05e1d1de8ff8fa",
        ),
        (
            1,
            "076a27c79e5ace2a3d47f9dd2e83e4ff6ea8872b3c2218f66c92b89b55f36560",
        ),
        (
            2,
            "6242cb7cb043b219a77ffa2bd0aedab6735389bbbe8b3b2e88410cf5f74247a5",
        ),
        (
            16,
            "00882984fac5e7298c45bae80bad8debc4456d06d5189bb91f9f3901ecee4b0f",
        ),
    ] {
        assert_eq!(sector_hash(&volume, lba), expected, "sector {lba}");
    }
}

#[test]
fn hidden_volume_matches_cryptsetup() {
    let Some(volume) = open("VC_HIDDEN_ORACLE", b"bbbbbbbbbbbb", HeaderSelection::Auto) else {
        return;
    };
    assert!(volume.info().header.is_hidden());
    assert_eq!(volume.info().encrypted_area_start, 165_888);
    for (lba, expected) in [
        (
            0,
            "79a162bda41b98338009c880004ce44d3d84c2bf412f76271fea08279bffed8e",
        ),
        (
            2,
            "6242cb7cb043b219a77ffa2bd0aedab6735389bbbe8b3b2e88410cf5f74247a5",
        ),
    ] {
        assert_eq!(sector_hash(&volume, lba), expected, "hidden sector {lba}");
    }
}

#[test]
fn serpent_matches_veracrypt() {
    let Some(volume) = open("VC_SERPENT_ORACLE", PASSWORD, HeaderSelection::Auto) else {
        return;
    };
    assert_eq!(volume.info().chain.name(), "Serpent");
    for lba in [0, 1] {
        assert_eq!(
            sector_hash(&volume, lba),
            "479ad71598de182171230acbe3322cdac3b9bb9f70894a7cc3e7b526be46693b"
        );
    }
}

#[test]
fn aes_twofish_cascade_matches_veracrypt() {
    let Some(volume) = open("VC_CASCADE_ORACLE", PASSWORD, HeaderSelection::Auto) else {
        return;
    };
    assert_eq!(volume.info().chain.name(), "AES-Twofish");
    assert_eq!(
        sector_hash(&volume, 0),
        "da09622b78baeeb1fa8e6532f1eb23afc733a8449097d3a08d612286d4161492"
    );
}

#[test]
fn aes_twofish_serpent_cascade_matches_veracrypt() {
    let Some(volume) = open("VC_CASCADE3_ORACLE", PASSWORD, HeaderSelection::Auto) else {
        return;
    };
    assert_eq!(volume.info().chain.ciphers().len(), 3);
    assert_eq!(
        sector_hash(&volume, 0),
        "9ae00053bc19932a4b069c522ab0141863c434732a0e50450fb01ffcd2c58142"
    );
}
