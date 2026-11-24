//! An opened database and the read-only queries pykeepass offers.

use zeroize::Zeroizing;

use crate::body;
use crate::crypto::InnerStream;
use crate::error::{Error, Result};
use crate::header::{Header, InnerStreamAlgorithm, Version};
use crate::key::Key;
use crate::model::{Attachment, AttachmentSource, Binary, DeletedObject, Entry, Group, Meta, Uuid};
use crate::parse;
use crate::xml::{self, Element};
use crate::Limits;

/// Whether the header SHA-256 recorded in the database matches the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderHash {
    /// KDBX 4 (checked before decryption), or a KDBX 3.1 `Meta/HeaderHash`
    /// that matches.
    Matches,
    /// A KDBX 3.1 `Meta/HeaderHash` that does not match: the header was
    /// changed after the database was written. KeePass refuses such a file.
    Mismatch,
    /// KDBX 3.0 and earlier do not record it.
    Absent,
}

/// A decrypted database.
pub struct Database {
    pub header: Header,
    pub meta: Meta,
    pub root: Group,
    pub deleted_objects: Vec<DeletedObject>,
    /// The attachment pool by id: the KDBX 4 inner header binaries in order,
    /// or the KDBX 3 `Meta/Binaries` by their `ID`.
    pub binaries: Vec<(u32, Binary)>,
    pub inner_stream: InnerStreamAlgorithm,
    pub header_hash: HeaderHash,
    /// Things read leniently or skipped.
    pub warnings: Vec<String>,
    transformed_key: Zeroizing<[u8; 32]>,
    document: Element,
}

impl std::fmt::Debug for Database {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Database")
            .field("version", &self.header.version)
            .field("cipher", &self.header.cipher)
            .field("kdf", &self.header.kdf.name())
            .field("entries", &self.entries().len())
            .finish_non_exhaustive()
    }
}

/// How deep field references may refer to further references (KeePass
/// stops at the same depth).
const MAX_REFERENCE_DEPTH: usize = 12;

impl Database {
    /// Open a database with the default [`Limits`].
    pub fn open(data: &[u8], key: &Key) -> Result<Self> {
        Self::open_with_limits(data, key, &Limits::default())
    }

    pub fn open_with_limits(data: &[u8], key: &Key, limits: &Limits) -> Result<Self> {
        let header = Header::parse(data)?;
        let plaintext = body::decrypt(&header, data, key, limits)?;
        let mut warnings = plaintext.warnings;
        let mut stream = InnerStream::new(plaintext.inner_stream, &plaintext.inner_stream_key);
        if let InnerStream::Unsupported(algorithm) = &stream {
            warnings.push(format!(
                "inner stream {} ({}) is not implemented; protected values stay encrypted",
                algorithm.name(),
                algorithm.id()
            ));
        }
        let document = xml::parse(&plaintext.xml, &mut stream, limits)?;
        let kdbx4 = header.version.is_kdbx4();
        let parsed = parse::read(&document, kdbx4, limits.max_payload, &mut warnings)?;
        let binaries = if kdbx4 {
            if !parsed.binaries.is_empty() {
                warnings.push(format!(
                    "{} Meta/Binaries entries ignored in a KDBX 4 file",
                    parsed.binaries.len()
                ));
            }
            plaintext
                .binaries
                .into_iter()
                .enumerate()
                .map(|(index, binary)| {
                    u32::try_from(index)
                        .map(|id| (id, binary))
                        .map_err(|_| Error::Malformed("binary count".into()))
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            parsed.binaries
        };
        let header_hash = match (&parsed.meta.header_hash, kdbx4) {
            (_, true) => HeaderHash::Matches,
            (Some(stored), false) if stored[..] == header.sha256 => HeaderHash::Matches,
            (Some(_), false) => {
                warnings.push("Meta/HeaderHash does not match the header".into());
                HeaderHash::Mismatch
            }
            (None, false) => HeaderHash::Absent,
        };
        Ok(Self {
            header,
            meta: parsed.meta,
            root: parsed.root,
            deleted_objects: parsed.deleted_objects,
            binaries,
            inner_stream: plaintext.inner_stream,
            header_hash,
            warnings,
            transformed_key: plaintext.transformed_key,
            document,
        })
    }

    pub fn version(&self) -> Version {
        self.header.version
    }

    /// The KDF output; opens the database again without the password and
    /// keyfile ([`Key::from_transformed`]).
    pub fn transformed_key(&self) -> &[u8; 32] {
        &self.transformed_key
    }

    /// The KDF seed or salt.
    pub fn database_salt(&self) -> &[u8] {
        self.header.kdf.salt()
    }

    /// The decrypted XML with protected values in plaintext.
    pub fn xml(&self) -> String {
        xml::write(&self.document)
    }

    pub fn binary(&self, id: u32) -> Option<&Binary> {
        self.binaries
            .iter()
            .find(|(binary_id, _)| *binary_id == id)
            .map(|(_, binary)| binary)
    }

    /// The bytes of an attachment, or `None` when its pool binary is missing.
    pub fn attachment_data<'a>(&'a self, attachment: &'a Attachment) -> Option<&'a [u8]> {
        match &attachment.source {
            AttachmentSource::Pool(id) => self.binary(*id).map(|binary| binary.data.as_slice()),
            AttachmentSource::Inline(binary) => Some(&binary.data),
        }
    }

    /// Every group, the root first, in document order.
    pub fn groups(&self) -> Vec<&Group> {
        let mut out = Vec::new();
        let mut pending = vec![&self.root];
        while let Some(group) = pending.pop() {
            out.push(group);
            pending.extend(group.groups.iter().rev());
        }
        out
    }

    /// Every entry in document order, without history versions.
    pub fn entries(&self) -> Vec<&Entry> {
        self.groups()
            .into_iter()
            .flat_map(|group| group.entries.iter())
            .collect()
    }

    pub fn find_groups(&self, predicate: impl Fn(&Group) -> bool) -> Vec<&Group> {
        self.groups()
            .into_iter()
            .filter(|group| predicate(group))
            .collect()
    }

    pub fn find_entries(&self, predicate: impl Fn(&Entry) -> bool) -> Vec<&Entry> {
        self.entries()
            .into_iter()
            .filter(|entry| predicate(entry))
            .collect()
    }

    pub fn group_by_uuid(&self, uuid: &Uuid) -> Option<&Group> {
        self.groups()
            .into_iter()
            .find(|group| group.uuid.as_ref() == Some(uuid))
    }

    pub fn entry_by_uuid(&self, uuid: &Uuid) -> Option<&Entry> {
        self.entries()
            .into_iter()
            .find(|entry| entry.uuid.as_ref() == Some(uuid))
    }

    /// The group names from below the root to the group holding `uuid` (an
    /// entry or a group), as pykeepass' `path` without the final name.
    pub fn parent_path(&self, uuid: &Uuid) -> Option<Vec<&str>> {
        fn search<'a>(group: &'a Group, uuid: &Uuid, path: &mut Vec<&'a str>) -> bool {
            if group
                .entries
                .iter()
                .any(|entry| entry.uuid.as_ref() == Some(uuid))
                || group
                    .groups
                    .iter()
                    .any(|child| child.uuid.as_ref() == Some(uuid))
            {
                return true;
            }
            for child in &group.groups {
                path.push(&child.name);
                if search(child, uuid, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
        let mut path = Vec::new();
        search(&self.root, uuid, &mut path).then_some(path)
    }

    /// The group `Meta/RecycleBinUUID` names, if it exists.
    pub fn recycle_bin(&self) -> Option<&Group> {
        let uuid = self.meta.recycle_bin_uuid.filter(|uuid| !uuid.is_nil())?;
        self.group_by_uuid(&uuid)
    }

    /// Resolve KeePass field references (`{REF:P@I:<uuid>}` …) in `value`.
    /// `None` when a reference names no entry, as in pykeepass.
    pub fn deref(&self, value: &str) -> Option<String> {
        let mut value = value.to_owned();
        for _ in 0..MAX_REFERENCE_DEPTH {
            let references = references(&value);
            if references.is_empty() {
                return Some(value);
            }
            for reference in references {
                let entry = self.referenced_entry(&reference)?;
                let replacement = match reference.wanted {
                    'I' => entry.uuid.map(|uuid| uuid.to_string()).unwrap_or_default(),
                    wanted => entry
                        .value(field_name(wanted).unwrap_or_default())
                        .unwrap_or_default()
                        .to_owned(),
                };
                value = value.replace(&reference.text, &replacement);
            }
        }
        Some(value)
    }

    fn referenced_entry(&self, reference: &Reference) -> Option<&Entry> {
        match reference.search_in {
            'I' => {
                let uuid = Uuid::parse_hex(&reference.search)?;
                self.entry_by_uuid(&uuid)
            }
            'O' => self.entries().into_iter().find(|entry| {
                entry
                    .custom_fields()
                    .any(|field| field.value == reference.search)
            }),
            field => {
                let name = field_name(field)?;
                self.entries()
                    .into_iter()
                    .find(|entry| entry.value(name) == Some(reference.search.as_str()))
            }
        }
    }
}

struct Reference {
    text: String,
    wanted: char,
    search_in: char,
    search: String,
}

fn field_name(letter: char) -> Option<&'static str> {
    Some(match letter {
        'T' => "Title",
        'U' => "UserName",
        'P' => "Password",
        'A' => "URL",
        'N' => "Notes",
        _ => return None,
    })
}

/// The `{REF:W@S:text}` references in `value`.
fn references(value: &str) -> Vec<Reference> {
    let mut out = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("{REF:") {
        let candidate = &rest[start..];
        let mut characters = candidate[5..].chars();
        let parsed = (|| {
            let wanted = characters.next().filter(|c| "TUPANI".contains(*c))?;
            characters.next().filter(|c| *c == '@')?;
            let search_in = characters.next().filter(|c| "TUPANIO".contains(*c))?;
            characters.next().filter(|c| *c == ':')?;
            let body = characters.as_str();
            let end = body.find('}')?;
            let search = &body[..end];
            if search.is_empty() {
                return None;
            }
            let length = candidate.len() - body.len() + end + 1;
            Some(Reference {
                text: candidate[..length].to_owned(),
                wanted,
                search_in,
                search: search.to_owned(),
            })
        })();
        match parsed {
            Some(reference) => {
                rest = &candidate[reference.text.len()..];
                if !out
                    .iter()
                    .any(|known: &Reference| known.text == reference.text)
                {
                    out.push(reference);
                }
            }
            None => rest = &candidate[5..],
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_references() {
        let found = references("x {REF:P@I:0011} y {REF:U@T:mail}{REF:bad} {REF:P@I:0011}");
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].text, "{REF:P@I:0011}");
        assert_eq!((found[0].wanted, found[0].search_in), ('P', 'I'));
        assert_eq!(found[1].search, "mail");
        assert_eq!(
            Uuid::parse_hex("cc5f7ecd-2a00-48ca-9621-c222a347b0bb").map(|uuid| uuid.to_string()),
            Some("CC5F7ECD2A0048CA9621C222A347B0BB".into())
        );
        assert_eq!(Uuid::parse_hex("cc5f7ecd"), None);
        assert_eq!(Uuid::parse_hex(&"g".repeat(32)), None);
    }
}
