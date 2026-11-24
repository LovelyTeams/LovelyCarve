//! The database as its XML describes it: groups, entries with their fields,
//! attachments and history, and the metadata.

use std::fmt;

use crate::time::Timestamp;

/// A KeePass UUID, shown as 32 hexadecimal digits as KeePass shows it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Uuid(pub [u8; 16]);

impl Uuid {
    pub fn is_nil(&self) -> bool {
        self.0 == [0; 16]
    }

    /// 32 hexadecimal digits, with or without the dashes of the usual UUID
    /// form, as KeePass field references write them.
    pub fn parse_hex(text: &str) -> Option<Self> {
        let digits: Vec<u8> = text.bytes().filter(|byte| *byte != b'-').collect();
        if digits.len() != 32 {
            return None;
        }
        let digit = |byte: u8| char::from(byte).to_digit(16);
        let mut bytes = [0u8; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (digit(digits[2 * index])? << 4 | digit(digits[2 * index + 1])?) as u8;
        }
        Some(Self(bytes))
    }
}

impl fmt::Display for Uuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Uuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "Uuid({self})")
    }
}

/// The `Times` of a group or entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Times {
    pub creation: Option<Timestamp>,
    pub last_modification: Option<Timestamp>,
    pub last_access: Option<Timestamp>,
    pub expiry: Option<Timestamp>,
    pub expires: bool,
    pub usage_count: Option<u64>,
    pub location_changed: Option<Timestamp>,
}

/// A string field of an entry.
#[derive(Clone, PartialEq, Eq)]
pub struct Field {
    pub key: String,
    pub value: String,
    /// Stored with `Protected="True"`.
    pub protected: bool,
    /// Why a protected value could not be freed; `value` then holds the
    /// stored text.
    pub unreadable: Option<&'static str>,
}

impl fmt::Debug for Field {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("Field");
        debug.field("key", &self.key);
        if self.protected {
            debug.field("value", &"<protected>");
        } else {
            debug.field("value", &self.value);
        }
        debug
            .field("protected", &self.protected)
            .field("unreadable", &self.unreadable)
            .finish()
    }
}

/// Binary data: an attachment in the database pool or inline in an entry.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Binary {
    pub data: Vec<u8>,
    /// Marked for in-memory protection (KDBX 4 inner header flag, or
    /// `Protected="True"` in KDBX 3).
    pub protected: bool,
    /// Stored GZip-compressed (KDBX 3 `Compressed="True"`).
    pub compressed: bool,
    /// Why the data could not be read; `data` is then empty.
    pub unreadable: Option<String>,
}

impl fmt::Debug for Binary {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Binary")
            .field("length", &self.data.len())
            .field("protected", &self.protected)
            .field("compressed", &self.compressed)
            .field("unreadable", &self.unreadable)
            .finish()
    }
}

/// Where an attachment's data is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentSource {
    /// A binary in the database pool, by its id.
    Pool(u32),
    /// Data stored in the entry itself (KDBX 2.x writers).
    Inline(Binary),
}

/// A file attached to an entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub source: AttachmentSource,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutoTypeAssociation {
    pub window: String,
    pub sequence: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AutoType {
    pub enabled: Option<bool>,
    pub obfuscation: Option<i64>,
    pub default_sequence: Option<String>,
    pub associations: Vec<AutoTypeAssociation>,
}

/// A `CustomData` item of the database, a group or an entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomDataItem {
    pub key: String,
    pub value: String,
    /// KDBX 4.1.
    pub last_modification: Option<Timestamp>,
}

/// An entry, or one of its history versions.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub uuid: Option<Uuid>,
    pub icon_id: Option<u32>,
    pub custom_icon: Option<Uuid>,
    pub foreground_color: Option<String>,
    pub background_color: Option<String>,
    pub override_url: Option<String>,
    /// KDBX 4.1.
    pub quality_check: Option<bool>,
    pub tags: Vec<String>,
    /// KDBX 4.1: the group the entry was moved from.
    pub previous_parent_group: Option<Uuid>,
    pub times: Times,
    pub fields: Vec<Field>,
    pub attachments: Vec<Attachment>,
    pub auto_type: Option<AutoType>,
    pub custom_data: Vec<CustomDataItem>,
    /// Earlier versions, oldest first.
    pub history: Vec<Entry>,
}

impl Entry {
    /// The fields KeePass shows in its own columns.
    pub const STANDARD_FIELDS: [&'static str; 5] =
        ["Title", "UserName", "Password", "URL", "Notes"];

    pub fn field(&self, key: &str) -> Option<&Field> {
        self.fields.iter().find(|field| field.key == key)
    }

    pub fn value(&self, key: &str) -> Option<&str> {
        self.field(key).map(|field| field.value.as_str())
    }

    pub fn title(&self) -> Option<&str> {
        self.value("Title")
    }

    pub fn username(&self) -> Option<&str> {
        self.value("UserName")
    }

    pub fn password(&self) -> Option<&str> {
        self.value("Password")
    }

    pub fn url(&self) -> Option<&str> {
        self.value("URL")
    }

    pub fn notes(&self) -> Option<&str> {
        self.value("Notes")
    }

    /// The KeePassXC `otp` field (an `otpauth://` URI), as pykeepass reads it.
    pub fn otp(&self) -> Option<&str> {
        self.value("otp")
    }

    /// Fields other than the five standard ones.
    pub fn custom_fields(&self) -> impl Iterator<Item = &Field> {
        self.fields
            .iter()
            .filter(|field| !Self::STANDARD_FIELDS.contains(&field.key.as_str()))
    }

    /// Whether the entry is set to expire and its expiry is before `now`
    /// (Unix seconds). An expiry without a known zone is not compared.
    pub fn expired_at(&self, now: i64) -> bool {
        self.times.expires
            && self
                .times
                .expiry
                .as_ref()
                .and_then(Timestamp::unix_seconds)
                .is_some_and(|expiry| expiry < now)
    }
}

/// A group and everything under it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Group {
    pub uuid: Option<Uuid>,
    pub name: String,
    pub notes: Option<String>,
    pub icon_id: Option<u32>,
    pub custom_icon: Option<Uuid>,
    pub times: Times,
    pub is_expanded: Option<bool>,
    pub default_auto_type_sequence: Option<String>,
    /// `None` inherits from the parent group.
    pub enable_auto_type: Option<bool>,
    /// `None` inherits from the parent group.
    pub enable_searching: Option<bool>,
    pub last_top_visible_entry: Option<Uuid>,
    /// KDBX 4.1.
    pub previous_parent_group: Option<Uuid>,
    /// KDBX 4.1.
    pub tags: Vec<String>,
    pub custom_data: Vec<CustomDataItem>,
    pub entries: Vec<Entry>,
    pub groups: Vec<Group>,
}

/// `Meta/MemoryProtection`: which standard fields KeePass protects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MemoryProtection {
    pub title: Option<bool>,
    pub username: Option<bool>,
    pub password: Option<bool>,
    pub url: Option<bool>,
    pub notes: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomIcon {
    pub uuid: Option<Uuid>,
    /// The image (normally PNG).
    pub data: Vec<u8>,
    /// KDBX 4.1.
    pub name: Option<String>,
    /// KDBX 4.1.
    pub last_modification: Option<Timestamp>,
}

/// The database metadata.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    pub generator: Option<String>,
    /// KDBX 3.1: SHA-256 of the outer header.
    pub header_hash: Option<Vec<u8>>,
    pub settings_changed: Option<Timestamp>,
    pub database_name: Option<String>,
    pub database_name_changed: Option<Timestamp>,
    pub database_description: Option<String>,
    pub database_description_changed: Option<Timestamp>,
    pub default_username: Option<String>,
    pub default_username_changed: Option<Timestamp>,
    pub maintenance_history_days: Option<u32>,
    pub color: Option<String>,
    pub master_key_changed: Option<Timestamp>,
    /// Days after which a key change is recommended; -1 never.
    pub master_key_change_rec: Option<i64>,
    /// Days after which a key change is forced; -1 never.
    pub master_key_change_force: Option<i64>,
    pub master_key_change_force_once: Option<bool>,
    pub memory_protection: MemoryProtection,
    pub custom_icons: Vec<CustomIcon>,
    pub recycle_bin_enabled: Option<bool>,
    pub recycle_bin_uuid: Option<Uuid>,
    pub recycle_bin_changed: Option<Timestamp>,
    pub entry_templates_group: Option<Uuid>,
    pub entry_templates_group_changed: Option<Timestamp>,
    pub history_max_items: Option<i64>,
    pub history_max_size: Option<i64>,
    pub last_selected_group: Option<Uuid>,
    pub last_top_visible_group: Option<Uuid>,
    pub custom_data: Vec<CustomDataItem>,
}

/// A `DeletedObjects` record: the UUID of something deleted and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeletedObject {
    pub uuid: Option<Uuid>,
    pub deletion_time: Option<Timestamp>,
}
