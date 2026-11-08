//! Just enough `NSKeyedArchiver` to read `Files.file`.
//!
//! The per-file metadata in `Manifest.db` is not a plain property list: it is
//! an `NSKeyedArchiver` archive, a flat `$objects` array in which every
//! reference is a `UID` index into that array. Reading `Size` straight off the
//! top-level dictionary therefore finds nothing, and reading `EncryptionKey`
//! finds a *number* rather than a key.
//!
//! This is a reader for that indirection and nothing more — it resolves UIDs
//! and returns typed leaves. A general unarchiver (class hints, cycles,
//! `NS.objects` containers) is a larger job that this format does not need, and
//! building one here would be scope the backup reader has to keep correct.

use crate::error::Error;

/// A parsed `NSKeyedArchiver` archive.
///
/// `Debug` reports the shape, not the contents: a `Files.file` archive holds a
/// wrapped per-file key, and rendering the whole object graph into a log would
/// put key material there.
pub struct Archive {
    objects: Vec<plist::Value>,
    root: usize,
}

impl core::fmt::Debug for Archive {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Archive")
            .field("objects", &self.objects.len())
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Archive {
    /// Parse an archive from a `Files.file` blob.
    ///
    /// # Errors
    /// [`Error::BadFileMetadata`] when the blob is not a property list, or is
    /// one without the `$objects`/`$top` structure an archive must have. The
    /// message names what was missing rather than reporting a generic failure.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let value = plist::Value::from_reader(std::io::Cursor::new(bytes))
            .map_err(|source| Error::BadFileMetadata(source.to_string()))?;

        let dict = value
            .as_dictionary()
            .ok_or_else(|| Error::BadFileMetadata("not a dictionary".into()))?;

        let objects = dict
            .get("$objects")
            .and_then(plist::Value::as_array)
            .ok_or_else(|| Error::BadFileMetadata("no $objects array".into()))?
            .clone();

        // `$top.root` names the archive's entry point. Defaulting to index 1
        // when it is absent would usually be right and occasionally silently
        // wrong, so an archive without it is rejected.
        let root = dict
            .get("$top")
            .and_then(plist::Value::as_dictionary)
            .and_then(|top| top.get("root"))
            .and_then(plist::Value::as_uid)
            .ok_or_else(|| Error::BadFileMetadata("no $top.root".into()))?
            .get();

        let root = usize::try_from(root)
            .map_err(|_| Error::BadFileMetadata(format!("$top.root out of range: {root}")))?;

        Ok(Self { objects, root })
    }

    /// The object at `index`, or `None` when the index is out of range. A UID
    /// pointing past the end of `$objects` is a malformed archive, never a
    /// reason to panic.
    fn object(&self, index: usize) -> Option<&plist::Value> {
        self.objects.get(index)
    }

    /// Follow `value` if it is a UID reference, or return it unchanged.
    #[must_use]
    pub fn resolve<'a>(&'a self, value: &'a plist::Value) -> Option<&'a plist::Value> {
        match value {
            plist::Value::Uid(uid) => {
                let index = usize::try_from(uid.get()).ok()?;
                self.object(index)
            }
            other => Some(other),
        }
    }

    /// The archive's root object as a dictionary.
    ///
    /// # Errors
    /// [`Error::BadFileMetadata`] when the root is absent or not a dictionary.
    pub fn root(&self) -> Result<&plist::Dictionary, Error> {
        self.object(self.root)
            .and_then(plist::Value::as_dictionary)
            .ok_or_else(|| {
                Error::BadFileMetadata(format!("root object {} is not a dictionary", self.root))
            })
    }

    /// Resolve `key` on the root object to an integer.
    #[must_use]
    pub fn root_i64(&self, key: &str) -> Option<i64> {
        let root = self.root().ok()?;
        self.resolve(root.get(key)?)?.as_signed_integer()
    }

    /// Resolve `key` on the root object to a string.
    #[must_use]
    pub fn root_string(&self, key: &str) -> Option<String> {
        let root = self.root().ok()?;
        self.resolve(root.get(key)?)?
            .as_string()
            .map(ToOwned::to_owned)
    }

    /// Resolve `key` on the root object to raw bytes.
    ///
    /// Handles both encodings the archive uses: a direct `data` leaf, and the
    /// `NSMutableData` wrapper whose bytes live under `NS.data`. `EncryptionKey`
    /// takes the second form, which is why reading it as a plain leaf yields
    /// nothing.
    #[must_use]
    pub fn root_data(&self, key: &str) -> Option<Vec<u8>> {
        let root = self.root().ok()?;
        let value = self.resolve(root.get(key)?)?;

        if let Some(data) = value.as_data() {
            return Some(data.to_vec());
        }
        let wrapper = value.as_dictionary()?;
        self.resolve(wrapper.get("NS.data")?)?
            .as_data()
            .map(<[u8]>::to_vec)
    }
}
