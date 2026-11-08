use std::collections::HashSet;

use time::OffsetDateTime;

use super::{DirectoryEntryHeader, PosixAttributes, PosixFileMode, PosixTimestamp, SuspExtension};

/// Holds information from system use (SUSP) entries.
///
/// ## See Also
/// * Rock Ridge Interchange Protocol v1.12
/// * System Use Sharing Protocol v1.12
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ExtraMeta {
    /// Used for e.g. Rock Ridge long filenames
    ///
    /// This contains an alternative name picked up from
    /// an `NM` entry in the system use table.  Joliet style
    /// filenames and ISO 9660 Level 3 filenames simply
    /// store the long filenames in the directory structures.
    ///
    /// ## See Also
    ///
    /// System Use Sharing Protocol § 4.1.4
    pub alt_name: Option<String>,

    /// POSIX attributes (permissions, ownership, links, inode)
    ///
    /// This contains a [`PosixAttributes`] struct generated
    /// from a `PX` entry in the system use table.
    ///
    /// ## See Also
    ///
    /// System Use Sharing Protocol § 4.1.1
    pub attributes: Option<PosixAttributes>,

    /// If the directory entry is a symbolic link, its target is stored here.
    ///
    /// This contains a path specified by one or more `SL` entries
    /// in the system use table.
    ///
    /// ## See Also
    ///
    /// System Use Sharing Protocol § 4.1.3
    pub symlink_target: Option<String>,

    /// [`HashSet`] of all the system use extensions used by this directory hierarchy.
    ///
    /// All SUSP-compliant extensions are required to include an `ER` entry in
    /// the system use table.
    ///
    /// ## See Also
    ///
    /// System Use Sharing Protocol § 5.5
    pub extensions: HashSet<SuspExtension>,

    /// POSIX style timestamps (access, creation, modification, etc.)
    ///
    /// This field cotains the timestamps collected from one or more `TF` entries
    /// in the system use table.
    ///
    /// ## See Also
    ///
    /// Rock Ridge Interchange Protocol § 4.1.6
    pub timestamps: PosixTimestamp,

    /// True if this directory actually exists at another location.
    ///
    /// To avoid misbehavior relocated directories should be hidden from view or given a distinct inode.
    ///
    /// ## See Also
    ///
    /// Rock Ridge Interchange Protocol § 4.1.5
    pub relocated: bool,
}

/// `ExtraAttributes` encapsulates various metadata specified by ISO-9660 / ECMA-119 extensions.
///
/// This is the preferred way to get [`DirectoryEntry`](crate::DirectoryEntry) metadata even if there is overlap with the
/// base standard.
pub trait ExtraAttributes {
    /// Returns the [`ExtraMeta`] attached to the current object.  Generally not something to be called directly.
    fn ext(&self) -> &ExtraMeta;

    /// Returns the `DirectoryEntryHeader` attached to the current object.  Generally not something to be called directly.
    fn header(&self) -> &DirectoryEntryHeader;

    /// Returns true if this directory has been relocated elsewhere to circumvent ISO 9660's limits
    /// on directory depth.
    ///
    /// ## See Also
    ///
    /// Rock Ridge Interchange Protocol § 4.1.5
    fn relocated(&self) -> bool {
        self.ext().relocated
    }

    /// Returns the ISO 9660 "recording" timestamp.
    ///
    /// # See Also
    ///
    /// ISO-9660 / ECMA-119 §§ 9.1.5
    fn time(&self) -> OffsetDateTime {
        self.header().time
    }

    /// Returns the file owner's user ID (`st_uid`), if available.
    ///
    /// # See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * Rock Ridge Interchange Protocol § 4.1.1
    fn owner(&self) -> Option<u32> {
        self.ext()
            .attributes
            .as_ref()
            .map(|attributes| attributes.uid)
    }

    /// Returns the file owner's group ID (`st_gid`), if available.
    ///
    /// # See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * Rock Ridge Interchange Protocol § 4.1.1
    fn group(&self) -> Option<u32> {
        self.ext()
            .attributes
            .as_ref()
            .map(|attributes| attributes.gid)
    }

    /// Returns the file protection mode / Unix permissions (a.k.a. `st_mode`), if available.
    ///
    /// # See Also
    ///
    /// [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    fn mode(&self) -> Option<PosixFileMode> {
        self.ext()
            .attributes
            .as_ref()
            .map(|attributes| attributes.mode)
    }

    /// Returns the last time the file contents were accessed (`st_atime`), if available.  If there
    /// is no access time available, the value of [`time()`](Self::time) is returned.
    ///
    /// # See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * Rock Ridge Interchange Protocol § 4.1.6
    fn access_time(&self) -> OffsetDateTime {
        self.ext().timestamps.access.unwrap_or(self.time())
    }

    /// Returns the last time the attributes were changed (`st_ctime`), if available.  If
    /// there is no ctime available, the value of [`time()`](Self::time) is returned.
    ///
    /// # See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * Rock Ridge Interchange Protocol § 4.1.6
    fn attribute_change_time(&self) -> OffsetDateTime {
        self.ext().timestamps.attributes.unwrap_or(self.time())
    }

    /// Returns the last backup time, if available.  If there is no backup time available, the value
    /// of [`time()`](Self::time) is returned.
    ///
    /// # See Also
    ///
    /// * Rock Ridge Interchange Protocol § 4.1.6
    fn backup_time(&self) -> OffsetDateTime {
        self.ext().timestamps.backup.unwrap_or(self.time())
    }

    /// Returns the creation time, if available.  If there is no creation time available, the value
    /// of [`time()`](Self::time) is returned.
    ///
    /// # See Also
    ///
    /// * ISO 9660 / ECMA-119 § 9.5.4
    /// * Rock Ridge Interchange Protocol § 4.1.6
    fn create_time(&self) -> OffsetDateTime {
        self.ext().timestamps.creation.unwrap_or(self.time())
    }

    /// Returns the effective time, if available.  If there is no expiration time available, the value
    /// of [`time()`](Self::time) is returned.
    ///
    /// # See Also
    ///
    /// * ISO 9660 / ECMA-119 § 9.5.7
    /// * Rock Ridge Interchange Protocol § 4.1.6
    fn effective_time(&self) -> OffsetDateTime {
        self.ext().timestamps.effective.unwrap_or(self.time())
    }

    /// Returns the expiration time, if available.  If there is no expiration time available, the value
    /// of [`time()`](Self::time) is returned.
    ///
    /// # See Also
    ///
    /// * ISO 9660 / ECMA-119 § 9.5.6
    /// * Rock Ridge Interchange Protocol § 4.1.6
    fn expire_time(&self) -> OffsetDateTime {
        self.ext().timestamps.expiration.unwrap_or(self.time())
    }

    /// Returns the last modification time (`st_mtime`), if available.  If there is no modification
    /// time available, the value of [`time()`](Self::time) is returned.
    ///
    /// # See Also
    ///
    /// * [POSIX.1](https://en.wikipedia.org/wiki/Stat_(system_call)#stat_structure)
    /// * ISO 9660 / ECMA-119 § 9.5.5
    /// * Rock Ridge Interchange Protocol § 4.1.6
    fn modify_time(&self) -> OffsetDateTime {
        self.ext().timestamps.modify.unwrap_or(self.time())
    }

    /// Returns the serial number (a.k.a. inode), if available.
    ///
    /// # See Also
    ///
    /// * ISO-9660 / ECMA-119 §§ 12.3.5
    /// * Rock Ridge Interchange Protocol § 4.1.1
    fn inode(&self) -> Option<u32> {
        // inodes weren't introduced until Rock Ridge v1.12,
        // but e.g. mkisofs marks its data as pre-IEEE Rock Ridge
        if let Some(attributes) = &self.ext().attributes {
            if let Some(inode) = attributes.inode {
                return Some(inode);
            }
        }
        None
    }
}
