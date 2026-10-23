use crate::{allocation::Allocation, exact, inode::Inode, require, Header, Run};
use lamfold::{BlockSource, FoldError, Result};

/// Allocated file ranges in logical order; preallocation is retained and
/// validated but never exposed past the recorded logical length.
#[derive(Clone, Debug)]
pub(crate) struct Map {
    pub length: u64,
    pub ranges: Vec<(u64, u64)>,
    pub allocations: Vec<(u64, u64)>,
    starts: Vec<u64>,
}
impl Map {
    pub fn parse(
        source: &mut impl BlockSource,
        header: &Header,
        allocation: &mut Allocation,
        inode: &Inode,
    ) -> Result<Self> {
        let owner = inode.number;
        let raw = &inode.stream;
        let endian = header.endian;
        require(raw.len() == 144, "befs: stream header size")?;
        let bounds = [
            endian.nonnegative(raw, 96)?,
            endian.nonnegative(raw, 112)?,
            endian.nonnegative(raw, 128)?,
        ];
        let length = endian.nonnegative(raw, 136)?;
        let mut plan = Builder {
            source,
            header,
            allocation,
            map: Self {
                length,
                ranges: Vec::new(),
                allocations: vec![(owner, 1)],
                starts: Vec::new(),
            },
            metadata_pages: 0,
        };
        for node in [Some(header.root), header.indices, inode.attributes]
            .into_iter()
            .flatten()
        {
            if !plan.map.allocations.iter().any(|&(first, _)| first == node) {
                plan.map.allocations.push((node, 1));
            }
        }
        let direct = plan.runs(&raw[..96], None)?;
        require(
            direct == bounds[0],
            "befs: direct range disagrees with runs",
        )?;
        let indirect = Run::parse(endian, raw, 104)?;
        let double = Run::parse(endian, raw, 120)?;
        let mut total = direct;
        if indirect.is_zero() {
            require(bounds[1] == 0, "befs: indirect bound without array")?;
        } else {
            total = total
                .checked_add(plan.array(indirect, None, false)?)
                .ok_or(FoldError::Corrupt("befs: indirect length overflow"))?;
            require(
                bounds[1] == total,
                "befs: indirect boundary disagrees with runs",
            )?;
        }
        if double.is_zero() {
            require(bounds[2] == 0, "befs: double-indirect bound without array")?;
        } else {
            require(
                !indirect.is_zero(),
                "befs: double-indirect without indirect boundary",
            )?;
            total = total
                .checked_add(plan.array(double, Some(double.length), true)?)
                .ok_or(FoldError::Corrupt("befs: double length overflow"))?;
            require(
                bounds[2] == total,
                "befs: double boundary disagrees with runs",
            )?;
        }
        require(
            length <= total && total <= header.blocks * u64::from(header.block_size),
            "befs: missing logical coverage or oversized stream",
        )?;
        let mut physical = plan.map.allocations.clone();
        physical.sort_unstable();
        require(
            physical
                .windows(2)
                .all(|pair| pair[0].0 + pair[0].1 <= pair[1].0),
            "befs: overlapping file data/inode/metadata runs",
        )?;
        let mut position = 0;
        for &(_, count) in &plan.map.ranges {
            plan.map.starts.push(position);
            position += count * u64::from(header.block_size);
        }
        Ok(plan.map)
    }
    pub fn read(
        &self,
        source: &mut impl BlockSource,
        header: &Header,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<usize> {
        let expected = self.length.saturating_sub(offset).min(bytes.len() as u64) as usize;
        if expected == 0 {
            return Ok(0);
        }
        let index = self
            .starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1);
        let mut logical = *self
            .starts
            .get(index)
            .ok_or(FoldError::Corrupt("befs: missing map prefix"))?;
        let mut done = 0;
        for &(first, blocks) in &self.ranges[index..] {
            if done == expected {
                break;
            }
            let length = blocks * u64::from(header.block_size);
            let position = offset + done as u64;
            if position < logical + length {
                let skip = position.saturating_sub(logical);
                let count = (length - skip).min((expected - done) as u64) as usize;
                exact(
                    source,
                    first * u64::from(header.block_size) + skip,
                    &mut bytes[done..done + count],
                )?;
                done += count;
            }
            logical += length;
        }
        require(done == expected, "befs: short logical read")?;
        Ok(done)
    }
    pub fn exact(
        &self,
        source: &mut impl BlockSource,
        header: &Header,
        offset: u64,
        bytes: &mut [u8],
    ) -> Result<()> {
        require(
            self.read(source, header, offset, bytes)? == bytes.len(),
            "befs: short metadata stream",
        )
    }
}
struct Builder<'a, S> {
    source: &'a mut S,
    header: &'a Header,
    allocation: &'a mut Allocation,
    map: Map,
    metadata_pages: usize,
}
impl<S: BlockSource> Builder<'_, S> {
    fn allocated(&mut self, run: Run) -> Result<(u64, u64)> {
        let (first, count) = self.header.run(run)?;
        self.header.data_region(first, count)?;
        self.allocation
            .check(self.source, self.header, first, count)?;
        require(
            self.map.allocations.len() < 1_000_000,
            "befs: file mapping resource limit",
        )?;
        self.map.allocations.push((first, count));
        Ok((first, count))
    }
    fn runs(&mut self, raw: &[u8], unit: Option<u16>) -> Result<u64> {
        let mut total = 0u64;
        let mut padding = false;
        for offset in (0..raw.len()).step_by(8) {
            let run = Run::parse(self.header.endian, raw, offset)?;
            if run.is_zero() {
                padding = true;
                continue;
            }
            require(
                !padding && unit.is_none_or(|unit| unit == run.length),
                "befs: array gap or double-indirect run length",
            )?;
            let (first, count) = self.allocated(run)?;
            total = total
                .checked_add(count * u64::from(self.header.block_size))
                .ok_or(FoldError::Corrupt("befs: stream size overflow"))?;
            if let Some(last) = self
                .map
                .ranges
                .last_mut()
                .filter(|last| last.0 + last.1 == first)
            {
                last.1 += count;
            } else {
                self.map.ranges.push((first, count));
            }
        }
        Ok(total)
    }
    fn array(&mut self, run: Run, unit: Option<u16>, double: bool) -> Result<u64> {
        if double {
            require(unit == Some(run.length), "befs: double array unit")?;
        }
        let (first, count) = self.allocated(run)?;
        let mut total = 0u64;
        for block in first..first + count {
            self.metadata_pages += 1;
            require(
                self.metadata_pages <= 131072,
                "befs: metadata page read limit",
            )?;
            let mut raw = vec![0; self.header.block_size as usize];
            exact(
                self.source,
                block * u64::from(self.header.block_size),
                &mut raw,
            )?;
            if double {
                let mut padding = false;
                for offset in (0..raw.len()).step_by(8) {
                    let child = Run::parse(self.header.endian, &raw, offset)?;
                    if child.is_zero() {
                        padding = true;
                        continue;
                    }
                    require(
                        !padding && unit == Some(child.length),
                        "befs: double-indirect array gap/length",
                    )?;
                    total = total
                        .checked_add(self.array(child, unit, false)?)
                        .ok_or(FoldError::Corrupt("befs: double array overflow"))?;
                }
            } else {
                total = total
                    .checked_add(self.runs(&raw, unit)?)
                    .ok_or(FoldError::Corrupt("befs: array size overflow"))?;
            }
        }
        Ok(total)
    }
}
