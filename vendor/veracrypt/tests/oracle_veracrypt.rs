//! Tier-1 oracle: unlock the cryptsetup-project `vc_1-sha512-xts-aes` VeraCrypt
//! volume (SHA-512 PRF, AES-256-XTS) with its published password and confirm the
//! decrypted sectors match `cryptsetup --veracrypt` byte-for-byte (SHA-256).
//!
//! Tier-1 (unimpeachable): a third party authored this real VeraCrypt volume with
//! a published password (`aaaaaaaaaaaa`), and the decrypted-sector ground truth is
//! confirmed by TWO fully independent reference implementations — **VeraCrypt
//! 1.26.20** (Idrix, the format's own implementation) and **cryptsetup 2.7.0** (an
//! independent reimplementation) — both of which agree with this crate byte-for-
//! byte. From `tests/tcrypt-images.tar.xz`. Env-gated on `VC_ORACLE` (path to the
//! image). Provenance: `/tmp/vc-oracle/GROUND-TRUTH.md`.
//!
//! ```bash
//! VC_ORACLE=/tmp/vc-oracle/vc_1-sha512-xts-aes \
//!   cargo test -p veracrypt-core --test oracle_veracrypt -- --nocapture
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs::File;

use common::sha256_hex;
use veracrypt::VeraVolume;

const PASSWORD: &[u8] = b"aaaaaaaaaaaa";

#[test]
fn tier1_cascade3_aes_twofish_serpent_matches_veracrypt() {
    // Three-cipher cascade (AES-Twofish-Serpent) — confirms the general n-cipher
    // key offsets and reverse-apply order, and Serpent-256 inside a cascade.
    let Ok(path) = std::env::var("VC_CASCADE3_ORACLE") else {
        eprintln!("VC_CASCADE3_ORACLE unset — skipping Tier-1 3-cipher cascade oracle");
        return;
    };
    let file = File::open(&path).expect("open 3-cascade vc image");
    let mut vol = VeraVolume::unlock_with_password(file, PASSWORD).expect("unlock 3-cascade");
    assert_eq!(
        vol.info().ciphers.len(),
        3,
        "must brute to a 3-cipher cascade"
    );
    let mut buf = [0u8; 512];
    vol.read_at(0, &mut buf).expect("read_at");
    let got = sha256_hex(&buf);
    println!("cascade3 sector 0: {got} ({})", vol.info().cipher_display());
    assert_eq!(
        got, "9ae00053bc19932a4b069c522ab0141863c434732a0e50450fb01ffcd2c58142",
        "3-cascade sector 0 does not match veracrypt"
    );
}

#[test]
fn tier1_cascade_aes_twofish_matches_veracrypt() {
    // Proves a VeraCrypt cipher CASCADE (AES-Twofish, stacked XTS) decrypts
    // correctly — key layout + apply order per cryptsetup TCRYPT_decrypt_hdr.
    let Ok(path) = std::env::var("VC_CASCADE_ORACLE") else {
        eprintln!("VC_CASCADE_ORACLE unset — skipping Tier-1 cascade oracle");
        return;
    };
    let file = File::open(&path).expect("open cascade vc image");
    let mut vol = VeraVolume::unlock_with_password(file, PASSWORD).expect("unlock cascade volume");
    assert_eq!(
        vol.info().cipher_display(),
        "aes-twofish",
        "must brute to the AES-Twofish cascade"
    );
    let mut buf = [0u8; 512];
    vol.read_at(0, &mut buf).expect("read_at");
    let got = sha256_hex(&buf);
    println!("cascade sector 0: {got}");
    assert_eq!(
        got, "da09622b78baeeb1fa8e6532f1eb23afc733a8449097d3a08d612286d4161492",
        "cascade sector 0 does not match veracrypt/cryptsetup"
    );
}

#[test]
fn tier1_serpent256_matches_cryptsetup() {
    // Proves 256-bit VeraCrypt Serpent decrypts correctly (via serpent
    // new_from_slice). Same plaintext as the AES oracle → same sector hashes.
    let Ok(path) = std::env::var("VC_SERPENT_ORACLE") else {
        eprintln!("VC_SERPENT_ORACLE unset — skipping Tier-1 Serpent oracle");
        return;
    };
    let file = File::open(&path).expect("open serpent vc image");
    let mut vol = VeraVolume::unlock_with_password(file, PASSWORD).expect("unlock serpent volume");
    assert_eq!(
        vol.info().cipher_display(),
        "serpent",
        "must brute to serpent"
    );
    // vcserp.vc minted with the real veracrypt binary (Serpent-256, SHA-512),
    // identical plaintext per sector → same hash; ground truth from veracrypt.
    let cases: [(u64, &str); 2] = [
        (
            0,
            "479ad71598de182171230acbe3322cdac3b9bb9f70894a7cc3e7b526be46693b",
        ),
        (
            1,
            "479ad71598de182171230acbe3322cdac3b9bb9f70894a7cc3e7b526be46693b",
        ),
    ];
    for (lba, want) in cases {
        let mut buf = [0u8; 512];
        vol.read_at(lba * 512, &mut buf).expect("read_at");
        let got = sha256_hex(&buf);
        println!("serpent sector {lba}: {got}");
        assert_eq!(got, want, "serpent sector {lba} does not match cryptsetup");
    }
}

#[test]
fn tier1_vc_sha512_xts_aes_matches_cryptsetup() {
    let Ok(path) = std::env::var("VC_ORACLE") else {
        eprintln!("VC_ORACLE unset — skipping Tier-1 VeraCrypt oracle");
        return;
    };
    let file = File::open(&path).expect("open vc image");
    let mut vol = VeraVolume::unlock_with_password(file, PASSWORD).expect("unlock vc volume");

    assert_eq!(vol.info().prf.name(), "sha512");
    assert_eq!(vol.info().cipher_display(), "aes");
    assert_eq!(vol.info().encrypted_area_start, 131_072);

    // (data-area LBA, expected decrypted-sector SHA-256) — cryptsetup ground truth.
    let cases: [(u64, &str); 4] = [
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
    ];

    for (lba, want) in cases {
        let mut buf = [0u8; 512];
        vol.read_at(lba * 512, &mut buf).expect("read_at");
        let got = sha256_hex(&buf);
        println!("sector {lba}: {got}");
        assert_eq!(
            got, want,
            "decrypted sector {lba} does not match cryptsetup"
        );
    }
}

#[test]
fn tier1_hidden_volume_matches_cryptsetup() {
    let Ok(path) = std::env::var("VC_HIDDEN_ORACLE") else {
        eprintln!("VC_HIDDEN_ORACLE unset — skipping Tier-1 hidden-volume oracle");
        return;
    };
    let file = File::open(&path).expect("open hidden vc image");
    let mut vol = VeraVolume::unlock_hidden_with_password(file, b"bbbbbbbbbbbb")
        .expect("unlock hidden volume");
    // Hidden data area begins at byte 165888 (its own encrypted_area_start).
    assert_eq!(vol.info().encrypted_area_start, 165_888);

    let cases: [(u64, &str); 2] = [
        (
            0,
            "79a162bda41b98338009c880004ce44d3d84c2bf412f76271fea08279bffed8e",
        ),
        (
            2,
            "6242cb7cb043b219a77ffa2bd0aedab6735389bbbe8b3b2e88410cf5f74247a5",
        ),
    ];
    for (lba, want) in cases {
        let mut buf = [0u8; 512];
        vol.read_at(lba * 512, &mut buf).expect("read_at");
        let got = sha256_hex(&buf);
        println!("hidden sector {lba}: {got}");
        assert_eq!(got, want, "hidden sector {lba} does not match cryptsetup");
    }
}

#[test]
fn wrong_password_fails() {
    let Ok(path) = std::env::var("VC_ORACLE") else {
        return;
    };
    let file = File::open(&path).unwrap();
    // Use a small PIM so the all-PRF brute (which must try every PRF for a wrong
    // password) runs at low iteration counts — the "no PRF matches" path, fast.
    assert!(VeraVolume::unlock_with_pim(file, b"wrongpassword", 1).is_err());
}
