//! Unconditional independent corpus; generation uses Python crypto libraries.
use base64::Engine;
use luks::{LuksError, LuksVolume};
use serde_json::Value;
use std::io::{Cursor, Read, Seek, SeekFrom};

fn corpus() -> Value {
    serde_json::from_str(include_str!("data/compatibility.json")).unwrap()
}
fn decode(value: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(value.as_str().unwrap())
        .unwrap()
}
fn key(value: &Value) -> Vec<u8> {
    value
        .as_str()
        .unwrap()
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}
fn plain() -> Vec<u8> {
    (0..1536).map(|index| (index * 17 + 7) as u8).collect()
}

#[test]
fn all_independent_containers_unlock_and_support_random_and_sequential_reads() {
    let expected = plain();
    for case in corpus()["containers"].as_array().unwrap() {
        let bytes = decode(&case["image"]);
        let mut volume =
            LuksVolume::unlock_with_passphrase(Cursor::new(&bytes), "LovelyCarve-密码".as_bytes())
                .unwrap_or_else(|error| {
                    panic!(
                        "{} / {} / {}: {error}",
                        case["cipher"], case["mode"], case["key"]
                    )
                });
        assert_eq!(volume.master_key(), key(&case["key"]));
        assert_eq!(volume.info().uuid, "f4e55035-9816-4c35-a82d-e4c6cf214a55");
        assert_eq!(volume.payload_size(), expected.len() as u64);
        // Reverse order, sector 2 first: ARC4 must not depend on read order.
        for (offset, length) in [(1023, 513), (511, 514), (0, 1536), (1536, 0)] {
            let mut actual = vec![0; length];
            volume.read_at(offset as u64, &mut actual).unwrap();
            assert_eq!(
                actual,
                expected[offset..offset + length],
                "{} / {}",
                case["cipher"],
                case["mode"]
            );
        }
        volume.seek(SeekFrom::Start(503)).unwrap();
        let mut actual = vec![0; 530];
        volume.read_exact(&mut actual).unwrap();
        assert_eq!(actual, expected[503..1033]);
        assert_eq!(volume.stream_position().unwrap(), 1033);
        assert!(volume.read_at(1535, &mut [0; 2]).is_err());
        volume.seek(SeekFrom::Start(0)).unwrap();
        assert!(volume.seek(SeekFrom::Current(i64::MAX)).is_ok());
        assert!(volume.seek(SeekFrom::Current(i64::MAX)).is_ok());
        assert!(volume.seek(SeekFrom::Current(i64::MAX)).is_err());
    }
}

#[test]
fn master_key_unlock_survives_disabled_password_slots_and_checks_digest() {
    for case in corpus()["containers"].as_array().unwrap() {
        let mut bytes = decode(&case["image"]);
        if bytes[7] == 1 {
            for slot in 0..8 {
                bytes[208 + slot * 48..212 + slot * 48].copy_from_slice(&0xdeadu32.to_be_bytes());
            }
        }
        let mut volume =
            LuksVolume::unlock_with_master_key(Cursor::new(&bytes), &key(&case["key"])).unwrap();
        let mut actual = vec![0; 1536];
        volume.read_at(0, &mut actual).unwrap();
        assert_eq!(actual, plain());
        let mut wrong = key(&case["key"]);
        wrong[0] ^= 1;
        assert!(matches!(
            LuksVolume::unlock_with_master_key(Cursor::new(&bytes), &wrong),
            Err(LuksError::AuthenticationFailed)
        ));
    }
}

#[test]
fn wrong_password_invalid_utf16_and_short_ciphertext_fail() {
    let case = &corpus()["containers"][0];
    let bytes = decode(&case["image"]);
    assert!(matches!(
        LuksVolume::unlock_with_passphrase(Cursor::new(&bytes), b"wrong"),
        Err(LuksError::AuthenticationFailed)
    ));
    assert!(LuksVolume::unlock_with_utf16_password(Cursor::new(&bytes), &[0xd800]).is_err());
    let password: Vec<u16> = "LovelyCarve-密码".encode_utf16().collect();
    let mut volume =
        LuksVolume::unlock_with_utf16_password(Cursor::new(&bytes), &password).unwrap();
    let mut actual = vec![0; 1536];
    volume.read_at(0, &mut actual).unwrap();
    assert_eq!(actual, plain());
    assert!(LuksVolume::unlock_with_passphrase(
        Cursor::new(&bytes[..bytes.len() - 1]),
        "LovelyCarve-密码".as_bytes()
    )
    .is_err());
    let mut zero_iterations = bytes.clone();
    zero_iterations[212..216].fill(0);
    assert!(matches!(
        LuksVolume::unlock_with_passphrase(
            Cursor::new(zero_iterations),
            "LovelyCarve-密码".as_bytes()
        ),
        Err(LuksError::Unsupported {
            what: "PBKDF2 iterations",
            ..
        })
    ));
}

#[test]
fn empty_luks2_digest_cannot_authenticate_any_master_key() {
    let data = corpus();
    let case = data["containers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| decode(&case["image"])[7] == 2)
        .unwrap();
    let mut bytes = decode(&case["image"]);
    let end = bytes[4096..16384]
        .iter()
        .position(|byte| *byte == 0)
        .unwrap()
        + 4096;
    let mut metadata: Value = serde_json::from_slice(&bytes[4096..end]).unwrap();
    metadata["digests"]["0"]["digest"] = Value::String(String::new());
    let json = serde_json::to_vec(&metadata).unwrap();
    bytes[4096..16384].fill(0);
    bytes[4096..4096 + json.len()].copy_from_slice(&json);
    assert!(matches!(
        LuksVolume::unlock_with_master_key(Cursor::new(&bytes), &key(&case["key"])),
        Err(LuksError::AuthenticationFailed)
    ));
}
