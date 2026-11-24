//! Read the element tree into the model.

use std::io::Read;
use std::str::FromStr;

use base64::Engine;

use crate::bytes::fixed;
use crate::error::{Error, Result};
use crate::model::{
    Attachment, AttachmentSource, AutoType, AutoTypeAssociation, Binary, CustomDataItem,
    CustomIcon, DeletedObject, Entry, Field, Group, MemoryProtection, Meta, Times, Uuid,
};
use crate::time::Timestamp;
use crate::xml::{Element, Unprotected};

/// What the XML holds.
pub(crate) struct Document {
    pub(crate) meta: Meta,
    pub(crate) root: Group,
    pub(crate) deleted_objects: Vec<DeletedObject>,
    /// KDBX 3 `Meta/Binaries`, by `ID`.
    pub(crate) binaries: Vec<(u32, Binary)>,
}

pub(crate) fn read(
    document: &Element,
    kdbx4: bool,
    max_binary: usize,
    warnings: &mut Vec<String>,
) -> Result<Document> {
    if document.name != "KeePassFile" {
        return Err(Error::Xml(format!(
            "root element <{}> is not <KeePassFile>",
            document.name
        )));
    }
    let mut reader = Reader {
        kdbx4,
        max_binary,
        warnings,
    };
    let (meta, binaries) = match document.child("Meta") {
        Some(meta) => (reader.meta(meta), reader.pool(meta)),
        None => {
            reader.warn("no Meta element".into());
            (Meta::default(), Vec::new())
        }
    };
    let root_element = document.child("Root");
    let mut groups = root_element
        .into_iter()
        .flat_map(|root| root.children_named("Group"));
    let root = match groups.next() {
        Some(group) => reader.group(group),
        None => {
            reader.warn("no root group".into());
            Group::default()
        }
    };
    let extra = groups.count();
    if extra > 0 {
        reader.warn(format!("{extra} further root groups not read"));
    }
    let deleted_objects = root_element
        .and_then(|root| root.child("DeletedObjects"))
        .map(|deleted| {
            deleted
                .children_named("DeletedObject")
                .map(|object| DeletedObject {
                    uuid: reader.uuid(object, "UUID"),
                    deletion_time: reader.time(object, "DeletionTime"),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Document {
        meta,
        root,
        deleted_objects,
        binaries,
    })
}

struct Reader<'a> {
    kdbx4: bool,
    max_binary: usize,
    warnings: &'a mut Vec<String>,
}

/// The element's value as text, protected values freed.
fn value_text(element: &Element) -> String {
    match &element.unprotected {
        Some(Unprotected::Plain(plain)) => String::from_utf8_lossy(plain).into_owned(),
        Some(Unprotected::Failed(_)) | None => element.text(),
    }
}

fn split_tags(text: &str) -> Vec<String> {
    text.split([';', ','])
        .map(str::trim)
        .filter(|tag| !tag.is_empty())
        .map(str::to_owned)
        .collect()
}

impl Reader<'_> {
    fn warn(&mut self, warning: String) {
        self.warnings.push(warning);
    }

    fn text(&self, element: &Element, name: &str) -> Option<String> {
        element.child(name).map(value_text)
    }

    fn parsed<T: FromStr>(&mut self, element: &Element, name: &str) -> Option<T> {
        let text = self.text(element, name)?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return None;
        }
        let parsed = trimmed.parse().ok();
        if parsed.is_none() {
            self.warn(format!("<{name}> {trimmed:?} is not a number"));
        }
        parsed
    }

    fn boolean(&mut self, element: &Element, name: &str) -> Option<bool> {
        let text = self.text(element, name)?;
        match text.trim() {
            value if value.eq_ignore_ascii_case("true") || value == "1" => Some(true),
            value if value.eq_ignore_ascii_case("false") || value == "0" => Some(false),
            value if value.is_empty() || value.eq_ignore_ascii_case("null") => None,
            value => {
                self.warn(format!("<{name}> {value:?} is not a boolean"));
                None
            }
        }
    }

    fn uuid(&mut self, element: &Element, name: &str) -> Option<Uuid> {
        let text = self.text(element, name)?;
        let compact: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        if compact.is_empty() {
            return None;
        }
        let uuid = base64::engine::general_purpose::STANDARD
            .decode(compact.as_bytes())
            .ok()
            .and_then(|bytes| fixed::<16>(&bytes))
            .map(Uuid);
        if uuid.is_none() {
            self.warn(format!("<{name}> {compact:?} is not a UUID"));
        }
        uuid
    }

    fn time(&mut self, element: &Element, name: &str) -> Option<Timestamp> {
        let text = self.text(element, name)?;
        if text.trim().is_empty() {
            return None;
        }
        Some(Timestamp::parse(&text, self.kdbx4))
    }

    fn times(&mut self, element: &Element) -> Times {
        let Some(times) = element.child("Times") else {
            return Times::default();
        };
        Times {
            creation: self.time(times, "CreationTime"),
            last_modification: self.time(times, "LastModificationTime"),
            last_access: self.time(times, "LastAccessTime"),
            expiry: self.time(times, "ExpiryTime"),
            expires: self.boolean(times, "Expires").unwrap_or(false),
            usage_count: self.parsed(times, "UsageCount"),
            location_changed: self.time(times, "LocationChanged"),
        }
    }

    fn custom_data(&mut self, element: &Element) -> Vec<CustomDataItem> {
        let Some(data) = element.child("CustomData") else {
            return Vec::new();
        };
        data.children_named("Item")
            .map(|item| CustomDataItem {
                key: self.text(item, "Key").unwrap_or_default(),
                value: self.text(item, "Value").unwrap_or_default(),
                last_modification: self.time(item, "LastModificationTime"),
            })
            .collect()
    }

    /// Binary data stored in an element: protected, plain base64, or
    /// GZip-compressed base64.
    fn binary(&self, element: &Element) -> Binary {
        let compressed = element.attribute("Compressed") == Some("True");
        match &element.unprotected {
            Some(Unprotected::Plain(plain)) => Binary {
                data: plain.to_vec(),
                protected: true,
                compressed,
                unreadable: None,
            },
            Some(Unprotected::Failed(reason)) => Binary {
                protected: true,
                compressed,
                unreadable: Some((*reason).to_owned()),
                ..Binary::default()
            },
            None => {
                let compact: String = element
                    .text()
                    .chars()
                    .filter(|c| !c.is_ascii_whitespace())
                    .collect();
                // An empty value is empty data, compressed or not.
                if compact.is_empty() {
                    return Binary {
                        compressed,
                        ..Binary::default()
                    };
                }
                let decoded = base64::engine::general_purpose::STANDARD
                    .decode(compact.as_bytes())
                    .map_err(|_| "binary data is not base64".to_owned())
                    .and_then(|data| {
                        if compressed {
                            gunzip(&data, self.max_binary)
                        } else {
                            Ok(data)
                        }
                    });
                match decoded {
                    Ok(data) => Binary {
                        data,
                        compressed,
                        ..Binary::default()
                    },
                    Err(reason) => Binary {
                        compressed,
                        unreadable: Some(reason),
                        ..Binary::default()
                    },
                }
            }
        }
    }

    fn pool(&mut self, meta: &Element) -> Vec<(u32, Binary)> {
        let Some(binaries) = meta.child("Binaries") else {
            return Vec::new();
        };
        let mut pool = Vec::new();
        for binary in binaries.children_named("Binary") {
            match binary.attribute("ID").map(|id| id.trim().parse::<u32>()) {
                Some(Ok(id)) => pool.push((id, self.binary(binary))),
                Some(Err(_)) | None => {
                    self.warn("Meta/Binaries entry without a numeric ID".into());
                }
            }
        }
        pool
    }

    fn meta(&mut self, meta: &Element) -> Meta {
        let header_hash = self.text(meta, "HeaderHash").and_then(|text| {
            let compact: String = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
            base64::engine::general_purpose::STANDARD
                .decode(compact.as_bytes())
                .ok()
        });
        let protection = meta.child("MemoryProtection");
        let memory_protection = match protection {
            Some(protection) => MemoryProtection {
                title: self.boolean(protection, "ProtectTitle"),
                username: self.boolean(protection, "ProtectUserName"),
                password: self.boolean(protection, "ProtectPassword"),
                url: self.boolean(protection, "ProtectURL"),
                notes: self.boolean(protection, "ProtectNotes"),
            },
            None => MemoryProtection::default(),
        };
        let custom_icons = match meta.child("CustomIcons") {
            Some(icons) => icons
                .children_named("Icon")
                .map(|icon| CustomIcon {
                    uuid: self.uuid(icon, "UUID"),
                    data: icon
                        .child("Data")
                        .map(|data| self.binary(data).data)
                        .unwrap_or_default(),
                    name: self.text(icon, "Name"),
                    last_modification: self.time(icon, "LastModificationTime"),
                })
                .collect(),
            None => Vec::new(),
        };
        Meta {
            generator: self.text(meta, "Generator"),
            header_hash,
            settings_changed: self.time(meta, "SettingsChanged"),
            database_name: self.text(meta, "DatabaseName"),
            database_name_changed: self.time(meta, "DatabaseNameChanged"),
            database_description: self.text(meta, "DatabaseDescription"),
            database_description_changed: self.time(meta, "DatabaseDescriptionChanged"),
            default_username: self.text(meta, "DefaultUserName"),
            default_username_changed: self.time(meta, "DefaultUserNameChanged"),
            maintenance_history_days: self.parsed(meta, "MaintenanceHistoryDays"),
            color: self.text(meta, "Color"),
            master_key_changed: self.time(meta, "MasterKeyChanged"),
            master_key_change_rec: self.parsed(meta, "MasterKeyChangeRec"),
            master_key_change_force: self.parsed(meta, "MasterKeyChangeForce"),
            master_key_change_force_once: self.boolean(meta, "MasterKeyChangeForceOnce"),
            memory_protection,
            custom_icons,
            recycle_bin_enabled: self.boolean(meta, "RecycleBinEnabled"),
            recycle_bin_uuid: self.uuid(meta, "RecycleBinUUID"),
            recycle_bin_changed: self.time(meta, "RecycleBinChanged"),
            entry_templates_group: self.uuid(meta, "EntryTemplatesGroup"),
            entry_templates_group_changed: self.time(meta, "EntryTemplatesGroupChanged"),
            history_max_items: self.parsed(meta, "HistoryMaxItems"),
            history_max_size: self.parsed(meta, "HistoryMaxSize"),
            last_selected_group: self.uuid(meta, "LastSelectedGroup"),
            last_top_visible_group: self.uuid(meta, "LastTopVisibleGroup"),
            custom_data: self.custom_data(meta),
        }
    }

    fn field(&self, string: &Element) -> Field {
        let key = self.text(string, "Key").unwrap_or_default();
        let Some(value) = string.child("Value") else {
            return Field {
                key,
                value: String::new(),
                protected: false,
                unreadable: None,
            };
        };
        let (value_text, unreadable) = match &value.unprotected {
            Some(Unprotected::Plain(plain)) => (String::from_utf8_lossy(plain).into_owned(), None),
            Some(Unprotected::Failed(reason)) => (value.text(), Some(*reason)),
            None => (value.text(), None),
        };
        Field {
            key,
            value: value_text,
            protected: value.is_protected(),
            unreadable,
        }
    }

    fn attachment(&mut self, binary: &Element) -> Attachment {
        let name = self.text(binary, "Key").unwrap_or_default();
        let source = match binary.child("Value") {
            Some(value) => match value.attribute("Ref") {
                Some(reference) => match reference.trim().parse::<u32>() {
                    Ok(id) => AttachmentSource::Pool(id),
                    Err(_) => {
                        self.warn(format!("attachment {name:?} refers to {reference:?}"));
                        AttachmentSource::Inline(Binary {
                            unreadable: Some(format!("reference {reference:?} is not a number")),
                            ..Binary::default()
                        })
                    }
                },
                None => AttachmentSource::Inline(self.binary(value)),
            },
            None => AttachmentSource::Inline(Binary::default()),
        };
        Attachment { name, source }
    }

    fn auto_type(&mut self, element: &Element) -> AutoType {
        AutoType {
            enabled: self.boolean(element, "Enabled"),
            obfuscation: self.parsed(element, "DataTransferObfuscation"),
            default_sequence: self.text(element, "DefaultSequence"),
            associations: element
                .children_named("Association")
                .map(|association| AutoTypeAssociation {
                    window: self.text(association, "Window").unwrap_or_default(),
                    sequence: self
                        .text(association, "KeystrokeSequence")
                        .unwrap_or_default(),
                })
                .collect(),
        }
    }

    fn entry(&mut self, element: &Element) -> Entry {
        let history = match element.child("History") {
            Some(history) => history
                .children_named("Entry")
                .map(|version| self.entry(version))
                .collect(),
            None => Vec::new(),
        };
        Entry {
            uuid: self.uuid(element, "UUID"),
            icon_id: self.parsed(element, "IconID"),
            custom_icon: self.uuid(element, "CustomIconUUID"),
            foreground_color: self.text(element, "ForegroundColor"),
            background_color: self.text(element, "BackgroundColor"),
            override_url: self.text(element, "OverrideURL"),
            quality_check: self.boolean(element, "QualityCheck"),
            tags: self
                .text(element, "Tags")
                .map(|tags| split_tags(&tags))
                .unwrap_or_default(),
            previous_parent_group: self.uuid(element, "PreviousParentGroup"),
            times: self.times(element),
            fields: element
                .children_named("String")
                .map(|string| self.field(string))
                .collect(),
            attachments: element
                .children_named("Binary")
                .map(|binary| self.attachment(binary))
                .collect(),
            auto_type: element.child("AutoType").map(|auto| self.auto_type(auto)),
            custom_data: self.custom_data(element),
            history,
        }
    }

    fn group(&mut self, element: &Element) -> Group {
        Group {
            uuid: self.uuid(element, "UUID"),
            name: self.text(element, "Name").unwrap_or_default(),
            notes: self.text(element, "Notes"),
            icon_id: self.parsed(element, "IconID"),
            custom_icon: self.uuid(element, "CustomIconUUID"),
            times: self.times(element),
            is_expanded: self.boolean(element, "IsExpanded"),
            default_auto_type_sequence: self.text(element, "DefaultAutoTypeSequence"),
            enable_auto_type: self.boolean(element, "EnableAutoType"),
            enable_searching: self.boolean(element, "EnableSearching"),
            last_top_visible_entry: self.uuid(element, "LastTopVisibleEntry"),
            previous_parent_group: self.uuid(element, "PreviousParentGroup"),
            tags: self
                .text(element, "Tags")
                .map(|tags| split_tags(&tags))
                .unwrap_or_default(),
            custom_data: self.custom_data(element),
            entries: element
                .children_named("Entry")
                .map(|entry| self.entry(entry))
                .collect(),
            groups: element
                .children_named("Group")
                .map(|group| self.group(group))
                .collect(),
        }
    }
}

/// GZip-decompress at most `limit` bytes.
pub(crate) fn gunzip(data: &[u8], limit: usize) -> std::result::Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut decoder = flate2::read::GzDecoder::new(data).take(limit as u64 + 1);
    decoder
        .read_to_end(&mut out)
        .map_err(|error| format!("GZip data: {error}"))?;
    if out.len() > limit {
        return Err(format!("GZip data larger than {limit} bytes"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::InnerStream;
    use crate::header::InnerStreamAlgorithm;
    use crate::Limits;

    #[test]
    fn reads_a_kdbx41_document() {
        let xml = r#"<?xml version="1.0" encoding="utf-8" standalone="yes"?>
<KeePassFile>
	<Meta>
		<Generator>KeePass</Generator>
		<DatabaseName>Case 7</DatabaseName>
		<RecycleBinEnabled>True</RecycleBinEnabled>
		<RecycleBinUUID>AAAAAAAAAAAAAAAAAAAAAA==</RecycleBinUUID>
		<HistoryMaxItems>-1</HistoryMaxItems>
		<CustomData><Item><Key>k</Key><Value>v</Value></Item></CustomData>
		<Binaries><Binary ID="0" Compressed="False">aGk=</Binary></Binaries>
	</Meta>
	<Root>
		<Group>
			<UUID>AQIDBAUGBwgJCgsMDQ4PEA==</UUID>
			<Name>Root</Name>
			<Tags>a; b,c</Tags>
			<EnableAutoType>null</EnableAutoType>
			<Entry>
				<UUID>EBESExQVFhcYGRobHB0eHw==</UUID>
				<Times><Expires>True</Expires><ExpiryTime>2018-06-10T11:04:01Z</ExpiryTime><UsageCount>3</UsageCount></Times>
				<String><Key>Title</Key><Value>Mail</Value></String>
				<String><Key>Password</Key><Value Protected="True">c2VjcmV0</Value></String>
				<String><Key>PIN</Key><Value>1234</Value></String>
				<Binary><Key>a.txt</Key><Value Ref="0" /></Binary>
				<AutoType><Enabled>True</Enabled><Association><Window>W</Window><KeystrokeSequence>S</KeystrokeSequence></Association></AutoType>
				<History><Entry><String><Key>Title</Key><Value>Old</Value></String></Entry></History>
			</Entry>
		</Group>
		<DeletedObjects><DeletedObject><UUID>ICEiIyQlJicoKSorLC0uLw==</UUID><DeletionTime>2018-06-10T11:04:01Z</DeletionTime></DeletedObject></DeletedObjects>
	</Root>
</KeePassFile>"#;
        let tree = crate::xml::parse(
            xml.as_bytes(),
            &mut InnerStream::new(InnerStreamAlgorithm::None, &[]),
            &Limits::default(),
        )
        .unwrap();
        let mut warnings = Vec::new();
        let document = read(&tree, false, 1 << 20, &mut warnings).unwrap();
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(document.meta.database_name.as_deref(), Some("Case 7"));
        assert_eq!(document.meta.recycle_bin_uuid, Some(Uuid([0; 16])));
        assert_eq!(document.meta.history_max_items, Some(-1));
        assert_eq!(document.binaries[0].0, 0);
        assert_eq!(document.binaries[0].1.data, b"hi");
        let root = &document.root;
        assert_eq!(root.name, "Root");
        assert_eq!(root.tags, ["a", "b", "c"]);
        assert_eq!(root.enable_auto_type, None);
        let entry = &root.entries[0];
        assert_eq!(entry.title(), Some("Mail"));
        assert_eq!(entry.password(), Some("secret"));
        assert!(entry.field("Password").unwrap().protected);
        assert_eq!(entry.custom_fields().count(), 1);
        assert_eq!(entry.times.usage_count, Some(3));
        assert!(entry.expired_at(1_600_000_000));
        assert_eq!(entry.attachments[0].source, AttachmentSource::Pool(0));
        assert_eq!(entry.auto_type.as_ref().unwrap().associations.len(), 1);
        assert_eq!(entry.history[0].title(), Some("Old"));
        assert_eq!(document.deleted_objects.len(), 1);
        assert_eq!(
            document.deleted_objects[0].uuid.unwrap().to_string(),
            "202122232425262728292A2B2C2D2E2F"
        );
    }
}
