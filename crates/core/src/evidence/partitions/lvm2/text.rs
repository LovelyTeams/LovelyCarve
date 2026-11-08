//! Bounded LVM2 text grammar and format metadata, independent of host paths.
use std::collections::{BTreeMap, HashSet};

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeGroup {
    pub name: String,
    pub identifier: String,
    pub sequence_number: u64,
    /// LVM units are always 512-byte sectors, including on 4Kn disks.
    pub extent_size_sectors: u64,
    pub physical_volumes: Vec<PhysicalVolume>,
    pub logical_volumes: Vec<LogicalVolume>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalVolume {
    pub name: String,
    pub identifier: String,
    pub device: Option<String>,
    pub size_sectors: u64,
    pub pe_start_sectors: u64,
    pub extent_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogicalVolume {
    pub name: String,
    pub identifier: String,
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub start_extent: u64,
    pub extent_count: u64,
    pub kind: String,
    pub stripe_count: u64,
    pub stripe_size_sectors: Option<u64>,
    pub stripes: Vec<Stripe>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stripe {
    pub physical_volume: String,
    pub physical_extent: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    Word(String),
    String(String),
    Array(Vec<Value>),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Block {
    fields: BTreeMap<String, Value>,
    /// Preserve metadata declaration order: LV indexes are persisted node IDs.
    names: Vec<String>,
}

impl std::ops::Deref for Block {
    type Target = BTreeMap<String, Value>;
    fn deref(&self) -> &Self::Target {
        &self.fields
    }
}

impl Block {
    fn keys(&self) -> impl Iterator<Item = &String> {
        self.names.iter()
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
    values: usize,
}

impl Parser<'_> {
    fn whitespace(&mut self) {
        loop {
            while self
                .bytes
                .get(self.position)
                .is_some_and(u8::is_ascii_whitespace)
            {
                self.position += 1;
            }
            if self.bytes.get(self.position) != Some(&b'#') {
                break;
            }
            while self
                .bytes
                .get(self.position)
                .is_some_and(|byte| *byte != b'\n')
            {
                self.position += 1;
            }
        }
    }

    fn word(&mut self) -> Result<String> {
        self.whitespace();
        let start = self.position;
        while self.bytes.get(self.position).is_some_and(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'+')
        }) {
            self.position += 1;
        }
        if start == self.position {
            bail!("LVM expected identifier at byte {start}");
        }
        Ok(std::str::from_utf8(&self.bytes[start..self.position])?.to_owned())
    }

    fn value(&mut self, depth: usize) -> Result<Value> {
        self.whitespace();
        self.values += 1;
        if depth >= 32 || self.values > 500_000 {
            bail!("LVM metadata grammar limit exceeded");
        }
        match self.bytes.get(self.position).copied() {
            Some(b'{') => {
                self.position += 1;
                Ok(Value::Block(self.block(depth + 1, true)?))
            }
            Some(b'[') => {
                self.position += 1;
                let mut values = Vec::new();
                loop {
                    self.whitespace();
                    if self.bytes.get(self.position) == Some(&b']') {
                        self.position += 1;
                        break;
                    }
                    values.push(self.value(depth + 1)?);
                    self.whitespace();
                    match self.bytes.get(self.position) {
                        Some(b',') => self.position += 1,
                        Some(b']') => {}
                        _ => bail!("LVM expected array separator"),
                    }
                }
                Ok(Value::Array(values))
            }
            Some(b'"') => {
                self.position += 1;
                let mut output = String::new();
                loop {
                    let byte = self
                        .bytes
                        .get(self.position)
                        .copied()
                        .context("LVM unterminated string")?;
                    self.position += 1;
                    match byte {
                        b'"' => break,
                        b'\\' => {
                            let escaped = self
                                .bytes
                                .get(self.position)
                                .copied()
                                .context("LVM incomplete escape")?;
                            if !matches!(escaped, b'"' | b'\\') {
                                bail!("LVM unsupported string escape");
                            }
                            self.position += 1;
                            output.push(char::from(escaped));
                        }
                        0x20..=0x7e => output.push(char::from(byte)),
                        _ => bail!("LVM non-ASCII or control byte in string"),
                    }
                }
                Ok(Value::String(output))
            }
            Some(_) => Ok(Value::Word(self.word()?)),
            None => bail!("LVM missing value"),
        }
    }

    fn block(&mut self, depth: usize, nested: bool) -> Result<Block> {
        let mut entries = Block::default();
        loop {
            self.whitespace();
            match self.bytes.get(self.position) {
                None if !nested => return Ok(entries),
                None => bail!("LVM unterminated section"),
                Some(b'}') if nested => {
                    self.position += 1;
                    return Ok(entries);
                }
                _ => {}
            }
            let name = self.word()?;
            self.whitespace();
            if self.bytes.get(self.position) == Some(&b'=') {
                self.position += 1;
            } else if self.bytes.get(self.position) != Some(&b'{') {
                bail!("LVM missing assignment for {name}");
            }
            let value = self.value(depth)?;
            if entries.fields.insert(name.clone(), value).is_some() {
                bail!("LVM duplicate key {name}");
            }
            entries.names.push(name);
        }
    }
}

fn number(block: &Block, key: &str) -> Result<u64> {
    match block.get(key) {
        Some(Value::Word(value)) => value.parse().with_context(|| format!("LVM invalid {key}")),
        _ => bail!("LVM missing numeric {key}"),
    }
}

fn string(block: &Block, key: &str) -> Result<String> {
    match block.get(key) {
        Some(Value::String(value)) => Ok(value.clone()),
        _ => bail!("LVM missing string {key}"),
    }
}

fn identifier(block: &Block) -> Result<String> {
    let identifier = string(block, "id")?;
    let compact = identifier.replace('-', "");
    if compact.len() != 32 || !compact.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        bail!("LVM invalid volume identifier");
    }
    Ok(identifier)
}

fn section<'a>(block: &'a Block, key: &str) -> Result<&'a Block> {
    match block.get(key) {
        Some(Value::Block(value)) => Ok(value),
        _ => bail!("LVM missing section {key}"),
    }
}

/// Parse a single committed metadata copy. Old ring contents are not candidates.
pub fn parse_volume_group(text: &[u8]) -> Result<VolumeGroup> {
    if text.len() > 16 * 1024 * 1024 {
        bail!("LVM text exceeds 16 MiB limit");
    }
    let text = text.strip_suffix(&[0]).unwrap_or(text);
    let mut parser = Parser {
        bytes: text,
        position: 0,
        values: 0,
    };
    let root = parser.block(0, false)?;
    if root.contains_key("version") && number(&root, "version")? != 1 {
        bail!("LVM unsupported text format version");
    }
    let groups: Vec<_> = root.iter().filter(|(_, value)|
        matches!(value, Value::Block(block) if block.contains_key("physical_volumes"))).collect();
    if groups.len() != 1 {
        bail!("LVM expected exactly one volume group");
    }
    let (name, _) = groups[0];
    let block = section(&root, name)?;
    let extent_size_sectors = number(block, "extent_size")?;
    if extent_size_sectors == 0 {
        bail!("LVM zero extent size");
    }
    let mut physical_volumes = Vec::new();
    let mut identifiers = HashSet::new();
    for name in section(block, "physical_volumes")?.keys() {
        let pv = section(section(block, "physical_volumes")?, name)?;
        let identifier = identifier(pv)?;
        let compact = identifier.replace('-', "");
        if !identifiers.insert(compact) {
            bail!("LVM duplicate physical volume UUID");
        }
        physical_volumes.push(PhysicalVolume {
            name: name.clone(),
            identifier,
            device: pv
                .contains_key("device")
                .then(|| string(pv, "device"))
                .transpose()?,
            size_sectors: number(pv, "dev_size")?,
            pe_start_sectors: number(pv, "pe_start")?,
            extent_count: number(pv, "pe_count")?,
        });
    }
    let mut logical_volumes = Vec::new();
    if block.contains_key("logical_volumes") {
        let volumes = section(block, "logical_volumes")?;
        let mut logical_identifiers = HashSet::new();
        for name in volumes.keys() {
            let lv = section(volumes, name)?;
            let identifier = identifier(lv)?;
            if !logical_identifiers.insert(identifier.replace('-', "")) {
                bail!("LVM duplicate logical volume UUID");
            }
            let mut segments = Vec::new();
            for segment_name in lv
                .keys()
                .filter(|key| key.starts_with("segment") && key[7..].parse::<u64>().is_ok())
            {
                let segment = section(lv, segment_name)?;
                let kind = string(segment, "type")?;
                let stripe_count = if segment.contains_key("stripe_count") {
                    number(segment, "stripe_count")?
                } else {
                    0
                };
                let mut stripes = Vec::new();
                if let Some(Value::Array(values)) = segment.get("stripes") {
                    if values.len() % 2 != 0 {
                        bail!("LVM odd stripes array");
                    }
                    for pair in values.as_chunks::<2>().0 {
                        match (&pair[0], &pair[1]) {
                            (Value::String(pv), Value::Word(extent)) => stripes.push(Stripe {
                                physical_volume: pv.clone(),
                                physical_extent: extent
                                    .parse()
                                    .context("LVM invalid stripe extent")?,
                            }),
                            _ => bail!("LVM invalid stripe pair"),
                        }
                    }
                }
                if kind == "striped" && (stripe_count == 0 || stripe_count != stripes.len() as u64)
                {
                    bail!("LVM stripe_count does not match stripes");
                }
                segments.push(Segment {
                    start_extent: number(segment, "start_extent")?,
                    extent_count: number(segment, "extent_count")?,
                    kind,
                    stripe_count,
                    stripe_size_sectors: segment
                        .contains_key("stripe_size")
                        .then(|| number(segment, "stripe_size"))
                        .transpose()?,
                    stripes,
                });
            }
            if number(lv, "segment_count")? != segments.len() as u64 || segments.is_empty() {
                bail!("LVM {name} segment_count mismatch");
            }
            segments.sort_by_key(|segment| segment.start_extent);
            let mut end = 0u64;
            for segment in &segments {
                if segment.start_extent != end || segment.extent_count == 0 {
                    bail!("LVM {name} segment gap, overlap or empty range");
                }
                end = end
                    .checked_add(segment.extent_count)
                    .context("LVM extent count overflow")?;
            }
            logical_volumes.push(LogicalVolume {
                name: name.clone(),
                identifier,
                segments,
            });
        }
    }
    Ok(VolumeGroup {
        name: name.clone(),
        identifier: identifier(block)?,
        sequence_number: number(block, "seqno")?,
        extent_size_sectors,
        physical_volumes,
        logical_volumes,
    })
}
