use std::collections::{BTreeMap, HashMap, HashSet};

use crate::{exact, kind, name, require, Endian, Info};
use lamfold::{BlockSource, DirEntry, FileKind, FileTimes, FoldError, Metadata, NodeId, Result};

const MAGIC: &[u8; 8] = b"QSSL_F3S";
const BOOT: usize = 24;
const UNIT_INFO: usize = 16;
const EXTENT: usize = 32;
const FILE: u32 = 0x300;
const DIRECTORY: u32 = 0x200;
const SYSTEM: u32 = 0x100;
const TYPE: u32 = 0x300;
const NO_NEXT: u32 = 2;
const NO_SUPER: u32 = 4;
const LAST: u32 = 128;
const EXTENT_LIMIT: usize = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct Pointer {
    unit: u16,
    index: u16,
}

impl Pointer {
    fn parse(raw: &[u8], order: Endian, at: usize) -> Result<Self> {
        Ok(Self {
            unit: order.u16(raw, at)?,
            index: order.u16(raw, at + 2)?,
        })
    }

    fn node(self) -> NodeId {
        (u64::from(self.unit) << 16) | u64::from(self.index)
    }

    fn from_node(node: NodeId) -> Result<Self> {
        let packed = u32::try_from(node).map_err(|_| FoldError::NotFound)?;
        Ok(Self {
            unit: (packed >> 16) as u16,
            index: packed as u16,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EfsLayout {
    pub endian: Endian,
    pub unit_size: u64,
    pub units: u16,
    pub spares: u16,
    alignment: u32,
    root: Pointer,
}

impl EfsLayout {
    pub fn locate(
        length: u64,
        mut read: impl FnMut(u64, &mut [u8]) -> Result<()>,
    ) -> Result<Option<Self>> {
        if length < 512 {
            return Ok(None);
        }
        let mut prefix = [0; UNIT_INFO];
        read(0, &mut prefix)?;
        let Some((endian, unit_size)) = unit_info(&prefix)? else {
            return Ok(None);
        };
        let mut raw = vec![0; unit_size.min(4096) as usize];
        for physical in 0..(length / unit_size).min(u16::MAX as u64) {
            let start = physical * unit_size;
            read(start, &mut raw)?;
            if unit_info(&raw)? != Some((endian, unit_size)) {
                continue;
            }
            for (position, record) in raw.windows(BOOT).enumerate() {
                if &record[4..12] != MAGIC
                    || record[2..4] != [3, 0]
                    || endian.u16(record, 0)? != BOOT as u16
                {
                    continue;
                }
                let units = endian.u16(record, 14)?;
                let spares = endian.u16(record, 16)?;
                let alignment = u32::from(endian.u16(record, 18)?);
                let root = Pointer::parse(record, endian, 20)?;
                if u64::from(endian.u16(record, 12)?) != physical
                    || physical >= u64::from(units)
                    || spares >= units
                    || alignment > unit_size.trailing_zeros()
                    || root.unit == 0
                    || root.unit > units - spares
                {
                    continue;
                }
                let mut extent = [0; EXTENT];
                read(start + unit_size - 3 * EXTENT as u64, &mut extent)?;
                let bootstrap = Extent::parse(&extent, endian, alignment)?;
                if bootstrap.kind() != SYSTEM
                    || !bootstrap.live()
                    || bootstrap.offset != position as u64
                    || usize::from(bootstrap.size) != BOOT
                {
                    continue;
                }
                require(
                    unit_size * u64::from(units) <= length,
                    "truncated QNX EFS volume",
                )?;
                return Ok(Some(Self {
                    endian,
                    unit_size,
                    units,
                    spares,
                    alignment,
                    root,
                }));
            }
        }
        Ok(None)
    }
}

fn unit_info(raw: &[u8]) -> Result<Option<(Endian, u64)>> {
    if raw.len() < UNIT_INFO || raw[3] != 255 || raw[6..8] != [255, 255] {
        return Ok(None);
    }
    let order = match raw[2] {
        b'L' => Endian::Little,
        b'B' => Endian::Big,
        _ => return Ok(None),
    };
    let shift = order.u16(raw, 4)?;
    if order.u16(raw, 0)? != UNIT_INFO as u16 || !(9..=30).contains(&shift) {
        return Ok(None);
    }
    Ok(Some((order, 1_u64 << shift)))
}

#[derive(Clone, Copy)]
struct Extent {
    status: u32,
    consistent: bool,
    offset: u64,
    size: u16,
    next: Pointer,
    superseded_by: Pointer,
}

impl Extent {
    fn parse(raw: &[u8], order: Endian, alignment: u32) -> Result<Self> {
        let status = order.u32(raw, 0)?;
        let offset = ((u64::from(raw[19]) << 16) | u64::from(order.u16(raw, 20)?)) << alignment;
        Ok(Self {
            status,
            consistent: order.u32(raw, 4)? == status
                && order.u32(raw, 8)? == status
                && raw[18] == 255,
            offset,
            size: order.u16(raw, 22)?,
            next: Pointer::parse(raw, order, 24)?,
            superseded_by: Pointer::parse(raw, order, 28)?,
        })
    }

    fn kind(self) -> u32 {
        self.status & TYPE
    }

    fn live(self) -> bool {
        self.consistent && self.status & 0x71 == 0x30
    }
}

struct Unit {
    base: u64,
    body_end: u64,
    extents: Vec<Extent>,
}

impl Unit {
    fn read(
        source: &mut impl BlockSource,
        layout: EfsLayout,
        physical: u16,
        max: usize,
    ) -> Result<(Self, Option<u16>)> {
        let base = u64::from(physical) * layout.unit_size;
        let mut extents = Vec::new();
        let mut bytes = [0; 4096];
        let mut end = layout.unit_size;
        let mut done = false;
        while end != 0 && !done {
            let count = end.min(bytes.len() as u64);
            let start = end - count;
            exact(source, base + start, &mut bytes[..count as usize])?;
            for raw in bytes[..count as usize].as_chunks::<EXTENT>().0.iter().rev() {
                require(
                    extents.len() < max && extents.len() <= u16::MAX as usize,
                    "QNX EFS extent limit",
                )?;
                let extent = Extent::parse(raw, layout.endian, layout.alignment)?;
                extents.push(extent);
                if extent.status & LAST != 0 {
                    done = true;
                    break;
                }
            }
            end = start;
        }
        require(done, "QNX EFS unit has no terminal extent header")?;
        let body_end = layout.unit_size - (extents.len() * EXTENT) as u64;
        let unit = Self {
            base,
            body_end,
            extents,
        };
        let mut header = [0; UNIT_INFO];
        let Some(first) = unit.extents.first() else {
            return Err(FoldError::Corrupt("empty QNX EFS unit"));
        };
        require(
            first.kind() == SYSTEM
                && first.live()
                && first.offset == 0
                && usize::from(first.size) == UNIT_INFO,
            "invalid QNX EFS unit header extent",
        )?;
        exact(source, unit.data(*first)?, &mut header)?;
        require(
            unit_info(&header)? == Some((layout.endian, layout.unit_size)),
            "inconsistent QNX EFS unit geometry",
        )?;
        let Some(logical) = unit
            .extents
            .get(1)
            .filter(|extent| extent.status != u32::MAX)
        else {
            return Ok((unit, None));
        };
        require(
            logical.kind() == SYSTEM && logical.live() && logical.size >= 24,
            "invalid QNX EFS logical unit mapping",
        )?;
        let mut raw = [0; 4];
        exact(source, unit.data(*logical)?, &mut raw)?;
        require(
            layout.endian.u16(&raw, 0)? == logical.size,
            "invalid QNX EFS logical unit size",
        )?;
        let id = layout.endian.u16(&raw, 2)?;
        Ok((unit, Some(id)))
    }

    fn data(&self, extent: Extent) -> Result<u64> {
        require(
            extent.offset + u64::from(extent.size) <= self.body_end,
            "QNX EFS data extent overlaps unit headers",
        )?;
        Ok(self.base + extent.offset)
    }
}

struct Node {
    name: String,
    info: Info,
    first: Pointer,
    spans: Vec<(u64, u64, u64)>,
}

pub struct Efs<S> {
    source: S,
    layout: EfsLayout,
    units: BTreeMap<u16, Unit>,
    nodes: HashMap<NodeId, Node>,
}

impl<S: BlockSource> Efs<S> {
    pub(super) fn probe_source(source: &mut S) -> Result<bool> {
        let length = source.len();
        Ok(EfsLayout::locate(length, |offset, raw| exact(source, offset, raw))?.is_some())
    }

    pub(super) fn open_source(mut source: S) -> Result<Self> {
        let length = source.len();
        let layout = EfsLayout::locate(length, |offset, raw| exact(&mut source, offset, raw))?
            .ok_or(FoldError::Corrupt("QNX EFS boot record missing"))?;
        let mut units = BTreeMap::new();
        let mut remaining = EXTENT_LIMIT;
        for physical in 0..layout.units {
            let (unit, id) = Unit::read(&mut source, layout, physical, remaining)?;
            remaining -= unit.extents.len();
            if let Some(id) = id {
                require(
                    (1..=layout.units - layout.spares).contains(&id)
                        && units.insert(id, unit).is_none(),
                    "duplicate or invalid QNX EFS logical unit",
                )?;
            }
        }
        require(
            units.len() == usize::from(layout.units - layout.spares),
            "incomplete QNX EFS logical units",
        )?;
        let mut reader = Self {
            source,
            layout,
            units,
            nodes: HashMap::new(),
        };
        let root = reader.root_node();
        require(
            reader.info(root)?.metadata.kind == FileKind::Directory,
            "invalid QNX EFS root entry",
        )?;
        Ok(reader)
    }

    pub(super) fn root_node(&self) -> NodeId {
        self.layout.root.node()
    }

    fn resolve(&self, mut pointer: Pointer) -> Result<(Pointer, &Unit, Extent)> {
        let mut path = HashSet::new();
        loop {
            require(
                path.len() < 65536 && path.insert(pointer),
                "QNX EFS supersession cycle or depth limit",
            )?;
            let unit = self
                .units
                .get(&pointer.unit)
                .ok_or(FoldError::Corrupt("missing QNX EFS extent unit"))?;
            let ext = *unit
                .extents
                .get(pointer.index as usize)
                .ok_or(FoldError::Corrupt("missing QNX EFS extent index"))?;
            require(ext.consistent, "inconsistent QNX EFS extent status copies")?;
            if ext.status & NO_SUPER != 0 {
                require(ext.live(), "inactive QNX EFS extent")?;
                return Ok((pointer, unit, ext));
            }
            pointer = ext.superseded_by;
        }
    }

    fn chain(&self, mut pointer: Pointer, kind: u32) -> Result<Vec<(Pointer, u64, u64)>> {
        let mut chain = Vec::new();
        let mut visited = HashSet::new();
        loop {
            let (id, unit, ext) = self.resolve(pointer)?;
            require(
                visited.len() < EXTENT_LIMIT && visited.insert(id),
                "cyclic or excessive QNX EFS extent chain",
            )?;
            require(
                ext.kind() == kind || (ext.size == 0 && ext.status & NO_NEXT != 0),
                "wrong QNX EFS extent type for file or directory",
            )?;
            chain.push((id, unit.data(ext)?, u64::from(ext.size)));
            if ext.status & NO_NEXT != 0 {
                return Ok(chain);
            }
            pointer = ext.next;
        }
    }

    fn node(&mut self, id: NodeId) -> Result<&Node> {
        if !self.nodes.contains_key(&id) {
            let (at, len) = {
                let (_, unit, extent) = self.resolve(Pointer::from_node(id)?)?;
                require(
                    extent.kind() == DIRECTORY && (8..=4096).contains(&extent.size),
                    "invalid QNX EFS directory entry extent",
                )?;
                (unit.data(extent)?, extent.size)
            };
            let mut raw = vec![0; len as usize];
            exact(&mut self.source, at, &mut raw)?;
            let endian = self.layout.endian;
            let name_size = usize::from(raw[3]);
            let stats = 8 + name_size.div_ceil(4) * 4;
            require(
                endian.u16(&raw, 0)? == 8 && name_size != 0 && stats + 20 <= raw.len(),
                "invalid QNX EFS directory entry layout",
            )?;
            let recorded_stats = usize::from(endian.u16(&raw, stats)?);
            require(
                recorded_stats >= 20 && stats + recorded_stats <= raw.len(),
                "invalid QNX EFS stat record length",
            )?;
            let first = Pointer::parse(&raw, endian, 4)?;
            let mode = endian.u16(&raw, stats + 2)?;
            let kind = kind(mode);
            let n = &raw[8..8 + name_size];
            require(n.last() == Some(&0), "unterminated QNX EFS stored name")?;
            let name = if id == self.root_node() {
                String::new()
            } else {
                name(&n[..n.len() - 1])?
            };
            require(
                !matches!(name.as_str(), "." | ".."),
                "invalid QNX EFS path component",
            )?;
            let times = FileTimes {
                modified: Some((i64::from(endian.u32(&raw, stats + 12)?), 0)),
                changed: Some((i64::from(endian.u32(&raw, stats + 16)?), 0)),
                accessed: None,
                created: None,
            };
            let mut size = 0_u64;
            let mut spans = Vec::new();
            if matches!(kind, FileKind::Regular | FileKind::Symlink) {
                for (_, source, length) in self.chain(first, FILE)? {
                    if length != 0 {
                        spans.push((size, source, length));
                        size += length;
                    }
                }
            }
            if self.nodes.len() == 65536 {
                self.nodes.clear();
            }
            self.nodes.insert(
                id,
                Node {
                    name,
                    info: Info {
                        metadata: Metadata {
                            kind,
                            mode: u32::from(mode),
                            size,
                        },
                        times,
                    },
                    first,
                    spans,
                },
            );
        }
        Ok(&self.nodes[&id])
    }

    pub(super) fn info(&mut self, node: NodeId) -> Result<Info> {
        Ok(self.node(node)?.info.clone())
    }

    pub(super) fn directory(&mut self, node: NodeId) -> Result<Vec<DirEntry>> {
        let directory = self.node(node)?;
        if directory.info.metadata.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        let first = directory.first;
        let mut entries = Vec::new();
        let mut names = HashSet::new();
        for (id, _, length) in self.chain(first, DIRECTORY)? {
            if length == 0 {
                break;
            }
            let child = id.node();
            require(child != node, "cyclic QNX EFS directory entry")?;
            let object = self.node(child)?;
            require(
                names.insert(object.name.clone()),
                "duplicate QNX EFS directory name",
            )?;
            entries.push(DirEntry {
                name: object.name.clone(),
                node: child,
                kind: object.info.metadata.kind,
            });
        }
        Ok(entries)
    }

    pub(super) fn content(&mut self, node: NodeId, offset: u64, bytes: &mut [u8]) -> Result<usize> {
        self.node(node)?;
        let node = &self.nodes[&node];
        let count = node
            .info
            .metadata
            .size
            .saturating_sub(offset)
            .min(bytes.len() as u64) as usize;
        let end = offset + count as u64;
        let index = node
            .spans
            .partition_point(|&(start, _, length)| start + length <= offset);
        for &(start, source, length) in &node.spans[index..] {
            if start >= end {
                break;
            }
            let from = start.max(offset);
            let to = (start + length).min(end);
            exact(
                &mut self.source,
                source + (from - start),
                &mut bytes[(from - offset) as usize..(to - offset) as usize],
            )?;
        }
        Ok(count)
    }
}
