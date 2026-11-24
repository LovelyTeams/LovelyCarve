//! Read-only KeePass KDBX reader, a Rust rewrite of the reading half of
//! [pykeepass](https://github.com/libkeepass/pykeepass).
//!
//! - [`Header::parse`] reads the outer header without a key: version, cipher,
//!   compression, seeds and the key derivation parameters.
//! - [`Database::open`] derives the key (AES-KDF, Argon2d, Argon2id) from a
//!   password and/or keyfile, or takes a transformed key; decrypts the payload
//!   (AES-256, Twofish, ChaCha20), checks every block (SHA-256 blocks in
//!   KDBX 3, HMAC-SHA-256 blocks in KDBX 4), frees protected values from the
//!   inner stream (Salsa20, ChaCha20) and reads the XML into groups, entries,
//!   history, attachments, metadata and deleted objects.
//!
//! Nothing is written: there is no save, and no seed is rotated.

mod blocks;
mod body;
mod bytes;
mod crypto;
mod database;
mod error;
mod header;
mod kdf;
mod key;
mod model;
mod parse;
mod salsa20;
mod time;
mod variant;
mod xml;

pub use database::{Database, HeaderHash};
pub use error::{Error, Result};
pub use header::{
    Cipher, Compression, Header, HeaderField, InnerStreamAlgorithm, Version, SIGNATURE_1,
    SIGNATURE_2, SIGNATURE_2_KEEPASS1, SIGNATURE_2_PRE_RELEASE,
};
pub use kdf::{Argon2Parameters, Argon2Variant, Kdf};
pub use key::{Key, Keyfile, KeyfileKind};
pub use model::{
    Attachment, AttachmentSource, AutoType, AutoTypeAssociation, Binary, CustomDataItem,
    CustomIcon, DeletedObject, Entry, Field, Group, MemoryProtection, Meta, Times, Uuid,
};
pub use time::Timestamp;
pub use variant::{Variant, VariantDictionary};

/// Bounds on the work and memory one database may demand. A header chooses
/// its own key derivation cost, so a hostile file could otherwise ask for
/// hours of work or more memory than the host has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Most AES-KDF rounds.
    pub max_aes_kdf_rounds: u64,
    /// Most Argon2 memory, in bytes.
    pub max_argon2_memory: u64,
    /// Most Argon2 memory × iterations, in bytes.
    pub max_argon2_work: u64,
    /// Largest decrypted and decompressed payload, and largest attachment.
    pub max_payload: usize,
    /// Deepest XML nesting.
    pub max_xml_depth: usize,
    /// Most XML elements.
    pub max_xml_elements: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_aes_kdf_rounds: 1 << 32,
            max_argon2_memory: 4 << 30,
            max_argon2_work: 256 << 30,
            max_payload: 1 << 30,
            max_xml_depth: 256,
            max_xml_elements: 4_000_000,
        }
    }
}
