//! ZFS leaf labels and position-bound uberblock checksums.

use crate::{spa::Uberblock, ZfsErrorKind};
use sha2::{Digest, Sha256};

pub const LABEL_SIZE: usize = 256 * 1024;
pub const UBERBLOCK_OFFSET: usize = LABEL_SIZE / 2;
const CONFIG_OFFSET: usize = 16 * 1024;
const ECK_SIZE: usize = 40;
const ECK_MAGIC: u64 = 0x0210_da7a_b10c_7a11;
const UBERBLOCK_MAGIC: u64 = 0x00ba_b10c;

pub fn label_offsets(size: u64) -> [Option<u64>; 4] {
    let label = LABEL_SIZE as u64;
    let aligned = size / label * label;
    let tail = (aligned >= 4 * label).then_some(aligned);
    [
        (aligned >= label).then_some(0),
        (aligned >= 2 * label).then_some(label),
        tail.map(|end| end - 2 * label),
        tail.map(|end| end - label),
    ]
}

#[derive(Clone, Copy)]
enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    fn read(self, bytes: &[u8]) -> u64 {
        let bytes = bytes.try_into().unwrap();
        match self {
            Self::Little => u64::from_le_bytes(bytes),
            Self::Big => u64::from_be_bytes(bytes),
        }
    }

    fn write(self, value: u64) -> [u8; 8] {
        match self {
            Self::Little => value.to_le_bytes(),
            Self::Big => value.to_be_bytes(),
        }
    }
}

fn checksum_order(bytes: &[u8]) -> Option<ByteOrder> {
    match bytes.try_into().ok()? {
        magic if u64::from_le_bytes(magic) == ECK_MAGIC => Some(ByteOrder::Little),
        magic if u64::from_be_bytes(magic) == ECK_MAGIC => Some(ByteOrder::Big),
        _ => None,
    }
}

pub const CONFIG_CHECKSUM_OFFSET: usize = UBERBLOCK_OFFSET - ECK_SIZE;

pub fn has_label_checksum_magic(bytes: &[u8]) -> bool {
    checksum_order(bytes).is_some()
}

fn verify_embedded(block: &[u8], offset: u64) -> Option<ByteOrder> {
    let (covered, stored) = block.split_at(block.len().checked_sub(32)?);
    let order = checksum_order(covered.get(covered.len().checked_sub(8)?..)?)?;
    let mut digest = Sha256::new();
    digest.update(covered);
    digest.update(order.write(offset));
    digest.update([0; 24]);
    let digest = digest.finalize();
    stored
        .chunks_exact(8)
        .zip(digest.chunks_exact(8))
        .all(|(word, hash)| order.read(word) == u64::from_be_bytes(hash.try_into().unwrap()))
        .then_some(order)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    pub pool_guid: u64,
    pub leaf_guid: u64,
}

use crate::config::{Config, NvList};

fn read_identity(config: &NvList<'_>) -> Option<Identity> {
    let identity = Identity {
        pool_guid: config.number(b"pool_guid")?,
        leaf_guid: config.number(b"guid")?,
    };
    (identity.pool_guid != 0 && identity.leaf_guid != 0).then_some(identity)
}

fn verified_config(label: &[u8], offset: u64) -> Option<NvList<'_>> {
    let config = label.get(CONFIG_OFFSET..UBERBLOCK_OFFSET)?;
    verify_embedded(config, offset.checked_add(CONFIG_OFFSET as u64)?)?;
    NvList::from_packed(&config[..config.len() - ECK_SIZE])
}

fn verified_slots(label: &[u8], offset: u64) -> impl Iterator<Item = (&[u8], ByteOrder)> {
    let ring = label.get(UBERBLOCK_OFFSET..LABEL_SIZE).unwrap_or_default();
    // OpenZFS caps the ashift-derived stride to 1–8 KiB. Each checksum binds
    // the slot size and its offset within the leaf, even on sparse rings.
    [1024, 2048, 4096, 8192].into_iter().flat_map(move |size| {
        ring.chunks_exact(size)
            .enumerate()
            .filter_map(move |(index, slot)| {
                let magic: [u8; 8] = slot[..8].try_into().unwrap();
                if u64::from_le_bytes(magic) != UBERBLOCK_MAGIC
                    && u64::from_be_bytes(magic) != UBERBLOCK_MAGIC
                {
                    return None;
                }
                let disk_offset = offset.checked_add((UBERBLOCK_OFFSET + index * size) as u64)?;
                let order = verify_embedded(slot, disk_offset)?;
                (order.read(&magic) == UBERBLOCK_MAGIC && order.read(&slot[16..24]) != 0)
                    .then_some((slot, order))
            })
    })
}

pub fn has_verified_label(label: &[u8], offset: u64) -> bool {
    verified_config(label, offset)
        .and_then(|config| read_identity(&config))
        .is_some()
        && verified_slots(label, offset).next().is_some()
}

#[derive(Debug, Clone)]
pub struct Label {
    pub uberblocks: Vec<Uberblock>,
    pub identity: Identity,
    pub config: Option<Config>,
}

impl Label {
    pub fn parse_at(label: &[u8], offset: u64) -> Result<Self, ZfsErrorKind> {
        let config = verified_config(label, offset).ok_or(ZfsErrorKind::Invalid)?;
        let identity = read_identity(&config).ok_or(ZfsErrorKind::Invalid)?;
        let config = Config::parse(&config).ok_or(ZfsErrorKind::Invalid)?;
        let mut slots = verified_slots(label, offset).peekable();
        if slots.peek().is_none() {
            return Err(ZfsErrorKind::Checksum);
        }
        let uberblocks: Vec<_> = slots
            .filter_map(|(slot, order)| match order {
                ByteOrder::Little => Uberblock::parse(slot, slot.len()).ok().map(|(_, ub)| ub),
                ByteOrder::Big => None,
            })
            .collect();
        if uberblocks.is_empty() {
            return Err(ZfsErrorKind::UnsupportedFeature);
        }
        Ok(Self {
            uberblocks,
            identity,
            config,
        })
    }
}
