use super::*;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn derive(prf: Prf, password: &[u8], salt: &[u8], iterations: u32, len: usize) -> String {
    let mut out = vec![0; len];
    pbkdf2_full(prf, password, salt, iterations, &mut out);
    hex(&out)
}

/// PRF, password, salt, iterations and the expected derivation in hex.
type Vector<'a> = (Prf, &'a [u8], &'a [u8], u32, &'a str);

/// VeraCrypt's own `test_pkcs5` vectors (`src/Common/Tests.c`), the RIPEMD-160
/// one from TrueCrypt 7.1a and the BLAKE2s/Whirlpool ones re-derived with
/// Python's hashlib/hmac.
#[test]
fn pbkdf2_matches_veracrypt_self_test_vectors() {
    let salt = [0x12, 0x34, 0x56, 0x78];
    let cases: [Vector; 8] = [
        (
            Prf::Sha256,
            b"passwd",
            b"salt",
            1,
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783",
        ),
        (
            Prf::Sha256,
            b"password",
            b"salt",
            2,
            "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43",
        ),
        (
            Prf::Sha256,
            b"password",
            &salt,
            5,
            "f2a04fb2d3e9a5d8510b5c06df708e24e9c7d9153d22cddeb8a6dbfd7185c69932c0ee3727f724cfeaa6ac73a14c4e529b94f35406fc0465a10a24fef0981da62228eb245574ce6a3a28e2043a5913ec3ff2dbcf58dd53d9f917f6da74063c0b66f50ff558a327528c5b0791d081ebb6bc30694271f2d71842bee802937066ad3565bcf7968e64f1c692dae0dc1fb5f4",
        ),
        (
            Prf::Sha512,
            b"password",
            &salt,
            5,
            "1364aef80df5576c30d5714ca7753ffd00e5258b39c7447fce233d0875e02f48d630d700b624dbe05ad747ef52caa6348347e5cbe987f120596ae6a9cf5178c6b623a6740de891be1ad028ccce16989abefbdc78c9e17d7267cee161565f9668e6e1ddf4bf1b80e0191cf4c4d3ddd5d5572d83c7a33787f44ee0f6d86d65dca052a313be81fc30be7d695834b6dd41c6",
        ),
        (
            Prf::Blake2s,
            b"password",
            &salt,
            5,
            "8d51fa3146253767a3296b3c6bc15db2eee16c280026ea08659c12f107de0db99b4f39fac68026b18f8e4889852d242d",
        ),
        (
            Prf::Whirlpool,
            b"password",
            &salt,
            5,
            "507c366fee102e9ae28ad582727d270fe84d7f687acfb5e74367aa9893522b096e42df2c594a916d7e10aeb21a898fb98fe631a9d89f9826f4dacd7d6565de109591b48426ae43a1005b1eb83897a41e4bd26564bcfa1f3585db4f97656fbd24",
        ),
        (
            Prf::Streebog,
            b"password",
            &salt,
            5,
            "d053a2306f4581ebbc0681c5e753a85dc7f123331ebe642c3b0f26d700e195c96526b185be1ee2f49bfc6b1484da2461a01b9e795cee696ef925b11dcaa031ba026f9e990fdb25015bf1c71019533b293f1800d6fc8503dcf2e5e95ab11e61de",
        ),
        (
            Prf::Ripemd160,
            b"password",
            &salt,
            5,
            "7a3d7c03e7266bf83d78fb29d2641f56eaf0e5f5ccc43a31a88470bfbd6f8e78245ac00af6faf0f6e900475f73cee143",
        ),
    ];
    for (prf, password, salt, iterations, expected) in cases {
        assert_eq!(
            derive(prf, password, salt, iterations, expected.len() / 2),
            expected,
            "{}",
            prf.name()
        );
    }
}

#[test]
fn block_split_derivation_matches_the_pbkdf2_crate() {
    let salt: Vec<u8> = (0..64).collect();
    let password = b"LovelyCarve \xe5\xaf\x86\xe7\xa0\x81";
    let mut expected = [0u8; HEADER_KEY_LEN];
    pbkdf2::pbkdf2_hmac::<sha2::Sha512>(password, &salt, 37, &mut expected);
    assert_eq!(
        derive(Prf::Sha512, password, &salt, 37, 192),
        hex(&expected)
    );
    pbkdf2::pbkdf2_hmac::<ripemd::Ripemd160>(password, &salt, 37, &mut expected);
    assert_eq!(
        derive(Prf::Ripemd160, password, &salt, 37, 192),
        hex(&expected)
    );
    pbkdf2::pbkdf2_hmac::<streebog::Streebog512>(password, &salt, 37, &mut expected);
    assert_eq!(
        derive(Prf::Streebog, password, &salt, 37, 192),
        hex(&expected)
    );
}

#[test]
fn a_stop_request_ends_the_block_without_output() {
    let mut out = [0xAAu8; 64];
    let outcome = pbkdf2_block(Prf::Sha512, b"x", b"salt", 5000, 1, &mut out, &|| true);
    assert_eq!(outcome, Err(Stopped));
    assert_eq!(out, [0xAAu8; 64]);
}

/// Python `cryptography`'s Argon2id with one lane: VeraCrypt passes one lane,
/// version 0x13 and a 64-byte salt.
#[test]
fn argon2id_matches_an_independent_derivation() {
    let salt: Vec<u8> = (0..64).collect();
    let key = argon2id(b"lovelycarve-test", &salt, 2, 256).unwrap();
    assert_eq!(
        hex(key.as_ref()),
        "4e54c051ed5919604a3622eeb4ee5de4fea1765a856b3b51ae4201c0db1fe9607a0587d4a99bcd69b5b350dc26c1e2b4917047efcd2c4702af98e0748261429a18ba2ef8d0083a4feb8fcf82a971146d6a3eadc35b94899444ccc16bf1ece50cc32450035fda11e7240484a7a3e33c58ad03610fd9e68e035f96fdecdee281de7964faa371ae11fcbc2739c86e75068b0a88045e10b866810acad188df0948cf361046c3f764997d48673084eb9eb0840a5e49367c46350124211f18084b2fd1"
    );
}

#[test]
fn argon2_cost_follows_veracrypt_get_argon2_params() {
    assert_eq!(argon2_cost(0), (6, 416 * 1024));
    assert_eq!(argon2_cost(12), (6, 416 * 1024));
    assert_eq!(argon2_cost(1), (3, 64 * 1024));
    assert_eq!(argon2_cost(31), (13, 1024 * 1024));
    assert_eq!(argon2_cost(32), (14, 1024 * 1024));
    assert_eq!(argon2_cost(MAX_PIM), (MAX_PIM - 18, 1024 * 1024));
}

fn costs(kdfs: &[Kdf]) -> Vec<(Prf, Family, u32)> {
    kdfs.iter()
        .map(|kdf| (kdf.prf, kdf.family, kdf.iterations))
        .collect()
}

#[test]
fn volume_candidates_follow_veracrypt_and_truecrypt_counts() {
    use Family::{TrueCrypt, VeraCrypt};
    let kdfs = candidates(KdfTarget::Volume, 0, None, u32::MAX).unwrap();
    assert_eq!(
        costs(&kdfs),
        [
            (Prf::Ripemd160, TrueCrypt, 2000),
            (Prf::Sha512, TrueCrypt, 1000),
            (Prf::Whirlpool, TrueCrypt, 1000),
            (Prf::Sha512, VeraCrypt, 500_000),
            (Prf::Sha256, VeraCrypt, 500_000),
            (Prf::Blake2s, VeraCrypt, 500_000),
            (Prf::Whirlpool, VeraCrypt, 500_000),
            (Prf::Streebog, VeraCrypt, 500_000),
            (Prf::Ripemd160, VeraCrypt, 655_331),
            (Prf::Argon2id, VeraCrypt, 6),
        ]
    );
    // A PIM drops TrueCrypt and gives every PBKDF2 PRF, RIPEMD-160
    // included, 15000 + 1000 * PIM iterations.
    let kdfs = candidates(KdfTarget::Volume, 10, None, u32::MAX).unwrap();
    assert!(kdfs.iter().all(|kdf| kdf.family == VeraCrypt));
    assert!(kdfs
        .iter()
        .filter(|kdf| kdf.prf != Prf::Argon2id)
        .all(|kdf| kdf.iterations == 25_000));
    assert_eq!(kdfs.last().map(|kdf| kdf.memory_kib), Some(352 * 1024));
}

#[test]
fn system_candidates_use_boot_counts_and_never_argon2() {
    use Family::{TrueCrypt, VeraCrypt};
    let kdfs = candidates(KdfTarget::System, 0, None, u32::MAX).unwrap();
    assert_eq!(
        costs(&kdfs),
        [
            (Prf::Ripemd160, TrueCrypt, 1000),
            (Prf::Sha256, VeraCrypt, 200_000),
            (Prf::Blake2s, VeraCrypt, 200_000),
            (Prf::Sha512, VeraCrypt, 500_000),
            (Prf::Whirlpool, VeraCrypt, 500_000),
            (Prf::Streebog, VeraCrypt, 200_000),
            (Prf::Streebog, VeraCrypt, 500_000),
            (Prf::Ripemd160, VeraCrypt, 327_661),
        ]
    );
    let kdfs = candidates(KdfTarget::System, 3, Some(Prf::Sha256), u32::MAX).unwrap();
    assert_eq!(costs(&kdfs), [(Prf::Sha256, VeraCrypt, 6144)]);
    assert!(candidates(KdfTarget::System, 0, Some(Prf::Argon2id), u32::MAX).is_err());
    assert!(candidates(KdfTarget::System, MAX_BOOT_PIM + 1, None, u32::MAX).is_err());
    assert!(candidates(KdfTarget::Volume, MAX_PIM + 1, None, u32::MAX).is_err());
}

#[test]
fn argon2id_is_left_out_of_automatic_searches_above_the_pass_budget() {
    let auto = candidates(KdfTarget::Volume, 40, None, 13).unwrap();
    assert!(auto.iter().all(|kdf| kdf.prf != Prf::Argon2id));
    let chosen = candidates(KdfTarget::Volume, 40, Some(Prf::Argon2id), 13).unwrap();
    assert_eq!(
        chosen,
        [Kdf {
            prf: Prf::Argon2id,
            family: Family::VeraCrypt,
            iterations: 22,
            memory_kib: 1024 * 1024
        }]
    );
}

#[test]
fn prf_ids_round_trip() {
    for prf in Prf::ALL {
        assert_eq!(Prf::from_id(prf.id()), Some(prf));
    }
    assert_eq!(Prf::from_id("md5"), None);
}
