use crate::{ByteOrder, FileType, Geometry, Node};
use anyhow::{Result, ensure};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Extra {
    pub parent: u32,
    pub kind: u32,
    pub shrink: bool,
    pub shadows: bool,
    pub value: u32,
}
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tags {
    pub sequence: u32,
    pub object: u32,
    pub chunk: u32,
    pub count: u32,
    pub extra: Option<Extra>,
}
impl Tags {
    pub fn parse(spare: &[u8], geometry: Geometry) -> Result<Option<Self>> {
        ensure!(spare.len() == geometry.spare_size, "truncated YAFFS2 spare");
        ensure!(
            spare[..geometry.tags_offset]
                .iter()
                .all(|byte| *byte == 255),
            "unsupported YAFFS2 bad-block marker"
        );
        let bytes = &spare[geometry.tags_offset..];
        let order = geometry.order;
        let sequence = order.word(bytes, 0)?;
        if sequence == u32::MAX {
            return Ok(None);
        }
        ensure!(
            (0x1000..0xefff_ff00).contains(&sequence) || sequence == 0x21,
            "invalid/unsupported YAFFS2 sequence {sequence:#x}"
        );
        if bytes.len() >= 28 && !bytes[16..28].iter().all(|byte| *byte == 255) {
            let (column, line, prime) = ecc(&bytes[..16]);
            ensure!(
                bytes[16] == column
                    && [ByteOrder::Little, ByteOrder::Big]
                        .into_iter()
                        .any(|order| {
                            order.word(bytes, 20).ok() == Some(line)
                                && order.word(bytes, 24).ok() == Some(prime)
                        }),
                "YAFFS2 tag ECC mismatch (correction is not enabled)"
            );
        }
        let mut object = order.word(bytes, 4)?;
        let mut chunk = order.word(bytes, 8)?;
        let mut count = order.word(bytes, 12)?;
        if sequence == 0x21 || object == 0x10 {
            return Ok(Some(Self {
                sequence,
                object,
                chunk,
                count,
                extra: None,
            }));
        }
        let extra = if chunk & 0x8000_0000 != 0 {
            ensure!(
                chunk & 0x1000_0000 == 0,
                "unsupported YAFFS2 extra tag flags"
            );
            let extra = Extra {
                parent: chunk & 0x0fff_ffff,
                kind: object >> 28,
                shrink: chunk & 0x4000_0000 != 0,
                shadows: chunk & 0x2000_0000 != 0,
                value: count,
            };
            object &= 0x0fff_ffff;
            chunk = 0;
            count = 0;
            Some(extra)
        } else {
            None
        };
        if sequence != 0x21 && object != 0x10 {
            ensure!(
                object > 4 || chunk == 0,
                "YAFFS2 reserved directory has data chunks"
            );
            ensure!(
                (1..0x40000).contains(&object) && chunk <= 0x0fff_ffff,
                "invalid YAFFS2 object/chunk identity"
            );
            ensure!(
                chunk == 0 || (count > 0 && count as usize <= geometry.page_size),
                "YAFFS2 data byte count exceeds page"
            );
        }
        Ok(Some(Self {
            sequence,
            object,
            chunk,
            count,
            extra,
        }))
    }
    pub fn validate_header(&self, node: &Node) -> Result<()> {
        if let Some(extra) = self.extra {
            let kind = match node.kind {
                FileType::File => 1,
                FileType::Symlink => 2,
                FileType::Directory => 3,
                FileType::HardLink => 4,
                FileType::Special => 5,
            };
            ensure!(
                extra.parent == node.parent_id
                    && extra.kind == kind
                    && extra.shrink == node.shrink
                    && extra.shadows == (node.shadow != 0),
                "YAFFS2 extra tags disagree with object header"
            );
            match node.kind {
                FileType::File => ensure!(
                    u64::from(extra.value) == node.size,
                    "YAFFS2 extra file size disagreement"
                ),
                FileType::HardLink => ensure!(
                    extra.value == node.equivalent,
                    "YAFFS2 extra hard-link disagreement"
                ),
                FileType::Directory | FileType::Symlink | FileType::Special => {
                    ensure!(extra.value == 0, "YAFFS2 unexpected extra value")
                }
            }
        }
        Ok(())
    }
}
/// Six column parities plus complementary row parities for 16 tag bytes.
/// Compute the parity matrix directly instead of importing a C lookup table.
pub(crate) fn ecc(bytes: &[u8]) -> (u8, u32, u32) {
    let mut column = 0;
    let mut line = 0;
    let mut prime = 0;
    for (index, byte) in bytes.iter().copied().enumerate() {
        for bit in 0..8 {
            if byte & (1 << bit) != 0 {
                for axis in 0..3 {
                    column ^= 1 << (axis * 2 + ((bit >> axis) & 1));
                }
            }
        }
        if byte.count_ones() & 1 != 0 {
            line ^= index as u32;
            prime ^= !(index as u32);
        }
    }
    (column, line, prime)
}
