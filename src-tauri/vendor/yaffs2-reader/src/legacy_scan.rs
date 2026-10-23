//! Forward YAFFS1 scanning; two-bit serial successors choose duplicate pages.
use crate::{
    Chunk, FileType, Geometry, Node, Object, ReadAt, cancelled, exact,
    legacy_tags::{self, Page},
    wire::parse_header,
};
use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};
use tokio_util::sync::CancellationToken;

/// YAFFS1 has no on-media flag for its mount-time file-size policy. Header is
/// the default for image browsing; DataPages reproduces the original scanner's
/// data-derived policy when the source configuration is independently known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileSizeSource {
    Header,
    DataPages,
}
#[derive(Default)]
struct Pending {
    header: Option<(u8, Node)>,
    chunks: BTreeMap<u32, (u8, Chunk)>,
}
pub(crate) fn scan(
    source: &dyn ReadAt,
    geometry: Geometry,
    policy: FileSizeSource,
    token: &CancellationToken,
) -> Result<(BTreeMap<u32, Object>, usize)> {
    cancelled(token)?;
    ensure!(
        source.size() > 0 && source.size().is_multiple_of(528),
        "YAFFS1 source ends within a page/spare"
    );
    let pages = source.size() / 528;
    ensure!(pages <= 4_000_000, "YAFFS1 page limit exceeded");
    let mut pending: BTreeMap<u32, Pending> = BTreeMap::new();
    let mut shadows = BTreeSet::new();
    let mut memory = 0usize;
    let mut erased = false;
    for number in 0..pages {
        cancelled(token)?;
        if number % 32 == 0 {
            erased = false;
        }
        let offset = number * 528;
        let spare = exact(source, offset + 512, 16)?;
        let page = legacy_tags::parse(&spare, geometry.order)
            .with_context(|| format!("YAFFS1 tags at page {number}"))?;
        let tags = match page {
            Page::Erased => {
                erased = true;
                continue;
            }
            Page::Deleted => {
                ensure!(
                    !erased,
                    "YAFFS1 programmed page follows erased page within a block"
                );
                continue;
            }
            Page::Used(tags) => tags,
        };
        ensure!(
            !erased,
            "YAFFS1 programmed page follows erased page within a block"
        );
        if let Entry::Vacant(entry) = pending.entry(tags.object) {
            memory = memory
                .checked_add(2048)
                .context("YAFFS1 metadata accounting overflow")?;
            ensure!(
                memory <= 256 * 1024 * 1024,
                "YAFFS1 metadata budget exceeded"
            );
            entry.insert(Pending::default());
        }
        let state = pending
            .get_mut(&tags.object)
            .context("YAFFS1 missing object state")?;
        if tags.chunk != 0 {
            if state
                .header
                .as_ref()
                .is_some_and(|(_, node)| node.kind != FileType::File)
            {
                continue;
            }
            let replace = match state.chunks.get(&tags.chunk) {
                Some((serial, _)) => (serial + 1) & 3 == tags.serial,
                None => {
                    memory = memory
                        .checked_add(192)
                        .context("YAFFS1 metadata accounting overflow")?;
                    ensure!(
                        memory <= 256 * 1024 * 1024,
                        "YAFFS1 metadata budget exceeded"
                    );
                    true
                }
            };
            if replace {
                state.chunks.insert(
                    tags.chunk,
                    (
                        tags.serial,
                        Chunk {
                            offset,
                            length: tags.count,
                        },
                    ),
                );
            }
            continue;
        }
        let mut bytes = exact(source, offset, 512)?;
        bytes.extend_from_slice(&spare);
        legacy_tags::verify_page(&bytes)
            .with_context(|| format!("YAFFS1 header at {offset:#x}"))?;
        let node = parse_header(&bytes[..512], geometry.order, tags.object)?;
        ensure!(
            node.size <= 512 * 0xfffffu64 || node.kind != FileType::File,
            "YAFFS1 file exceeds chunk address space"
        );
        if node.shadow != 0 {
            shadows.insert(node.shadow);
        }
        if state
            .header
            .as_ref()
            .is_some_and(|(_, previous)| previous.kind != node.kind)
        {
            *state = Pending::default();
        }
        let replace = state
            .header
            .as_ref()
            .is_none_or(|(serial, _)| (serial + 1) & 3 == tags.serial);
        if replace {
            if node.kind != FileType::File {
                state.chunks.clear();
            }
            state.header = Some((tags.serial, node));
        }
    }
    let mut objects = BTreeMap::new();
    let mut orphans = 0;
    for (number, state) in pending {
        cancelled(token)?;
        if shadows.contains(&number) {
            continue;
        }
        if let Some((_, mut node)) = state.header {
            let chunks: BTreeMap<_, _> = state
                .chunks
                .into_iter()
                .map(|(number, (_, chunk))| (number, chunk))
                .collect();
            if node.kind == FileType::File && policy == FileSizeSource::DataPages {
                node.size = chunks
                    .iter()
                    .map(|(&number, chunk)| u64::from(number - 1) * 512 + u64::from(chunk.length))
                    .max()
                    .unwrap_or(0);
            }
            objects.insert(number, Object { node, chunks });
        } else if !state.chunks.is_empty() {
            orphans += 1;
        }
    }
    crate::scan::complete(&mut objects)?;
    Ok((objects, orphans))
}
