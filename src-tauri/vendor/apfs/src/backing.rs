//! Bound all object and data reads to container geometry, even on a larger source.
use crate::error::{ApfsError, Result};
use crate::fusion::{CacheRange, FusionTier, SourceSpan, TIER2_OFFSET};
use std::io::{self, Read, Seek, SeekFrom};

pub(crate) struct ContainerReader<R> {
    source: R,
    length: u64,
    tier2: Option<(R, u64)>,
    cache: Vec<CacheRange>,
    position: u64,
}

impl<R> ContainerReader<R> {
    pub(crate) fn new(source: R, length: u64) -> Self {
        Self {
            source,
            length,
            tier2: None,
            cache: Vec::new(),
            position: 0,
        }
    }

    pub(crate) fn new_fusion(main: R, main_len: u64, tier2: R, tier2_len: u64) -> Self {
        let mut reader = Self::new(main, main_len);
        reader.tier2 = Some((tier2, tier2_len));
        reader
    }

    pub(crate) fn limit_fusion(
        &mut self,
        main_blocks: u64,
        tier2_blocks: u64,
        block_size: u32,
    ) -> Result<()> {
        let bytes = |blocks: u64, limit: u64| {
            blocks
                .checked_mul(u64::from(block_size))
                .filter(|size| *size != 0 && *size <= limit)
                .ok_or_else(|| {
                    ApfsError::CorruptedData(
                        "APFS allocated device size exceeds Fusion source".into(),
                    )
                })
        };
        let (_, length) = self
            .tier2
            .as_mut()
            .ok_or_else(|| ApfsError::CorruptedData("missing Fusion tier 2".into()))?;
        self.length = bytes(main_blocks, self.length)?;
        *length = bytes(tier2_blocks, *length)?;
        Ok(())
    }

    pub(crate) fn install_fusion_cache(&mut self, ranges: Vec<CacheRange>) {
        self.cache = ranges;
    }

    pub(crate) fn range_limits(&self) -> (u64, Option<u64>) {
        (self.length, self.tier2.as_ref().map(|(_, size)| *size))
    }

    pub(crate) fn source_spans(&self, start: u64, length: u64) -> Result<Vec<SourceSpan>> {
        let (main, tier2) = self.range_limits();
        if !contains(start, length, main, tier2) {
            return Err(ApfsError::CorruptedData(
                "APFS extent exceeds its device".into(),
            ));
        }
        let end = start + length;
        let mut position = start;
        let mut spans = Vec::new();
        while position < end {
            let span = self.span(position, end - position);
            position += span.length;
            spans.push(span);
        }
        Ok(spans)
    }

    fn span(&self, position: u64, wanted: u64) -> SourceSpan {
        if position < TIER2_OFFSET || self.tier2.is_none() {
            return SourceSpan {
                tier: FusionTier::Main,
                offset: position,
                length: wanted,
            };
        }
        let index = self.cache.partition_point(|range| range.end <= position);
        if let Some(range) = self.cache.get(index) {
            if position >= range.start {
                return SourceSpan {
                    tier: FusionTier::Main,
                    offset: range.main + position - range.start,
                    length: wanted.min(range.end - position),
                };
            }
            return SourceSpan {
                tier: FusionTier::Secondary,
                offset: position - TIER2_OFFSET,
                length: wanted.min(range.start - position),
            };
        }
        SourceSpan {
            tier: FusionTier::Secondary,
            offset: position - TIER2_OFFSET,
            length: wanted,
        }
    }
}

pub(crate) fn contains(start: u64, length: u64, main: u64, tier2: Option<u64>) -> bool {
    match (start.checked_sub(TIER2_OFFSET), tier2) {
        (Some(offset), Some(tier2)) => offset.checked_add(length).is_some_and(|end| end <= tier2),
        _ => start.checked_add(length).is_some_and(|end| end <= main),
    }
}

impl<R: Read + Seek> Read for ContainerReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let remaining = match (self.position.checked_sub(TIER2_OFFSET), self.tier2.as_ref()) {
            (Some(offset), Some((_, length))) => length.saturating_sub(offset),
            _ => self.length.saturating_sub(self.position),
        };
        let wanted = remaining.min(buffer.len() as u64);
        if wanted == 0 {
            return Ok(0);
        }
        let span = self.span(self.position, wanted);
        let source = match span.tier {
            FusionTier::Main => &mut self.source,
            FusionTier::Secondary => {
                &mut self.tier2.as_mut().ok_or(io::ErrorKind::UnexpectedEof)?.0
            }
        };
        source.seek(SeekFrom::Start(span.offset))?;
        let count = source.read(&mut buffer[..span.length as usize])?;
        self.position += count as u64;
        Ok(count)
    }
}

impl<R: Read + Seek> Seek for ContainerReader<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let target = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
            SeekFrom::End(delta) => {
                let end = self
                    .tier2
                    .as_ref()
                    .map_or(self.length, |(_, size)| TIER2_OFFSET + size);
                i128::from(end) + i128::from(delta)
            }
        };
        self.position = u64::try_from(target).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "APFS container seek out of range",
            )
        })?;
        Ok(self.position)
    }
}
