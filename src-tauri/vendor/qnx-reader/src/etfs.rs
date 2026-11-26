use std::collections::{BTreeMap, HashMap};

use crate::{exact, kind, name, require, Endian, Info};
use lamfold::{BlockSource, DirEntry, FileKind, FileTimes, FoldError, Metadata, NodeId, Result};

const HEADER: usize = 16;
const RECORD: usize = 64;
const ROOT: u16 = 0;
const FILE_TABLE: u16 = 1;
const FILE_LIMIT: usize = 32768;
const PAGE_LIMIT: u64 = 16 * 1024 * 1024;
const MAPPED_LIMIT: usize = 4 * 1024 * 1024;
const PROBE_WINDOW: u64 = 64 * 1024;
const CLUSTERS: [usize; 6] = [1024, 2048, 4096, 512, 8192, 16384];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EtfsLayout {
    pub cluster_size: usize,
    pub endian: Endian,
}

#[derive(Clone, Copy)]
struct Transaction {
    fid: u16,
    cluster: u32,
    count: u16,
    sequence: u32,
    readable: bool,
}

impl Transaction {
    fn parse(bytes: &[u8], endian: Endian) -> Result<Option<Self>> {
        let id = endian.u32(bytes, 0)?;
        let state = bytes[10] & 15;
        if id >= FILE_LIMIT as u32 || state > 1 || bytes.iter().all(|&byte| byte == 0) {
            return Ok(None);
        }
        let count = endian.u16(bytes, 8)?;
        let cluster = endian.u32(bytes, 4)?;
        require(
            count != 0 && cluster.checked_add(u32::from(count)).is_some(),
            "invalid QNX ETFS transaction span",
        )?;
        Ok(Some(Self {
            fid: id as u16,
            cluster,
            count,
            readable: state == 0 && bytes[11] & 15 == 0,
            sequence: endian.u32(bytes, 12)?,
        }))
    }
}

impl EtfsLayout {
    fn stride(self) -> u64 {
        (self.cluster_size + HEADER) as u64
    }

    /// Probe complete pages in small windows near the start and end of the source.
    pub fn locate(
        length: u64,
        mut read: impl FnMut(u64, &mut [u8]) -> Result<()>,
    ) -> Result<Option<Self>> {
        if length < 512 + HEADER as u64 {
            return Ok(None);
        }
        let head_len = length.min(PROBE_WINDOW) as usize;
        let mut head = vec![0; head_len];
        read(0, &mut head)?;
        let tail_at = length
            .saturating_sub(PROBE_WINDOW)
            .max(head_len as u64)
            .min(length);
        let mut tail = vec![0; (length - tail_at) as usize];
        if !tail.is_empty() {
            read(tail_at, &mut tail)?;
        }
        let mut found = None;
        for cluster_size in CLUSTERS {
            for endian in [Endian::Little, Endian::Big] {
                let layout = Self {
                    cluster_size,
                    endian,
                };
                if layout.matches(0, &head) || layout.matches(tail_at, &tail) {
                    require(found.is_none(), "ambiguous QNX ETFS cluster geometry")?;
                    found = Some(layout);
                }
            }
        }
        Ok(found)
    }

    fn matches(self, offset: u64, raw: &[u8]) -> bool {
        let stride = self.stride();
        let first = ((stride - offset % stride) % stride) as usize;
        raw.get(first..).is_some_and(|aligned| {
            aligned.chunks_exact(stride as usize).any(|page| {
                let header = &page[self.cluster_size..];
                let Ok(Some(tx)) = Transaction::parse(header, self.endian) else {
                    return false;
                };
                tx.fid == FILE_TABLE
                    && tx.cluster == 0
                    && tx.readable
                    && table_size(&page[..2 * RECORD], self.endian).is_ok()
            })
        })
    }
}

fn table_size(raw: &[u8], endian: Endian) -> Result<usize> {
    require(raw.len() >= 2 * RECORD, "short QNX ETFS file-table anchor")?;
    let number = |at| endian.u16(raw, at);
    let word = |at| endian.u32(raw, at);
    require(
        number(0)? == 0
            && number(2)? == ROOT
            && kind(word(4)? as u16) == FileKind::Directory
            && raw[32] == 0
            && word(28)? == 0,
        "invalid QNX ETFS root record",
    )?;
    let length = word(RECORD + 28)? as usize;
    require(
        number(RECORD)? == 0
            && number(RECORD + 2)? == ROOT
            && kind(word(RECORD + 4)? as u16) == FileKind::Regular
            && raw[RECORD + 32..].starts_with(b".filetable\0")
            && (2 * RECORD..=FILE_LIMIT * RECORD).contains(&length)
            && length.is_multiple_of(RECORD),
        "invalid QNX ETFS file-table record",
    )?;
    Ok(length)
}

#[derive(Clone, Copy)]
struct Page {
    source: u64,
    sequence: u32,
    readable: bool,
}

type Map = HashMap<(u16, u32), Page>;

fn scan(source: &mut impl BlockSource, layout: EtfsLayout) -> Result<Map> {
    let length = source.len();
    let stride = layout.stride();
    let pages = length / stride;
    let trailing = length % stride;
    require(pages <= PAGE_LIMIT, "QNX ETFS page limit")?;
    if trailing != 0 {
        let mut bytes = vec![0; trailing as usize];
        exact(source, pages * stride, &mut bytes)?;
        require(
            trailing < 512 && bytes.iter().all(|byte| matches!(*byte, 0 | 255)),
            "truncated QNX ETFS page",
        )?;
    }
    let batch = (1024 * 1024 / stride).max(1);
    let mut map = Map::new();
    let mut buffer = vec![0; (batch * stride) as usize];
    let mut pending = Vec::<(Transaction, Page)>::new();
    let mut index = 0;
    while index < pages {
        let count = batch.min(pages - index);
        exact(
            source,
            index * stride,
            &mut buffer[..(count * stride) as usize],
        )?;
        for (relative, page) in buffer[..(count * stride) as usize]
            .chunks_exact(stride as usize)
            .enumerate()
        {
            let Some(tx) = Transaction::parse(&page[layout.cluster_size..], layout.endian)? else {
                pending.clear();
                continue;
            };
            if let Some((previous, _)) = pending.last() {
                if previous.fid != tx.fid
                    || previous.sequence != tx.sequence
                    || previous.cluster + 1 != tx.cluster
                    || previous.count.checked_sub(1) != Some(tx.count)
                {
                    pending.clear();
                }
            }
            let page = Page {
                source: (index + relative as u64) * stride,
                sequence: tx.sequence,
                readable: tx.readable,
            };
            pending.push((tx, page));
            if tx.count == 1 {
                for (record, page) in pending.drain(..) {
                    let key = (record.fid, record.cluster);
                    // Equal sequences can be rewritten in-place in a newer physical copy.
                    if map
                        .get(&key)
                        .is_none_or(|older| older.sequence <= page.sequence)
                    {
                        map.insert(key, page);
                    }
                }
                require(map.len() <= MAPPED_LIMIT, "QNX ETFS mapped page limit")?;
            }
        }
        index += count;
    }
    Ok(map)
}

struct File {
    parent: u16,
    name: String,
    info: Info,
}

pub struct Etfs<S> {
    source: S,
    layout: EtfsLayout,
    map: Map,
    files: Vec<Option<File>>,
    children: HashMap<u16, BTreeMap<String, u16>>,
}

impl<S: BlockSource> Etfs<S> {
    pub(super) fn probe_source(source: &mut S) -> Result<bool> {
        let length = source.len();
        Ok(EtfsLayout::locate(length, |at, bytes| exact(source, at, bytes))?.is_some())
    }

    pub(super) fn open_source(mut source: S) -> Result<Self> {
        let length = source.len();
        let layout = EtfsLayout::locate(length, |at, bytes| exact(&mut source, at, bytes))?
            .ok_or(FoldError::Corrupt("QNX ETFS file-table anchor missing"))?;
        let map = scan(&mut source, layout)?;
        let mut anchor = [0; 2 * RECORD];
        read_file(&mut source, layout, &map, FILE_TABLE, 0, &mut anchor)?;
        let mut table = vec![0; table_size(&anchor, layout.endian)?];
        read_file(&mut source, layout, &map, FILE_TABLE, 0, &mut table)?;
        let (files, children) = decode_table(&table, layout.endian)?;
        Ok(Self {
            source,
            layout,
            map,
            files,
            children,
        })
    }

    pub(super) fn root_node(&self) -> NodeId {
        ROOT as NodeId
    }

    pub(super) fn info(&self, node: NodeId) -> Result<Info> {
        let id = usize::try_from(node).map_err(|_| FoldError::NotFound)?;
        self.files
            .get(id)
            .and_then(Option::as_ref)
            .map(|file| file.info.clone())
            .ok_or(FoldError::NotFound)
    }

    pub(super) fn directory(&self, node: NodeId) -> Result<Vec<DirEntry>> {
        if self.info(node)?.metadata.kind != FileKind::Directory {
            return Err(FoldError::NotDirectory);
        }
        self.children
            .get(&(node as u16))
            .into_iter()
            .flatten()
            .map(|(name, &id)| {
                let node = NodeId::from(id);
                Ok(DirEntry {
                    name: name.clone(),
                    node,
                    kind: self.info(node)?.metadata.kind,
                })
            })
            .collect()
    }

    pub(super) fn content(&mut self, node: NodeId, offset: u64, bytes: &mut [u8]) -> Result<usize> {
        let size = self.info(node)?.metadata.size;
        let count = size.saturating_sub(offset).min(bytes.len() as u64) as usize;
        read_file(
            &mut self.source,
            self.layout,
            &self.map,
            node as u16,
            offset,
            &mut bytes[..count],
        )?;
        Ok(count)
    }
}

fn read_file(
    source: &mut impl BlockSource,
    layout: EtfsLayout,
    map: &Map,
    fid: u16,
    offset: u64,
    bytes: &mut [u8],
) -> Result<()> {
    let cluster = layout.cluster_size as u64;
    let mut done = 0;
    while done < bytes.len() {
        let at = offset + done as u64;
        let number = u32::try_from(at / cluster)
            .map_err(|_| FoldError::Corrupt("QNX ETFS cluster number overflow"))?;
        let within = (at % cluster) as usize;
        let count = (layout.cluster_size - within).min(bytes.len() - done);
        let page = map
            .get(&(fid, number))
            .ok_or(FoldError::Corrupt("missing QNX ETFS data cluster"))?;
        require(
            page.readable,
            "QNX ETFS data cluster records a flash read error",
        )?;
        exact(
            source,
            page.source + within as u64,
            &mut bytes[done..done + count],
        )?;
        done += count;
    }
    Ok(())
}

type Children = HashMap<u16, BTreeMap<String, u16>>;

fn decode_table(table: &[u8], order: Endian) -> Result<(Vec<Option<File>>, Children)> {
    let entries = table.as_chunks::<RECORD>().0;
    let mut files = Vec::with_capacity(entries.len());
    let fragment = |bytes: &[u8]| {
        bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len())
    };
    for (id, raw) in entries.iter().enumerate() {
        let extension = order.u16(raw, 0)?;
        let parent = order.u16(raw, 2)?;
        if parent == u16::MAX || matches!(extension, 0x8000 | u16::MAX) {
            files.push(None);
            continue;
        }
        let mode = order.u32(raw, 4)?;
        let kind = kind(mode as u16);
        let size = u64::from(order.u32(raw, 28)?);
        let times = FileTimes {
            accessed: Some((i64::from(order.u32(raw, 16)?), 0)),
            modified: Some((i64::from(order.u32(raw, 20)?), 0)),
            changed: Some((i64::from(order.u32(raw, 24)?), 0)),
            created: None,
        };
        let short = &raw[32..];
        let mut full = short[..fragment(short)].to_vec();
        if extension != 0 {
            let long = entries
                .get(extension as usize)
                .ok_or(FoldError::Corrupt("missing QNX ETFS file-name extension"))?;
            require(
                order.u16(long, 0)? == 0x8000
                    && usize::from(order.u16(long, 2)?) == id
                    && long[63] == 0,
                "invalid QNX ETFS file-name extension",
            )?;
            let suffix = &long[4..63];
            full.extend_from_slice(&suffix[..fragment(suffix)]);
        }
        let name = if id == 0 { String::new() } else { name(&full)? };
        require(
            !matches!(name.as_str(), "." | ".."),
            "invalid QNX ETFS path component",
        )?;
        files.push(Some(File {
            parent,
            name,
            info: Info {
                metadata: Metadata { kind, mode, size },
                times,
            },
        }));
    }
    let mut children = Children::new();
    for (id, file) in files.iter().enumerate().skip(1) {
        let Some(file) = file else {
            continue;
        };
        let parent = files.get(file.parent as usize).and_then(Option::as_ref);
        require(
            parent.is_some_and(|directory| directory.info.metadata.kind == FileKind::Directory),
            "missing QNX ETFS parent directory",
        )?;
        require(
            children
                .entry(file.parent)
                .or_default()
                .insert(file.name.clone(), id as u16)
                .is_none(),
            "duplicate QNX ETFS directory name",
        )?;
    }
    let mut resolved = vec![0_u8; files.len()];
    resolved[0] = 2;
    let mut path = Vec::new();
    for (id, file) in files.iter().enumerate().skip(1) {
        if file.is_none() || resolved[id] == 2 {
            continue;
        }
        let mut cursor = id;
        while resolved[cursor] == 0 {
            resolved[cursor] = 1;
            path.push(cursor);
            cursor = files[cursor]
                .as_ref()
                .ok_or(FoldError::Corrupt("missing QNX ETFS ancestor"))?
                .parent as usize;
        }
        require(
            resolved[cursor] == 2,
            "cycle in QNX ETFS directory ancestry",
        )?;
        for node in path.drain(..) {
            resolved[node] = 2;
        }
    }
    Ok((files, children))
}
