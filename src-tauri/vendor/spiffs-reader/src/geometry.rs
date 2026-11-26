use std::collections::HashMap;

use lamfold::{FoldError, Result};

pub const PROBE_BYTES: usize = 4096;
pub(crate) const FREE: u16 = u16::MAX;
pub(crate) const INDEX: u16 = 0x8000;
pub(crate) const PAGE_HEADER: usize = 5;
const MAGIC_SEED: u16 = 0x0529;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ByteOrder {
    Little,
    Big,
}

impl ByteOrder {
    pub(crate) fn u16(self, bytes: &[u8]) -> u16 {
        let bytes = [bytes[0], bytes[1]];
        match self {
            Self::Little => u16::from_le_bytes(bytes),
            Self::Big => u16::from_be_bytes(bytes),
        }
    }

    pub(crate) fn u32(self, bytes: &[u8]) -> u32 {
        let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
        match self {
            Self::Little => u32::from_le_bytes(bytes),
            Self::Big => u32::from_be_bytes(bytes),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Signature {
    None,
    Constant,
    VolumeLength,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Geometry {
    pub page_bytes: usize,
    pub block_bytes: usize,
    pub blocks: usize,
    pub order: ByteOrder,
    pub signature: Signature,
}

impl Geometry {
    pub(crate) fn pages_per_block(self) -> usize {
        self.block_bytes / self.page_bytes
    }

    pub(crate) fn lookup_pages(self) -> usize {
        (self.pages_per_block() * 2 / self.page_bytes).max(1)
    }

    pub(crate) fn entries(self) -> usize {
        self.pages_per_block() - self.lookup_pages()
    }

    pub(crate) fn lookup_bytes(self) -> usize {
        self.lookup_pages() * self.page_bytes
    }

    pub(crate) fn pages(self) -> usize {
        self.blocks * self.pages_per_block()
    }

    pub(crate) fn data_bytes(self) -> usize {
        self.page_bytes - PAGE_HEADER
    }

    pub fn signature_offset(self) -> u64 {
        (self.lookup_bytes() - 4) as u64
    }

    fn magic(self, block: usize, signature: Signature) -> u16 {
        let seed = MAGIC_SEED ^ self.page_bytes as u16;
        match signature {
            Signature::VolumeLength => seed ^ (self.blocks - block) as u16,
            Signature::Constant => seed,
            Signature::None => FREE,
        }
    }

    pub fn locate(
        head: &[u8],
        length: u64,
        mut read: impl FnMut(u64, &mut [u8]) -> Result<()>,
    ) -> Result<Option<Self>> {
        let mut signed = Vec::new();
        let mut unsigned = Vec::new();
        let mut sampled = HashMap::new();
        let mut sample = |offset: usize| {
            if let Some(bytes) = head.get(offset..offset + 2) {
                return Some([bytes[0], bytes[1]]);
            }
            *sampled.entry(offset).or_insert_with(|| {
                let mut bytes = [0; 2];
                read(offset as u64, &mut bytes).ok().map(|()| bytes)
            })
        };
        for page_bytes in [64, 128, 256, 512, 1024, 2048, 4096] {
            for block_bytes in [512, 1024, 2048, 4096, 8192, 16384, 32768, 65536] {
                if block_bytes < page_bytes * 2 || !length.is_multiple_of(block_bytes as u64) {
                    continue;
                }
                let blocks = length / block_bytes as u64;
                if blocks < 3 || length / page_bytes as u64 > u64::from(FREE) {
                    continue;
                }
                for order in [ByteOrder::Little, ByteOrder::Big] {
                    let mut layout = Self {
                        page_bytes,
                        block_bytes,
                        blocks: blocks as usize,
                        order,
                        signature: Signature::None,
                    };
                    let offset = layout.signature_offset() as usize;
                    if let Some(bytes) = head
                        .get(offset..offset + 2)
                        .filter(|_| layout.entries() * 2 <= offset)
                    {
                        let value = order.u16(bytes);
                        for signature in [Signature::VolumeLength, Signature::Constant] {
                            if (value == FREE || value == layout.magic(0, signature))
                                && layout.signature_matches(signature, &mut sample)
                            {
                                layout.signature = signature;
                                signed.push(layout);
                            }
                        }
                    }
                    if layout.signature == Signature::None && layout.unsigned_anchor(head) {
                        unsigned.push(layout);
                    }
                }
            }
        }
        if !signed.is_empty() {
            signed.sort_unstable_by_key(|layout| {
                (std::cmp::Reverse(layout.signature), layout.block_bytes)
            });
            let strongest = signed[0];
            return Ok((!signed.iter().skip(1).any(|layout| {
                layout.signature == strongest.signature
                    && layout.block_bytes == strongest.block_bytes
                    && *layout != strongest
            }))
            .then_some(strongest));
        }

        let mut matched = None;
        for layout in unsigned {
            if layout.unsigned_matches(head, &mut read)? {
                if matched.is_some() {
                    return Ok(None);
                }
                matched = Some(layout);
            }
        }
        Ok(matched)
    }

    fn signature_matches(
        self,
        signature: Signature,
        sample: &mut impl FnMut(usize) -> Option<[u8; 2]>,
    ) -> bool {
        let mut matches = 0;
        for block in (0..self.blocks.min(4)).chain((self.blocks > 4).then_some(self.blocks - 1)) {
            let offset = block * self.block_bytes + self.signature_offset() as usize;
            let Some(bytes) = sample(offset) else {
                return false;
            };
            let observed = self.order.u16(&bytes);
            if observed == self.magic(block, signature) {
                matches += 1;
            } else if observed != FREE {
                return false;
            }
        }
        matches >= 3 && matches + 1 >= (self.blocks.min(4) + usize::from(self.blocks > 4))
    }

    pub(crate) fn verify_signature(
        self,
        block: usize,
        lookups: &[u8],
        missing: &mut bool,
    ) -> Result<()> {
        if self.signature != Signature::None {
            let actual = self.order.u16(&lookups[self.signature_offset() as usize..]);
            if actual == FREE && !*missing {
                *missing = true;
            } else if actual != self.magic(block, self.signature) {
                return Err(FoldError::Corrupt("invalid SPIFFS lookup block signature"));
            }
        }
        Ok(())
    }

    fn unsigned_anchor(self, head: &[u8]) -> bool {
        let mut matched = 0;
        let mut file = false;
        for entry in 0..self.entries().min(16) {
            let at = (self.lookup_pages() + entry) * self.page_bytes;
            let Some(bytes) = head.get(at..at + PAGE_HEADER) else {
                break;
            };
            let id = self.order.u16(&head[2 * entry..]);
            if id == 0 || id == FREE {
                continue;
            }
            let page = Page::decode(bytes, self.order);
            if page.live() && page.id == id {
                matched += 1;
                file |= page.is_index() && page.span == 0;
            }
        }
        matched >= 2 && file
    }

    fn unsigned_matches(
        self,
        head: &[u8],
        read: &mut impl FnMut(u64, &mut [u8]) -> Result<()>,
    ) -> Result<bool> {
        let mut scratch = vec![0; self.block_bytes];
        let mut populated_blocks = 0;
        for block in 0..self.blocks.min(8) {
            let offset = block * self.block_bytes;
            let bytes = if let Some(bytes) = head.get(offset..offset + self.block_bytes) {
                bytes
            } else {
                read(offset as u64, &mut scratch)?;
                &scratch
            };
            let luts = &bytes[..self.lookup_bytes()];
            // In a magic-less volume the final lookup word is the erase count.
            if luts[self.entries() * 2..luts.len() - 2]
                .iter()
                .any(|&byte| byte != 0xff)
            {
                return Ok(false);
            }
            let mut populated = false;
            for (entry, bytes) in bytes[self.lookup_bytes()..]
                .chunks_exact(self.page_bytes)
                .enumerate()
            {
                let id = self.order.u16(&luts[entry * 2..]);
                let page = Page::decode(bytes, self.order);
                if id == FREE {
                    if page.live() {
                        return Ok(false);
                    }
                } else if id != 0 && page.live() {
                    if page.id != id {
                        return Ok(false);
                    }
                    populated = true;
                }
            }
            populated_blocks += usize::from(populated);
        }
        Ok(populated_blocks >= 2)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Page {
    pub id: u16,
    pub span: u16,
    flags: u8,
}

impl Page {
    pub fn decode(bytes: &[u8], order: ByteOrder) -> Self {
        Self {
            id: order.u16(bytes),
            span: order.u16(&bytes[2..]),
            flags: bytes[4],
        }
    }

    pub fn is_index(self) -> bool {
        self.id & INDEX != 0
    }

    pub fn live(self) -> bool {
        let index = self.flags & 0x04 == 0;
        self.flags & 0x83 == 0x80
            && index == self.is_index()
            && self.id != FREE
            && self.id & !INDEX != 0
            && (!index || self.span != 0 || self.flags & 0x40 != 0)
    }
}
