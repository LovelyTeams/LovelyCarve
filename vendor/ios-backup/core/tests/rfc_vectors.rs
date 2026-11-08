//! Published test vectors for the primitives the backup keybag composes (T1 —
//! a third party authored both the artifact and the answer key).
//!
//! These pin *our usage* — key widths, byte order, which digest goes with which
//! salt — not the `RustCrypto` crates themselves. That is where the mistakes in a
//! keybag implementation actually live: every primitive below is individually
//! correct in every wrong implementation of this format too.
//!
//! Sources:
//!   * AES key wrap — RFC 3394 §4.1–§4.6
//!     <https://www.rfc-editor.org/rfc/rfc3394#section-4>
//!   * PBKDF2-HMAC-SHA1 — RFC 6070 §2
//!     <https://www.rfc-editor.org/rfc/rfc6070#section-2>
//!   * PBKDF2-HMAC-SHA256 — RFC 7914 §11
//!     <https://www.rfc-editor.org/rfc/rfc7914#section-11>

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ios_backup_core::crypto;

fn unhex(s: &str) -> Vec<u8> {
    hex::decode(s).expect("test vector is valid hex")
}

// ---------------------------------------------------------------- AES key wrap

/// RFC 3394 §4.1 — 128-bit KEK, 128-bit key.
#[test]
fn rfc3394_4_1_wrap_128_with_128() {
    let kek = unhex("000102030405060708090A0B0C0D0E0F");
    let wrapped = unhex("1FA68B0A8112B447AEF34BD8FB5A7B829D3E862371D2CFE5");
    let expected = unhex("00112233445566778899AABBCCDDEEFF");

    assert_eq!(crypto::aes_key_unwrap(&kek, &wrapped).unwrap(), expected);
}

/// RFC 3394 §4.2 — 192-bit KEK, 128-bit key.
#[test]
fn rfc3394_4_2_wrap_128_with_192() {
    let kek = unhex("000102030405060708090A0B0C0D0E0F1011121314151617");
    let wrapped = unhex("96778B25AE6CA435F92B5B97C050AED2468AB8A17AD84E5D");
    let expected = unhex("00112233445566778899AABBCCDDEEFF");

    assert_eq!(crypto::aes_key_unwrap(&kek, &wrapped).unwrap(), expected);
}

/// RFC 3394 §4.3 — 256-bit KEK, 128-bit key.
#[test]
fn rfc3394_4_3_wrap_128_with_256() {
    let kek = unhex("000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F");
    let wrapped = unhex("64E8C3F9CE0F5BA263E9777905818A2A93C8191E7D6E8AE7");
    let expected = unhex("00112233445566778899AABBCCDDEEFF");

    assert_eq!(crypto::aes_key_unwrap(&kek, &wrapped).unwrap(), expected);
}

/// RFC 3394 §4.6 — 256-bit KEK, 256-bit key. **This is the backup case**: a
/// 32-byte class key wrapped to 40 bytes under a 32-byte password-derived key.
#[test]
fn rfc3394_4_6_wrap_256_with_256_is_the_backup_shape() {
    let kek = unhex("000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F");
    let wrapped =
        unhex("28C9F404C4B810F4CBCCB35CFB87F8263F5786E2D80ED326CBC7F0E71A99F43BFB988B9B7A02DD21");
    let expected = unhex("00112233445566778899AABBCCDDEEFF000102030405060708090A0B0C0D0E0F");

    assert_eq!(wrapped.len(), 40, "a wrapped 32-byte key is 40 bytes");
    assert_eq!(crypto::aes_key_unwrap(&kek, &wrapped).unwrap(), expected);
}

/// The integrity check is the whole point: RFC 3394 prepends a known A6A6…
/// value, so unwrapping under the wrong KEK **fails** rather than returning 32
/// bytes of garbage. This is how a wrong backup password is detected — without
/// it, a wrong password yields a plausible key and silently produces rubbish
/// plaintext that would be reported as recovered evidence.
#[test]
fn unwrapping_under_the_wrong_kek_fails_rather_than_returning_garbage() {
    let mut kek = unhex("000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F");
    kek[0] ^= 0x01;
    let wrapped =
        unhex("28C9F404C4B810F4CBCCB35CFB87F8263F5786E2D80ED326CBC7F0E71A99F43BFB988B9B7A02DD21");

    assert!(crypto::aes_key_unwrap(&kek, &wrapped).is_err());
}

#[test]
fn a_wrapped_key_of_the_wrong_length_is_rejected() {
    let kek = unhex("000102030405060708090A0B0C0D0E0F101112131415161718191A1B1C1D1E1F");
    // 39 bytes: not a whole number of 8-byte semiblocks.
    assert!(crypto::aes_key_unwrap(&kek, &[0u8; 39]).is_err());
}

#[test]
fn an_unsupported_kek_width_is_reported_with_its_value() {
    let err = crypto::aes_key_unwrap(&[0u8; 17], &[0u8; 40]).unwrap_err();
    assert!(
        format!("{err}").contains("17"),
        "the offending width must appear verbatim, got: {err}"
    );
}

// ------------------------------------------------------------------- PBKDF2

/// RFC 6070 §2 — PBKDF2-HMAC-SHA1, the digest the keybag's `SALT`/`ITER` use.
#[test]
fn rfc6070_pbkdf2_hmac_sha1_vectors() {
    let cases: &[(&str, &str, u32, usize, &str)] = &[
        (
            "password",
            "salt",
            1,
            20,
            "0c60c80f961f0e71f3a9b524af6012062fe037a6",
        ),
        (
            "password",
            "salt",
            2,
            20,
            "ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957",
        ),
        (
            "password",
            "salt",
            4096,
            20,
            "4b007901b765489abead49d926f721d065a429c1",
        ),
        (
            "passwordPASSWORDpassword",
            "saltSALTsaltSALTsaltSALTsaltSALTsalt",
            4096,
            25,
            "3d2eec4fe41c849b80c8d83662c0e44a8b291a964cf2f07038",
        ),
    ];

    for (password, salt, rounds, len, expected) in cases {
        let mut out = vec![0u8; *len];
        crypto::pbkdf2_hmac_sha1(password.as_bytes(), salt.as_bytes(), *rounds, &mut out);
        assert_eq!(out, unhex(expected), "PBKDF2-SHA1 c={rounds} dkLen={len}");
    }
}

/// RFC 7914 §11 — PBKDF2-HMAC-SHA256, the digest the *double protection*
/// `DPSL`/`DPIC` pre-round uses. Using SHA-1 here (or SHA-256 for the second
/// round) yields a wrong key that is indistinguishable from a wrong password.
#[test]
fn rfc7914_pbkdf2_hmac_sha256_vectors() {
    let cases: &[(&str, &str, u32, usize, &str)] = &[
        (
            "passwd",
            "salt",
            1,
            64,
            "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc\
             49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783",
        ),
        (
            "Password",
            "NaCl",
            80_000,
            64,
            "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56\
             a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d",
        ),
    ];

    for (password, salt, rounds, len, expected) in cases {
        let mut out = vec![0u8; *len];
        crypto::pbkdf2_hmac_sha256(password.as_bytes(), salt.as_bytes(), *rounds, &mut out);
        assert_eq!(
            out,
            unhex(&expected.replace([' ', '\n'], "")),
            "PBKDF2-SHA256 c={rounds} dkLen={len}"
        );
    }
}

// ------------------------------------------------------------------- AES-CBC

/// NIST SP 800-38A §F.2.5 — AES-256-CBC decryption. The backup uses a zero IV,
/// so the vector is applied by prepending the vector's IV as a ciphertext block
/// would be: here we check the raw CBC core against its published answer.
#[test]
fn sp800_38a_aes256_cbc_first_block() {
    // Key and IV from SP 800-38A F.2.5/F.2.6.
    let key = unhex("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4");
    let iv = unhex("000102030405060708090A0B0C0D0E0F");
    let ciphertext = unhex("f58c4c04d6e5f1ba779eabfb5f7bfbd6");
    let expected = unhex("6bc1bee22e409f96e93d7e117393172a");

    assert_eq!(
        crypto::decrypt_aes_cbc(&key, &iv, &ciphertext).unwrap(),
        expected
    );
}

#[test]
fn a_ciphertext_that_is_not_a_whole_number_of_blocks_is_rejected() {
    let key = [0u8; 32];
    assert!(crypto::decrypt_aes_cbc(&key, &[0u8; 16], &[0u8; 17]).is_err());
}
