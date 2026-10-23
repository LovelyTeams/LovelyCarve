//! Segment summaries bind payload blocks to inode/virtual identities. Full
//! partial-log CRCs are checked on original source bytes, without recovery.
use crate::{crc, exact, header::Header, le16, le32, le64, require};
use lamfold::{BlockSource, FoldError, Result};
use std::collections::VecDeque;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Payload {
    DatData {
        logical: u64,
    },
    DatNode {
        key: u64,
        level: u8,
    },
    Data {
        owner: u64,
        virtual_block: u64,
        logical: u64,
    },
    Node {
        owner: u64,
        virtual_block: u64,
    },
    SuperRoot,
}
struct Log {
    start: u64,
    payload_start: u64,
    end: u64,
    sequence: u64,
    flags: u16,
    payloads: Vec<Payload>,
}
pub(crate) struct Logs {
    cache: VecDeque<Log>,
}
impl Logs {
    pub fn new() -> Self {
        Self {
            cache: VecDeque::new(),
        }
    }
    fn load(source: &mut impl BlockSource, header: &Header, start: u64) -> Result<Log> {
        let raw = header.read_block(source, start)?;
        require(
            le32(&raw, 8)? == 0x1eaffa11,
            "nilfs2: partial-log signature",
        )?;
        let bytes = le16(&raw, 12)? as usize;
        let flags = le16(&raw, 14)?;
        let sequence = le64(&raw, 16)?;
        let next = le64(&raw, 32)?;
        let count = le32(&raw, 40)? as u64;
        let files = le32(&raw, 44)? as usize;
        let summary_bytes = le32(&raw, 48)? as usize;
        let summary_blocks = (summary_bytes as u64).div_ceil(header.block_size as u64);
        let segment = start / header.blocks_per_segment as u64;
        let segment_end = (segment + 1) * header.blocks_per_segment as u64;
        require(
            bytes >= 64
                && bytes <= header.block_size as usize
                && flags & !0x1f == 0
                && sequence <= header.last_sequence
                && count >= 2
                && start
                    .checked_add(count)
                    .is_some_and(|end| end <= segment_end)
                && summary_bytes >= bytes
                && summary_blocks < count
                && files <= 1_000_000,
            "nilfs2: partial-log geometry/flags/sequence",
        )?;
        require(
            next < header.segments * header.blocks_per_segment as u64
                && next.is_multiple_of(header.blocks_per_segment as u64),
            "nilfs2: next-segment position/alignment",
        )?;
        require(
            count * header.block_size as u64 <= 64 * 1024 * 1024,
            "nilfs2: partial-log CRC work limit",
        )?;
        let mut summary = vec![0; summary_bytes];
        exact(source, header.block_offset(start)?, &mut summary)?;
        require(
            crc(header.crc_seed, &summary[8..]) == le32(&summary, 4)?,
            "nilfs2: summary CRC mismatch",
        )?;
        let mut hasher = crc32fast::Hasher::new_with_initial(!header.crc_seed);
        hasher.update(&raw[4..]);
        let mut buffer = vec![0; 64 * 1024];
        let mut at = header.block_offset(start)? + header.block_size as u64;
        let end = header.block_offset(start)? + count * header.block_size as u64;
        while at < end {
            let take = buffer.len().min((end - at) as usize);
            exact(source, at, &mut buffer[..take])?;
            hasher.update(&buffer[..take]);
            at += take as u64;
        }
        require(
            !hasher.finalize() == le32(&raw, 0)?,
            "nilfs2: log payload CRC mismatch",
        )?;
        let mut cursor = Cursor {
            raw: &summary,
            at: bytes,
            block_size: header.block_size as usize,
        };
        let mut payloads = Vec::new();
        let payload_count = count - summary_blocks;
        for _ in 0..files {
            let info = cursor.take(24)?;
            let owner = le64(info, 0)?;
            let count = le32(info, 16)? as usize;
            let data = le32(info, 20)? as usize;
            require(
                owner != 0
                    && count != 0
                    && data <= count
                    && payloads.len() as u64 + count as u64 <= payload_count,
                "nilfs2: file-summary payload count/owner",
            )?;
            for _ in 0..data {
                payloads.push(if owner == 3 {
                    Payload::DatData {
                        logical: le64(cursor.take(8)?, 0)?,
                    }
                } else {
                    let record = cursor.take(16)?;
                    Payload::Data {
                        owner,
                        virtual_block: le64(record, 0)?,
                        logical: le64(record, 8)?,
                    }
                });
            }
            for _ in data..count {
                payloads.push(if owner == 3 {
                    let record = cursor.take(16)?;
                    require(
                        (1..14).contains(&record[8]),
                        "nilfs2: DAT node summary level",
                    )?;
                    Payload::DatNode {
                        key: le64(record, 0)?,
                        level: record[8],
                    }
                } else {
                    Payload::Node {
                        owner,
                        virtual_block: le64(cursor.take(8)?, 0)?,
                    }
                });
            }
        }
        if flags & 4 != 0 {
            payloads.push(Payload::SuperRoot);
        }
        require(
            payloads.len() as u64 == payload_count && cursor.at == summary_bytes,
            "nilfs2: summary/payload coverage",
        )?;
        Ok(Log {
            start,
            payload_start: start + summary_blocks,
            end: start + count,
            sequence,
            flags,
            payloads,
        })
    }
    fn insert(&mut self, log: Log) {
        while self.cache.len() >= 64
            || self
                .cache
                .iter()
                .map(|cached| cached.payloads.len())
                .sum::<usize>()
                + log.payloads.len()
                > 262144
        {
            self.cache.pop_front();
        }
        self.cache.push_back(log);
    }
    pub fn latest_root(&mut self, source: &mut impl BlockSource, header: &Header) -> Result<u64> {
        let log = Self::load(source, header, header.last_partial_segment)?;
        require(
            log.sequence == header.last_sequence
                && log.flags & 6 == 6
                && log.payloads.last() == Some(&Payload::SuperRoot),
            "nilfs2: latest log/super-root checkpoint boundary",
        )?;
        let root = log.end - 1;
        self.insert(log);
        Ok(root)
    }
    pub fn identity(
        &mut self,
        source: &mut impl BlockSource,
        header: &Header,
        block: u64,
    ) -> Result<Payload> {
        header.block_offset(block)?;
        if let Some(log) = self
            .cache
            .iter()
            .find(|log| log.start <= block && block < log.end)
        {
            return log
                .payloads
                .get(block.checked_sub(log.payload_start).unwrap_or(u64::MAX) as usize)
                .copied()
                .ok_or(FoldError::Corrupt(
                    "nilfs2: file pointer enters log summary",
                ));
        }
        let segment = block / header.blocks_per_segment as u64;
        let mut start = if segment == 0 {
            header.first_data_block
        } else {
            segment * header.blocks_per_segment as u64
        };
        let mut scanned = 0;
        loop {
            require(scanned < 131072, "nilfs2: partial-log scan work limit")?;
            scanned += 1;
            let log = Self::load(source, header, start)?;
            let end = log.end;
            self.insert(log);
            if block < end {
                return self.identity(source, header, block);
            }
            start = end;
        }
    }
}
struct Cursor<'a> {
    raw: &'a [u8],
    at: usize,
    block_size: usize,
}
impl<'a> Cursor<'a> {
    fn take(&mut self, bytes: usize) -> Result<&'a [u8]> {
        let remaining = self.block_size - self.at % self.block_size;
        if bytes > remaining {
            self.at += remaining;
        }
        let end = self
            .at
            .checked_add(bytes)
            .ok_or(FoldError::Corrupt("nilfs2: summary cursor overflow"))?;
        let raw = self
            .raw
            .get(self.at..end)
            .ok_or(FoldError::Corrupt("nilfs2: truncated summary record"))?;
        self.at = end;
        Ok(raw)
    }
}
