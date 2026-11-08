//! Keybag parsing — the `BackupKeyBag` blob carried in `Manifest.plist`.
//!
//! Fixtures here are **synthetic** (T3): the keybag is assembled by
//! [`KeyBagBuilder`] from the documented TLV grammar, so these tests prove the
//! parser agrees with our reading of the format, not that the format reading is
//! itself right. The independent checks live elsewhere:
//!   * `tests/real_backup.rs` parses a genuine `BackupKeyBag` (env-gated, T2);
//!   * `tests/rfc_vectors.rs` pins the crypto primitives to published vectors (T1).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use ios_backup_core::keybag::{KeyBag, KeyBagKind};

/// Assemble a keybag from `(tag, value)` pairs in the documented TLV encoding:
/// 4-byte ASCII tag, 4-byte big-endian length, then that many bytes of value.
#[derive(Default)]
struct KeyBagBuilder {
    bytes: Vec<u8>,
}

impl KeyBagBuilder {
    fn tlv(mut self, tag: &str, value: &[u8]) -> Self {
        self.bytes.extend_from_slice(tag.as_bytes());
        self.bytes
            .extend_from_slice(&u32::try_from(value.len()).unwrap().to_be_bytes());
        self.bytes.extend_from_slice(value);
        self
    }

    fn u32(self, tag: &str, value: u32) -> Self {
        self.tlv(tag, &value.to_be_bytes())
    }

    fn build(self) -> Vec<u8> {
        self.bytes
    }
}

/// A keybag header followed by two class blocks, the shape every backup keybag
/// has. Values are arbitrary but distinct so a field mix-up is visible.
fn two_class_keybag() -> Vec<u8> {
    KeyBagBuilder::default()
        .u32("VERS", 3)
        .u32("TYPE", 1) // 1 = Backup
        .tlv("UUID", &[0xAA; 16])
        .tlv("HMCK", &[0xBB; 40])
        .u32("WRAP", 1)
        .tlv("SALT", &[0xCC; 20])
        .u32("ITER", 10_000)
        .tlv("DPSL", &[0xDD; 20])
        .u32("DPIC", 5_000)
        // class block 1
        .tlv("UUID", &[0x11; 16])
        .u32("CLAS", 1)
        .u32("WRAP", 2)
        .u32("KTYP", 0)
        .tlv("WPKY", &[0x01; 40])
        // class block 2
        .tlv("UUID", &[0x22; 16])
        .u32("CLAS", 3)
        .u32("WRAP", 3)
        .u32("KTYP", 0)
        .tlv("WPKY", &[0x02; 40])
        .build()
}

#[test]
fn parses_header_fields() {
    let kb = KeyBag::parse(&two_class_keybag()).expect("well-formed keybag must parse");

    assert_eq!(kb.version, 3);
    assert_eq!(kb.kind, KeyBagKind::Backup);
    assert_eq!(kb.uuid, [0xAA; 16]);
    assert_eq!(kb.salt, vec![0xCC; 20]);
    assert_eq!(kb.iterations, 10_000);
    assert_eq!(kb.double_protection_salt.as_deref(), Some(&[0xDD; 20][..]));
    assert_eq!(kb.double_protection_iterations, Some(5_000));
}

#[test]
fn header_uuid_and_wrap_are_not_mistaken_for_a_class_block() {
    // The header carries its own UUID and WRAP; only a *subsequent* UUID opens a
    // class block. A parser that treats every UUID alike invents a third class.
    let kb = KeyBag::parse(&two_class_keybag()).unwrap();

    assert_eq!(kb.classes.len(), 2, "header must not become a class block");
    assert_eq!(kb.wrap, 1, "header WRAP, not the first class's WRAP");
}

#[test]
fn parses_each_class_block() {
    let kb = KeyBag::parse(&two_class_keybag()).unwrap();

    let first = &kb.classes[0];
    assert_eq!(first.protection_class, 1);
    assert_eq!(first.wrap, 2);
    assert_eq!(first.wrapped_key, vec![0x01; 40]);

    let second = &kb.classes[1];
    assert_eq!(second.protection_class, 3);
    assert_eq!(second.wrapped_key, vec![0x02; 40]);
}

#[test]
fn class_lookup_is_by_protection_class_not_position() {
    let kb = KeyBag::parse(&two_class_keybag()).unwrap();

    assert_eq!(
        kb.class(3).map(|c| c.wrapped_key.as_slice()),
        Some(&[0x02; 40][..])
    );
    assert!(kb.class(2).is_none(), "class 2 is absent from this keybag");
}

#[test]
fn keybag_without_double_protection_reports_none() {
    let bytes = KeyBagBuilder::default()
        .u32("VERS", 3)
        .u32("TYPE", 1)
        .tlv("UUID", &[0xAA; 16])
        .u32("WRAP", 1)
        .tlv("SALT", &[0xCC; 20])
        .u32("ITER", 10_000)
        .build();

    let kb = KeyBag::parse(&bytes).unwrap();

    assert_eq!(kb.double_protection_salt, None);
    assert_eq!(kb.double_protection_iterations, None);
    assert!(kb.classes.is_empty());
}

// ---- hostile input: never panic, never trust a length field (ADR-0012) ----

#[test]
fn truncated_tlv_header_is_an_error_not_a_panic() {
    // Six bytes: a tag and a half-written length.
    let err = KeyBag::parse(&[b'V', b'E', b'R', b'S', 0, 0]).unwrap_err();
    assert!(
        format!("{err}").contains("truncated"),
        "error must name the defect, got: {err}"
    );
}

#[test]
fn length_field_running_past_the_buffer_is_rejected() {
    // Declares 0xFFFF_FFFF bytes of value with none present — the classic
    // trust-the-length-field crash.
    let mut bytes = b"SALT".to_vec();
    bytes.extend_from_slice(&u32::MAX.to_be_bytes());
    let err = KeyBag::parse(&bytes).unwrap_err();
    assert!(format!("{err}").contains("truncated"), "got: {err}");
}

#[test]
fn empty_input_is_an_error_not_an_empty_keybag() {
    // Degrading to a default-constructed keybag would be "refusal counted as
    // zero" — a caller could not tell an absent keybag from an empty one.
    assert!(KeyBag::parse(&[]).is_err());
}

#[test]
fn unrecognised_keybag_type_is_surfaced_with_its_value() {
    let bytes = KeyBagBuilder::default()
        .u32("VERS", 3)
        .u32("TYPE", 99)
        .u32("ITER", 1)
        .build();

    let err = KeyBag::parse(&bytes).unwrap_err();
    assert!(
        format!("{err}").contains("99"),
        "the offending value must appear verbatim, got: {err}"
    );
}

#[test]
fn a_wrong_width_integer_field_is_rejected() {
    // ITER declared as 3 bytes. Reading it as a u32 anyway would silently
    // produce a wrong iteration count and a wrong derived key.
    let bytes = KeyBagBuilder::default()
        .u32("VERS", 3)
        .u32("TYPE", 1)
        .tlv("ITER", &[0, 0, 1])
        .build();

    assert!(KeyBag::parse(&bytes).is_err());
}
