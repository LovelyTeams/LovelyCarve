//! The backup keybag: `Manifest.plist`'s `BackupKeyBag` blob.
//!
//! A keybag is a flat sequence of TLVs — a four-character ASCII tag, a 4-byte
//! big-endian length, then that many bytes of value. A header describes the bag
//! itself (version, kind, the PBKDF2 salt and iteration count), and is followed
//! by one block per protection class carrying that class's wrapped key.
//!
//! The one rule worth stating: `UUID` and `WRAP` appear in **both** the header
//! and each class block. Position disambiguates them — the first `UUID` is the
//! bag's own, and every subsequent `UUID` opens a new class block. A parser that
//! treats every `UUID` alike reports one class too many and mis-attributes the
//! header's `WRAP`.
//!
//! This module only *reads* the bag. Turning a password into class keys is
//! [`crate::crypto`]'s job.

use crate::error::Error;

/// What a keybag is for. Only [`KeyBagKind::Backup`] appears in a backup's
/// `Manifest.plist`; the others are recognised so an unexpected one is reported
/// as the wrong *kind* rather than as a corrupt bag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum KeyBagKind {
    /// On-device system keybag.
    System,
    /// Backup keybag — the kind carried by `Manifest.plist`.
    Backup,
    /// Escrow (pairing-record) keybag.
    Escrow,
    /// Over-the-air / iCloud backup keybag.
    Ota,
}

impl KeyBagKind {
    /// Map the on-disk `TYPE` value, or `None` for a value the format does not
    /// define — the caller surfaces it verbatim rather than guessing.
    fn from_raw(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::System),
            1 => Some(Self::Backup),
            2 => Some(Self::Escrow),
            3 => Some(Self::Ota),
            _ => None,
        }
    }
}

/// One protection class's entry: the class key, still wrapped.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClassKey {
    /// Per-class UUID, when the block carried one.
    pub uuid: Option<[u8; 16]>,
    /// The protection class this key belongs to (`NSFileProtection*`, 1..=11).
    pub protection_class: u32,
    /// How the key is wrapped, as recorded by this block's `WRAP`.
    pub wrap: u32,
    /// Key type (`KTYP`) — 0 for the AES class keys a backup uses.
    pub key_type: u32,
    /// `WPKY`: the class key wrapped under the password-derived key. Unwrapping
    /// it is [`crate::crypto::unwrap_class_keys`]'s job.
    pub wrapped_key: Vec<u8>,
    /// `PBKY`, present for the asymmetric classes.
    pub public_key: Option<Vec<u8>>,
}

/// A parsed backup keybag.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct KeyBag {
    /// Keybag format version (`VERS`).
    pub version: u32,
    /// What the bag is for (`TYPE`).
    pub kind: KeyBagKind,
    /// The bag's own UUID — the first `UUID` TLV.
    pub uuid: [u8; 16],
    /// `HMCK`, the bag's HMAC key, when present.
    pub hmac_key: Option<Vec<u8>>,
    /// The bag's own `WRAP` — the first one, before any class block.
    pub wrap: u32,
    /// `SALT` for the password PBKDF2.
    pub salt: Vec<u8>,
    /// `ITER`, the PBKDF2-HMAC-SHA1 iteration count.
    pub iterations: u32,
    /// `DPSL` — the double-protection salt, present on iOS 10.2+ backups.
    pub double_protection_salt: Option<Vec<u8>>,
    /// `DPIC` — the double-protection iteration count (PBKDF2-HMAC-SHA256).
    pub double_protection_iterations: Option<u32>,
    /// One entry per protection class, in the order the bag records them.
    pub classes: Vec<ClassKey>,
}

/// A class block under construction, before its `WPKY` has been seen.
#[derive(Default)]
struct PartialClass {
    uuid: Option<[u8; 16]>,
    protection_class: Option<u32>,
    wrap: Option<u32>,
    key_type: Option<u32>,
    wrapped_key: Option<Vec<u8>>,
    public_key: Option<Vec<u8>>,
}

impl PartialClass {
    /// Finish the block. A block with no `CLAS` is not a class key and is
    /// dropped rather than being given a fabricated class number.
    fn finish(self) -> Option<ClassKey> {
        Some(ClassKey {
            uuid: self.uuid,
            protection_class: self.protection_class?,
            wrap: self.wrap.unwrap_or_default(),
            key_type: self.key_type.unwrap_or_default(),
            wrapped_key: self.wrapped_key.unwrap_or_default(),
            public_key: self.public_key,
        })
    }
}

/// One TLV and the offset just past it.
struct Tlv<'a> {
    tag: [u8; 4],
    value: &'a [u8],
    next: usize,
}

/// Read the TLV starting at `off`. Every bound is checked: the tag window, the
/// length window, the length's addition, and the value window. A length field is
/// never trusted to be in range.
fn read_tlv(bytes: &[u8], off: usize) -> Result<Tlv<'_>, Error> {
    let tag =
        safe_read::try_bytes::<4>(bytes, off).ok_or(Error::TruncatedKeyBag { offset: off })?;
    let len_off = off
        .checked_add(4)
        .ok_or(Error::TruncatedKeyBag { offset: off })?;
    let len = safe_read::try_be_u32(bytes, len_off)
        .ok_or(Error::TruncatedKeyBag { offset: len_off })? as usize;
    let start = len_off
        .checked_add(4)
        .ok_or(Error::TruncatedKeyBag { offset: len_off })?;
    let next = start
        .checked_add(len)
        .ok_or(Error::TruncatedKeyBag { offset: start })?;
    let value = bytes
        .get(start..next)
        .ok_or(Error::TruncatedKeyBag { offset: start })?;
    Ok(Tlv { tag, value, next })
}

/// Decode a fixed-width big-endian `u32` field, rejecting a wrong declared
/// width rather than reading four bytes out of a three-byte field.
fn field_u32(tag: &'static str, value: &[u8]) -> Result<u32, Error> {
    if value.len() != 4 {
        return Err(Error::BadFieldWidth {
            tag,
            expected: 4,
            actual: value.len(),
        });
    }
    safe_read::try_be_u32(value, 0).ok_or(Error::BadFieldWidth {
        tag,
        expected: 4,
        actual: value.len(),
    })
}

/// Decode a 16-byte UUID field under the same width rule.
fn field_uuid(value: &[u8]) -> Result<[u8; 16], Error> {
    if value.len() != 16 {
        return Err(Error::BadFieldWidth {
            tag: "UUID",
            expected: 16,
            actual: value.len(),
        });
    }
    safe_read::try_bytes::<16>(value, 0).ok_or(Error::BadFieldWidth {
        tag: "UUID",
        expected: 16,
        actual: value.len(),
    })
}

impl KeyBag {
    /// Parse a `BackupKeyBag` blob.
    ///
    /// # Errors
    /// [`Error::TruncatedKeyBag`] if any TLV runs past the buffer,
    /// [`Error::BadFieldWidth`] for a wrong-width fixed field,
    /// [`Error::UnknownKeyBagType`] for an undefined `TYPE`, and
    /// [`Error::MissingField`] when a field the format requires is absent —
    /// never a default-constructed bag.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let mut version = None;
        let mut kind = None;
        let mut uuid = None;
        let mut hmac_key = None;
        let mut wrap = None;
        let mut salt = None;
        let mut iterations = None;
        let mut dp_salt = None;
        let mut dp_iterations = None;

        let mut classes = Vec::new();
        let mut current: Option<PartialClass> = None;

        let mut off = 0usize;
        while off < bytes.len() {
            let tlv = read_tlv(bytes, off)?;
            off = tlv.next;

            match &tlv.tag {
                b"VERS" => version = Some(field_u32("VERS", tlv.value)?),
                b"TYPE" => {
                    let raw = field_u32("TYPE", tlv.value)?;
                    kind = Some(KeyBagKind::from_raw(raw).ok_or(Error::UnknownKeyBagType(raw))?);
                }
                b"HMCK" => hmac_key = Some(tlv.value.to_vec()),
                b"SALT" => salt = Some(tlv.value.to_vec()),
                b"ITER" => iterations = Some(field_u32("ITER", tlv.value)?),
                b"DPSL" => dp_salt = Some(tlv.value.to_vec()),
                b"DPIC" => dp_iterations = Some(field_u32("DPIC", tlv.value)?),

                // The header's UUID is the first one; every later UUID opens a
                // class block, so the preceding block (if any) is complete.
                b"UUID" => {
                    let value = field_uuid(tlv.value)?;
                    if uuid.is_none() {
                        uuid = Some(value);
                    } else {
                        classes.extend(current.take().and_then(PartialClass::finish));
                        current = Some(PartialClass {
                            uuid: Some(value),
                            ..PartialClass::default()
                        });
                    }
                }

                // Inside a class block this is the class's wrap; before the first
                // block it is the bag's own.
                b"WRAP" => {
                    let value = field_u32("WRAP", tlv.value)?;
                    match current.as_mut() {
                        Some(class) => class.wrap = Some(value),
                        None => wrap = Some(value),
                    }
                }

                // Class-only tags. Seeing one with no block open means the bag
                // orders its blocks differently than the common UUID-first
                // layout; open a block rather than discarding the key.
                b"CLAS" => {
                    current
                        .get_or_insert_with(PartialClass::default)
                        .protection_class = Some(field_u32("CLAS", tlv.value)?);
                }
                b"KTYP" => {
                    current.get_or_insert_with(PartialClass::default).key_type =
                        Some(field_u32("KTYP", tlv.value)?);
                }
                b"WPKY" => {
                    current
                        .get_or_insert_with(PartialClass::default)
                        .wrapped_key = Some(tlv.value.to_vec());
                }
                b"PBKY" => {
                    current.get_or_insert_with(PartialClass::default).public_key =
                        Some(tlv.value.to_vec());
                }

                // An unknown tag is skipped, not fatal: the keybag grammar is
                // extensible and a future iOS may add fields we do not read.
                _ => {}
            }
        }
        classes.extend(current.and_then(PartialClass::finish));

        Ok(Self {
            version: version.ok_or(Error::MissingField("VERS"))?,
            kind: kind.ok_or(Error::MissingField("TYPE"))?,
            uuid: uuid.ok_or(Error::MissingField("UUID"))?,
            hmac_key,
            wrap: wrap.unwrap_or_default(),
            salt: salt.ok_or(Error::MissingField("SALT"))?,
            iterations: iterations.ok_or(Error::MissingField("ITER"))?,
            double_protection_salt: dp_salt,
            double_protection_iterations: dp_iterations,
            classes,
        })
    }

    /// The entry for `protection_class`, or `None` when the bag has no key for
    /// it. Looked up by class number, never by position — a keybag does not
    /// promise its blocks are ordered or contiguous.
    #[must_use]
    pub fn class(&self, protection_class: u32) -> Option<&ClassKey> {
        self.classes
            .iter()
            .find(|c| c.protection_class == protection_class)
    }
}
