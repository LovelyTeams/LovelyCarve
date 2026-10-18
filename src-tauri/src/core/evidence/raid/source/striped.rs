//! Chunk-striped arrays: RAID0, RAID4, RAID5 and RAID6.
//!
//! `members` is slot-indexed (`members[i]` occupies RAID slot `i`). The caller
//! establishes that order from the superblock's role table — `dev_number`
//! alone is insufficient because md reuses device numbers across array events.
//!
//! Reads touch only data chunks. An absent column (`None`, or the slot passed
//! to [`RaidVolume::set_degraded`]) is rebuilt per stripe row from the
//! surviving columns ([`super::rebuild`]): XOR with P for one missing data
//! column, the Q syndrome when P is gone too, and the two-erasure RAID6
//! solution for two missing data columns. RAID0 has no redundancy, so every
//! slot must be present.
//!
//! Bytes a member synthesized (zero-filled bad sectors) count as erased too:
//! a data chunk read with synthesized bytes is repaired from parity where the
//! row's redundancy covers them, and a rebuild never computes data from
//! synthesized bytes. What no authentic column can supply stays zero and is
//! reported at its logical offset.

use std::ops::Range;

use anyhow::bail;

use super::parity_layout::{place, ParityKind, Raid5Layout, Raid6Layout, RowPlacement};
use super::rebuild::{self, Plan};
use super::traced::{RepairLog, TracedBytes, TracedRead};
use super::{present_member, smallest_present, StripedMember};
use crate::core::evidence::image::BadRegion;
use crate::core::evidence::partitions::SectorSource;

/// Striped level and its parity rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RaidLevel {
    /// Striped without parity. No rotation.
    Raid0,
    /// Dedicated parity on the last column.
    Raid4,
    /// One rotating parity column; the layout is md's `layout` field.
    Raid5(Raid5Layout),
    /// P and Q parity columns; the layout is md's `layout` field.
    Raid6(Raid6Layout),
}

impl RaidLevel {
    /// Build a striped level from md's raw `level` + `layout`. Returns `None`
    /// for non-striped levels (linear / RAID1 / RAID10) and unknown layouts.
    pub fn from_raw(level: i32, layout: u32) -> Option<Self> {
        match level {
            0 => Some(Self::Raid0),
            4 => Some(Self::Raid4),
            5 => Raid5Layout::from_raw(layout).map(Self::Raid5),
            6 => Raid6Layout::from_raw(layout).map(Self::Raid6),
            _ => None,
        }
    }

    pub fn parity_disks(&self) -> usize {
        match self {
            Self::Raid0 => 0,
            Self::Raid4 | Self::Raid5(_) => 1,
            Self::Raid6(_) => 2,
        }
    }

    /// Smallest member count the layout math is defined for (md's minimum
    /// for RAID6).
    pub fn min_members(&self) -> usize {
        match self {
            Self::Raid0 => 1,
            Self::Raid4 | Self::Raid5(_) => 2,
            Self::Raid6(_) => 4,
        }
    }

    fn parity_kind(&self) -> Option<ParityKind> {
        match self {
            Self::Raid0 => None,
            Self::Raid4 => Some(ParityKind::Raid4),
            Self::Raid5(layout) => Some(ParityKind::Raid5(*layout)),
            Self::Raid6(layout) => Some(ParityKind::Raid6(*layout)),
        }
    }

    /// Logical capacity: whole chunks of the smallest present member times
    /// the data columns, the way md sizes an array (a partial last chunk is
    /// not addressable).
    pub fn capacity_bytes(&self, members: &[Option<StripedMember>], chunk_bytes: u64) -> u64 {
        let n = members.len();
        if chunk_bytes == 0 || n < self.min_members() {
            return 0;
        }
        let per_member = smallest_present(members).unwrap_or(0);
        let per_member = per_member - per_member % chunk_bytes;
        per_member.saturating_mul((n - self.parity_disks()) as u64)
    }
}

/// Virtual RAID0 / RAID4 / RAID5 / RAID6 reader.
pub struct RaidVolume {
    level: RaidLevel,
    sector_size: u32,
    members: Vec<Option<StripedMember>>,
    chunk_bytes: u64,
    /// Logical capacity reported to the filesystem layer: `Some` honours a
    /// size recorded in metadata, `None` derives it from the members.
    logical_size_bytes: Option<u64>,
    repair_log: RepairLog,
}

impl RaidVolume {
    /// Construct a reader over fully present members.
    pub fn set(
        level: RaidLevel,
        sector_size: u32,
        chunk_bytes: u64,
        logical_size_bytes: Option<u64>,
        members: Vec<StripedMember>,
    ) -> Self {
        Self::set_degraded(
            level,
            sector_size,
            chunk_bytes,
            logical_size_bytes,
            members,
            None,
        )
    }

    /// Like [`set`](Self::set) but treats `missing_slot` as absent: its reader
    /// may still exist (a dropped disk with pre-dropout data) but is never read,
    /// and its chunks are rebuilt from parity. Out-of-range slots are ignored.
    pub fn set_degraded(
        level: RaidLevel,
        sector_size: u32,
        chunk_bytes: u64,
        logical_size_bytes: Option<u64>,
        members: Vec<StripedMember>,
        missing_slot: Option<usize>,
    ) -> Self {
        let members = members
            .into_iter()
            .enumerate()
            .map(|(slot, member)| (missing_slot != Some(slot)).then_some(member))
            .collect();
        Self::from_slots(level, sector_size, chunk_bytes, logical_size_bytes, members)
    }

    /// Construct a reader over slot-indexed members where `None` is absent.
    pub fn from_slots(
        level: RaidLevel,
        sector_size: u32,
        chunk_bytes: u64,
        logical_size_bytes: Option<u64>,
        members: Vec<Option<StripedMember>>,
    ) -> Self {
        Self {
            level,
            sector_size,
            members,
            chunk_bytes,
            logical_size_bytes,
            repair_log: RepairLog::default(),
        }
    }

    /// First absent slot, if any.
    pub fn missing_slot(&self) -> Option<usize> {
        self.members.iter().position(Option::is_none)
    }

    pub fn level(&self) -> RaidLevel {
        self.level
    }

    pub fn sector_size(&self) -> u32 {
        self.sector_size
    }

    pub fn chunk_bytes(&self) -> u64 {
        self.chunk_bytes
    }

    pub fn members(&self) -> &[Option<StripedMember>] {
        &self.members
    }

    /// Total logical bytes addressable through this volume.
    pub fn total_bytes(&self) -> u64 {
        self.logical_size_bytes
            .unwrap_or_else(|| self.level.capacity_bytes(&self.members, self.chunk_bytes))
    }

    /// Inherent reader; named `read_at` to leave `read` for the
    /// `SectorSource` trait impl.
    pub fn read_at(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_traced_at(offset, len).map(|(bytes, _)| bytes)
    }

    /// [`Self::read_at`] plus the synthesized spans, as logical offsets.
    pub fn read_traced_at(
        &self,
        offset: u64,
        len: usize,
    ) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        if self.chunk_bytes == 0 || len == 0 {
            return Ok((Vec::new(), Vec::new()));
        }
        let total = self.total_bytes();
        if offset >= total {
            return Ok((Vec::new(), Vec::new()));
        }
        let n = self.members.len();
        if n < self.level.min_members() {
            bail!(
                "{:?} needs at least {} members, got {n}",
                self.level,
                self.level.min_members()
            );
        }
        let mut remaining = (total - offset).min(len as u64) as usize;
        let mut out = TracedRead::new(offset, remaining);
        let mut cursor = offset;
        while remaining > 0 {
            let step = self.step(cursor, remaining);
            let piece = match self.members.get(step.column) {
                Some(Some(member)) => {
                    let mut piece = member.read_exact_traced(step.member_offset, step.take_len)?;
                    if !piece.is_authentic() && self.level.parity_kind().is_some() {
                        let repaired = piece.repair_from(|range| self.rebuild(&step, range));
                        if repaired > 0 {
                            let device = format!("{:?}", self.level);
                            self.repair_log.note(&device, step.column, cursor, repaired);
                        }
                    }
                    piece
                }
                Some(None) => self.rebuild(&step, 0..step.take_len)?,
                None => bail!("RAID column {} is outside the array", step.column),
            };
            out.push(piece);
            cursor += step.take_len as u64;
            remaining -= step.take_len;
        }
        Ok(out.finish())
    }

    fn step(&self, cursor: u64, remaining: usize) -> StripeStep {
        let n = self.members.len() as u64;
        match self.level.parity_kind() {
            None => stripe_step(cursor, self.chunk_bytes, n, remaining),
            Some(kind) => parity_step(cursor, self.chunk_bytes, n, kind, remaining),
        }
    }

    /// Rebuild `range` (step-relative) of the step's data chunk from the rest
    /// of its stripe row, with the chunk itself and absent members erased.
    /// Fails when absent members alone leave the row without the redundancy
    /// to rebuild it; spans only synthesized member bytes leave uncovered
    /// come back synthesized.
    fn rebuild(&self, step: &StripeStep, range: Range<usize>) -> anyhow::Result<TracedBytes> {
        let Some(kind) = self.level.parity_kind() else {
            bail!(
                "RAID0 member slot {} is absent; striping has no redundancy",
                step.column
            );
        };
        let n = self.members.len();
        let row = RowPlacement::new(kind, n as u64, step.stripe);
        let unread = |column: usize| {
            column == step.column || !matches!(self.members.get(column), Some(Some(_)))
        };
        let Some(plan) = rebuild::plan(&row, step.column, unread) else {
            let absent_data = row.data.iter().filter(|&&column| unread(column)).count();
            bail!(
                "stripe row {} cannot rebuild column {}: {absent_data} data column(s) unavailable, P {}, Q {}",
                step.stripe,
                step.column,
                if unread(row.p) { "absent" } else { "present" },
                if row.q.is_some_and(|q| !unread(q)) { "present" } else { "absent" },
            );
        };

        let member_offset = step.member_offset + range.start as u64;
        let len = range.len();
        let mut columns: Vec<Option<TracedBytes>> = (0..n).map(|_| None).collect();
        let read = |column: usize| {
            present_member(&self.members, column)?.read_exact_traced(member_offset, len)
        };
        // Q only when the plan needs it: a single erasure with P present is
        // the XOR of the row, as md computes it.
        let reads_q = matches!(plan, Plan::FromQ | Plan::FromPq { .. });
        let needed = row
            .data
            .iter()
            .copied()
            .chain([row.p])
            .chain(row.q.filter(|_| reads_q));
        for column in needed.filter(|&column| !unread(column)) {
            columns[column] = Some(read(column)?);
        }
        let rebuilt = rebuild::rebuild(&row, n, step.column, &columns, len)?;
        if rebuilt.is_authentic() || reads_q {
            return Ok(rebuilt);
        }
        // Synthesized bytes in another column may still be covered by Q.
        let Some(q) = row.q.filter(|&q| !unread(q)) else {
            return Ok(rebuilt);
        };
        match read(q) {
            Ok(bytes) => {
                columns[q] = Some(bytes);
                rebuild::rebuild(&row, n, step.column, &columns, len)
            }
            Err(error) => {
                tracing::debug!(
                    "RAID6 Q column {q} unavailable for stripe row {} ({error:#})",
                    step.stripe
                );
                Ok(rebuilt)
            }
        }
    }
}

impl SectorSource for RaidVolume {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_at(offset, len)
    }
    fn size(&self) -> u64 {
        self.total_bytes()
    }
    fn read_traced(&self, offset: u64, len: usize) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        self.read_traced_at(offset, len)
    }
}

/// One contiguous piece of a read: `take_len` bytes at `member_offset` of the
/// member in `column`, inside stripe row `stripe`.
pub(crate) struct StripeStep {
    pub column: usize,
    pub member_offset: u64,
    pub take_len: usize,
    pub stripe: u64,
}

/// RAID0: chunk `c` goes to member `c % n`, at member offset
/// `(c / n) * chunk_bytes + offset_within_chunk`.
pub(crate) fn stripe_step(cursor: u64, chunk_bytes: u64, n: u64, remaining: usize) -> StripeStep {
    let chunk_index = cursor / chunk_bytes;
    let in_chunk = cursor % chunk_bytes;
    let stripe = chunk_index / n;
    StripeStep {
        column: (chunk_index % n) as usize,
        member_offset: stripe * chunk_bytes + in_chunk,
        take_len: (chunk_bytes - in_chunk).min(remaining as u64) as usize,
        stripe,
    }
}

/// RAID4/5/6: logical chunk `c` is data position `c % data_disks` of stripe
/// row `c / data_disks`; the layout picks the column.
pub(crate) fn parity_step(
    cursor: u64,
    chunk_bytes: u64,
    n: u64,
    kind: ParityKind,
    remaining: usize,
) -> StripeStep {
    let data_disks = n - kind.parity_disks();
    let logical_chunk = cursor / chunk_bytes;
    let in_chunk = cursor % chunk_bytes;
    let stripe = logical_chunk / data_disks;
    let position = logical_chunk % data_disks;
    StripeStep {
        column: place(kind, n, stripe, position).column,
        member_offset: stripe * chunk_bytes + in_chunk,
        take_len: (chunk_bytes - in_chunk).min(remaining as u64) as usize,
        stripe,
    }
}

/// Encode `logical` onto `n` members the way md writes it: data chunks placed
/// by the layout, P the XOR of the row's data, Q its GF(2^8) syndrome. Members
/// hold whole stripe rows; a short last row is zero-padded.
#[cfg(test)]
pub(crate) fn encode_striped(
    level: RaidLevel,
    logical: &[u8],
    n: usize,
    chunk: usize,
) -> Vec<Vec<u8>> {
    use super::gf256;
    use super::rebuild::xor_into;

    let Some(kind) = level.parity_kind() else {
        let rows = logical.len().div_ceil(chunk * n);
        let mut members = vec![vec![0u8; rows * chunk]; n];
        for (index, block) in logical.chunks(chunk).enumerate() {
            let offset = (index / n) * chunk;
            members[index % n][offset..offset + block.len()].copy_from_slice(block);
        }
        return members;
    };
    let data_disks = n - level.parity_disks();
    let rows = logical.len().div_ceil(chunk * data_disks);
    let mut members = vec![vec![0u8; rows * chunk]; n];
    for stripe in 0..rows {
        let row = RowPlacement::new(kind, n as u64, stripe as u64);
        let mut p = vec![0u8; chunk];
        let mut q = vec![0u8; chunk];
        for (position, &column) in row.data.iter().enumerate() {
            let start = (stripe * data_disks + position) * chunk;
            let mut block = vec![0u8; chunk];
            if start < logical.len() {
                let end = (start + chunk).min(logical.len());
                block[..end - start].copy_from_slice(&logical[start..end]);
            }
            let offset = stripe * chunk;
            members[column][offset..offset + chunk].copy_from_slice(&block);
            xor_into(&mut p, &block);
            gf256::mul_xor_into(&mut q, &block, gf256::pow2(row.q_exponent(column, n)));
        }
        let offset = stripe * chunk;
        members[row.p][offset..offset + chunk].copy_from_slice(&p);
        if let Some(column) = row.q {
            members[column][offset..offset + chunk].copy_from_slice(&q);
        }
    }
    members
}

#[cfg(test)]
#[path = "striped_tests.rs"]
mod tests;
