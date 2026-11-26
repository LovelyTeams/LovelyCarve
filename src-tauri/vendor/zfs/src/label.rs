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
    pool_guid: u64,
    leaf_guid: u64,
}

fn xdr_u32(bytes: &[u8]) -> Option<usize> {
    Some(u32::from_be_bytes(bytes.get(..4)?.try_into().ok()?) as usize)
}

/// Read the scalar identities from the top-level XDR nvlist. Embedded nvlists
/// and arrays are skipped using their parent pair's encoded length.
fn read_identity(mut bytes: &[u8]) -> Option<Identity> {
    if bytes.first() != Some(&1) || xdr_u32(bytes.get(4..)?)? != 0 {
        return None;
    }
    bytes = bytes.get(12..)?;
    let (mut pool, mut leaf) = (None, None);
    loop {
        let encoded = xdr_u32(bytes)?;
        let decoded = xdr_u32(bytes.get(4..)?)?;
        if encoded == 0 && decoded == 0 {
            break;
        }
        if encoded < 20 || encoded % 4 != 0 || decoded == 0 {
            return None;
        }
        let (pair, rest) = bytes.split_at_checked(encoded)?;
        bytes = rest;
        let name_size = xdr_u32(pair.get(8..)?)?;
        let name = pair.get(12..12usize.checked_add(name_size)?)?;
        let dest = match name {
            b"pool_guid" => &mut pool,
            b"guid" => &mut leaf,
            _ => continue,
        };
        let aligned_name_size = name_size.checked_add(3)? & !3;
        let data = pair.get(12usize.checked_add(aligned_name_size)?..)?;
        if xdr_u32(data)? != 8 || xdr_u32(data.get(4..)?)? != 1 || dest.is_some() {
            return None;
        }
        *dest = Some(u64::from_be_bytes(data.get(8..16)?.try_into().ok()?));
    }
    let identity = Identity {
        pool_guid: pool?,
        leaf_guid: leaf?,
    };
    (identity.pool_guid != 0 && identity.leaf_guid != 0).then_some(identity)
}

fn config_identity(label: &[u8], offset: u64) -> Option<Identity> {
    let config = label.get(CONFIG_OFFSET..UBERBLOCK_OFFSET)?;
    verify_embedded(config, offset.checked_add(CONFIG_OFFSET as u64)?)?;
    read_identity(&config[..config.len() - ECK_SIZE])
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
    config_identity(label, offset).is_some() && verified_slots(label, offset).next().is_some()
}

#[derive(Debug, Clone)]
pub struct Label {
    pub uberblocks: Vec<Uberblock>,
    pub identity: Identity,
}

impl Label {
    pub fn parse_at(label: &[u8], offset: u64) -> Result<Self, ZfsErrorKind> {
        let identity = config_identity(label, offset).ok_or(ZfsErrorKind::Invalid)?;
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
        })
    }
}
