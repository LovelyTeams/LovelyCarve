use crate::{
    Chunk, FileType, Geometry, Node, Object, ReadAt, cancelled, exact, tags::Tags,
    wire::parse_header,
};
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, btree_map::Entry};
use tokio_util::sync::CancellationToken;

const MAX_PAGES: u64 = 4_000_000;
const MAX_METADATA: usize = 256 * 1024 * 1024;
#[derive(Default)]
struct Pending {
    header: Option<Node>,
    chunks: BTreeMap<u32, Chunk>,
    boundary: Option<u64>,
    newer_end: u64,
    shadowed: bool,
}
pub(crate) fn scan(
    source: &dyn ReadAt,
    geometry: Geometry,
    token: &CancellationToken,
) -> Result<(BTreeMap<u32, Object>, usize)> {
    cancelled(token)?;
    ensure!(
        source.size() > 0 && source.size().is_multiple_of(geometry.stride() as u64),
        "YAFFS2 source ends within a NAND page/spare"
    );
    let pages = source.size() / geometry.stride() as u64;
    ensure!(pages <= MAX_PAGES, "YAFFS2 page limit exceeded");
    let mut records = Vec::new();
    let mut memory = 0usize;
    for number in 0..pages {
        cancelled(token)?;
        let offset = number * geometry.stride() as u64;
        let spare = exact(
            source,
            offset + geometry.page_size as u64,
            geometry.spare_size,
        )?;
        if let Some(tags) = Tags::parse(&spare, geometry)
            .with_context(|| format!("YAFFS2 tags at page {number}"))?
        {
            if tags.sequence == 0x21 || tags.object == 0x10 {
                continue;
            }
            memory = memory
                .checked_add(192)
                .context("YAFFS2 metadata accounting overflow")?;
            ensure!(memory <= MAX_METADATA, "YAFFS2 metadata budget exceeded");
            records
                .try_reserve(1)
                .context("YAFFS2 metadata allocation")?;
            records.push((tags, offset));
        }
    }
    records.sort_unstable_by_key(|(tags, offset)| std::cmp::Reverse((tags.sequence, *offset)));
    let mut pending: BTreeMap<u32, Pending> = BTreeMap::new();
    for (tags, offset) in records {
        cancelled(token)?;
        if let Entry::Vacant(entry) = pending.entry(tags.object) {
            memory = memory
                .checked_add(2048)
                .context("YAFFS2 metadata accounting overflow")?;
            ensure!(memory <= MAX_METADATA, "YAFFS2 metadata budget exceeded");
            entry.insert(Pending::default());
        }
        let state = pending
            .get_mut(&tags.object)
            .context("YAFFS2 object state missing")?;
        if state.shadowed {
            continue;
        }
        if tags.chunk != 0 {
            if state
                .header
                .as_ref()
                .is_some_and(|node| node.kind != FileType::File)
            {
                continue;
            }
            let start = u64::from(tags.chunk - 1) * geometry.page_size as u64;
            let end = (start + u64::from(tags.count)).min(state.boundary.unwrap_or(u64::MAX));
            if end <= start {
                continue;
            }
            if let Entry::Vacant(entry) = state.chunks.entry(tags.chunk) {
                entry.insert(Chunk {
                    offset,
                    length: (end - start) as u32,
                });
                if state.header.is_none() {
                    state.newer_end = state.newer_end.max(end);
                }
            }
            continue;
        }
        let header = exact(source, offset, 512)?;
        let mut node = parse_header(&header, geometry.order, tags.object)
            .with_context(|| format!("YAFFS2 object header at {offset:#x}"))?;
        ensure!(
            node.size <= geometry.page_size as u64 * 0x0fff_ffff,
            "YAFFS2 file exceeds chunk address space"
        );
        tags.validate_header(&node)?;
        if state.header.is_some() {
            if node.kind != FileType::File || node.hidden() {
                state.boundary = Some(0);
            } else if node.shrink {
                state.boundary = Some(state.boundary.unwrap_or(u64::MAX).min(node.size));
            }
            continue;
        }
        let declared_size = node.size;
        if node.kind == FileType::File {
            node.size = node.size.max(state.newer_end);
            state.boundary = Some(if node.hidden() { 0 } else { declared_size });
        } else {
            ensure!(
                state.chunks.is_empty(),
                "YAFFS2 non-file has newer data chunks"
            );
        }
        let shadow = node.shadow;
        state.header = Some(node);
        if shadow != 0 {
            if let Entry::Vacant(entry) = pending.entry(shadow) {
                memory = memory
                    .checked_add(2048)
                    .context("YAFFS2 metadata accounting overflow")?;
                ensure!(memory <= MAX_METADATA, "YAFFS2 metadata budget exceeded");
                entry.insert(Pending::default());
            }
            let target = pending
                .get_mut(&shadow)
                .context("YAFFS2 shadow state missing")?;
            if target.header.is_none() {
                target.shadowed = true;
                target.chunks.clear();
            }
        }
    }
    let mut objects = BTreeMap::new();
    let mut orphans = 0;
    for (number, state) in pending {
        cancelled(token)?;
        if state.shadowed {
            continue;
        }
        if let Some(node) = state.header {
            objects.insert(
                number,
                Object {
                    node,
                    chunks: state.chunks,
                },
            );
        } else if !state.chunks.is_empty() {
            orphans += 1;
        }
    }
    complete(&mut objects)?;
    Ok((objects, orphans))
}
pub(crate) fn complete(objects: &mut BTreeMap<u32, Object>) -> Result<()> {
    objects.entry(1).or_insert_with(|| Object {
        node: Node::pseudo(1, ""),
        chunks: BTreeMap::new(),
    });
    ensure!(
        objects.get(&1).context("YAFFS2 root missing")?.node.kind == FileType::Directory,
        "YAFFS2 root is not a directory"
    );
    if objects.values().any(|object| object.node.parent_id == 2) {
        objects.entry(2).or_insert_with(|| Object {
            node: Node::pseudo(2, "lost+found"),
            chunks: BTreeMap::new(),
        });
    }
    Ok(())
}
