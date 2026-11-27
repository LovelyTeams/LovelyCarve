use std::{collections::HashSet, ops::RangeInclusive};

use lamfold::{BlockSource, FoldError, Result};

use crate::{
    format::{BTREE_BYTES, DATA, Header, INODE, Key, Record},
    require,
};

pub(crate) struct Media<S> {
    source: S,
    pub header: Header,
}

struct Branch {
    bounds: Option<RangeInclusive<Key>>,
    address: u64,
    expected_parent: u64,
    expected_kind: Option<u8>,
    depth: u8,
}

impl<S: BlockSource> Media<S> {
    pub fn new(source: S, header: Header) -> Self {
        Self { source, header }
    }

    pub fn payload(&mut self, record: Record) -> Result<Vec<u8>> {
        let zones: &[u64] = if record.key.kind == DATA {
            &[10, 11]
        } else {
            &[9]
        };
        let at = self
            .header
            .physical(record.address, record.length, zones, 16)?;
        let mut bytes = vec![0; record.length];
        self.source.read_at(at, &mut bytes)?;
        let check = if record.key.kind == INODE {
            require(bytes.len() == 128, "invalid HAMMER inode record length")?;
            &bytes[..112]
        } else {
            &bytes
        };
        require(
            self.header.checksum().valid(record.crc, check),
            "HAMMER record CRC mismatch",
        )?;
        Ok(bytes)
    }

    pub fn records(&mut self, wanted: RangeInclusive<Key>, as_of: u64) -> Result<Vec<Record>> {
        let order = self.header.order;
        let mut pending = vec![Branch {
            bounds: None,
            address: self.header.root,
            expected_parent: 0,
            expected_kind: None,
            depth: 0,
        }];
        let mut visited = HashSet::new();
        let mut matches = Vec::new();
        let mut bytes = [0; BTREE_BYTES];
        while let Some(branch) = pending.pop() {
            require(
                branch.depth <= 64 && visited.insert(branch.address),
                "HAMMER B-tree cycle or depth limit",
            )?;
            let address =
                self.header
                    .physical(branch.address, BTREE_BYTES, &[8], BTREE_BYTES as u64)?;
            self.source.read_at(address, &mut bytes)?;
            require(
                self.header.checksum().valid(order.u32(&bytes), &bytes[4..]),
                "HAMMER B-tree node CRC mismatch",
            )?;
            require(
                order.u64(&bytes[8..]) == branch.expected_parent,
                "unexpected HAMMER B-tree parent",
            )?;
            let count = order.u32(&bytes[16..]) as usize;
            let kind = bytes[20];
            require(
                branch.expected_kind.is_none_or(|expected| expected == kind),
                "unexpected HAMMER B-tree node type",
            )?;
            let elements = bytes[64..].as_chunks::<64>().0;
            match kind {
                b'L' => {
                    require(count <= elements.len(), "invalid HAMMER leaf count")?;
                    let mut previous = None;
                    for raw in &elements[..count] {
                        require(
                            matches!(raw[35], b'R' | b'D'),
                            "invalid HAMMER B-tree leaf element type",
                        )?;
                        let key = Key::decode(raw, order);
                        let created = order.u64(&raw[16..]);
                        let sorted_key = (key, if created == 0 { u64::MAX } else { created });
                        require(
                            previous.is_none_or(|previous| previous <= sorted_key)
                                && branch
                                    .bounds
                                    .as_ref()
                                    .is_none_or(|bounds| bounds.contains(&key)),
                            "HAMMER leaf keys are out of order or outside their branch",
                        )?;
                        previous = Some(sorted_key);
                        if raw[35] == b'R' && wanted.contains(&key) {
                            let record = Record::decode(raw, order)?;
                            if record.visible_at(as_of) {
                                matches.push(record)
                            }
                        }
                    }
                }
                b'I' => {
                    require(
                        count > 0 && count < elements.len(),
                        "invalid HAMMER internal node count",
                    )?;
                    for index in (0..count).rev() {
                        let left = &elements[index];
                        let first = Key::decode(left, order);
                        let last = Key::decode(&elements[index + 1], order);
                        require(
                            first <= last
                                && branch.bounds.as_ref().is_none_or(|bounds| {
                                    bounds.contains(&first) && bounds.contains(&last)
                                })
                                && matches!(left[35], b'I' | b'L'),
                            "invalid HAMMER B-tree child bounds",
                        )?;
                        if wanted.end() < &first || &last < wanted.start() {
                            continue;
                        }
                        let child = order.u64(&left[48..]);
                        pending.push(Branch {
                            bounds: Some(first..=last),
                            address: child,
                            expected_parent: branch.address,
                            expected_kind: Some(left[35]),
                            depth: branch.depth + 1,
                        });
                    }
                }
                _ => return Err(FoldError::Corrupt("unknown HAMMER B-tree node type")),
            }
        }
        require(
            matches.windows(2).all(|pair| pair[0].key < pair[1].key),
            "multiple visible HAMMER records or out-of-order leaf keys",
        )?;
        Ok(matches)
    }
}
