//! Bounded CoreFoundation XML property lists, including ID/IDREF references.
use crate::FileVaultError;
use base64::{engine::general_purpose::STANDARD, Engine};
use quick_xml::{
    events::{BytesStart, Event},
    Reader,
};
use std::collections::BTreeMap;

#[derive(Clone)]
pub(crate) enum Value {
    Dictionary(BTreeMap<String, Value>),
    Array(Vec<Value>),
    String(String),
    Integer(u64),
    Data(Vec<u8>),
    Boolean,
}
impl Value {
    fn allocation_size(&self) -> usize {
        match self {
            Self::Dictionary(values) => values
                .iter()
                .map(|(key, value)| key.len() + value.allocation_size() + 64)
                .sum(),
            Self::Array(values) => values
                .iter()
                .map(|value| value.allocation_size() + 32)
                .sum(),
            Self::String(value) => value.len(),
            Self::Data(value) => value.len(),
            Self::Integer(_) | Self::Boolean => 16,
        }
    }
    pub(crate) fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Dictionary(values) => values.get(key),
            Self::Array(_) | Self::String(_) | Self::Integer(_) | Self::Data(_) | Self::Boolean => {
                None
            }
        }
    }
    pub(crate) fn string(&self) -> Option<&str> {
        if let Self::String(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub(crate) fn integer(&self) -> Option<u64> {
        if let Self::Integer(value) = self {
            Some(*value)
        } else {
            None
        }
    }
    pub(crate) fn data(&self) -> Option<&[u8]> {
        if let Self::Data(value) = self {
            Some(value)
        } else {
            None
        }
    }
    pub(crate) fn array(&self) -> &[Self] {
        if let Self::Array(values) = self {
            values
        } else {
            &[]
        }
    }
}
fn invalid() -> FileVaultError {
    FileVaultError::OutOfRange {
        what: "invalid or oversized CoreFoundation plist",
    }
}
struct Decoder<'a> {
    reader: Reader<&'a [u8]>,
    references: BTreeMap<String, Value>,
    nodes: usize,
    allocations: usize,
}
impl Decoder<'_> {
    fn next(&mut self) -> Result<Event<'static>, FileVaultError> {
        loop {
            let event = self
                .reader
                .read_event()
                .map_err(|_| invalid())?
                .into_owned();
            match event {
                Event::Decl(_) | Event::Comment(_) | Event::DocType(_) | Event::PI(_) => {}
                Event::Text(ref text)
                    if text.as_ref().as_bytes().iter().all(u8::is_ascii_whitespace) => {}
                _ => return Ok(event),
            }
        }
    }
    fn attribute(tag: &BytesStart<'_>, name: &str) -> Result<Option<String>, FileVaultError> {
        for attribute in tag.attributes() {
            let attribute = attribute.map_err(|_| invalid())?;
            if attribute.key.as_ref() == name {
                return Ok(Some(
                    attribute
                        .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .map_err(|_| invalid())?
                        .into_owned(),
                ));
            }
        }
        Ok(None)
    }
    fn value(&mut self, event: Event<'_>, depth: usize) -> Result<Value, FileVaultError> {
        self.nodes += 1;
        if depth > 64 || self.nodes > 100_000 {
            return Err(invalid());
        }
        let (tag, empty) = match event {
            Event::Start(tag) => (tag, false),
            Event::Empty(tag) => (tag, true),
            _ => return Err(invalid()),
        };
        if let Some(reference) = Self::attribute(&tag, "IDREF")? {
            if !empty {
                return Err(invalid());
            }
            let value = self.references.get(&reference).ok_or_else(invalid)?;
            self.allocations = self
                .allocations
                .checked_add(value.allocation_size())
                .ok_or_else(invalid)?;
            if self.allocations > 32 * 1024 * 1024 {
                return Err(invalid());
            }
            return Ok(value.clone());
        }
        let name = tag.name();
        let value = match name.as_ref() {
            "plist" => {
                let event = self.next()?;
                let value = self.value(event, depth + 1)?;
                match self.next()? {
                    Event::End(end) if end.name() == name => value,
                    _ => return Err(invalid()),
                }
            }
            "dict" => {
                let mut values = BTreeMap::new();
                if !empty {
                    loop {
                        let event = self.next()?;
                        match event {
                            Event::End(end) if end.name() == name => break,
                            Event::Start(key) if key.name().as_ref() == "key" => {
                                let text =
                                    self.reader.read_text(key.name()).map_err(|_| invalid())?;
                                let key = quick_xml::escape::unescape(&text)
                                    .map_err(|_| invalid())?
                                    .into_owned();
                                let event = self.next()?;
                                let value = self.value(event, depth + 1)?;
                                if values.insert(key, value).is_some() {
                                    return Err(invalid());
                                }
                            }
                            _ => return Err(invalid()),
                        }
                    }
                }
                Value::Dictionary(values)
            }
            "array" => {
                let mut values = Vec::new();
                if !empty {
                    loop {
                        let event = self.next()?;
                        if let Event::End(ref end) = event {
                            if end.name() == name {
                                break;
                            }
                        }
                        values.push(self.value(event, depth + 1)?);
                    }
                }
                Value::Array(values)
            }
            "string" | "data" | "integer" => {
                let text = if empty {
                    String::new()
                } else {
                    let text = self.reader.read_text(name).map_err(|_| invalid())?;
                    quick_xml::escape::unescape(&text)
                        .map_err(|_| invalid())?
                        .into_owned()
                };
                match name.as_ref() {
                    "string" => Value::String(text),
                    "data" => Value::Data(
                        STANDARD
                            .decode(
                                text.bytes()
                                    .filter(|byte| !byte.is_ascii_whitespace())
                                    .collect::<Vec<_>>(),
                            )
                            .map_err(|_| invalid())?,
                    ),
                    "integer" => {
                        Value::Integer(if let Some(hex) = text.trim().strip_prefix("0x") {
                            u64::from_str_radix(hex, 16).map_err(|_| invalid())?
                        } else {
                            text.trim().parse().map_err(|_| invalid())?
                        })
                    }
                    _ => return Err(invalid()),
                }
            }
            "true" | "false" if empty => Value::Boolean,
            _ => return Err(invalid()),
        };
        if let Some(identifier) = Self::attribute(&tag, "ID")? {
            self.allocations = self
                .allocations
                .checked_add(value.allocation_size())
                .ok_or_else(invalid)?;
            if self.allocations > 32 * 1024 * 1024 {
                return Err(invalid());
            }
            if self.references.insert(identifier, value.clone()).is_some() {
                return Err(invalid());
            }
        }
        Ok(value)
    }
}
pub(crate) fn parse(bytes: &[u8]) -> Result<Value, FileVaultError> {
    if bytes.len() > 16 * 1024 * 1024 {
        return Err(invalid());
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid())?
        .trim_end_matches('\0');
    let mut decoder = Decoder {
        reader: Reader::from_str(text),
        references: BTreeMap::new(),
        nodes: 0,
        allocations: 0,
    };
    let first = decoder.next()?;
    let value = decoder.value(first, 0)?;
    if !matches!(decoder.next()?, Event::Eof) {
        return Err(invalid());
    }
    Ok(value)
}
