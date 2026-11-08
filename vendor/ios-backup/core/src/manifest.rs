//! `Manifest.db` — the `SQLite` index of every captured file.
//!
//! One `Files` row per file, directory and symlink:
//!
//! ```sql
//! CREATE TABLE Files (fileID TEXT PRIMARY KEY, domain TEXT,
//!                     relativePath TEXT, flags INTEGER, file BLOB)
//! ```
//!
//! `file` is an `NSKeyedArchiver` archive ([`crate::nskeyed`]) carrying the
//! size, mode, timestamps and — in an encrypted backup — the wrapped per-file
//! key and its protection class.
//!
//! Columns are located **by name**, never by position: a schema that gains a
//! column would silently shift a positional read onto the wrong field, and the
//! resulting values would be individually plausible.

use crate::error::Error;
use crate::nskeyed::Archive;

/// What a `Files` row describes. The `flags` column is the authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileKind {
    /// A regular file with a content blob on disk.
    File,
    /// A directory: metadata only, no blob.
    Directory,
    /// A symbolic link; its target is in the metadata, not in a blob.
    Symlink,
    /// A `flags` value the format does not define. Carried verbatim rather than
    /// coerced into `File`, so an unexpected backup is visible as unexpected.
    Unknown(i64),
}

impl FileKind {
    /// Map the `Files.flags` column.
    #[must_use]
    pub fn from_flags(flags: i64) -> Self {
        match flags {
            1 => Self::File,
            2 => Self::Directory,
            4 => Self::Symlink,
            other => Self::Unknown(other),
        }
    }

    /// Whether a content blob is expected for this kind.
    #[must_use]
    pub fn has_content(self) -> bool {
        matches!(self, Self::File)
    }
}

/// One entry in a backup's file tree.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BackupFile {
    /// `SHA1(domain + "-" + relativePath)`, lowercase hex. Also the blob's name
    /// on disk, under a directory named for its first two hex characters.
    pub file_id: String,
    /// The backup domain, e.g. `HomeDomain` or
    /// `AppDomainGroup-group.net.whatsapp.WhatsApp.shared`.
    pub domain: String,
    /// Path within the domain, `/`-separated, with no leading slash.
    pub relative_path: String,
    /// File, directory, symlink, or an undefined `flags` value.
    pub kind: FileKind,
    /// Length in bytes as the manifest records it, or `None` when the metadata
    /// records no `Size` at all.
    ///
    /// This is the *true* length: an encrypted blob is PKCS#7-padded out to the
    /// AES block size, and the reader truncates to this rather than guessing at
    /// padding (ADR-0003).
    ///
    /// `None` is deliberately distinct from `Some(0)`. An absent size collapsed
    /// to zero would truncate a real file to nothing and report success — and a
    /// crafted backup that omits `Size` would make a file disappear from an
    /// examiner's view. `Some(0)` is a genuinely empty file.
    pub size: Option<u64>,
    /// Protection class, when the metadata records one.
    pub protection_class: Option<u32>,
    /// The wrapped per-file key from `EncryptionKey`, still wrapped. `None` in
    /// an unencrypted backup and for entries with no content.
    pub encryption_key: Option<Vec<u8>>,
    /// POSIX mode, when recorded.
    pub mode: Option<u32>,
    /// Inode number on the source device, when recorded.
    pub inode: Option<u64>,
    /// Last-modified time, seconds since the Unix epoch.
    pub modified: Option<i64>,
    /// Creation ("birth") time, seconds since the Unix epoch.
    pub created: Option<i64>,
    /// Last status change, seconds since the Unix epoch.
    pub status_changed: Option<i64>,
}

impl BackupFile {
    /// The blob's path relative to the backup root: `<first-2-hex>/<fileID>`.
    #[must_use]
    pub fn blob_relative_path(&self) -> Option<std::path::PathBuf> {
        // `Manifest.db` is attacker-controllable and this value becomes a
        // filesystem path, so the shape is validated before any path is built.
        //
        // Validating rather than sanitising is deliberate: a `fileID` *is* a
        // SHA-1 hex digest, and hex contains no separator, no dot and no drive
        // letter. Accepting only that shape makes traversal impossible by
        // construction — there is no path to sanitise because none is built.
        //
        // The escape this closes is not only the obvious `..` climb.
        // `Path::join` REPLACES the whole path when given an absolute one, so a
        // `fileID` of `/etc/passwd` would have addressed the examiner's
        // filesystem directly rather than merely leaving the backup.
        if !is_file_id(&self.file_id) {
            return None;
        }
        let prefix = self.file_id.get(..2)?;
        Some(std::path::Path::new(prefix).join(&self.file_id))
    }
}

/// Whether `value` has the shape of a `fileID`: exactly 40 hex characters.
///
/// Case-insensitive. Refusing an uppercase digest would drop a legitimate row
/// over a case difference, which is its own way of losing evidence.
#[must_use]
pub fn is_file_id(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Locate a column by name and return its index.
///
/// By name rather than by position: `Files` is a schema this crate does not
/// control, and a positional read that lands one column over yields values that
/// are individually plausible and collectively wrong.
fn column_index(columns: &[String], name: &str) -> Result<usize, Error> {
    columns
        .iter()
        .position(|c| c.eq_ignore_ascii_case(name))
        .ok_or_else(|| Error::ManifestSchema {
            column: name.to_owned(),
            found: columns.join(", "),
        })
}

/// Read every `Files` row out of a decrypted `Manifest.db`.
///
/// # Errors
/// [`Error::Sqlite`] when the bytes are not a readable database (in an
/// encrypted backup, that usually means the manifest key was wrong),
/// [`Error::ManifestTableMissing`] when there is no `Files` table, and
/// [`Error::ManifestSchema`] when it lacks a column this reader needs.
pub fn read_files(db_bytes: Vec<u8>) -> Result<ManifestFiles, Error> {
    let db = sqlite_core::Database::open(db_bytes).map_err(Error::Sqlite)?;

    let table = db
        .live_table_rows()
        .into_iter()
        .find(|t| t.name.eq_ignore_ascii_case("Files"))
        .ok_or(Error::ManifestTableMissing)?;

    let file_id_col = column_index(&table.column_names, "fileID")?;
    let domain_col = column_index(&table.column_names, "domain")?;
    let path_col = column_index(&table.column_names, "relativePath")?;
    let flags_col = column_index(&table.column_names, "flags")?;
    let file_col = column_index(&table.column_names, "file")?;

    let mut files = Vec::with_capacity(table.rows.len());
    let mut unreadable = 0usize;

    for row in table.rows {
        let Some(file_id) = file_id_of(&row.values, file_id_col) else {
            // This row identifies nothing, so it cannot be offered as a file.
            // But it is COUNTED: a tree smaller than the manifest it came from
            // is a fact an examiner needs, and dropping the row silently would
            // make the loss invisible at the collection stage — where no
            // analyzer can flag it and nobody knows to look.
            unreadable += 1;
            continue;
        };
        let kind = FileKind::from_flags(integer(&row.values, flags_col).unwrap_or_default());

        let metadata = row
            .values
            .get(file_col)
            .and_then(|v| match v {
                sqlite_core::Value::Blob(bytes) => Some(bytes.as_slice()),
                _ => None,
            })
            .map(Archive::parse)
            .transpose()?;

        files.push(build_file(
            file_id,
            text(&row.values, domain_col).unwrap_or_default(),
            text(&row.values, path_col).unwrap_or_default(),
            kind,
            metadata.as_ref(),
        ));
    }
    Ok(ManifestFiles { files, unreadable })
}

/// What [`read_files`] recovered, and what it could not.
///
/// The second number exists so a shortfall is never silent. A reader that
/// returns only the rows it liked has quietly redefined the evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ManifestFiles {
    /// Every row that could be identified.
    pub files: Vec<BackupFile>,
    /// Rows whose `fileID` identified nothing, and which are therefore absent
    /// from `files`.
    pub unreadable: usize,
}

/// A row's `fileID`, accepting either storage class.
///
/// `Manifest.db` declares the column `TEXT`, but `SQLite` enforces **affinity,
/// not type**: a BLOB stored there is legal and round-trips unchanged. A
/// `fileID` is ASCII hex, so a BLOB holding one decodes to exactly the same
/// string — refusing it would lose a file over a storage detail that changes
/// nothing about the evidence.
///
/// Returns `None` only when the value identifies nothing at all: absent, NULL,
/// a non-UTF-8 blob, or a numeric.
fn file_id_of(values: &[sqlite_core::Value], index: usize) -> Option<String> {
    match values.get(index)? {
        sqlite_core::Value::Text(s) => Some(s.clone()),
        sqlite_core::Value::Blob(bytes) => String::from_utf8(bytes.clone()).ok(),
        _ => None,
    }
}

/// Project an archive's fields onto a [`BackupFile`].
fn build_file(
    file_id: String,
    domain: String,
    relative_path: String,
    kind: FileKind,
    metadata: Option<&Archive>,
) -> BackupFile {
    // No `unwrap_or_default()`: absent must stay absent. Collapsing it to 0
    // here is what made a size-less row read as an empty file.
    let size = metadata
        .and_then(|a| a.root_i64("Size"))
        .and_then(|s| u64::try_from(s).ok());

    // The stored EncryptionKey is a 4-byte little-endian protection class
    // followed by the 40-byte wrapped key. The class is also recorded as its
    // own field; prefer that, and fall back to the prefix.
    let encryption_key = metadata.and_then(|a| a.root_data("EncryptionKey"));
    let protection_class = metadata
        .and_then(|a| a.root_i64("ProtectionClass"))
        .and_then(|c| u32::try_from(c).ok())
        .or_else(|| {
            encryption_key
                .as_deref()
                .and_then(|k| safe_read::try_le_u32(k, 0))
        });

    BackupFile {
        file_id,
        domain,
        relative_path,
        kind,
        size,
        protection_class,
        encryption_key,
        mode: metadata
            .and_then(|a| a.root_i64("Mode"))
            .and_then(|m| u32::try_from(m).ok()),
        inode: metadata
            .and_then(|a| a.root_i64("InodeNumber"))
            .and_then(|i| u64::try_from(i).ok()),
        modified: metadata.and_then(|a| a.root_i64("LastModified")),
        created: metadata.and_then(|a| a.root_i64("Birth")),
        status_changed: metadata.and_then(|a| a.root_i64("LastStatusChange")),
    }
}

/// A `TEXT` column's value, or `None` when it is absent or another type.
fn text(values: &[sqlite_core::Value], index: usize) -> Option<String> {
    match values.get(index)? {
        sqlite_core::Value::Text(s) => Some(s.clone()),
        _ => None,
    }
}

/// An `INTEGER` column's value.
fn integer(values: &[sqlite_core::Value], index: usize) -> Option<i64> {
    match values.get(index)? {
        sqlite_core::Value::Integer(i) => Some(*i),
        _ => None,
    }
}
