//! The independent compatibility corpus (`tests/data/compatibility.json`),
//! written by `scripts/make-veracrypt-compatibility-fixtures.py` with Python
//! crypto libraries and no code of this crate. Every check either opens with
//! the facts it expects and reads back the plaintext pattern, or fails the
//! way it expects.

use std::collections::HashMap;

use base64::Engine;
use serde_json::Value;
use veracrypt::{
    unlock, Control, HeaderSelection, KeyfilePool, Prf, SystemRange, UnlockRequest, VeraError,
};

fn corpus() -> Value {
    serde_json::from_str(include_str!("data/compatibility.json")).unwrap()
}

fn decode(value: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(value.as_str().unwrap())
        .unwrap()
}

fn image(container: &Value) -> Vec<u8> {
    let mut bytes = vec![0u8; container["size"].as_u64().unwrap() as usize];
    for segment in container["segments"].as_array().unwrap() {
        let offset = segment["offset"].as_u64().unwrap() as usize;
        let data = decode(&segment["data"]);
        bytes[offset..offset + data.len()].copy_from_slice(&data);
    }
    bytes
}

/// The plaintext byte at `index` of a volume with `seed`.
fn pattern(length: usize, seed: u64) -> Vec<u8> {
    (0..length)
        .map(|index| ((index as u64 * 29 + seed) % 256) as u8)
        .collect()
}

fn headers(request: &Value) -> HeaderSelection {
    match request["header"].as_str().unwrap() {
        "auto" => HeaderSelection::Auto,
        "normal" => HeaderSelection::Normal,
        "hidden" => HeaderSelection::Hidden,
        "backup" => HeaderSelection::Backup,
        "system" => HeaderSelection::System(match &request["system"] {
            Value::String(drive) if drive == "drive" => SystemRange::Drive,
            range => SystemRange::Partition {
                offset: range["offset"].as_u64().unwrap(),
                length: range["length"].as_u64().unwrap(),
            },
        }),
        other => panic!("unknown header {other}"),
    }
}

fn password(request: &Value, keyfiles: &HashMap<String, Vec<u8>>) -> Vec<u8> {
    let typed = request["password"].as_str().unwrap().as_bytes();
    let names = request["keyfiles"].as_array().unwrap();
    if names.is_empty() {
        return typed.to_vec();
    }
    let mut pool = KeyfilePool::new(typed.len());
    for name in names {
        pool.add(&keyfiles[name.as_str().unwrap()]).unwrap();
    }
    pool.apply(typed).unwrap().to_vec()
}

#[test]
fn every_independent_container_opens_as_expected() {
    let corpus = corpus();
    let keyfiles: HashMap<String, Vec<u8>> = corpus["keyfiles"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(name, data)| (name.clone(), decode(data)))
        .collect();
    let control = Control {
        cancelled: &|| false,
        progress: None,
        threads: None,
    };
    let containers = corpus["containers"].as_array().unwrap();
    assert!(containers.len() >= 24);
    for container in containers {
        let bytes = image(container);
        for check in container["checks"].as_array().unwrap() {
            let request = &check["request"];
            let expect = &check["expect"];
            let label = format!("{} {request}", container["name"]);
            let secret = password(request, &keyfiles);
            let prf = request["prf"].as_str().map(|id| Prf::from_id(id).unwrap());
            let unlock_request = UnlockRequest {
                password: &secret,
                pim: request["pim"].as_u64().unwrap() as u32,
                prf,
                headers: headers(request),
            };
            let outcome = unlock(&bytes, &unlock_request, &control);
            if let Some(error) = expect["error"].as_str() {
                match (error, &outcome) {
                    ("authentication", Err(VeraError::AuthenticationFailed))
                    | ("unsupported", Err(VeraError::Unsupported(_)))
                    | ("corrupt", Err(VeraError::Corrupt(_))) => {}
                    (_, Err(other)) => panic!("{label}: expected {error}, got {other}"),
                    (_, Ok(_)) => panic!("{label}: expected {error}, but it opened"),
                }
                continue;
            }
            let volume = outcome.unwrap_or_else(|error| panic!("{label}: {error}"));
            let info = volume.info();
            assert_eq!(info.flavor.name(), expect["flavor"], "{label}");
            assert_eq!(info.header.id(), expect["header"], "{label}");
            assert_eq!(
                info.view_start,
                expect["view_start"].as_u64().unwrap(),
                "{label}"
            );
            assert_eq!(volume.size(), expect["size"].as_u64().unwrap(), "{label}");
            let Some(seed) = expect["seed"].as_u64() else {
                continue;
            };
            assert_eq!(
                (
                    info.kdf.prf.id(),
                    u64::from(info.kdf.iterations),
                    info.chain.name(),
                    u64::from(info.sector_size),
                    info.encryption_complete,
                    info.plaintext_boot_sector,
                ),
                (
                    expect["prf"].as_str().unwrap(),
                    expect["iterations"].as_u64().unwrap(),
                    expect["cipher"].as_str().unwrap().to_string(),
                    expect["sector_size"].as_u64().unwrap(),
                    expect["complete"].as_bool().unwrap(),
                    expect["plaintext_boot_sector"].as_bool().unwrap(),
                ),
                "{label}"
            );
            let size = volume.size() as usize;
            let start = expect["compare"][0].as_u64().unwrap() as usize;
            let end = expect["compare"][1].as_u64().unwrap() as usize;
            let expected = pattern(size, seed);
            let mut whole = vec![0u8; size];
            volume.read_at(0, &mut whole).unwrap();
            assert!(
                whole[start..end] == expected[start..end],
                "{label}: whole read"
            );
            for (offset, length) in [
                (start + 1, 1000),
                (start + 511, 2),
                (end - 3, 3),
                (start, end - start),
            ] {
                let length = length.min(end - offset);
                let mut part = vec![0u8; length];
                volume.read_at(offset as u64, &mut part).unwrap();
                assert!(
                    part == expected[offset..offset + length],
                    "{label}: {offset}+{length}"
                );
            }
        }
    }
}
