//! Credentials: the composite key made from a password and a keyfile, or a
//! transformed key computed earlier (for example recovered from memory).

use std::fmt;

use base64::Engine;
use quick_xml::events::Event;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::error::{Error, Result};

/// How a keyfile's contents became key material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyfileKind {
    /// KeePass XML keyfile 1.0: base64 key data.
    XmlV1,
    /// KeePass XML keyfile 2.0 (`.keyx`): hex key data with a hash check.
    XmlV2,
    /// Exactly 32 bytes, used as they are.
    Binary32,
    /// Exactly 64 hexadecimal characters.
    Hex64,
    /// Any other file: its SHA-256.
    Hashed,
}

impl KeyfileKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::XmlV1 => "XML 1.0",
            Self::XmlV2 => "XML 2.0",
            Self::Binary32 => "32-byte binary",
            Self::Hex64 => "64-character hex",
            Self::Hashed => "SHA-256 of file",
        }
    }
}

/// A keyfile read into key material.
pub struct Keyfile {
    pub kind: KeyfileKind,
    data: Zeroizing<Vec<u8>>,
}

impl fmt::Debug for Keyfile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Keyfile")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl Keyfile {
    /// Read a keyfile as KeePass does: a KeePass XML keyfile, then 32 raw
    /// bytes, then 64 hex characters, otherwise the SHA-256 of the contents.
    pub fn parse(contents: &[u8]) -> Result<Self> {
        if let Some(keyfile) = xml_keyfile(contents)? {
            return Ok(keyfile);
        }
        if contents.len() == 32 {
            return Ok(Self {
                kind: KeyfileKind::Binary32,
                data: Zeroizing::new(contents.to_vec()),
            });
        }
        if contents.len() == 64 && contents.iter().all(u8::is_ascii_hexdigit) {
            if let Some(data) = decode_hex(contents) {
                return Ok(Self {
                    kind: KeyfileKind::Hex64,
                    data,
                });
            }
        }
        Ok(Self {
            kind: KeyfileKind::Hashed,
            data: Zeroizing::new(Sha256::digest(contents).to_vec()),
        })
    }
}

fn decode_hex(text: &[u8]) -> Option<Zeroizing<Vec<u8>>> {
    if text.len() % 2 != 0 {
        return None;
    }
    let digit = |byte: u8| char::from(byte).to_digit(16);
    let mut out = Zeroizing::new(Vec::with_capacity(text.len() / 2));
    for pair in text.chunks_exact(2) {
        let value = digit(pair[0])? << 4 | digit(pair[1])?;
        out.push(value as u8);
    }
    Some(out)
}

/// What a KeePass XML keyfile holds.
#[derive(Default)]
struct XmlKeyfile {
    root: Option<String>,
    version: Option<String>,
    data: Option<Zeroizing<String>>,
    hash: Option<String>,
}

/// Parse `contents` as a KeePass XML keyfile. `None` when it is not XML
/// with a `KeyFile` root, so the caller falls back to the other forms.
fn scan_xml(contents: &[u8]) -> Option<XmlKeyfile> {
    let text = std::str::from_utf8(contents).ok()?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if !text.trim_start().starts_with('<') {
        return None;
    }
    let mut reader = quick_xml::Reader::from_str(text);
    let mut path: Vec<String> = Vec::new();
    let mut found = XmlKeyfile::default();
    loop {
        match reader.read_event().ok()? {
            Event::Start(element) => {
                let name = element.name().as_ref().to_owned();
                if path.is_empty() {
                    found.root = Some(name.clone());
                }
                path.push(name);
                if path == ["KeyFile", "Key", "Data"] {
                    for attribute in element.attributes().flatten() {
                        if attribute.key.as_ref() == "Hash" {
                            let value = attribute
                                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                                .ok()?;
                            found.hash = Some(value.into_owned());
                        }
                    }
                    found.data.get_or_insert_with(Default::default);
                }
            }
            Event::Empty(element) => {
                if path.is_empty() {
                    found.root = Some(element.name().as_ref().to_owned());
                }
            }
            Event::End(_) => {
                path.pop();
            }
            Event::Text(content) => {
                let content = content.xml10_content();
                if path == ["KeyFile", "Meta", "Version"] {
                    found
                        .version
                        .get_or_insert_with(String::new)
                        .push_str(&content);
                } else if path == ["KeyFile", "Key", "Data"] {
                    if let Some(data) = found.data.as_mut() {
                        data.push_str(&content);
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    (found.root.as_deref() == Some("KeyFile")).then_some(found)
}

fn xml_keyfile(contents: &[u8]) -> Result<Option<Keyfile>> {
    let Some(found) = scan_xml(contents) else {
        return Ok(None);
    };
    let version = found.version.unwrap_or_default();
    let version = version.trim();
    let data = found
        .data
        .ok_or_else(|| Error::Keyfile("XML keyfile without Key/Data".into()))?;
    let compact: Zeroizing<String> =
        Zeroizing::new(data.chars().filter(|c| !c.is_whitespace()).collect());
    if version.starts_with("1.0") {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(compact.as_bytes())
            .map_err(|_| Error::Keyfile("XML keyfile 1.0 data is not base64".into()))?;
        return Ok(Some(Keyfile {
            kind: KeyfileKind::XmlV1,
            data: Zeroizing::new(decoded),
        }));
    }
    if version.starts_with("2.0") {
        let decoded = decode_hex(compact.as_bytes())
            .ok_or_else(|| Error::Keyfile("XML keyfile 2.0 data is not hex".into()))?;
        if let Some(hash) = found.hash {
            let stored = decode_hex(hash.trim().as_bytes())
                .ok_or_else(|| Error::Keyfile("XML keyfile 2.0 hash is not hex".into()))?;
            let computed = Sha256::digest(&decoded[..]);
            if stored.len() > computed.len() || stored[..] != computed[..stored.len()] {
                return Err(Error::Keyfile(
                    "XML keyfile 2.0 hash does not match its data".into(),
                ));
            }
        }
        return Ok(Some(Keyfile {
            kind: KeyfileKind::XmlV2,
            data: decoded,
        }));
    }
    Err(Error::Keyfile(format!(
        "XML keyfile version {version:?} is not supported"
    )))
}

/// The credentials that open a database: any combination of a password and a
/// keyfile, or a transformed key that skips the key derivation.
#[derive(Default)]
pub struct Key {
    password: Option<Zeroizing<[u8; 32]>>,
    keyfile: Option<Keyfile>,
    transformed: Option<Zeroizing<[u8; 32]>>,
}

impl fmt::Debug for Key {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Key")
            .field("password", &self.password.is_some())
            .field(
                "keyfile",
                &self.keyfile.as_ref().map(|keyfile| keyfile.kind),
            )
            .field("transformed", &self.transformed.is_some())
            .finish()
    }
}

impl Key {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a password. An empty password is a component of its own, distinct
    /// from no password.
    pub fn with_password(mut self, password: &str) -> Self {
        self.password = Some(Zeroizing::new(Sha256::digest(password.as_bytes()).into()));
        self
    }

    /// Add a keyfile from its contents.
    pub fn with_keyfile(mut self, contents: &[u8]) -> Result<Self> {
        self.keyfile = Some(Keyfile::parse(contents)?);
        Ok(self)
    }

    /// Add a keyfile that is already read.
    pub fn with_parsed_keyfile(mut self, keyfile: Keyfile) -> Self {
        self.keyfile = Some(keyfile);
        self
    }

    /// Open with a transformed key, the output of the database's KDF.
    pub fn from_transformed(transformed: [u8; 32]) -> Self {
        Self {
            transformed: Some(Zeroizing::new(transformed)),
            ..Self::default()
        }
    }

    pub fn has_password(&self) -> bool {
        self.password.is_some()
    }

    pub fn keyfile_kind(&self) -> Option<KeyfileKind> {
        self.keyfile.as_ref().map(|keyfile| keyfile.kind)
    }

    pub fn is_transformed(&self) -> bool {
        self.transformed.is_some()
    }

    pub(crate) fn transformed(&self) -> Option<&[u8; 32]> {
        self.transformed.as_deref()
    }

    /// SHA-256 over the password hash and the keyfile data, as KeePass and
    /// pykeepass build it.
    pub(crate) fn composite(&self) -> Zeroizing<[u8; 32]> {
        let mut hasher = Sha256::new();
        if let Some(password) = &self.password {
            hasher.update(&password[..]);
        }
        if let Some(keyfile) = &self.keyfile {
            hasher.update(&keyfile.data[..]);
        }
        Zeroizing::new(hasher.finalize().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(keyfile: &Keyfile) -> &[u8] {
        &keyfile.data
    }

    #[test]
    fn reads_every_keyfile_form() {
        let v1 = b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<KeyFile>\n\t<Meta>\n\t\t<Version>1.00</Version>\n\t</Meta>\n\t<Key>\n\t\t<Data>AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=</Data>\n\t</Key>\n</KeyFile>\n";
        let keyfile = Keyfile::parse(v1).unwrap();
        assert_eq!(keyfile.kind, KeyfileKind::XmlV1);
        assert_eq!(data(&keyfile), (0..32).collect::<Vec<u8>>());

        let key: Vec<u8> = (0..32).map(|index| index * 7).collect();
        let hash = crate::bytes::hex(&Sha256::digest(&key)[..4]).to_uppercase();
        let hex = crate::bytes::hex(&key).to_uppercase();
        let v2 = format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<KeyFile>\n\t<Meta>\n\t\t<Version>2.0</Version>\n\t</Meta>\n\t<Key>\n\t\t<Data Hash=\"{hash}\">\n\t\t\t{} {}\n\t\t\t{}\n\t\t</Data>\n\t</Key>\n</KeyFile>\n",
            &hex[..8], &hex[8..16], &hex[16..]
        );
        let keyfile = Keyfile::parse(v2.as_bytes()).unwrap();
        assert_eq!(keyfile.kind, KeyfileKind::XmlV2);
        assert_eq!(data(&keyfile), key);

        let tampered = v2.replace(&hash, "00000000");
        assert!(matches!(
            Keyfile::parse(tampered.as_bytes()),
            Err(Error::Keyfile(_))
        ));
        let future = v2.replace("<Version>2.0", "<Version>3.0");
        assert!(matches!(
            Keyfile::parse(future.as_bytes()),
            Err(Error::Keyfile(_))
        ));

        let raw = [0x5a; 32];
        let keyfile = Keyfile::parse(&raw).unwrap();
        assert_eq!(keyfile.kind, KeyfileKind::Binary32);
        assert_eq!(data(&keyfile), raw);

        let hex64 = crate::bytes::hex(&raw);
        let keyfile = Keyfile::parse(hex64.as_bytes()).unwrap();
        assert_eq!(keyfile.kind, KeyfileKind::Hex64);
        assert_eq!(data(&keyfile), raw);

        // XML with another root, and 64 characters that are not all hex,
        // are hashed like any other file.
        for other in [
            &b"<svg><Meta><Version>1.0</Version></Meta></svg>"[..],
            &[b'g'; 64][..],
        ] {
            let keyfile = Keyfile::parse(other).unwrap();
            assert_eq!(keyfile.kind, KeyfileKind::Hashed);
            assert_eq!(data(&keyfile), &Sha256::digest(other)[..]);
        }
    }

    #[test]
    fn composite_key_separates_empty_and_missing_passwords() {
        let none = Key::new().composite();
        let empty = Key::new().with_password("").composite();
        assert_ne!(none[..], empty[..]);
        let expected: [u8; 32] = Sha256::digest(Sha256::digest(b"")).into();
        assert_eq!(empty[..], expected);
    }
}
