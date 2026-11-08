// SPDX-License-Identifier: (MIT OR Apache-2.0)

use std::{fmt, str::FromStr};

use super::{DirectoryEntryHeader, ExtraAttributes, ExtraMeta};
use crate::Result;

/// [`DirectoryEntry`](crate::DirectoryEntry) for symbolic links. Typically generated from `SL` entries.
///
/// # See Also
///
/// * ISO-9660 / ECMA-119 § 9
/// * Rock Ridge Interchange Protocol § 4.1.3
#[derive(Clone)]
pub struct Symlink {
    pub(crate) header: DirectoryEntryHeader,

    /// The name encoded with UTF-8.
    pub identifier: String,

    /// File version; ranges from 1 to 32767
    pub version: u16,

    pub(super) ext: ExtraMeta,
}

impl fmt::Debug for Symlink {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_struct("Symlink")
            .field("header", &self.header)
            .field("identifier", &self.identifier)
            .field("version", &self.version)
            .field("ext", &self.ext)
            .finish()
    }
}

impl ExtraAttributes for Symlink {
    fn ext(&self) -> &ExtraMeta {
        &self.ext
    }

    fn header(&self) -> &DirectoryEntryHeader {
        &self.header
    }
}

impl Symlink {
    pub(crate) fn new(
        header: DirectoryEntryHeader,
        ext: ExtraMeta,
        identifier: String,
    ) -> Result<Self> {
        let mut identifier = match ext.alt_name.as_ref() {
            Some(alt_name) => alt_name.clone(),
            None => identifier,
        };

        // Files (not directories) in ISO 9660 have a version number, which is
        // provided at the end of the identifier, seperated by ';'.
        // If not, assume 1.
        let version = match identifier.rfind(';') {
            Some(idx) => {
                let version = u16::from_str(&identifier[idx + 1..])?;
                identifier.truncate(idx);
                version
            }
            None => 1,
        };

        // Files without an extension have a '.' at the end
        if identifier.ends_with('.') {
            identifier.pop();
        }

        Ok(Self {
            header,
            identifier,
            version,
            ext,
        })
    }

    /// Returns the path that the symbolic link points to.
    pub fn target(&self) -> Option<&String> {
        self.ext.symlink_target.as_ref()
    }
}
