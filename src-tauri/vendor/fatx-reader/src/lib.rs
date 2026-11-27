//! Bounded Xbox FATX and Xbox 360 XTAF reads.
#![forbid(unsafe_code)]

mod directory;
mod geometry;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet};

use lamfold::{
    BlockSource, DirEntry, FileKind, FoldError, FoldFrontend, LocalFileTimes, Metadata, NodeId,
    Result, SubstrateCtx,
};

use geometry::Geometry;
pub use geometry::{FAT_START, Format, HEADER_BYTES, Header, SECTOR_BYTES};

const ROOT: NodeId = 1;
const PAGE: usize = 4096;

#[derive(Clone, Copy)]
struct Node {
    cluster: u32,
    size: u32,
    kind: FileKind,
    times: LocalFileTimes,
}

pub struct Fatx<S> {
    source: S,
    geometry: Geometry,
    page: [u8; PAGE],
    fat_page: u64,
    nodes: HashMap<NodeId, Node>,
    directories: HashMap<NodeId, Vec<DirEntry>>,
    directory_roots: HashSet<u32>,
    file_chain: Option<(NodeId, Vec<u32>)>,
}

impl<S: BlockSource> Fatx<S> {
    fn header(source: &mut S) -> Result<Option<Header>> {
        if source.len() < HEADER_BYTES as u64 {
            return Ok(None);
        }
        let mut bytes = [0; HEADER_BYTES];
        source.read_at(0, &mut bytes)?;
        Header::inspect(&bytes, source.len())
    }

    fn node(&self, id: NodeId) -> Result<Node> {
        self.nodes.get(&id).copied().ok_or(FoldError::NotFound)
    }

    fn fat_entry(&mut self, cluster: u32) -> Result<u32> {
        let offset = self.geometry.fat_offset(cluster);
        let base = offset / PAGE as u64 * PAGE as u64;
        if self.fat_page != base {
            self.source.read_at(base, &mut self.page)?;
            self.fat_page = base;
        }
        Ok(self
            .geometry
            .decode_entry(&self.page[(offset - base) as usize..]))
    }

    fn next_cluster(&mut self, cluster: u32) -> Result<Option<u32>> {
        let next = self.fat_entry(cluster)?;
        if self.geometry.end_of_chain(next) {
            return Ok(None);
        }
        self.geometry.cluster_offset(next)?;
        Ok(Some(next))
    }

    fn file_clusters(&mut self, node: Node) -> Result<Vec<u32>> {
        let count = u64::from(node.size).div_ceil(self.geometry.cluster_size) as usize;
        let mut cluster = node.cluster;
        let mut result = Vec::new();
        for index in 0..count {
            self.geometry.cluster_offset(cluster)?;
            result.push(cluster);
            let next = self.next_cluster(cluster)?;
            if index + 1 == count {
                if let Some(next) = next {
                    // Extra allocation may follow EOF. Check that the visible
                    // chain has no repeated cluster before ignoring the slack.
                    let mut sorted = result.clone();
                    sorted.sort_unstable();
                    require(
                        sorted.windows(2).all(|pair| pair[0] != pair[1])
                            && sorted.binary_search(&next).is_err(),
                        "FATX circular file chain",
                    )?;
                }
            } else {
                cluster = next.ok_or(FoldError::Corrupt(
                    "FATX file chain ends before logical EOF",
                ))?;
            }
        }
        Ok(result)
    }
}

impl<S: BlockSource> FoldFrontend<S> for Fatx<S> {
    const TAG: &'static str = "fatx";

    fn probe(source: &mut S) -> Result<bool> {
        Ok(Self::header(source)?.is_some())
    }

    fn open(mut source: S, _: &mut SubstrateCtx<'_>) -> Result<Self> {
        let header =
            Self::header(&mut source)?.ok_or(FoldError::Corrupt("FATX signature is absent"))?;
        let geometry = header.geometry(&mut source)?;
        let root = Node {
            cluster: geometry.root,
            size: 0,
            kind: FileKind::Directory,
            times: LocalFileTimes::default(),
        };
        let mut volume = Self {
            source,
            geometry,
            page: [0; PAGE],
            fat_page: 0,
            nodes: HashMap::from([(ROOT, root)]),
            directories: HashMap::new(),
            directory_roots: HashSet::from([geometry.root]),
            file_chain: None,
        };
        let media = volume.fat_entry(0)?;
        require(geometry.valid_media(media), "FATX missing FAT media entry")?;
        volume.next_cluster(geometry.root)?;
        Ok(volume)
    }

    fn root(&self) -> NodeId {
        ROOT
    }

    fn lookup(
        &mut self,
        dir: NodeId,
        name: &str,
        _: &mut SubstrateCtx<'_>,
    ) -> Result<Option<NodeId>> {
        Ok(self
            .entries(dir)?
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(name))
            .map(|entry| entry.node))
    }

    fn read_dir(&mut self, dir: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Vec<DirEntry>> {
        Ok(self.entries(dir)?.to_vec())
    }

    fn metadata(&mut self, id: NodeId, _: &mut SubstrateCtx<'_>) -> Result<Metadata> {
        let node = self.node(id)?;
        Ok(Metadata {
            size: u64::from(node.size),
            kind: node.kind,
            mode: 0,
        })
    }

    fn local_timestamps(&mut self, id: NodeId, _: &mut SubstrateCtx<'_>) -> Result<LocalFileTimes> {
        Ok(self.node(id)?.times)
    }

    fn read_at(
        &mut self,
        id: NodeId,
        offset: u64,
        bytes: &mut [u8],
        _: &mut SubstrateCtx<'_>,
    ) -> Result<usize> {
        let node = self.node(id)?;
        if node.kind == FileKind::Directory {
            return Err(FoldError::IsDirectory);
        }
        let count = u64::from(node.size)
            .saturating_sub(offset)
            .min(bytes.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        if self.file_chain.as_ref().is_none_or(|(key, _)| *key != id) {
            self.file_chain = Some((id, self.file_clusters(node)?));
        }
        let (_, clusters) = self.file_chain.as_ref().unwrap();
        let cluster_size = self.geometry.cluster_size;
        let mut consumed = 0;
        while consumed < count {
            let logical = offset + consumed as u64;
            let index = (logical / cluster_size) as usize;
            let within = logical % cluster_size;
            let start = self.geometry.cluster_offset(clusters[index])? + within;
            let chunk = (count - consumed).min((cluster_size - within) as usize);
            self.source
                .read_at(start, &mut bytes[consumed..consumed + chunk])?;
            consumed += chunk;
        }
        Ok(count)
    }
}

fn require(condition: bool, detail: &'static str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(FoldError::Corrupt(detail))
    }
}
