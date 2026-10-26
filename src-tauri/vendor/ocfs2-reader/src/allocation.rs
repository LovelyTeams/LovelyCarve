//! Global cluster allocation and per-slot block suballocation. The first
//! cluster group's bitmap starts at cluster zero, not at its descriptor.
use crate::{
    extents::{Root, Tree},
    header::{Header, Record},
    inode::Inode,
    le16, le32, le64, require,
};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub(crate) enum Key {
    GlobalInode,
    Inode(u16),
    Extent(u16),
}
#[derive(Clone, Copy)]
enum PoolKind {
    Clusters,
    GlobalInodes,
    Slot,
}
struct Segment {
    bit: u64,
    blocks: u64,
    physical: u64,
}
struct Group {
    address: u64,
    bits: u64,
    bitmap: Vec<u8>,
    segments: Vec<Segment>,
    cluster: u64,
}
impl Group {
    fn set(&self, bit: u64) -> bool {
        bit < self.bits
            && self
                .bitmap
                .get((bit / 8) as usize)
                .is_some_and(|byte| byte & (1 << (bit % 8)) != 0)
    }
}
pub(crate) struct Allocation {
    clusters: Vec<Group>,
    pools: HashMap<Key, HashMap<u64, Group>>,
    reserved: Vec<(u64, u64)>,
}
impl Allocation {
    pub fn load(
        source: &mut impl BlockSource,
        header: &Header,
        global: &Inode,
        pools: Vec<(Key, Inode)>,
    ) -> Result<Self> {
        let mut seen = HashSet::new();
        let mut bytes = 0;
        let mut clusters = Self::groups(
            source,
            header,
            global,
            PoolKind::Clusters,
            &mut seen,
            &mut bytes,
        )?;
        clusters.sort_by_key(|group| group.cluster);
        let mut end = 0;
        for group in &clusters {
            require(group.cluster == end, "ocfs2: cluster bitmap gap/overlap")?;
            end += group.bits;
        }
        require(
            end == header.clusters as u64,
            "ocfs2: global bitmap/volume cluster count",
        )?;
        let mut pool_map = HashMap::new();
        for (key, inode) in pools {
            let groups = Self::groups(
                source,
                header,
                &inode,
                if matches!(key, Key::GlobalInode) {
                    PoolKind::GlobalInodes
                } else {
                    PoolKind::Slot
                },
                &mut seen,
                &mut bytes,
            )?;
            require(
                pool_map
                    .insert(
                        key,
                        groups
                            .into_iter()
                            .map(|group| (group.address, group))
                            .collect(),
                    )
                    .is_none(),
                "ocfs2: duplicate allocator slot",
            )?;
        }
        let mut reserved = vec![(
            0,
            (header.first_cluster_group * header.block_size as u64 / header.cluster_size as u64
                + 1)
                * header.cluster_size as u64,
        )];
        for group in &clusters {
            let start = group.address * header.block_size as u64 / header.cluster_size as u64
                * header.cluster_size as u64;
            reserved.push((start, start + header.cluster_size as u64));
        }
        for group in pool_map.values().flat_map(HashMap::values) {
            for segment in &group.segments {
                reserved.push((
                    segment.physical * header.block_size as u64,
                    (segment.physical + segment.blocks) * header.block_size as u64,
                ));
            }
        }
        reserved.sort_unstable();
        let mut maximum = 0;
        for range in &mut reserved {
            maximum = maximum.max(range.1);
            range.1 = maximum;
        }
        let allocation = Self {
            clusters,
            pools: pool_map,
            reserved,
        };
        for group in allocation
            .clusters
            .iter()
            .chain(allocation.pools.values().flat_map(HashMap::values))
        {
            allocation.check_range(
                header,
                group.address * header.block_size as u64,
                header.block_size as u64,
            )?;
            for segment in &group.segments {
                allocation.check_range(
                    header,
                    segment.physical * header.block_size as u64,
                    segment.blocks * header.block_size as u64,
                )?;
            }
        }
        Ok(allocation)
    }
    fn groups(
        source: &mut impl BlockSource,
        header: &Header,
        inode: &Inode,
        kind: PoolKind,
        seen: &mut HashSet<u64>,
        bytes: &mut usize,
    ) -> Result<Vec<Group>> {
        require(
            inode.flags & 0x490 == 0x490 && inode.dynamic & 1 == 0,
            "ocfs2: allocator inode flags",
        )?;
        let global = matches!(kind, PoolKind::Clusters);
        let list = &inode.raw[192..];
        let per_group = le16(list, 0)? as u64;
        let per_cluster = le16(list, 2)? as u64;
        require(
            per_group != 0
                && per_cluster
                    == if global {
                        1
                    } else {
                        header.cluster_size as u64 / header.block_size as u64
                    },
            "ocfs2: allocator bitmap units",
        )?;
        let capacity = le16(list, 4)? as usize;
        let used = le16(list, 6)? as usize;
        require(
            used <= capacity && 16 + capacity * 16 <= list.len(),
            "ocfs2: chain list bounds",
        )?;
        let mut output = Vec::new();
        let mut total = 0u64;
        let mut allocated = 0u64;
        for chain in 0..used {
            let raw = &list[16 + chain * 16..32 + chain * 16];
            let expected_free = le32(raw, 0)? as u64;
            let expected_total = le32(raw, 4)? as u64;
            let mut next = le64(raw, 8)?;
            let mut chain_free = 0u64;
            let mut chain_total = 0u64;
            while next != 0 {
                require(
                    seen.len() < 131072 && seen.insert(next),
                    "ocfs2: allocator chain cycle/alias/work limit",
                )?;
                let address = next;
                let raw = header.record(source, address, Record::Group)?;
                let bitmap_size = le16(&raw, 8)? as usize;
                let bits = le16(&raw, 10)? as u64;
                let free = le16(&raw, 12)? as u64;
                let limit = if matches!(kind, PoolKind::Slot) && header.incompatible & 0x2000 != 0 {
                    256
                } else {
                    raw.len() - 64
                };
                require(
                    bitmap_size != 0
                        && bitmap_size <= limit
                        && bits != 0
                        && bits <= (bitmap_size * 8) as u64
                        && bits <= per_group * per_cluster
                        && free <= bits,
                    "ocfs2: allocation-group bitmap geometry",
                )?;
                require(
                    le16(&raw, 14)? as usize == chain && le64(&raw, 32)? == inode.address,
                    "ocfs2: allocation group chain/parent identity",
                )?;
                *bytes += bitmap_size;
                require(*bytes <= 64 * 1024 * 1024, "ocfs2: bitmap memory limit")?;
                let bitmap = raw[64..64 + bitmap_size].to_vec();
                let counted = (0..bits)
                    .filter(|bit| bitmap[(*bit / 8) as usize] & (1 << (*bit % 8)) != 0)
                    .count() as u64;
                require(
                    counted == bits - free,
                    "ocfs2: bitmap/free-count disagreement",
                )?;
                let mut group = Group {
                    address,
                    bits,
                    bitmap,
                    segments: Vec::new(),
                    cluster: 0,
                };
                require(group.set(0), "ocfs2: descriptor/reserved bit not allocated")?;
                if global {
                    group.cluster = if address == header.first_cluster_group {
                        0
                    } else {
                        require(
                            address.is_multiple_of(
                                header.cluster_size as u64 / header.block_size as u64,
                            ),
                            "ocfs2: cluster-group alignment",
                        )?;
                        address / (header.cluster_size as u64 / header.block_size as u64)
                    };
                    require(
                        group.cluster.is_multiple_of(per_group),
                        "ocfs2: cluster-group position",
                    )?;
                } else if header.incompatible & 0x2000 != 0
                    && bitmap_size == 256
                    && le16(&raw, 324)? != 0
                {
                    require(
                        bitmap_size == 256 && le16(&raw, 320)? == 0,
                        "ocfs2: discontiguous group list layout",
                    )?;
                    let tree = Tree::parse(
                        source,
                        header,
                        Root {
                            raw: &raw,
                            offset: 320,
                            end: raw.len(),
                            last_leaf: 0,
                        },
                    )?;
                    let mut bit = 0;
                    for extent in tree.extents {
                        require(
                            extent.flags == 0 && extent.logical / header.block_size as u64 == bit,
                            "ocfs2: discontiguous group gap/flags",
                        )?;
                        let blocks = extent.length / header.block_size as u64;
                        group.segments.push(Segment {
                            bit,
                            blocks,
                            physical: extent.physical / header.block_size as u64,
                        });
                        bit += blocks;
                    }
                    require(
                        bit == bits
                            && group
                                .segments
                                .first()
                                .is_some_and(|segment| segment.physical == address),
                        "ocfs2: discontiguous group coverage/descriptor",
                    )?;
                } else {
                    require(
                        address
                            .checked_add(bits)
                            .is_some_and(|end| end <= header.blocks),
                        "ocfs2: contiguous block group outside volume",
                    )?;
                    group.segments.push(Segment {
                        bit: 0,
                        blocks: bits,
                        physical: address,
                    });
                }
                chain_free += free;
                chain_total += bits;
                next = le64(&raw, 24)?;
                output.push(group);
            }
            require(
                chain_free == expected_free && chain_total == expected_total,
                "ocfs2: chain counters",
            )?;
            total += chain_total;
            allocated += chain_total - chain_free;
        }
        require(
            total == le32(&inode.raw, 188)? as u64 && allocated == le32(&inode.raw, 184)? as u64,
            "ocfs2: allocator inode counters",
        )?;
        Ok(output)
    }
    pub fn check_range(&self, header: &Header, physical: u64, length: u64) -> Result<()> {
        if length == 0 {
            return Ok(());
        }
        let end = physical
            .checked_add(length)
            .ok_or(FoldError::Corrupt("ocfs2: allocation span overflow"))?;
        require(
            end <= header.blocks * header.block_size as u64,
            "ocfs2: allocation span outside volume",
        )?;
        let mut cluster = physical / header.cluster_size as u64;
        let end = end.div_ceil(header.cluster_size as u64);
        while cluster < end {
            let index = self
                .clusters
                .partition_point(|group| group.cluster <= cluster);
            let group = index
                .checked_sub(1)
                .and_then(|index| self.clusters.get(index))
                .ok_or(FoldError::Corrupt(
                    "ocfs2: cluster outside allocation groups",
                ))?;
            let stop = end.min(group.cluster + group.bits);
            require(stop > cluster, "ocfs2: cluster bitmap gap")?;
            while cluster < stop {
                require(
                    group.set(cluster - group.cluster),
                    "ocfs2: unallocated cluster reference",
                )?;
                cluster += 1;
            }
        }
        Ok(())
    }
    pub fn check_data(&self, header: &Header, physical: u64, length: u64) -> Result<()> {
        self.check_range(header, physical, length)?;
        let index = self
            .reserved
            .partition_point(|range| range.0 < physical + length);
        require(
            index
                .checked_sub(1)
                .is_none_or(|index| self.reserved[index].1 <= physical),
            "ocfs2: file data overlaps reserved metadata allocation",
        )
    }
    pub fn check_record(
        &self,
        source: &mut impl BlockSource,
        header: &Header,
        block: u64,
        kind: Record,
    ) -> Result<()> {
        let raw = header.record(source, block, kind)?;
        self.check_raw(header, block, kind, &raw)
    }
    pub fn check_raw(&self, header: &Header, block: u64, kind: Record, raw: &[u8]) -> Result<()> {
        self.check_range(
            header,
            block * header.block_size as u64,
            header.block_size as u64,
        )?;
        let (slot_offset, bit_offset, group_offset) = match kind {
            Record::Inode => (12, 14, 152),
            Record::Extent => (16, 18, 32),
            Record::IndexRoot => (16, 18, 72),
            Record::Xattr => (8, 10, 40),
            Record::Refcount => (8, 10, 72),
            Record::Group | Record::IndexLeaf => return Ok(()),
        };
        let slot = le16(raw, slot_offset)?;
        let bit = le16(raw, bit_offset)? as u64;
        let key = if slot == u16::MAX {
            Key::GlobalInode
        } else {
            require(slot < header.slots, "ocfs2: metadata suballocator slot")?;
            if matches!(kind, Record::Inode) {
                Key::Inode(slot)
            } else {
                Key::Extent(slot)
            }
        };
        let explicit = le64(raw, group_offset)?;
        require(
            explicit == 0 || header.incompatible & 0x2000 != 0,
            "ocfs2: discontiguous group feature mismatch",
        )?;
        let address = if explicit == 0 {
            block
                .checked_sub(bit)
                .ok_or(FoldError::Corrupt("ocfs2: suballocator group underflow"))?
        } else {
            explicit
        };
        let group = self
            .pools
            .get(&key)
            .and_then(|pool| pool.get(&address))
            .ok_or(FoldError::Corrupt(
                "ocfs2: metadata group not in allocator chain",
            ))?;
        require(group.set(bit), "ocfs2: free metadata bit")?;
        let segment = group
            .segments
            .iter()
            .find(|segment| segment.bit <= bit && bit < segment.bit + segment.blocks)
            .ok_or(FoldError::Corrupt(
                "ocfs2: suballocator bit outside group extents",
            ))?;
        require(
            segment.physical + bit - segment.bit == block,
            "ocfs2: suballocator physical identity",
        )
    }
}
