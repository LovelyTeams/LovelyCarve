//! Bounded XML metadata and unambiguous snapshot-chain selection.
use crate::{invalid, unique_guids};
use quick_xml::events::Event;
use quick_xml::Reader;
use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Guid(pub [u8; 16]);
impl Guid {
    pub const ZERO: Self = Self([0; 16]);
}
impl FromStr for Guid {
    type Err = io::Error;
    fn from_str(value: &str) -> io::Result<Self> {
        let value = value.trim();
        let value = if value.starts_with('{') && value.ends_with('}') {
            &value[1..value.len() - 1]
        } else {
            value
        };
        if value.len() != 36
            || [8, 13, 18, 23]
                .iter()
                .any(|index| value.as_bytes()[*index] != b'-')
        {
            return Err(invalid("invalid PHDI GUID"));
        }
        let digits: String = value
            .chars()
            .filter(|character| *character != '-')
            .collect();
        if digits.len() != 32 || !digits.is_ascii() {
            return Err(invalid("invalid PHDI GUID digits"));
        }
        let mut bytes = [0; 16];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16)
                .map_err(|_| invalid("invalid PHDI GUID hex"))?;
        }
        Ok(Self(bytes))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageType {
    Plain,
    Sparse,
}

#[derive(Clone, Debug)]
pub struct ImageDescriptor {
    pub identifier: Guid,
    pub kind: ImageType,
    pub filename: String,
}
#[derive(Clone, Debug)]
pub struct ExtentDescriptor {
    /// Byte range in the guest disk; XML sectors always use 512 bytes.
    pub offset: u64,
    pub size: u64,
    pub block_size: u64,
    pub images: Vec<ImageDescriptor>,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub identifier: Guid,
    pub parent: Option<Guid>,
}
#[derive(Clone, Debug)]
pub struct Descriptor {
    pub media_size: u64,
    pub logical_sector_size: u32,
    pub physical_sector_size: Option<u32>,
    pub name: Option<String>,
    pub identifier: Option<Guid>,
    pub extents: Vec<ExtentDescriptor>,
    pub snapshots: Vec<Snapshot>,
    pub top_identifier: Option<Guid>,
}

#[derive(Default)]
struct Node {
    name: String,
    text: String,
    children: Vec<Node>,
}
impl Node {
    fn one(&self, name: &str) -> io::Result<Option<&Node>> {
        let mut found = self.children.iter().filter(|child| child.name == name);
        let first = found.next();
        if found.next().is_some() {
            return Err(invalid(format!("duplicate PHDI XML element {name}")));
        }
        Ok(first)
    }
    fn required(&self, name: &str) -> io::Result<&Node> {
        self.one(name)?
            .ok_or_else(|| invalid(format!("missing PHDI XML element {name}")))
    }
    fn value(&self) -> io::Result<&str> {
        if !self.children.is_empty() {
            return Err(invalid("nested XML where scalar was expected"));
        }
        Ok(self.text.trim())
    }
    fn number(&self) -> io::Result<u64> {
        self.value()?
            .parse()
            .map_err(|_| invalid(format!("invalid PHDI numeric field {}", self.name)))
    }
    fn guid(&self) -> io::Result<Guid> {
        self.value()?.parse()
    }
    fn sectors(&self) -> io::Result<u64> {
        self.number()?
            .checked_mul(512)
            .ok_or_else(|| invalid("PHDI sector arithmetic overflow"))
    }
}

impl Descriptor {
    /// Parses UTF-8 XML. DTD/entity declarations and ambiguous fields fail.
    pub fn parse(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(invalid("PHDI XML exceeds 8 MiB"));
        }
        let text =
            std::str::from_utf8(bytes).map_err(|_| invalid("PHDI descriptor must be UTF-8"))?;
        let mut reader = Reader::from_str(text.trim_start_matches('\u{feff}'));
        let mut stack = vec![Node::default()];
        let mut count = 0;
        loop {
            let event = reader
                .read_event()
                .map_err(|error| invalid(format!("PHDI XML: {error}")))?;
            let empty = matches!(&event, Event::Empty(_));
            match event {
                Event::Start(start) | Event::Empty(start) => {
                    count += 1;
                    if count > 32768 || stack.len() > 64 {
                        return Err(invalid("PHDI XML node/depth limit exceeded"));
                    }
                    let name = start.name().as_ref().to_owned();
                    for attribute in start.attributes() {
                        let attribute = attribute.map_err(|error| invalid(error.to_string()))?;
                        if name == "Parallels_disk_image"
                            && attribute.key.as_ref() == "Version"
                            && attribute.value.as_ref() != "1.0"
                        {
                            return Err(invalid("unsupported PHDI descriptor version"));
                        }
                    }
                    let node = Node {
                        name,
                        ..Node::default()
                    };
                    if empty {
                        stack
                            .last_mut()
                            .ok_or_else(|| invalid("XML stack empty"))?
                            .children
                            .push(node);
                    } else {
                        stack.push(node);
                    }
                }
                Event::End(_) => {
                    if stack.len() < 2 {
                        return Err(invalid("unexpected XML closing tag"));
                    }
                    let node = stack.pop().ok_or_else(|| invalid("XML stack empty"))?;
                    stack
                        .last_mut()
                        .ok_or_else(|| invalid("XML stack empty"))?
                        .children
                        .push(node);
                }
                Event::Text(value) => {
                    let decoded = value.xml_content(quick_xml::XmlVersion::Implicit1_0);
                    stack
                        .last_mut()
                        .ok_or_else(|| invalid("XML stack empty"))?
                        .text
                        .push_str(&decoded);
                }
                Event::GeneralRef(reference) => {
                    let reference = reference.as_ref();
                    let value = quick_xml::escape::unescape(&format!("&{reference};"))
                        .map_err(|error| invalid(error.to_string()))?
                        .into_owned();
                    stack
                        .last_mut()
                        .ok_or_else(|| invalid("XML stack empty"))?
                        .text
                        .push_str(&value);
                }
                Event::CData(value) => {
                    stack
                        .last_mut()
                        .ok_or_else(|| invalid("XML stack empty"))?
                        .text
                        .push_str(&value.xml_content(quick_xml::XmlVersion::Implicit1_0));
                }
                Event::DocType(_) => return Err(invalid("PHDI XML DTD is unsupported")),
                Event::Decl(declaration) => {
                    if let Some(encoding) = declaration.encoding() {
                        let encoding = encoding.map_err(|error| invalid(error.to_string()))?;
                        if !encoding.eq_ignore_ascii_case("utf-8") {
                            return Err(invalid("PHDI XML encoding must be UTF-8"));
                        }
                    }
                }
                Event::Comment(_) | Event::PI(_) => {}
                Event::Eof => break,
            }
        }
        if stack.len() != 1 {
            return Err(invalid("incomplete PHDI XML"));
        }
        let document = stack.pop().ok_or_else(|| invalid("missing XML document"))?;
        if document.children.len() != 1 || !document.text.trim().is_empty() {
            return Err(invalid("PHDI XML must have one root"));
        }
        let root = document.required("Parallels_disk_image")?;
        let parameters = root.required("Disk_Parameters")?;
        let media_size = parameters.required("Disk_size")?.sectors()?;
        if media_size == 0 {
            return Err(invalid("PHDI disk size is zero"));
        }
        let sector_size = |name| -> io::Result<Option<u32>> {
            parameters
                .one(name)?
                .map(|node| {
                    let size = node.number()?;
                    if !matches!(size, 512 | 1024 | 2048 | 4096) {
                        return Err(invalid("unsupported PHDI sector size"));
                    }
                    Ok(size as u32)
                })
                .transpose()
        };
        if let Some(encryption) = parameters.one("Encryption")? {
            if encryption
                .one("Engine")?
                .map(Node::guid)
                .transpose()?
                .is_some_and(|identifier| identifier != Guid::ZERO)
                || encryption
                    .one("Data")?
                    .map(Node::value)
                    .transpose()?
                    .is_some_and(|data| !data.is_empty())
            {
                return Err(invalid("encrypted Parallels images are unsupported"));
            }
        }
        let mut extents = Vec::new();
        for storage in &root.required("StorageData")?.children {
            if storage.name != "Storage" {
                return Err(invalid("unsupported PHDI storage layout"));
            }
            let offset = storage.required("Start")?.sectors()?;
            let end = storage.required("End")?.sectors()?;
            let size = end
                .checked_sub(offset)
                .filter(|size| *size != 0)
                .ok_or_else(|| invalid("invalid PHDI extent range"))?;
            let block_size = storage.required("Blocksize")?.sectors()?;
            if block_size == 0 {
                return Err(invalid("PHDI block size is zero"));
            }
            let mut images = Vec::new();
            for image in storage
                .children
                .iter()
                .filter(|child| child.name == "Image")
            {
                let kind = match image.required("Type")?.value()? {
                    "Plain" => ImageType::Plain,
                    "Compressed" => ImageType::Sparse,
                    _ => return Err(invalid("unsupported PHDI image type")),
                };
                let filename = image.required("File")?.value()?.to_owned();
                if filename.is_empty() {
                    return Err(invalid("empty PHDI extent filename"));
                }
                images.push(ImageDescriptor {
                    identifier: image.required("GUID")?.guid()?,
                    kind,
                    filename,
                });
            }
            if images.is_empty() {
                return Err(invalid("PHDI extent has no images"));
            }
            unique_guids(images.iter().map(|image| image.identifier))?;
            extents.push(ExtentDescriptor {
                offset,
                size,
                block_size,
                images,
            });
        }
        extents.sort_by_key(|extent| extent.offset);
        let mut covered = 0;
        for extent in &extents {
            if extent.offset != covered {
                return Err(invalid("PHDI extents overlap or have a coverage gap"));
            }
            covered = covered
                .checked_add(extent.size)
                .ok_or_else(|| invalid("PHDI extent size overflow"))?;
        }
        if covered != media_size {
            return Err(invalid("PHDI extents do not cover declared disk size"));
        }
        let mut snapshots = Vec::new();
        let snapshot_node = root.one("Snapshots")?;
        if let Some(node) = snapshot_node {
            for shot in node.children.iter().filter(|child| child.name == "Shot") {
                let parent = shot
                    .one("ParentGUID")?
                    .map(Node::guid)
                    .transpose()?
                    .filter(|identifier| *identifier != Guid::ZERO);
                snapshots.push(Snapshot {
                    identifier: shot.required("GUID")?.guid()?,
                    parent,
                });
            }
        }
        unique_guids(snapshots.iter().map(|snapshot| snapshot.identifier))?;
        let descriptor = Self {
            media_size,
            logical_sector_size: sector_size("LogicSectorSize")?.unwrap_or(512),
            physical_sector_size: sector_size("PhysicalSectorSize")?,
            name: parameters
                .one("Name")?
                .map(Node::value)
                .transpose()?
                .map(str::to_owned),
            identifier: parameters.one("UID")?.map(Node::guid).transpose()?,
            extents,
            snapshots,
            top_identifier: snapshot_node
                .map(|node| node.one("TopGUID"))
                .transpose()?
                .flatten()
                .map(Node::guid)
                .transpose()?,
        };
        // Validate every branch, including inactive snapshots.
        for snapshot in &descriptor.snapshots {
            descriptor.chain(Some(snapshot.identifier), 128)?;
        }
        descriptor.chain(None, 128)?;
        Ok(descriptor)
    }

    pub(crate) fn standalone(media_size: u64, block_size: u64, path: &Path) -> Self {
        Self {
            media_size,
            logical_sector_size: 512,
            physical_sector_size: None,
            name: None,
            identifier: None,
            extents: vec![ExtentDescriptor {
                offset: 0,
                size: media_size,
                block_size,
                images: vec![ImageDescriptor {
                    identifier: Guid::ZERO,
                    kind: ImageType::Sparse,
                    filename: path.to_string_lossy().into_owned(),
                }],
            }],
            snapshots: Vec::new(),
            top_identifier: None,
        }
    }

    pub(crate) fn chain(
        &self,
        requested: Option<Guid>,
        maximum_depth: usize,
    ) -> io::Result<Vec<Guid>> {
        if self.snapshots.is_empty() {
            if requested.is_some()
                || self.top_identifier.is_some()
                || self.extents.iter().any(|extent| extent.images.len() != 1)
            {
                return Err(invalid(
                    "PHDI image selection is ambiguous without snapshots",
                ));
            }
            let identifier = self
                .extents
                .first()
                .ok_or_else(|| invalid("PHDI has no extents"))?
                .images[0]
                .identifier;
            if self
                .extents
                .iter()
                .any(|extent| extent.images[0].identifier != identifier)
            {
                return Err(invalid("PHDI split extents disagree on image GUID"));
            }
            return Ok(vec![identifier]);
        }
        // libphdi chooses the first Shot; honor TopGUID when explicitly present.
        let mut current = Some(
            requested
                .or(self.top_identifier)
                .unwrap_or(self.snapshots[0].identifier),
        );
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        while let Some(identifier) = current {
            if !seen.insert(identifier) || chain.len() >= maximum_depth {
                return Err(invalid(
                    "PHDI parent chain has a cycle or exceeds its limit",
                ));
            }
            let snapshot = self
                .snapshots
                .iter()
                .find(|snapshot| snapshot.identifier == identifier)
                .ok_or_else(|| invalid("PHDI parent/snapshot GUID is missing"))?;
            chain.push(identifier);
            current = snapshot.parent;
        }
        Ok(chain)
    }
}
