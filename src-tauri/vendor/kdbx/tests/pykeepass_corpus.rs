//! The KeePass 2.x and KeePassXC databases from pykeepass's test suite (see
//! `tests/data/pykeepass/README.md`), read and checked against the facts
//! pykeepass's own tests assert about them.

use kdbx::{
    Argon2Variant, AttachmentSource, Cipher, Compression, Database, Entry, Error, Group, Header,
    HeaderHash, InnerStreamAlgorithm, Kdf, Key, KeyfileKind, Uuid, Version,
};

fn data(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/data/pykeepass/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn with_keyfile(password: Option<&str>, keyfile: &str) -> Key {
    let key = match password {
        Some(password) => Key::new().with_password(password),
        None => Key::new(),
    };
    key.with_keyfile(&data(keyfile)).unwrap()
}

fn password(password: &str) -> Key {
    Key::new().with_password(password)
}

fn open(name: &str, key: &Key) -> Database {
    Database::open(&data(name), key).unwrap_or_else(|error| panic!("{name}: {error}"))
}

fn uuid(hex: &str) -> Uuid {
    let digits: String = hex.chars().filter(|c| *c != '-').collect();
    Uuid(std::array::from_fn(|index| {
        u8::from_str_radix(&digits[2 * index..2 * index + 2], 16).unwrap()
    }))
}

const TEST3_TRANSFORMED: [u8; 32] = *b"\xfb\xb1!\x0e0\x94\xd4\x868\xa5\x04\xe6T\x9b<\xf9+\xb8\x82EN\xbc\xbe\xbc\xc8\xd3\xbbf\xfb\xde\xff.";
const TEST4_TRANSFORMED: [u8; 32] = *b"\x95\x0be\x9ca\x9e<\xe0\x07\x02\x7f\xc3\xd8\xa1\xa6&\x985\x8f!\xa6\x18k\x13\xa2\xd2\r=\xf3\xebd\xc5";

/// pykeepass `test_open_save`: every database opens and names its cipher,
/// KDF and version.
#[test]
fn every_database_opens_with_its_cipher_kdf_and_version() {
    let kdbx31 = Version { major: 3, minor: 1 };
    let kdbx40 = Version { major: 4, minor: 0 };
    let cases: Vec<(&str, Key, Cipher, &str, Version, Compression)> = vec![
        (
            "test3.kdbx",
            with_keyfile(Some("password"), "test3.key"),
            Cipher::Aes256,
            "AES-KDF",
            kdbx31,
            Compression::Gzip,
        ),
        (
            "test4_aes.kdbx",
            with_keyfile(Some("password"), "test4.key"),
            Cipher::Aes256,
            "Argon2d",
            kdbx40,
            Compression::Gzip,
        ),
        (
            "test4_aeskdf.kdbx",
            with_keyfile(Some("password"), "test4.key"),
            Cipher::Aes256,
            "AES-KDF",
            kdbx40,
            Compression::Gzip,
        ),
        (
            "test4_chacha20.kdbx",
            with_keyfile(Some("password"), "test4.key"),
            Cipher::ChaCha20,
            "Argon2d",
            kdbx40,
            Compression::Gzip,
        ),
        (
            "test4_twofish.kdbx",
            with_keyfile(Some("password"), "test4.key"),
            Cipher::Twofish,
            "Argon2d",
            kdbx40,
            Compression::Gzip,
        ),
        (
            "test4_hex.kdbx",
            with_keyfile(Some("password"), "test4_hex.key"),
            Cipher::ChaCha20,
            "Argon2d",
            kdbx40,
            Compression::Gzip,
        ),
        (
            "test3_transformed.kdbx",
            Key::from_transformed(TEST3_TRANSFORMED),
            Cipher::Aes256,
            "AES-KDF",
            kdbx31,
            Compression::Gzip,
        ),
        (
            "test4_transformed.kdbx",
            Key::from_transformed(TEST4_TRANSFORMED),
            Cipher::ChaCha20,
            "Argon2d",
            kdbx40,
            Compression::Gzip,
        ),
        (
            "test4_aes_uncompressed.kdbx",
            password("password"),
            Cipher::Aes256,
            "Argon2d",
            kdbx40,
            Compression::None,
        ),
        (
            "test4_twofish_uncompressed.kdbx",
            password("password"),
            Cipher::Twofish,
            "Argon2d",
            kdbx40,
            Compression::None,
        ),
        (
            "test4_chacha20_uncompressed.kdbx",
            password("password"),
            Cipher::ChaCha20,
            "Argon2d",
            kdbx40,
            Compression::None,
        ),
        (
            "test4_argon2id.kdbx",
            password("password"),
            Cipher::Aes256,
            "Argon2id",
            kdbx40,
            Compression::Gzip,
        ),
        (
            "test4.kdbx",
            with_keyfile(Some("password"), "test4.key"),
            Cipher::ChaCha20,
            "Argon2d",
            kdbx40,
            Compression::Gzip,
        ),
        (
            "test4_blankpass.kdbx",
            with_keyfile(Some(""), "test4.key"),
            Cipher::Aes256,
            "Argon2d",
            kdbx40,
            Compression::Gzip,
        ),
        (
            "test4_keyx.kdbx",
            with_keyfile(Some("password"), "test4_keyx.keyx"),
            Cipher::ChaCha20,
            "Argon2d",
            kdbx40,
            Compression::Gzip,
        ),
        (
            "extra_content.kdbx",
            password("password"),
            Cipher::Aes256,
            "Argon2d",
            kdbx40,
            Compression::None,
        ),
    ];
    for (name, key, cipher, kdf, version, compression) in cases {
        let database = open(name, &key);
        let header = &database.header;
        assert_eq!(
            (
                header.cipher,
                header.kdf.name(),
                header.version,
                header.compression
            ),
            (cipher, kdf, version, compression),
            "{name}"
        );
        let expected_stream = if version.is_kdbx4() {
            InnerStreamAlgorithm::ChaCha20
        } else {
            InnerStreamAlgorithm::Salsa20
        };
        assert_eq!(database.inner_stream, expected_stream, "{name}");
        assert_eq!(database.header_hash, HeaderHash::Matches, "{name}");
        assert!(
            database.warnings.is_empty(),
            "{name}: {:?}",
            database.warnings
        );
        assert!(header.warnings.is_empty(), "{name}: {:?}", header.warnings);
        // The transformed key opens the database again without the password.
        let again = Database::open(
            &data(name),
            &Key::from_transformed(*database.transformed_key()),
        )
        .unwrap_or_else(|error| panic!("{name} by transformed key: {error}"));
        assert_eq!(again.entries().len(), database.entries().len(), "{name}");
    }
}

#[test]
fn transformed_keys_match_pykeepass() {
    let test3 = open(
        "test3_transformed.kdbx",
        &Key::from_transformed(TEST3_TRANSFORMED),
    );
    assert_eq!(*test3.transformed_key(), TEST3_TRANSFORMED);
    let test4 = open(
        "test4_transformed.kdbx",
        &Key::from_transformed(TEST4_TRANSFORMED),
    );
    assert_eq!(*test4.transformed_key(), TEST4_TRANSFORMED);
    assert_eq!(test4.database_salt().len(), 32);
}

fn titled<'a>(database: &'a Database, title: &str) -> Vec<&'a Entry> {
    database.find_entries(|entry| entry.title() == Some(title))
}

fn subtree_entries(group: &Group) -> Vec<&Entry> {
    let mut out: Vec<&Entry> = group.entries.iter().collect();
    for child in &group.groups {
        out.extend(subtree_entries(child));
    }
    out
}

fn subtree_groups(group: &Group) -> Vec<&Group> {
    let mut out = Vec::new();
    for child in &group.groups {
        out.push(child);
        out.extend(subtree_groups(child));
    }
    out
}

/// pykeepass `EntryFindTests`, `GroupFindTests`, `EntryTests` and
/// `AttachmentTests`, read-only parts, on the KDBX 3.1 and 4.0 databases.
#[test]
fn contents_match_pykeepass_tests() {
    for (name, keyfile) in [("test3.kdbx", "test3.key"), ("test4.kdbx", "test4.key")] {
        let database = open(name, &with_keyfile(Some("password"), keyfile));
        let entries = database.entries();

        assert_eq!(titled(&database, "root_entry").len(), 1, "{name}");
        let count = |predicate: &dyn Fn(&Entry) -> bool| {
            entries.iter().filter(|entry| predicate(entry)).count()
        };
        assert_eq!(
            count(&|entry| entry.username() == Some("foobar_user")),
            2,
            "{name}"
        );
        assert_eq!(
            count(&|entry| entry.password() == Some("passw0rd")),
            2,
            "{name}"
        );
        assert_eq!(
            count(&|entry| entry.url() == Some("http://example.com")),
            2,
            "{name}"
        );
        assert_eq!(
            count(&|entry| entry.notes() == Some("entry notes")),
            1,
            "{name}"
        );
        assert_eq!(
            count(&|entry| entry
                .notes()
                .is_some_and(|notes| notes.contains("entry notes"))),
            2,
            "{name}"
        );

        let group_entry = titled(&database, "group_entry")[0];
        assert_eq!(
            database.parent_path(group_entry.uuid.as_ref().unwrap()),
            Some(vec!["foobar_group"]),
            "{name}"
        );
        let by_uuid = database
            .entry_by_uuid(&uuid("cc5f7ecd-2a00-48ca-9621-c222a347b0bb"))
            .unwrap();
        assert_eq!(by_uuid.username(), Some("foobar_user"), "{name}");

        // pykeepass matches tags by substring, so test3's "tag1tag2" counts.
        let tagged = database.find_entries(|entry| {
            let tags = entry.tags.join(";");
            tags.contains("tag1") && tags.contains("tag2")
        });
        assert_eq!(tagged[0].title(), Some("foobar_entry"), "{name}");

        let custom = database
            .find_entries(|entry| entry.value("custom_field") == Some("custom field value"));
        assert_eq!(
            custom[0].uuid,
            Some(uuid("1e73786c-7495-8c4c-9b3d-ff273aad5b54")),
            "{name}"
        );

        let sequence = database.find_entries(|entry| {
            entry
                .auto_type
                .as_ref()
                .and_then(|auto| auto.default_sequence.as_deref())
                .is_some_and(|sequence| sequence.contains("{TAB}"))
        });
        assert_eq!(sequence.len(), 1, "{name}");
        assert_eq!(
            sequence[0]
                .auto_type
                .as_ref()
                .unwrap()
                .default_sequence
                .as_deref(),
            Some("{USERNAME}{TAB}{PASSWORD}{ENTER}")
        );
        let window = database.find_entries(|entry| {
            entry.auto_type.as_ref().is_some_and(|auto| {
                auto.associations
                    .iter()
                    .any(|association| association.window.to_lowercase().contains("test"))
            })
        });
        assert_eq!(window.len(), 1, "{name}");
        assert_eq!(
            window[0].auto_type.as_ref().unwrap().associations[0].window,
            "TEST"
        );
        assert_eq!(
            count(&|entry| entry.auto_type.as_ref().and_then(|auto| auto.enabled) == Some(true)),
            entries.len() - 1,
            "{name}"
        );

        let otp = database
            .find_entries(|entry| entry.otp().is_some_and(|otp| otp.contains("OTPSECRETT")));
        assert_eq!(otp.len(), 1, "{name}");
        assert_eq!(otp[0].title(), Some("foobar_entry"));
        assert!(otp[0].field("otp").unwrap().protected);
        assert_eq!(otp[0].value("multiline"), Some("hello\nworld"));

        assert_eq!(titled(&database, "foobar_entry").len(), 3, "{name}");
        let foobar_group = database
            .group_by_uuid(&uuid("95155a32-5317-a10f-d4e4-d0c2030264b6"))
            .unwrap();
        assert_eq!(foobar_group.name, "foobar_group");
        assert_eq!(foobar_group.notes.as_deref(), Some("group notes"));
        assert_eq!(
            subtree_entries(foobar_group)
                .iter()
                .filter(|entry| entry.title() == Some("foobar_entry"))
                .count(),
            2,
            "{name}"
        );
        let named = |group: &Group| group.name.contains("group");
        assert_eq!(
            subtree_groups(foobar_group)
                .into_iter()
                .filter(|group| named(group))
                .count(),
            2,
            "{name}"
        );
        assert_eq!(
            foobar_group
                .groups
                .iter()
                .filter(|group| named(group))
                .count(),
            1,
            "{name}"
        );
        assert_eq!(
            database.find_groups(|group| group.name == "subgroup").len(),
            1
        );
        assert_eq!(database.groups().len(), 7, "{name}");

        assert!(titled(&database, "subentry2")[0].history.is_empty());
        let subentry = titled(&database, "subentry")[0];
        assert_eq!(subentry.history.len(), 4, "{name}");
        let mut modified: Vec<_> = subentry
            .history
            .iter()
            .map(|version| version.times.last_modification.clone())
            .collect();
        modified.dedup();
        assert_eq!(modified.len(), 4, "{name}");

        // Field references.
        let original = titled(&database, "foobar_entry")[0];
        let clone2 = titled(&database, "foobar_entry - Clone of clone")[0];
        let prefixed = titled(&database, "foobar_entry - Clone with prefix and suffix")[0];
        assert_eq!(
            database.deref(clone2.username().unwrap()).as_deref(),
            original.username()
        );
        assert_eq!(
            database.deref(clone2.password().unwrap()).as_deref(),
            original.password()
        );
        assert_eq!(
            database.deref(prefixed.username().unwrap()),
            Some(format!("domain\\{}2", original.username().unwrap()))
        );
        assert_eq!(
            database.deref(prefixed.password().unwrap()),
            Some(format!("A{}BC", original.password().unwrap()))
        );
        assert_eq!(
            database.deref("{REF:U@I:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA}"),
            None
        );

        assert!(database.recycle_bin().is_none(), "{name}");
        let attachment = entries
            .iter()
            .flat_map(|entry| entry.attachments.iter())
            .find(|attachment| attachment.name == "foo.txt")
            .unwrap();
        assert_eq!(attachment.source, AttachmentSource::Pool(0));
        assert_eq!(database.attachment_data(attachment), Some(&[][..]));

        // pykeepass `test_dump_xml`: protected values come out in plaintext.
        let xml = database.xml();
        assert!(xml.starts_with(
            "<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?>\n<KeePassFile>"
        ));
        assert!(
            xml.contains("<Value Protected=\"True\">passw0rd</Value>"),
            "{name}"
        );
    }
}

#[test]
fn reads_headers_without_a_key() {
    let test3 = Header::parse(&data("test3.kdbx")).unwrap();
    assert_eq!(
        (test3.version, test3.cipher),
        (Version { major: 3, minor: 1 }, Cipher::Aes256)
    );
    assert_eq!(test3.sha256_matches(), None);
    let test4 = Header::parse(&data("test4.kdbx")).unwrap();
    assert_eq!(
        (test4.version, test4.cipher),
        (Version { major: 4, minor: 0 }, Cipher::ChaCha20)
    );
    assert_eq!(test4.sha256_matches(), Some(true));
    let Kdf::Argon2(argon2) = &test4.kdf else {
        panic!("{:?}", test4.kdf)
    };
    assert_eq!(
        (
            argon2.variant,
            argon2.memory,
            argon2.iterations,
            argon2.parallelism,
            argon2.version
        ),
        (Argon2Variant::D, 64 << 20, 1, 4, 0x13)
    );
    assert!(test4.kdf_parameters.is_some());
}

/// pykeepass `test_open_error`, plus the empty-password and keyfile rules.
#[test]
fn wrong_credentials_and_other_files_are_refused() {
    let refused = |name: &str, key: Key| Database::open(&data(name), &key).unwrap_err();
    assert!(matches!(
        refused("test3.kdbx", with_keyfile(Some("invalid"), "test3.key")),
        Error::InvalidCredentials
    ));
    assert!(matches!(
        refused("test3.kdbx", with_keyfile(Some("password"), "test4.key")),
        Error::InvalidCredentials
    ));
    assert!(matches!(
        refused("test4.kdbx", with_keyfile(Some("invalid"), "test4.key")),
        Error::InvalidCredentials
    ));
    assert!(matches!(
        refused("test4.kdbx", with_keyfile(Some("password"), "test3.key")),
        Error::InvalidCredentials
    ));
    assert!(matches!(
        refused("test4_keyx.kdbx", with_keyfile(None, "test4_keyx.keyx")),
        Error::InvalidCredentials
    ));
    // An empty password is a key component of its own.
    assert!(matches!(
        refused("test4_blankpass.kdbx", with_keyfile(None, "test4.key")),
        Error::InvalidCredentials
    ));
    // KeePass refuses an XML keyfile of an unknown version (pykeepass hashes
    // it instead, which fails later as invalid credentials).
    assert!(matches!(
        Key::new().with_keyfile(&data("test_invalidversion.key")),
        Err(Error::Keyfile(_))
    ));
    assert!(matches!(
        Database::open(&data("test3.key"), &password("password")),
        Err(Error::NotKeePass)
    ));

    let kinds = [
        ("test3.key", KeyfileKind::XmlV1),
        ("test4.key", KeyfileKind::Hashed),
        ("test4_hex.key", KeyfileKind::Hex64),
        ("test4_keyx.keyx", KeyfileKind::XmlV2),
    ];
    for (name, kind) in kinds {
        assert_eq!(
            kdbx::Keyfile::parse(&data(name)).unwrap().kind,
            kind,
            "{name}"
        );
    }
}

#[test]
fn tampering_is_detected() {
    let key4 = with_keyfile(Some("password"), "test4.key");
    let original = data("test4.kdbx");
    let header = Header::parse(&original).unwrap();

    let mut seed = original.clone();
    let master_seed = header.fields.iter().find(|field| field.id == 4).unwrap();
    seed[master_seed.offset + 5] ^= 1;
    assert!(matches!(
        Database::open(&seed, &key4),
        Err(Error::HeaderChecksum)
    ));

    let mut payload = original.clone();
    let middle = header.payload_offset() + (original.len() - header.payload_offset()) / 2;
    payload[middle] ^= 1;
    assert!(matches!(
        Database::open(&payload, &key4),
        Err(Error::BlockChecksum { .. })
    ));

    let truncated = &original[..original.len() - 40];
    assert!(Database::open(truncated, &key4).is_err());

    // KDBX 3: the end-of-header bytes are not used to decrypt, so changing
    // them leaves the payload readable and only Meta/HeaderHash notices.
    let key3 = with_keyfile(Some("password"), "test3.key");
    let mut end = data("test3.kdbx");
    let header3 = Header::parse(&end).unwrap();
    let end_field = header3.fields.last().unwrap();
    end[end_field.offset + 3] ^= 0x20;
    let database = Database::open(&end, &key3).unwrap();
    assert_eq!(database.header_hash, HeaderHash::Mismatch);
    assert_eq!(
        database.warnings,
        ["Meta/HeaderHash does not match the header"]
    );
}
