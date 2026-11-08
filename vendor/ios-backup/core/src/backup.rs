//! The reader's front door.

use std::path::{Path, PathBuf};

use crate::credentials::Credentials;
use crate::crypto::{self, ClassKeys};
use crate::error::Error;
use crate::keybag::KeyBag;
use crate::manifest::{self, BackupFile};
use crate::metadata::{BackupMetadata, Manifest};

mod stream;

/// What `Manifest.plist` **says** about encryption, and what the bytes show.
///
/// Kept as two values because they can disagree, and the disagreement is
/// evidence. `IsEncrypted` is a declaration; only `Manifest.db` itself settles
/// whether anything has to be decrypted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct EncryptionState {
    /// `Manifest.plist`'s `IsEncrypted` flag, as written.
    pub declared: bool,
    /// Whether `Manifest.db` actually had to be decrypted to be read.
    pub observed: bool,
    /// Whether `Manifest.plist` carries a `BackupKeyBag`.
    ///
    /// The tie-breaker between *encrypted* and *corrupt*: a manifest that will
    /// not parse and has no keybag is damaged, not locked.
    pub keybag_present: bool,
}

impl EncryptionState {
    /// Whether the declaration disagrees with the bytes.
    #[must_use]
    pub fn is_contradictory(&self) -> bool {
        self.declared != self.observed
    }
}

/// The first sixteen bytes of every `SQLite` database (file format section 1.3).
const SQLITE_MAGIC: &[u8] = b"SQLite format 3\0";

/// Whether `bytes` opens with the `SQLite` magic.
///
/// This is the *effective* test for "is this manifest encrypted": ciphertext
/// begins with the magic only by a coincidence of odds around 2^-128.
fn is_plaintext_sqlite(bytes: &[u8]) -> bool {
    bytes.starts_with(SQLITE_MAGIC)
}

/// An opened iOS backup.
///
/// Holds the file tree and, for an encrypted backup, the unwrapped class keys.
/// Content blobs are read and decrypted on demand — opening a backup does not
/// decrypt it, and no plaintext is ever written to disk.
pub struct Backup {
    root: PathBuf,
    metadata: BackupMetadata,
    files: Vec<BackupFile>,
    /// `Some` only for an encrypted backup that was successfully unlocked.
    class_keys: Option<ClassKeys>,
    encryption: EncryptionState,
    unreadable_rows: usize,
}

/// A backup never renders its key material.
impl core::fmt::Debug for Backup {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Backup")
            .field("root", &self.root)
            .field("files", &self.files.len())
            .field("encryption", &self.encryption)
            .field("unreadable_rows", &self.unreadable_rows)
            .field("unlocked", &self.class_keys.is_some())
            .finish_non_exhaustive()
    }
}

impl Backup {
    /// Open a backup with no credentials.
    ///
    /// # Errors
    /// [`Error::PasswordRequired`] when the backup is encrypted — the caller is
    /// told to supply a password, never handed an empty file list. Otherwise as
    /// [`Self::open_with`].
    pub fn open(path: &Path) -> Result<Self, Error> {
        Self::open_with(path, &Credentials::None)
    }

    /// Open a backup, offering `credentials`.
    ///
    /// A password supplied for an unencrypted backup is unused, not an error:
    /// a mounting tool that always passes `--password` through should not fail
    /// on the backups that do not need it.
    ///
    /// # Errors
    /// [`Error::NotABackup`] when `Manifest.plist` is absent,
    /// [`Error::PasswordRequired`] when the backup is encrypted and no password
    /// was offered, [`Error::WrongPassword`] when one was offered and no class
    /// key unwrapped, and [`Error::Sqlite`] when `Manifest.db` will not read.
    pub fn open_with(path: &Path, credentials: &Credentials) -> Result<Self, Error> {
        let manifest = Manifest::read(path)?;
        let raw_manifest_db = read_file(&path.join("Manifest.db"))?;

        // Survey the effective state rather than trusting the declaration.
        // `IsEncrypted` says what the plist claims; the SQLite magic says what
        // the bytes are, and only the second decides whether a key is needed.
        //
        // A manifest that is not plaintext SQLite is treated as encrypted only
        // when a keybag exists to decrypt it with. Without one it is damaged,
        // and reporting it as locked would send an examiner hunting for a
        // password that does not exist — the distinction a parse-probe alone
        // cannot draw.
        let keybag_present = manifest.keybag.is_some();
        let observed_encrypted = !is_plaintext_sqlite(&raw_manifest_db) && keybag_present;

        let encryption = EncryptionState {
            declared: manifest.metadata.is_encrypted,
            observed: observed_encrypted,
            keybag_present,
        };

        // Declared encrypted, unreadable as SQLite, and carrying no keys: the
        // plist is internally inconsistent, and saying so is more useful than
        // "not a database". Distinct from the same shape declared *plaintext*,
        // which is ordinary corruption.
        if manifest.metadata.is_encrypted
            && !keybag_present
            && !is_plaintext_sqlite(&raw_manifest_db)
        {
            return Err(Error::MissingKeyBag);
        }

        let (class_keys, manifest_bytes) = if observed_encrypted {
            Self::unlock(&manifest, credentials, &raw_manifest_db)?
        } else {
            (None, raw_manifest_db)
        };

        let read = manifest::read_files(manifest_bytes)?;

        Ok(Self {
            root: path.to_path_buf(),
            metadata: manifest.metadata,
            files: read.files,
            class_keys,
            encryption,
            unreadable_rows: read.unreadable,
        })
    }

    /// What the plist declared about encryption, and what the bytes showed.
    ///
    /// [`EncryptionState::is_contradictory`] is the one worth reporting: a
    /// backup whose declaration disagrees with its own data.
    #[must_use]
    pub fn encryption_state(&self) -> &EncryptionState {
        &self.encryption
    }

    /// Derive the class keys and decrypt `Manifest.db` into memory.
    ///
    /// The decrypted manifest is returned as bytes and never written to disk:
    /// a temporary file would be a plaintext copy of the evidence with a
    /// lifetime nobody is tracking.
    fn unlock(
        manifest: &Manifest,
        credentials: &Credentials,
        ciphertext: &[u8],
    ) -> Result<(Option<ClassKeys>, Vec<u8>), Error> {
        let Some(password) = credentials.as_password() else {
            return Err(Error::PasswordRequired);
        };
        let keybag_bytes = manifest.keybag.as_deref().ok_or(Error::MissingKeyBag)?;
        let keybag = KeyBag::parse(keybag_bytes)?;

        let backup_key = crypto::derive_backup_key(password.as_bytes(), &keybag);
        let class_keys = crypto::unwrap_class_keys(&keybag, &backup_key)?;

        let manifest_key = manifest
            .manifest_key
            .as_deref()
            .ok_or(Error::MissingManifestKey)?;

        // ManifestKey is a 4-byte little-endian protection class followed by
        // the wrapped key. Read the class from the blob rather than assuming
        // one: it is not the same class on every iOS version.
        let protection_class =
            safe_read::try_le_u32(manifest_key, 0).ok_or(Error::MissingManifestKey)?;
        let key = crypto::unwrap_file_key(&class_keys, protection_class, manifest_key)?;

        let plaintext = crypto::decrypt_aes_cbc_zero_iv(&key, ciphertext)?;

        Ok((Some(class_keys), plaintext))
    }

    /// Whether this backup's data actually had to be decrypted.
    ///
    /// Reports the **bytes**, not `Manifest.plist`'s declaration: the question a
    /// caller is really asking is "must this be decrypted", and only the data
    /// answers it. Use [`Self::encryption_state`] when the declaration itself
    /// matters.
    #[must_use]
    pub fn is_encrypted(&self) -> bool {
        self.encryption.observed
    }

    /// The backup's own directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Device and backup metadata.
    #[must_use]
    pub fn metadata(&self) -> &BackupMetadata {
        &self.metadata
    }

    /// How many `Files` rows could not be identified and are therefore absent
    /// from [`Self::files`].
    ///
    /// Non-zero means the tree is **smaller than the manifest it came from**.
    /// The count exists so that shortfall is never silent: a reader returning
    /// only the rows it could read, without saying how many it could not, has
    /// quietly redefined the evidence.
    #[must_use]
    pub fn unreadable_rows(&self) -> usize {
        self.unreadable_rows
    }

    /// Every entry in the manifest, in the order the manifest records them.
    #[must_use]
    pub fn files(&self) -> &[BackupFile] {
        &self.files
    }

    /// The entry for `domain` and `relative_path`, or `None`.
    #[must_use]
    pub fn find(&self, domain: &str, relative_path: &str) -> Option<&BackupFile> {
        self.files
            .iter()
            .find(|f| f.domain == domain && f.relative_path == relative_path)
    }

    /// The entry with `file_id`, or `None`.
    #[must_use]
    pub fn find_by_id(&self, file_id: &str) -> Option<&BackupFile> {
        self.files.iter().find(|f| f.file_id == file_id)
    }

    /// Where `entry`'s content blob lives on disk.
    #[must_use]
    pub fn blob_path(&self, entry: &BackupFile) -> Option<PathBuf> {
        Some(self.root.join(entry.blob_relative_path()?))
    }

    /// Read `entry`'s content, decrypting it when the backup is encrypted.
    ///
    /// The result is truncated to the length the manifest records. That length
    /// is evidence; the padding on an encrypted blob is not, and guessing at it
    /// by stripping PKCS#7 would corrupt any file whose final plaintext byte
    /// happens to look like a pad.
    ///
    /// # Errors
    /// [`Error::NotAFile`] for a directory or symlink entry,
    /// [`Error::BlobMissing`] when the manifest names a blob that is not on
    /// disk, [`Error::NotUnlocked`] if the backup was never unlocked, and
    /// [`Error::KeyUnwrapFailed`] if the per-file key will not unwrap.
    pub fn read(&mut self, entry: &BackupFile) -> Result<Vec<u8>, Error> {
        if !entry.kind.has_content() {
            return Err(Error::NotAFile {
                path: format!("{}-{}", entry.domain, entry.relative_path),
                kind: format!("{:?}", entry.kind),
            });
        }
        // A fileID that is not a SHA-1 digest names no blob. Reported as its
        // own error rather than as a missing blob: "not present" and "refused
        // to resolve" are different facts, and an examiner needs the second one
        // to know a backup tried to walk the reader out of its own directory.
        let path = self
            .blob_path(entry)
            .ok_or_else(|| Error::UnsafeFileId(entry.file_id.clone()))?;

        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::BlobMissing(entry.file_id.clone()));
            }
            Err(source) => {
                return Err(Error::Io {
                    path: path.display().to_string(),
                    source,
                })
            }
        };

        let plaintext = if self.encryption.observed {
            let class_keys = self.class_keys.as_ref().ok_or(Error::NotUnlocked)?;
            let wrapped = entry
                .encryption_key
                .as_deref()
                .ok_or_else(|| Error::NoFileKey(entry.file_id.clone()))?;
            let protection_class = entry
                .protection_class
                .ok_or_else(|| Error::NoFileKey(entry.file_id.clone()))?;

            let key = crypto::unwrap_file_key(class_keys, protection_class, wrapped)?;
            crypto::decrypt_aes_cbc_zero_iv(&key, &bytes)?
        } else {
            bytes
        };

        Ok(truncate_to_manifest_size(
            plaintext,
            entry.size,
            self.encryption.observed,
        ))
    }
}

/// Cut a decrypted blob back to the length the manifest records.
///
/// A blob shorter than its recorded size is left as it is: the shortfall is
/// evidence of a truncated backup, and padding it out would manufacture bytes
/// that were never captured.
///
/// When the manifest records **no** size, the recorded length cannot be used and
/// the PKCS#7 trailer is stripped instead — the fallback the reference
/// implementations use as their only strategy. Truncating to zero because
/// nothing was recorded would be refusal counted as zero.
fn truncate_to_manifest_size(mut bytes: Vec<u8>, size: Option<u64>, encrypted: bool) -> Vec<u8> {
    let Some(size) = size else {
        return if encrypted { strip_pkcs7(bytes) } else { bytes };
    };
    if let Ok(size) = usize::try_from(size) {
        if bytes.len() > size {
            bytes.truncate(size);
        }
    }
    bytes
}

/// Remove a PKCS#7 trailer, or leave the bytes untouched when it is not valid.
///
/// Only reached when the manifest recorded no size. A malformed trailer returns
/// the block intact rather than erroring: with no recorded length there is
/// nothing to check it against, and handing back slightly too many real bytes
/// beats discarding a file an examiner needs.
fn strip_pkcs7(mut bytes: Vec<u8>) -> Vec<u8> {
    let Some(&pad) = bytes.last() else {
        return bytes;
    };
    let pad = pad as usize;
    if pad == 0 || pad > 16 || pad > bytes.len() {
        return bytes;
    }
    // Every padding byte must equal the pad length, or this is not PKCS#7.
    if bytes[bytes.len() - pad..]
        .iter()
        .all(|&b| b as usize == pad)
    {
        bytes.truncate(bytes.len() - pad);
    }
    bytes
}

/// Read a file, reporting its path on failure.
fn read_file(path: &Path) -> Result<Vec<u8>, Error> {
    std::fs::read(path).map_err(|source| Error::Io {
        path: path.display().to_string(),
        source,
    })
}
