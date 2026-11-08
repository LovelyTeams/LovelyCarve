//! LovelyCarve range/stream regressions against independently encrypted fixtures.
#![allow(clippy::unwrap_used, clippy::expect_used)]
use std::{io, path::PathBuf};

use ios_backup_core::{Backup, Credentials, Password};

fn open(name: &str) -> Backup {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data")
        .join(name);
    Backup::open_with(
        &path,
        &Credentials::password(Password::new("test-password-1234")),
    )
    .unwrap()
}

#[test]
fn stream_and_seek_match_oracle_for_all_fixture_files() {
    for name in [
        "plain-backup",
        "encrypted-backup",
        "no-size-backup",
        "no-size-encrypted",
        "streaming-plain",
        "streaming-encrypted",
    ] {
        let mut backup = open(name);
        for entry in backup
            .files()
            .to_vec()
            .into_iter()
            .filter(|file| file.kind.has_content())
        {
            let expected = backup.read(&entry).unwrap();
            let mut streamed = Vec::new();
            let count = backup
                .stream(&entry, 64 * 1024, |chunk| {
                    streamed.extend_from_slice(chunk);
                    Ok(true)
                })
                .unwrap();
            assert_eq!(
                (streamed, count, backup.content_size(&entry).unwrap()),
                (
                    expected.clone(),
                    expected.len() as u64,
                    expected.len() as u64
                )
            );
            for offset in [
                0,
                1,
                15,
                16,
                17,
                255,
                65535,
                65536,
                131073,
                expected.len(),
                expected.len() + 1,
            ] {
                for length in [0, 1, 17, 257, usize::MAX] {
                    let start = offset.min(expected.len());
                    let end = start.saturating_add(length).min(expected.len());
                    assert_eq!(
                        backup.read_range(&entry, offset as u64, length).unwrap(),
                        expected[start..end]
                    );
                }
            }
        }
    }
}

#[test]
fn multi_chunk_cbc_stream_matches_independent_plaintext_and_keeps_last_byte() {
    let backup = open("streaming-encrypted");
    let file = backup.find("HomeDomain", "Documents/stream.bin").unwrap();
    let mut expected: Vec<u8> = (0..=255).cycle().take(256 * 1025).collect();
    expected.push(1);
    let mut actual = Vec::new();
    let mut visits = 0;
    let count = backup
        .stream(file, 64 * 1024, |chunk| {
            visits += 1;
            actual.extend_from_slice(chunk);
            Ok(true)
        })
        .unwrap();
    assert_eq!(
        (actual, count, visits),
        (expected.clone(), expected.len() as u64, 5)
    );
}

#[test]
fn visitor_errors_propagate_and_prefix_stops_after_one_chunk() {
    let backup = open("encrypted-backup");
    let entry = backup.find("HomeDomain", "Library/SMS/sms.db").unwrap();
    let result = backup.stream(entry, 64 * 1024, |_| Err(io::Error::other("cancelled")));
    assert_eq!(result.unwrap_err().to_string(), "cancelled");
    let mut visits = 0;
    backup
        .stream(entry, 64 * 1024, |_| {
            visits += 1;
            Ok(false)
        })
        .unwrap();
    assert_eq!(visits, 1);
}
