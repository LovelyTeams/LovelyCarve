//! The independent corpus written by `scripts/make-kdbx-fixtures.py` (see
//! `tests/data/README.md`): KDBX 3.0 / 3.1 / 4.0 / 4.1 with every cipher,
//! KDF, inner stream and keyfile form, multiple blocks, protected binaries
//! and history.

use base64::Engine;
use kdbx::{Database, Entry, Error, Group, HeaderHash, Key};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn decode(value: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(value.as_str().unwrap())
        .unwrap()
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn key_of(case: &Value, password_suffix: &str) -> Key {
    let mut key = Key::new();
    if let Some(password) = case["password"].as_str() {
        key = key.with_password(&format!("{password}{password_suffix}"));
    } else if !password_suffix.is_empty() {
        key = key.with_password(password_suffix);
    }
    if !case["keyfile"].is_null() {
        key = key.with_keyfile(&decode(&case["keyfile"])).unwrap();
    }
    key
}

fn walk<'a>(
    group: &'a Group,
    path: &mut Vec<&'a str>,
    groups: &mut Vec<String>,
    entries: &mut Vec<(String, &'a Entry)>,
) {
    groups.push(path.join("/"));
    for entry in &group.entries {
        entries.push((path.join("/"), entry));
    }
    for child in &group.groups {
        path.push(&child.name);
        walk(child, path, groups, entries);
        path.pop();
    }
}

#[test]
fn every_synthetic_database_reads_as_written() {
    let corpus: Value = serde_json::from_str(include_str!("data/synthetic.json")).unwrap();
    let cases = corpus["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 7);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let file = decode(&case["file"]);
        let expect = &case["expect"];
        let database = Database::open(&file, &key_of(case, ""))
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let header = &database.header;
        assert_eq!(header.version.to_string(), expect["version"], "{name}");
        assert_eq!(header.cipher.name(), expect["cipher"], "{name}");
        assert_eq!(header.kdf.name(), expect["kdf"], "{name}");
        assert_eq!(header.compression.name(), expect["compression"], "{name}");
        assert_eq!(
            database.inner_stream.name(),
            expect["inner_stream"],
            "{name}"
        );
        let header_hash = match database.header_hash {
            HeaderHash::Matches => "matches",
            HeaderHash::Mismatch => "mismatch",
            HeaderHash::Absent => "absent",
        };
        assert_eq!(header_hash, expect["header_hash"], "{name}");
        assert_eq!(
            header.warnings.len() as u64,
            expect["header_warnings"],
            "{name}: {:?}",
            header.warnings
        );
        assert!(
            database.warnings.is_empty(),
            "{name}: {:?}",
            database.warnings
        );
        assert_eq!(
            hex(database.transformed_key()),
            case["transformed_key"],
            "{name}"
        );
        assert_eq!(
            database.meta.database_name.as_deref(),
            expect["database_name"].as_str(),
            "{name}"
        );

        let (mut groups, mut entries) = (Vec::new(), Vec::new());
        walk(&database.root, &mut Vec::new(), &mut groups, &mut entries);
        let expected_groups: Vec<String> = expect["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|group| group.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(groups, expected_groups, "{name}");

        let expected_entries = expect["entries"].as_array().unwrap();
        assert_eq!(entries.len(), expected_entries.len(), "{name}");
        for ((path, entry), wanted) in entries.iter().zip(expected_entries) {
            let title = wanted["title"].as_str().unwrap();
            assert_eq!(path, wanted["path"].as_str().unwrap(), "{name} {title}");
            assert_eq!(entry.title(), Some(title), "{name}");
            let fields = wanted["fields"].as_object().unwrap();
            assert_eq!(entry.fields.len(), fields.len(), "{name} {title}");
            for field in &entry.fields {
                let wanted_field = &fields[&field.key];
                assert_eq!(field.value, wanted_field[0], "{name} {title} {}", field.key);
                assert_eq!(
                    field.protected, wanted_field[1],
                    "{name} {title} {}",
                    field.key
                );
                assert_eq!(field.unreadable, None, "{name} {title} {}", field.key);
            }
            let tags: Vec<&str> = wanted["tags"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tag| tag.as_str().unwrap())
                .collect();
            assert_eq!(entry.tags, tags, "{name} {title}");
            let attachments: Vec<&str> = entry
                .attachments
                .iter()
                .map(|attachment| attachment.name.as_str())
                .collect();
            let wanted_attachments: Vec<&str> = wanted["attachments"]
                .as_array()
                .unwrap()
                .iter()
                .map(|attachment| attachment.as_str().unwrap())
                .collect();
            assert_eq!(attachments, wanted_attachments, "{name} {title}");
            assert_eq!(
                entry.history.len() as u64,
                wanted["history"],
                "{name} {title}"
            );
            assert_eq!(entry.times.expires, wanted["expires"], "{name} {title}");
            assert_eq!(
                entry
                    .times
                    .creation
                    .as_ref()
                    .and_then(|time| time.unix_seconds()),
                wanted["created"].as_i64(),
                "{name} {title}"
            );
            for version in &entry.history {
                assert!(
                    version
                        .fields
                        .iter()
                        .all(|field| field.unreadable.is_none()),
                    "{name}"
                );
            }
        }
        assert_eq!(
            database.recycle_bin().map(|group| group.name.as_str()),
            expect["recycle_bin"].as_str(),
            "{name}"
        );
        assert_eq!(
            database.deleted_objects.len() as u64,
            expect["deleted"],
            "{name}"
        );

        let binaries = expect["binaries"].as_array().unwrap();
        assert_eq!(database.binaries.len(), binaries.len(), "{name}");
        for ((id, binary), wanted) in database.binaries.iter().zip(binaries) {
            assert_eq!(binary.unreadable, None, "{name} binary {id}");
            assert_eq!(
                hex(&Sha256::digest(&binary.data)),
                wanted["sha256"],
                "{name} binary {id}"
            );
            assert_eq!(
                binary.data.len() as u64,
                wanted["length"],
                "{name} binary {id}"
            );
            assert_eq!(binary.protected, wanted["protected"], "{name} binary {id}");
        }
        let bank = &entries[0].1;
        for (attachment, wanted) in bank.attachments.iter().zip(binaries) {
            let data = database.attachment_data(attachment).unwrap();
            assert_eq!(hex(&Sha256::digest(data)), wanted["sha256"], "{name}");
        }

        let reference = entries
            .iter()
            .find(|(_, entry)| entry.title() == Some("Ref"))
            .unwrap()
            .1;
        assert_eq!(
            database.deref(reference.username().unwrap()).as_deref(),
            expect["reference"][0].as_str()
        );
        assert_eq!(
            database.deref(reference.password().unwrap()).as_deref(),
            expect["reference"][1].as_str()
        );

        assert!(
            matches!(
                Database::open(&file, &key_of(case, "x")),
                Err(Error::InvalidCredentials)
            ),
            "{name}"
        );
        let mut transformed = [0u8; 32];
        transformed.copy_from_slice(&decode_hex(case["transformed_key"].as_str().unwrap()));
        assert!(
            Database::open(&file, &Key::from_transformed(transformed)).is_ok(),
            "{name}"
        );
    }
}

fn decode_hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}
