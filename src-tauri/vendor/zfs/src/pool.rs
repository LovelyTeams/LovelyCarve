//! Match verified leaf labels and read virtual device addresses from the pool.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, ErrorKind};

use crate::config::{Config, Vdev};
use crate::label::{Label, LABEL_SIZE};
use crate::spa::DVA;
use crate::{read_device_label, Device, RawDevice, ZfsError, ZfsErrorKind};
use crate::{DefaultDMU, DefaultDSL, DefaultSPA, DefaultZAP, DefaultZPL};

const DATA_START: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy)]
enum Layout {
    Leaf,
    Mirror,
    Raidz { parity: u8 },
}

#[derive(Debug)]
struct Top {
    layout: Layout,
    ashift: u8,
    allocated_size: u64,
    members: Vec<u64>,
}

impl Top {
    fn from_config(config: &Vdev) -> Result<Self, ZfsError> {
        let layout = match (config.kind.as_str(), config.children.len(), config.parity) {
            ("disk" | "file", 0, None) => Layout::Leaf,
            ("mirror", 2.., None) => Layout::Mirror,
            ("raidz", width, Some(parity @ 1..=3)) if width > usize::from(parity) => {
                Layout::Raidz { parity }
            }
            _ => {
                return Err(topology(format!(
                    "unsupported vdev {} ({})",
                    config.id, config.kind
                )))
            }
        };
        let ashift = config
            .ashift
            .filter(|shift| (9..=16).contains(shift))
            .ok_or_else(|| topology(format!("invalid sector size for vdev {}", config.id)))?;
        let allocated_size = config
            .allocated_size
            .filter(|size| *size > 0 && *size % (1 << ashift) == 0)
            .ok_or_else(|| topology(format!("invalid allocated size for vdev {}", config.id)))?;
        let members = match layout {
            Layout::Leaf => vec![config.guid],
            _ => {
                let mut members = vec![0; config.children.len()];
                for child in &config.children {
                    let slot = members
                        .get_mut(child.id as usize)
                        .filter(|guid| **guid == 0)
                        .ok_or_else(|| {
                            topology(format!("invalid child slot in vdev {}", config.id))
                        })?;
                    if !matches!(child.kind.as_str(), "disk" | "file")
                        || !child.children.is_empty()
                        || child.guid == 0
                    {
                        return Err(topology(format!("unsupported child in vdev {}", config.id)));
                    }
                    *slot = child.guid;
                }
                members
            }
        };
        Ok(Self {
            layout,
            ashift,
            allocated_size,
            members,
        })
    }
}

#[derive(Debug)]
pub struct PoolLayout {
    pub guid: u64,
    pub name: String,
    top: BTreeMap<u32, Top>,
    label: Label,
}

fn topology(message: impl Into<String>) -> ZfsError {
    ZfsErrorKind::Topology(message.into()).into()
}

impl PoolLayout {
    pub fn from_labels<'a>(labels: impl IntoIterator<Item = &'a Label>) -> Result<Self, ZfsError> {
        let labels: Vec<_> = labels.into_iter().collect();
        let first = labels
            .first()
            .copied()
            .ok_or_else(|| topology("no members supplied"))?;
        let guid = first.identity.pool_guid;
        if labels.len() == 1 && first.config.is_none() {
            let top = Top {
                layout: Layout::Leaf,
                ashift: 9,
                allocated_size: u64::MAX,
                members: vec![first.identity.leaf_guid],
            };
            return Ok(Self {
                guid,
                name: format!("ZFS {guid}"),
                top: BTreeMap::from([(0, top)]),
                label: first.clone(),
            });
        }
        let mut leaves = BTreeMap::new();
        let mut configs: BTreeMap<u32, &Config> = BTreeMap::new();
        let mut newest: Option<&Config> = None;
        for &label in &labels {
            let id = label.identity;
            if id.pool_guid != guid || leaves.insert(id.leaf_guid, label).is_some() {
                return Err(topology("mixed pool identities or duplicate leaf GUIDs"));
            }
            let config = label
                .config
                .as_ref()
                .ok_or_else(|| topology("member has no configuration"))?;
            if config.expanded_raidz {
                return Err(topology("expanded RAIDZ vdevs are unsupported"));
            }
            if let Some(old) = newest {
                if old.txg == config.txg
                    && (old.top_count != config.top_count || old.holes != config.holes)
                {
                    return Err(topology(
                        "conflicting pool namespace at the same transaction group",
                    ));
                }
            }
            if newest.is_none_or(|old| config.txg > old.txg) {
                newest = Some(config);
            }
            if let Some(old) = configs.get(&config.top.id) {
                if old.top.guid != config.top.guid
                    || (old.txg == config.txg && old.top != config.top)
                {
                    return Err(topology(format!(
                        "conflicting configurations for vdev {}",
                        config.top.id
                    )));
                }
                if old.txg > config.txg {
                    continue;
                }
            }
            configs.insert(config.top.id, config);
        }
        let newest = newest.ok_or_else(|| topology("no pool configuration"))?;
        let holes: BTreeSet<u32> = newest.holes.iter().copied().collect();
        if let Some(count) = newest.top_count {
            if holes.len() != newest.holes.len()
                || configs.keys().chain(holes.iter()).any(|id| *id >= count)
                || configs.keys().any(|id| holes.contains(id))
                || configs.len() + holes.len() != count as usize
            {
                return Err(topology(format!(
                    "incomplete set of {count} top-level vdevs"
                )));
            }
        }
        let mut seen = BTreeSet::new();
        let mut guid_sum = guid;
        let mut top = BTreeMap::new();
        for (id, config) in configs {
            let vdev = Top::from_config(&config.top)?;
            guid_sum = guid_sum.wrapping_add(config.top.guid);
            if !matches!(vdev.layout, Layout::Leaf) {
                guid_sum = vdev
                    .members
                    .iter()
                    .fold(guid_sum, |sum, member| sum.wrapping_add(*member));
            }
            for member in &vdev.members {
                let leaf = leaves
                    .get(member)
                    .ok_or_else(|| topology(format!("vdev {id} is missing leaf {member}")))?;
                if !seen.insert(*member)
                    || leaf.config.as_ref().is_none_or(|label| label.top.id != id)
                {
                    return Err(topology(format!(
                        "leaf {member} has conflicting membership"
                    )));
                }
            }
            top.insert(id, vdev);
        }
        if seen.len() != leaves.len() {
            return Err(topology("member is absent from the newest topology"));
        }
        let committed = labels
            .iter()
            .flat_map(|&label| label.uberblocks.iter().map(move |uber| (label, uber)))
            .filter(|(_, uber)| uber.guid_sum == guid_sum && uber.txg >= newest.txg);
        let (anchor, latest) = committed
            .clone()
            .max_by_key(|(_, uber)| (uber.txg, uber.timestamp))
            .ok_or_else(|| topology("no verified uberblock for the complete topology"))?;
        if committed
            .filter(|(_, uber)| uber.txg == latest.txg)
            .any(|(_, uber)| uber.rootbp != latest.rootbp)
        {
            return Err(topology(
                "conflicting uberblocks at the latest transaction group",
            ));
        }
        let label = Label {
            uberblocks: vec![latest.clone()],
            identity: anchor.identity,
            config: anchor.config.clone(),
        };
        Ok(Self {
            guid,
            name: newest.name.clone(),
            top,
            label,
        })
    }

    pub fn transaction(&self) -> u64 {
        self.label
            .uberblocks
            .iter()
            .map(|uber| uber.txg)
            .max()
            .unwrap_or(0)
    }

    pub fn single_leaf(&self) -> bool {
        self.top.len() == 1
            && self
                .top
                .values()
                .all(|vdev| matches!(vdev.layout, Layout::Leaf))
    }
}

pub struct Pool<D> {
    layout: PoolLayout,
    members: BTreeMap<u64, D>,
}

impl<D: RawDevice> Pool<D> {
    pub fn open(sources: Vec<D>) -> Result<(Self, Label), ZfsError> {
        let members = sources
            .into_iter()
            .map(|source| read_device_label(&source).map(|label| (source, label)))
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_members(members)
    }

    pub fn from_members(members: Vec<(D, Label)>) -> Result<(Self, Label), ZfsError> {
        let layout = PoolLayout::from_labels(members.iter().map(|(_, label)| label))?;
        let label = layout.label.clone();
        let members = members
            .into_iter()
            .map(|(source, label)| (label.identity.leaf_guid, source))
            .collect();
        Ok((Self { layout, members }, label))
    }

    fn read_leaf(&self, guid: u64, offset: u64, length: u64) -> io::Result<D::Block> {
        let source = self.members.get(&guid).ok_or(ErrorKind::NotFound)?;
        let start = DATA_START
            .checked_add(offset)
            .ok_or(ErrorKind::InvalidData)?;
        let end = start.checked_add(length).ok_or(ErrorKind::InvalidData)?;
        if source
            .size_bytes()
            .is_some_and(|size| end > size.saturating_sub(2 * LABEL_SIZE as u64))
        {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        let data = source.read_raw(start, length)?;
        if data.as_ref().len() as u64 != length {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        Ok(data)
    }
}

impl<D: RawDevice> Device for Pool<D> {
    type Block = Vec<u8>;

    fn get_label(&self, index: u8) -> Result<Option<Label>, ZfsError> {
        Ok((index == 0).then(|| self.layout.label.clone()))
    }

    fn read_offset(&self, vdev: u32, offset: u64, length: u32) -> io::Result<Self::Block> {
        self.read_copy(
            DVA {
                vdev,
                offset,
                asize: 0,
                grid: 0,
                gang: false,
            },
            length,
            0,
        )
    }

    fn copies(&self, vdev: u32) -> usize {
        match self.layout.top.get(&vdev) {
            Some(top) if matches!(top.layout, Layout::Mirror) => top.members.len(),
            _ => 1,
        }
    }

    fn read_copy(&self, address: DVA, length: u32, copy: usize) -> io::Result<Self::Block> {
        let top = self
            .layout
            .top
            .get(&address.vdev)
            .ok_or(ErrorKind::NotFound)?;
        let offset = address
            .offset
            .checked_mul(512)
            .ok_or(ErrorKind::InvalidData)?;
        let length = u64::from(length) * 512;
        let allocated = u64::from(address.asize) * 512;
        if address.gang || address.grid != 0 || length == 0 || length > 32 * 1024 * 1024 {
            return Err(ErrorKind::InvalidData.into());
        }
        let checked_size = if allocated == 0 { length } else { allocated };
        if offset
            .checked_add(checked_size)
            .is_none_or(|end| end > top.allocated_size)
        {
            return Err(ErrorKind::UnexpectedEof.into());
        }
        match top.layout {
            Layout::Leaf | Layout::Mirror => {
                let guid = *top.members.get(copy).ok_or(ErrorKind::NotFound)?;
                if checked_size < length {
                    return Err(ErrorKind::InvalidData.into());
                }
                self.read_leaf(guid, offset, length)
                    .map(|data| data.as_ref().to_vec())
            }
            Layout::Raidz { parity } if copy == 0 => {
                let sector = 1u64 << top.ashift;
                if offset % sector != 0 {
                    return Err(ErrorKind::InvalidData.into());
                }
                let stripe = offset / sector;
                let columns = top.members.len() as u64;
                let parity = u64::from(parity);
                let sectors = length.div_ceil(sector);
                let rows = sectors / (columns - parity);
                let remainder = sectors % (columns - parity);
                let stripe_size = sectors + parity * sectors.div_ceil(columns - parity);
                let rounded = stripe_size.div_ceil(parity + 1) * (parity + 1) * sector;
                if allocated != 0 && rounded != allocated {
                    return Err(ErrorKind::InvalidData.into());
                }
                let mut output = Vec::with_capacity((sectors * sector) as usize);
                for column in parity..columns {
                    let size = (rows + u64::from(column - parity < remainder)) * sector;
                    if size == 0 {
                        break;
                    }
                    // RAIDZ1 alternates the parity and first data locations every MiB.
                    let location = if parity == 1 && column == 1 && offset & (1 << 20) != 0 {
                        0
                    } else {
                        column
                    };
                    let leaf_sector = (stripe + location) / columns;
                    let member = top.members[((stripe + location) % columns) as usize];
                    let data = self.read_leaf(member, leaf_sector * sector, size)?;
                    output.extend_from_slice(data.as_ref());
                }
                output.truncate(length as usize);
                Ok(output)
            }
            Layout::Raidz { .. } => Err(ErrorKind::NotFound.into()),
        }
    }
}

impl<D: RawDevice> DefaultSPA for Pool<D> {}
impl<D: RawDevice> DefaultDMU for Pool<D> {}
impl<D: RawDevice> DefaultZAP for Pool<D> {}
impl<D: RawDevice> DefaultDSL for Pool<D> {}
impl<D: RawDevice> DefaultZPL for Pool<D> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::label::Identity;
    use crate::spa::Uberblock;

    fn leaves() -> Vec<Label> {
        let mut uber = Uberblock::parse(&[0; 1024], 1024).unwrap().1;
        uber.txg = 12;
        uber.guid_sum = 42 + 19 + 7 + 11 + 13;
        let children = [7, 11, 13]
            .into_iter()
            .enumerate()
            .map(|(id, guid)| Vdev {
                id: id as u32,
                guid,
                kind: "disk".into(),
                ashift: None,
                allocated_size: None,
                parity: None,
                children: Vec::new(),
            })
            .collect();
        let config = Config {
            name: "invented".into(),
            txg: 10,
            top_count: Some(1),
            holes: Vec::new(),
            top: Vdev {
                id: 0,
                guid: 19,
                kind: "raidz".into(),
                ashift: Some(9),
                allocated_size: Some(8 * 1024 * 1024),
                parity: Some(1),
                children,
            },
            expanded_raidz: false,
        };
        [13, 7, 11]
            .into_iter()
            .map(|leaf_guid| Label {
                uberblocks: vec![uber.clone()],
                identity: Identity {
                    pool_guid: 42,
                    leaf_guid,
                },
                config: Some(config.clone()),
            })
            .collect()
    }

    #[test]
    fn rejects_missing_duplicate_and_conflicting_members() {
        let mut labels = leaves();
        assert_eq!(PoolLayout::from_labels(&labels).unwrap().transaction(), 12);
        assert!(PoolLayout::from_labels(&labels[..2])
            .unwrap_err()
            .to_string()
            .contains("missing leaf"));
        labels[2] = labels[0].clone();
        assert!(PoolLayout::from_labels(&labels)
            .unwrap_err()
            .to_string()
            .contains("duplicate"));

        let mut labels = leaves();
        for label in &mut labels {
            label.uberblocks[0].guid_sum ^= 1;
        }
        assert!(PoolLayout::from_labels(&labels)
            .unwrap_err()
            .to_string()
            .contains("no verified uberblock"));

        let mut labels = leaves();
        labels[2].uberblocks[0].rootbp.kind ^= 1;
        assert!(PoolLayout::from_labels(&labels)
            .unwrap_err()
            .to_string()
            .contains("conflicting uberblocks"));
    }

    struct Pattern(u8);

    impl RawDevice for Pattern {
        type Block = Vec<u8>;

        fn read_raw(&self, addr: u64, size: u64) -> io::Result<Self::Block> {
            assert!(addr >= DATA_START);
            Ok(vec![self.0; size as usize])
        }
    }

    #[test]
    fn reads_raidz_data_columns_in_order_and_swaps_the_first_column() {
        let members = [Pattern(13), Pattern(7), Pattern(11)]
            .into_iter()
            .zip(leaves())
            .collect();
        let (pool, _) = Pool::from_members(members).unwrap();
        let mut dva = DVA {
            vdev: 0,
            offset: 2,
            asize: 4,
            grid: 0,
            gang: false,
        };
        let bytes = pool.read_copy(dva, 2, 0).unwrap();
        assert_eq!(
            (&bytes[..512], &bytes[512..]),
            (&[7; 512][..], &[11; 512][..])
        );
        // One MiB starts on slot two. Parity rotation moves the first
        // data column back to slot two instead of wrapping to slot zero.
        dva.offset = 2048;
        let bytes = pool.read_copy(dva, 2, 0).unwrap();
        assert_eq!(
            (&bytes[..512], &bytes[512..]),
            (&[13; 512][..], &[11; 512][..])
        );
        dva.asize -= 1;
        assert!(pool.read_copy(dva, 2, 0).is_err());
    }
}
