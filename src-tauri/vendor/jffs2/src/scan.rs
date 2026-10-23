//! Strict node scan. Damaged headers fail explicitly rather than resynchronizing
//! into an older file tree. Erased flash and dirty zero padding are recognized.
use crate::nodes::{self, ACCURATE, checksum};
use crate::{ByteOrder, DataNode, DirectoryEntry, ReadAt, read_exact};
use anyhow::{Context, Result, bail, ensure};

pub(crate) struct Scan {
    pub order: ByteOrder,
    pub nodes: Vec<DataNode>,
    pub dirents: Vec<DirectoryEntry>,
}
struct Buffer<'a> {
    source: &'a dyn ReadAt,
    offset: u64,
    bytes: Vec<u8>,
}
impl Buffer<'_> {
    fn get(&mut self, offset: u64, length: usize) -> Result<Vec<u8>> {
        ensure!(
            offset
                .checked_add(length as u64)
                .is_some_and(|end| end <= self.source.size()),
            "JFFS2 truncated node"
        );
        if length > 65536 {
            return read_exact(self.source, offset, length);
        }
        if offset < self.offset || offset + length as u64 > self.offset + self.bytes.len() as u64 {
            self.offset = offset;
            self.bytes = read_exact(
                self.source,
                offset,
                (self.source.size() - offset).min(65536) as usize,
            )?;
        }
        let start = (offset - self.offset) as usize;
        Ok(self.bytes[start..start + length].to_vec())
    }
    fn padding(&self, offset: u64, value: u8) -> u64 {
        let start = (offset - self.offset) as usize;
        let length = self.bytes[start..]
            .iter()
            .take_while(|byte| **byte == value)
            .count();
        (length & !3) as u64
    }
}

pub(crate) fn scan(source: &dyn ReadAt, cancelled: &dyn Fn() -> bool) -> Result<Scan> {
    let mut buffer = Buffer {
        source,
        offset: 0,
        bytes: Vec::new(),
    };
    let mut offset = 0u64;
    let mut order = None;
    let mut nodes = Vec::new();
    let mut dirents = Vec::new();
    let mut count = 0usize;
    let mut metadata = 0usize;
    while offset < source.size() {
        ensure!(!cancelled(), "JFFS2 scan cancelled");
        let remaining = source.size() - offset;
        let head = buffer.get(offset, remaining.min(12) as usize)?;
        if head.iter().all(|byte| *byte == 0xff) || head.iter().all(|byte| *byte == 0) {
            if remaining < 12 {
                break;
            }
            offset += buffer.padding(offset, head[0]).max(4);
            continue;
        }
        if head
            .get(..4)
            .is_some_and(|word| word == [0xff; 4] || word == [0; 4])
        {
            offset += 4;
            continue;
        }
        let (node_order, kind, length) =
            nodes::validate_header(&head).with_context(|| format!("JFFS2 node at {offset:#x}"))?;
        ensure!(
            order.is_none_or(|order| order == node_order),
            "JFFS2 mixed byte order"
        );
        order = Some(node_order);
        let end = offset
            .checked_add(u64::from(length))
            .context("JFFS2 node length overflow")?;
        ensure!(end <= source.size(), "JFFS2 node extends past source");
        count += 1;
        ensure!(count <= 1_000_000, "JFFS2 node count exceeds limit");
        if kind & ACCURATE != 0 {
            match kind {
                0xe002 => {
                    ensure!(length >= 68, "JFFS2 truncated inode header");
                    let bytes = buffer.get(offset, 68)?;
                    let node = nodes::inode(&bytes, node_order, length, offset)?;
                    metadata += 256;
                    nodes.push(node);
                }
                0xe001 => {
                    ensure!((41..=294).contains(&length), "JFFS2 invalid dirent size");
                    let bytes = buffer.get(offset, length as usize)?;
                    let entry = nodes::dirent(&bytes, node_order)?;
                    metadata += 128 + entry.name.len();
                    dirents.push(entry);
                }
                0x2003 | 0x2004 => {}
                0x2006 | 0xe008 | 0xe009 => {
                    validate_auxiliary(&mut buffer, offset, length, kind, node_order)?
                }
                unknown if unknown & 0xc000 == 0xc000 => {
                    bail!("unsupported JFFS2 incompatible node {unknown:#x}")
                }
                _ => {}
            }
        }
        ensure!(
            metadata <= 512 * 1024 * 1024,
            "JFFS2 metadata exceeds limit"
        );
        offset = (end.checked_add(3).context("JFFS2 alignment overflow")? & !3).min(source.size());
    }
    ensure!(
        !nodes.is_empty() || !dirents.is_empty(),
        "JFFS2 contains no namespace nodes"
    );
    Ok(Scan {
        order: order.context("JFFS2 header not found")?,
        nodes,
        dirents,
    })
}

fn validate_auxiliary(
    buffer: &mut Buffer<'_>,
    offset: u64,
    length: u32,
    kind: u16,
    order: ByteOrder,
) -> Result<()> {
    ensure!(
        length as usize <= nodes::MAX_DATA,
        "JFFS2 auxiliary node exceeds limit"
    );
    let bytes = buffer.get(offset, length as usize)?;
    match kind {
        0x2006 => {
            ensure!(bytes.len() >= 32, "JFFS2 truncated summary");
            ensure!(
                checksum(&bytes[..24]) == order.u32(&bytes, 28)?,
                "JFFS2 summary node CRC mismatch"
            );
            ensure!(
                checksum(&bytes[32..]) == order.u32(&bytes, 24)?,
                "JFFS2 summary data CRC mismatch"
            );
        }
        0xe008 => {
            ensure!(bytes.len() >= 32, "JFFS2 truncated xattr");
            let name_size = bytes[21] as usize;
            let value_size = order.u16(&bytes, 22)? as usize;
            let data_end = 33 + name_size + value_size;
            ensure!(
                bytes.len() == (data_end + 3) & !3 && bytes[32 + name_size] == 0,
                "JFFS2 xattr length mismatch"
            );
            ensure!(
                checksum(&bytes[..28]) == order.u32(&bytes, 28)?,
                "JFFS2 xattr node CRC mismatch"
            );
            ensure!(
                checksum(&bytes[32..data_end]) == order.u32(&bytes, 24)?,
                "JFFS2 xattr data CRC mismatch"
            );
        }
        0xe009 => {
            ensure!(bytes.len() == 28, "JFFS2 xref length mismatch");
            ensure!(
                checksum(&bytes[..24]) == order.u32(&bytes, 24)?,
                "JFFS2 xref CRC mismatch"
            );
        }
        _ => bail!("unknown JFFS2 auxiliary node {kind:#x}"),
    }
    Ok(())
}
