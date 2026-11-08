//! The crate's error type.
//!
//! Every variant names the offending value or location: an error that says only
//! "unrecognised" sends the reader back to the hex editor for something the
//! parser already knew (CLAUDE.md, show-the-unrecognized-value).

/// Anything that can go wrong reading an iOS backup.
///
/// A malformed backup is always a typed error, never a panic and never a
/// silently-empty success — degrading to `Ok(empty)` when the cause is a failed
/// bootstrap is the worst bug class for a forensic reader (ADR-0012).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A TLV ran past the end of the keybag: its tag, length, or value window is
    /// not fully present. Carries the byte offset the read was attempted at.
    #[error("truncated keybag: read past end of buffer at offset {offset}")]
    TruncatedKeyBag {
        /// Offset into the keybag at which the out-of-range read was attempted.
        offset: usize,
    },

    /// A keybag `TYPE` field held a value that is not a known keybag kind.
    #[error("unrecognised keybag type {0} (known: 0 System, 1 Backup, 2 Escrow, 3 OTA)")]
    UnknownKeyBagType(u32),

    /// A fixed-width keybag field was declared with the wrong length. Reading it
    /// anyway would yield a plausible but wrong value — a wrong iteration count
    /// derives a wrong key and looks exactly like a wrong password.
    #[error("keybag field {tag} must be {expected} bytes, got {actual}")]
    BadFieldWidth {
        /// The four-character TLV tag.
        tag: &'static str,
        /// Width the format requires.
        expected: usize,
        /// Width actually declared.
        actual: usize,
    },

    /// A field the format requires was absent. Named so the caller can tell
    /// which, rather than receiving a default-constructed keybag.
    #[error("keybag is missing the required {0} field")]
    MissingField(&'static str),

    /// A key width AES does not define. Carries the offending width.
    #[error("unsupported AES key width {0} bytes (expected 16, 24 or 32)")]
    UnsupportedKeyWidth(usize),

    /// A wrapped key was not a whole number of 64-bit semiblocks, or was too
    /// short to be an RFC 3394 wrapping. Carries the offending length.
    #[error("wrapped key of {0} bytes is not a valid RFC 3394 wrapping (need a multiple of 8, at least 24)")]
    BadWrappedKeyLength(usize),

    /// The RFC 3394 integrity check failed: the unwrapped value did not carry
    /// the expected `A6A6A6A6A6A6A6A6` prefix.
    ///
    /// For a backup this almost always means the key-encrypting key is wrong,
    /// which means the **password** is wrong. It never means "here is a key
    /// that might work" — a wrapped key either verifies or it does not.
    #[error("key unwrap failed the RFC 3394 integrity check (wrong key)")]
    KeyUnwrapFailed,

    /// No class key in the keybag could be unwrapped with the derived key.
    ///
    /// Distinct from an empty result: reporting zero recovered keys as success
    /// would be refusal counted as zero, and the caller could not tell a wrong
    /// password from a keybag that carries no passcode-wrapped class.
    #[error("wrong backup password: no class key in the keybag could be unwrapped")]
    WrongPassword,

    /// A file declares a protection class the keybag has no key for.
    #[error("no class key for protection class {0}")]
    NoKeyForClass(u32),

    /// A CBC ciphertext was not a whole number of 16-byte blocks. Carries the
    /// offending length.
    #[error("ciphertext of {0} bytes is not a whole number of AES blocks")]
    BadCiphertextLength(usize),

    /// An IV that was not 16 bytes. Carries the offending length.
    #[error("IV must be 16 bytes, got {0}")]
    BadIvLength(usize),

    /// The directory holds no `Manifest.plist`, so it is not a backup.
    ///
    /// Named rather than generic: "not a backup" sends the examiner looking for
    /// a corrupt image, while "no Manifest.plist here" usually means the path
    /// points one level above or below the UDID directory.
    #[error("{path} is not an iOS backup: no {missing}")]
    NotABackup {
        /// The file whose absence settled it.
        missing: String,
        /// The directory that was examined.
        path: String,
    },

    /// A property list is present but will not parse.
    #[error("{file} is not a readable property list: {detail}")]
    BadPlist {
        /// Which plist.
        file: String,
        /// What the parser said.
        detail: String,
    },

    /// The backup is encrypted and no password was offered.
    ///
    /// Deliberately distinct from [`Self::WrongPassword`]: one means "ask the
    /// examiner", the other means "the examiner already answered, wrongly".
    #[error("this backup is encrypted — a password is required to read it")]
    PasswordRequired,

    /// `Manifest.plist` says the backup is encrypted but carries no
    /// `BackupKeyBag`, so there is nothing to derive keys from.
    #[error("backup is marked encrypted but carries no BackupKeyBag")]
    MissingKeyBag,

    /// `Manifest.plist` carries no `ManifestKey`, so `Manifest.db` cannot be
    /// decrypted even with the right password.
    #[error("backup is marked encrypted but carries no ManifestKey")]
    MissingManifestKey,

    /// A read was attempted on an encrypted backup that was never unlocked.
    #[error("backup is encrypted and has not been unlocked")]
    NotUnlocked,

    /// A file in an encrypted backup records no per-file key.
    #[error("no per-file encryption key recorded for {0}")]
    NoFileKey(String),

    /// `Manifest.db` will not read as a `SQLite` database. In an encrypted backup
    /// this is what a wrong manifest key looks like.
    #[error("Manifest.db is not a readable SQLite database: {0:?}")]
    Sqlite(sqlite_core::Error),

    /// `Manifest.db` has no `Files` table.
    #[error("Manifest.db has no Files table")]
    ManifestTableMissing,

    /// The `Files` table lacks a column this reader needs. Both the wanted
    /// column and the ones actually present are named.
    #[error("Files table has no {column} column (found: {found})")]
    ManifestSchema {
        /// The column that was required.
        column: String,
        /// The columns the table actually declares.
        found: String,
    },

    /// A `Files.file` blob is not a readable `NSKeyedArchiver` archive.
    #[error("file metadata is not a readable NSKeyedArchiver archive: {0}")]
    BadFileMetadata(String),

    /// The manifest names a content blob that is not on disk. Carries the
    /// `fileID`, which is also the blob's filename.
    #[error("content blob {0} named by the manifest is not present in the backup")]
    BlobMissing(String),

    /// A `fileID` does not have the shape of a SHA-1 digest, so it cannot name
    /// a blob and no path is built from it.
    ///
    /// `Manifest.db` is attacker-controllable and `fileID` becomes a filesystem
    /// path, so a value that is not 40 hex characters is refused outright
    /// rather than sanitised. Carries the offending value verbatim.
    #[error("fileID {0:?} is not a SHA-1 digest and cannot name a content blob")]
    UnsafeFileId(String),

    /// A read was attempted on an entry that has no content.
    #[error("{path} is a {kind}, not a file with content")]
    NotAFile {
        /// `domain-relativePath` of the entry.
        path: String,
        /// What it actually is.
        kind: String,
    },

    /// A logical-view index that is out of range.
    #[error("no entry at index {0}")]
    NoSuchEntry(usize),

    /// No controlling terminal to prompt on.
    #[error("no controlling terminal available to prompt for a password")]
    NoTerminal,

    /// An I/O failure, with the path that caused it.
    #[error("I/O error reading {path}: {source}")]
    Io {
        /// The path being read.
        path: String,
        /// The underlying failure.
        #[source]
        source: std::io::Error,
    },
}
