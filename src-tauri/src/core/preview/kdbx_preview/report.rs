//! Serializable reports of a KDBX header and a decrypted database.

use serde::Serialize;
use sha2::{Digest, Sha256};

use kdbx::{
    Attachment, AttachmentSource, CustomDataItem, Database, Entry, Group, HeaderHash, Times,
    Timestamp, Uuid,
};

use super::header::{header_report, KdbxHeaderReport};

/// A time as the database states it.
#[derive(Debug, Serialize)]
pub struct KdbxTime {
    /// `utc`: an instant; `unzoned`: a KDBX 3 text time without zone;
    /// `invalid`: text that is not a time.
    pub kind: &'static str,
    /// Unix seconds, for `utc`.
    pub seconds: Option<i64>,
    /// The stored text, for `unzoned` and `invalid`.
    pub text: Option<String>,
}

fn time(time: Option<&Timestamp>) -> Option<KdbxTime> {
    time.map(|time| match time {
        Timestamp::Utc(seconds) => KdbxTime {
            kind: "utc",
            seconds: Some(*seconds),
            text: None,
        },
        Timestamp::Unzoned(text) => KdbxTime {
            kind: "unzoned",
            seconds: None,
            text: Some(text.clone()),
        },
        Timestamp::Invalid(text) => KdbxTime {
            kind: "invalid",
            seconds: None,
            text: Some(text.clone()),
        },
    })
}

fn uuid(uuid: Option<&Uuid>) -> Option<String> {
    uuid.map(Uuid::to_string)
}

/// The credentials that opened the database.
#[derive(Debug, Serialize)]
pub struct KdbxKeyUsed {
    /// `set`, `empty` (an empty password component) or `none`.
    pub password: &'static str,
    /// The keyfile form, when one was used.
    pub keyfile: Option<&'static str>,
    pub transformed_key: bool,
}

#[derive(Debug, Serialize)]
pub struct KdbxTimes {
    pub creation: Option<KdbxTime>,
    pub last_modification: Option<KdbxTime>,
    pub last_access: Option<KdbxTime>,
    pub expiry: Option<KdbxTime>,
    pub expires: bool,
    pub usage_count: Option<u64>,
    pub location_changed: Option<KdbxTime>,
}

fn times(times: &Times) -> KdbxTimes {
    KdbxTimes {
        creation: time(times.creation.as_ref()),
        last_modification: time(times.last_modification.as_ref()),
        last_access: time(times.last_access.as_ref()),
        expiry: time(times.expiry.as_ref()),
        expires: times.expires,
        usage_count: times.usage_count,
        location_changed: time(times.location_changed.as_ref()),
    }
}

#[derive(Debug, Serialize)]
pub struct KdbxCustomData {
    pub key: String,
    pub value: String,
    pub last_modification: Option<KdbxTime>,
}

fn custom_data(items: &[CustomDataItem]) -> Vec<KdbxCustomData> {
    items
        .iter()
        .map(|item| KdbxCustomData {
            key: item.key.clone(),
            value: item.value.clone(),
            last_modification: time(item.last_modification.as_ref()),
        })
        .collect()
}

#[derive(Debug, Serialize)]
pub struct KdbxField {
    pub key: String,
    pub value: String,
    pub protected: bool,
    /// Why a protected value could not be freed; `value` is then as stored.
    pub unreadable: Option<&'static str>,
    /// The value with KeePass field references resolved, when it has any.
    pub resolved: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct KdbxAttachment {
    pub name: String,
    /// The pool binary; `None` for data stored in the entry.
    pub binary_id: Option<u32>,
    /// `None` when the pool has no such binary.
    pub size: Option<usize>,
    pub sha256: Option<String>,
    pub protected: bool,
    pub unreadable: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct KdbxAutoTypeAssociation {
    pub window: String,
    pub sequence: String,
}

#[derive(Debug, Serialize)]
pub struct KdbxAutoType {
    pub enabled: Option<bool>,
    pub obfuscation: Option<i64>,
    pub default_sequence: Option<String>,
    pub associations: Vec<KdbxAutoTypeAssociation>,
}

#[derive(Debug, Serialize)]
pub struct KdbxEntry {
    pub uuid: Option<String>,
    pub icon_id: Option<u32>,
    pub custom_icon: Option<String>,
    pub foreground_color: Option<String>,
    pub background_color: Option<String>,
    pub override_url: Option<String>,
    pub quality_check: Option<bool>,
    pub tags: Vec<String>,
    pub previous_parent_group: Option<String>,
    pub times: KdbxTimes,
    pub fields: Vec<KdbxField>,
    pub attachments: Vec<KdbxAttachment>,
    pub auto_type: Option<KdbxAutoType>,
    pub custom_data: Vec<KdbxCustomData>,
    /// Earlier versions, oldest first.
    pub history: Vec<KdbxEntry>,
}

#[derive(Debug, Serialize)]
pub struct KdbxGroup {
    pub uuid: Option<String>,
    pub name: String,
    pub notes: Option<String>,
    pub icon_id: Option<u32>,
    pub custom_icon: Option<String>,
    pub times: KdbxTimes,
    pub is_expanded: Option<bool>,
    pub default_auto_type_sequence: Option<String>,
    pub enable_auto_type: Option<bool>,
    pub enable_searching: Option<bool>,
    pub last_top_visible_entry: Option<String>,
    pub previous_parent_group: Option<String>,
    pub tags: Vec<String>,
    pub custom_data: Vec<KdbxCustomData>,
    pub entries: Vec<KdbxEntry>,
    pub groups: Vec<KdbxGroup>,
}

#[derive(Debug, Serialize)]
pub struct KdbxMemoryProtection {
    pub title: Option<bool>,
    pub username: Option<bool>,
    pub password: Option<bool>,
    pub url: Option<bool>,
    pub notes: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct KdbxCustomIcon {
    pub uuid: Option<String>,
    pub name: Option<String>,
    pub size: usize,
    pub last_modification: Option<KdbxTime>,
}

#[derive(Debug, Serialize)]
pub struct KdbxMeta {
    pub generator: Option<String>,
    pub settings_changed: Option<KdbxTime>,
    pub database_name: Option<String>,
    pub database_name_changed: Option<KdbxTime>,
    pub database_description: Option<String>,
    pub database_description_changed: Option<KdbxTime>,
    pub default_username: Option<String>,
    pub default_username_changed: Option<KdbxTime>,
    pub maintenance_history_days: Option<u32>,
    pub color: Option<String>,
    pub master_key_changed: Option<KdbxTime>,
    pub master_key_change_rec: Option<i64>,
    pub master_key_change_force: Option<i64>,
    pub master_key_change_force_once: Option<bool>,
    pub memory_protection: KdbxMemoryProtection,
    pub custom_icons: Vec<KdbxCustomIcon>,
    pub recycle_bin_enabled: Option<bool>,
    pub recycle_bin_uuid: Option<String>,
    pub recycle_bin_changed: Option<KdbxTime>,
    pub entry_templates_group: Option<String>,
    pub entry_templates_group_changed: Option<KdbxTime>,
    pub history_max_items: Option<i64>,
    pub history_max_size: Option<i64>,
    pub last_selected_group: Option<String>,
    pub last_top_visible_group: Option<String>,
    pub custom_data: Vec<KdbxCustomData>,
}

#[derive(Debug, Serialize)]
pub struct KdbxDeletedObject {
    pub uuid: Option<String>,
    pub deletion_time: Option<KdbxTime>,
}

#[derive(Debug, Serialize)]
pub struct KdbxBinary {
    pub id: u32,
    pub size: usize,
    pub protected: bool,
    pub compressed: bool,
    pub sha256: String,
    pub unreadable: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct KdbxCounts {
    pub groups: usize,
    pub entries: usize,
    pub history: usize,
    pub attachments: usize,
}

/// A decrypted database.
#[derive(Debug, Serialize)]
pub struct KdbxDatabaseReport {
    pub header: KdbxHeaderReport,
    pub key: KdbxKeyUsed,
    /// Reopens the database without the password and keyfile.
    pub transformed_key: String,
    /// `matches`, `mismatch` (KDBX 3.1 header changed after writing) or
    /// `absent` (KDBX 3.0).
    pub header_hash: &'static str,
    pub inner_stream: &'static str,
    pub meta: KdbxMeta,
    pub root: KdbxGroup,
    /// The recycle bin group's UUID, when the group exists.
    pub recycle_bin: Option<String>,
    pub deleted_objects: Vec<KdbxDeletedObject>,
    pub binaries: Vec<KdbxBinary>,
    pub counts: KdbxCounts,
    pub warnings: Vec<String>,
}

struct Builder<'a> {
    database: &'a Database,
    counts: KdbxCounts,
}

impl Builder<'_> {
    fn attachment(&mut self, attachment: &Attachment) -> KdbxAttachment {
        self.counts.attachments += 1;
        let (binary_id, binary) = match &attachment.source {
            AttachmentSource::Pool(id) => (Some(*id), self.database.binary(*id)),
            AttachmentSource::Inline(binary) => (None, Some(binary)),
        };
        KdbxAttachment {
            name: attachment.name.clone(),
            binary_id,
            size: binary.map(|binary| binary.data.len()),
            sha256: binary.map(|binary| hex::encode(Sha256::digest(&binary.data))),
            protected: binary.is_some_and(|binary| binary.protected),
            unreadable: match binary {
                Some(binary) => binary.unreadable.clone(),
                None => Some("附件数据不在数据库中".to_owned()),
            },
        }
    }

    fn entry(&mut self, entry: &Entry, current: bool) -> KdbxEntry {
        if current {
            self.counts.entries += 1;
        } else {
            self.counts.history += 1;
        }
        let database = self.database;
        KdbxEntry {
            uuid: uuid(entry.uuid.as_ref()),
            icon_id: entry.icon_id,
            custom_icon: uuid(entry.custom_icon.as_ref()),
            foreground_color: entry.foreground_color.clone(),
            background_color: entry.background_color.clone(),
            override_url: entry.override_url.clone(),
            quality_check: entry.quality_check,
            tags: entry.tags.clone(),
            previous_parent_group: uuid(entry.previous_parent_group.as_ref()),
            times: times(&entry.times),
            fields: entry
                .fields
                .iter()
                .map(|field| KdbxField {
                    key: field.key.clone(),
                    value: field.value.clone(),
                    protected: field.protected,
                    unreadable: field.unreadable,
                    resolved: field
                        .value
                        .contains("{REF:")
                        .then(|| database.deref(&field.value))
                        .flatten()
                        .filter(|resolved| *resolved != field.value),
                })
                .collect(),
            attachments: entry
                .attachments
                .iter()
                .map(|attachment| self.attachment(attachment))
                .collect(),
            auto_type: entry.auto_type.as_ref().map(|auto| KdbxAutoType {
                enabled: auto.enabled,
                obfuscation: auto.obfuscation,
                default_sequence: auto.default_sequence.clone(),
                associations: auto
                    .associations
                    .iter()
                    .map(|association| KdbxAutoTypeAssociation {
                        window: association.window.clone(),
                        sequence: association.sequence.clone(),
                    })
                    .collect(),
            }),
            custom_data: custom_data(&entry.custom_data),
            history: entry
                .history
                .iter()
                .map(|version| self.entry(version, false))
                .collect(),
        }
    }

    fn group(&mut self, group: &Group) -> KdbxGroup {
        self.counts.groups += 1;
        KdbxGroup {
            uuid: uuid(group.uuid.as_ref()),
            name: group.name.clone(),
            notes: group.notes.clone(),
            icon_id: group.icon_id,
            custom_icon: uuid(group.custom_icon.as_ref()),
            times: times(&group.times),
            is_expanded: group.is_expanded,
            default_auto_type_sequence: group.default_auto_type_sequence.clone(),
            enable_auto_type: group.enable_auto_type,
            enable_searching: group.enable_searching,
            last_top_visible_entry: uuid(group.last_top_visible_entry.as_ref()),
            previous_parent_group: uuid(group.previous_parent_group.as_ref()),
            tags: group.tags.clone(),
            custom_data: custom_data(&group.custom_data),
            entries: group
                .entries
                .iter()
                .map(|entry| self.entry(entry, true))
                .collect(),
            groups: group.groups.iter().map(|child| self.group(child)).collect(),
        }
    }
}

pub fn database_report(
    database: &Database,
    key: KdbxKeyUsed,
    file_size: u64,
) -> KdbxDatabaseReport {
    let meta = &database.meta;
    let mut builder = Builder {
        database,
        counts: KdbxCounts::default(),
    };
    let root = builder.group(&database.root);
    KdbxDatabaseReport {
        header: header_report(&database.header, file_size),
        key,
        transformed_key: hex::encode(database.transformed_key()),
        header_hash: match database.header_hash {
            HeaderHash::Matches => "matches",
            HeaderHash::Mismatch => "mismatch",
            HeaderHash::Absent => "absent",
        },
        inner_stream: database.inner_stream.name(),
        meta: KdbxMeta {
            generator: meta.generator.clone(),
            settings_changed: time(meta.settings_changed.as_ref()),
            database_name: meta.database_name.clone(),
            database_name_changed: time(meta.database_name_changed.as_ref()),
            database_description: meta.database_description.clone(),
            database_description_changed: time(meta.database_description_changed.as_ref()),
            default_username: meta.default_username.clone(),
            default_username_changed: time(meta.default_username_changed.as_ref()),
            maintenance_history_days: meta.maintenance_history_days,
            color: meta.color.clone(),
            master_key_changed: time(meta.master_key_changed.as_ref()),
            master_key_change_rec: meta.master_key_change_rec,
            master_key_change_force: meta.master_key_change_force,
            master_key_change_force_once: meta.master_key_change_force_once,
            memory_protection: KdbxMemoryProtection {
                title: meta.memory_protection.title,
                username: meta.memory_protection.username,
                password: meta.memory_protection.password,
                url: meta.memory_protection.url,
                notes: meta.memory_protection.notes,
            },
            custom_icons: meta
                .custom_icons
                .iter()
                .map(|icon| KdbxCustomIcon {
                    uuid: uuid(icon.uuid.as_ref()),
                    name: icon.name.clone(),
                    size: icon.data.len(),
                    last_modification: time(icon.last_modification.as_ref()),
                })
                .collect(),
            recycle_bin_enabled: meta.recycle_bin_enabled,
            recycle_bin_uuid: uuid(meta.recycle_bin_uuid.as_ref()),
            recycle_bin_changed: time(meta.recycle_bin_changed.as_ref()),
            entry_templates_group: uuid(meta.entry_templates_group.as_ref()),
            entry_templates_group_changed: time(meta.entry_templates_group_changed.as_ref()),
            history_max_items: meta.history_max_items,
            history_max_size: meta.history_max_size,
            last_selected_group: uuid(meta.last_selected_group.as_ref()),
            last_top_visible_group: uuid(meta.last_top_visible_group.as_ref()),
            custom_data: custom_data(&meta.custom_data),
        },
        root,
        recycle_bin: database
            .recycle_bin()
            .and_then(|group| uuid(group.uuid.as_ref())),
        deleted_objects: database
            .deleted_objects
            .iter()
            .map(|object| KdbxDeletedObject {
                uuid: uuid(object.uuid.as_ref()),
                deletion_time: time(object.deletion_time.as_ref()),
            })
            .collect(),
        binaries: database
            .binaries
            .iter()
            .map(|(id, binary)| KdbxBinary {
                id: *id,
                size: binary.data.len(),
                protected: binary.protected,
                compressed: binary.compressed,
                sha256: hex::encode(Sha256::digest(&binary.data)),
                unreadable: binary.unreadable.clone(),
            })
            .collect(),
        counts: builder.counts,
        warnings: database.warnings.clone(),
    }
}
