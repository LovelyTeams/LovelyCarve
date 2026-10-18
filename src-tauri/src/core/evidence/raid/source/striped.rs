//! Chunk-striped arrays: RAID0, RAID4, RAID5 and RAID6.
//!
//! `members` is slot-indexed (`members[i]` occupies RAID slot `i`). The caller
//! establishes that order from the superblock's role table — `dev_number`
//! alone is insufficient because md reuses device numbers across array events.
//!
//! Reads touch only data chunks. An absent column (`None`, or the slot passed
//! to [`RaidVolume::set_degraded`]) is rebuilt per stripe row from the
//! surviving columns: XOR with P for one missing data column, the Q syndrome
//! when P is gone too, and the two-erasure RAID6 solution for two missing data
//! columns. RAID0 has no redundancy, so every slot must be present.

use anyhow::bail;

use super::gf256;
use super::parity_layout::{place, ParityKind, Raid5Layout, Raid6Layout, RowPlacement};
use super::{present_member, smallest_present, StripedMember};
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
        if self.chunk_bytes == 0 || len == 0 {
            return Ok(Vec::new());
        }
        let total = self.total_bytes();
        if offset >= total {
            return Ok(Vec::new());
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
        let mut out = Vec::with_capacity(remaining);
        let mut cursor = offset;
        while remaining > 0 {
            let step = self.step(cursor, remaining);
            let bytes = match self.members.get(step.column) {
                Some(Some(member)) => member.read_exact(step.member_offset, step.take_len)?,
                Some(None) => self.reconstruct(&step)?,
                None => bail!("RAID column {} is outside the array", step.column),
            };
            out.extend_from_slice(&bytes);
            cursor += step.take_len as u64;
            remaining -= step.take_len;
        }
        Ok(out)
    }

    fn step(&self, cursor: u64, remaining: usize) -> StripeStep {
        let n = self.members.len() as u64;
        match self.level.parity_kind() {
            None => stripe_step(cursor, self.chunk_bytes, n, remaining),
            Some(kind) => parity_step(cursor, self.chunk_bytes, n, kind, remaining),
        }
    }

    /// Rebuild an absent data chunk of one stripe row from the surviving
    /// columns at the same member offset.
    fn reconstruct(&self, step: &StripeStep) -> anyhow::Result<Vec<u8>> {
        let Some(kind) = self.level.parity_kind() else {
            bail!(
                "RAID0 member slot {} is absent; striping has no redundancy",
                step.column
            );
        };
        let n = self.members.len();
        let row = RowPlacement::new(kind, n as u64, step.stripe);
        let absent: Vec<usize> = row
            .data
            .iter()
            .copied()
            .filter(|&column| self.members[column].is_none())
            .collect();
        let read = |column: usize| {
            present_member(&self.members, column)?.read_exact(step.member_offset, step.take_len)
        };
        let p_present = self.members[row.p].is_some();
        let q_present = row.q.is_some_and(|q| self.members[q].is_some());

        // P ⊕ every present data column = XOR of the absent data columns.
        let p_delta = || -> anyhow::Result<Vec<u8>> {
            let mut acc = read(row.p)?;
            for &column in row.data.iter().filter(|column| !absent.contains(column)) {
                xor_into(&mut acc, &read(column)?);
            }
            Ok(acc)
        };
        // Q ⊕ Σ g^e·D over present data columns = Σ g^e·D over absent ones.
        let q_delta = |q: usize| -> anyhow::Result<Vec<u8>> {
            let mut acc = read(q)?;
            for &column in row.data.iter().filter(|column| !absent.contains(column)) {
                let coefficient = gf256::pow2(row.q_exponent(column, n));
                gf256::mul_xor_into(&mut acc, &read(column)?, coefficient);
            }
            Ok(acc)
        };

        match (absent.as_slice(), p_present, q_present) {
            ([_], true, _) => p_delta(),
            ([missing], false, true) => {
                let Some(q) = row.q else {
                    bail!("stripe row {} has no Q column", step.stripe);
                };
                let mut bytes = q_delta(q)?;
                let coefficient = gf256::pow2(row.q_exponent(*missing, n));
                let Some(inverse) = gf256::inverse(coefficient) else {
                    bail!("RAID6 Q coefficient has no inverse");
                };
                gf256::scale(&mut bytes, inverse);
                Ok(bytes)
            }
            ([first, second], true, true) => {
                let Some(q) = row.q else {
                    bail!("stripe row {} has no Q column", step.stripe);
                };
                let (wanted, other) = if *first == step.column {
                    (*first, *second)
                } else {
                    (*second, *first)
                };
                // With x the wanted column and y the other:
                //   p = Dx ⊕ Dy,  q = g^ex·Dx ⊕ g^ey·Dy
                //   Dx = (q ⊕ g^ey·p) / (g^ex ⊕ g^ey)
                let g_wanted = gf256::pow2(row.q_exponent(wanted, n));
                let g_other = gf256::pow2(row.q_exponent(other, n));
                let p = p_delta()?;
                let mut bytes = q_delta(q)?;
                gf256::mul_xor_into(&mut bytes, &p, g_other);
                let Some(inverse) = gf256::inverse(g_wanted ^ g_other) else {
                    bail!("RAID6 columns {wanted} and {other} share a Q coefficient");
                };
                gf256::scale(&mut bytes, inverse);
                Ok(bytes)
            }
            _ => bail!(
                "stripe row {} cannot rebuild column {}: {} data column(s) absent, P {}, Q {}",
                step.stripe,
                step.column,
                absent.len(),
                if p_present { "present" } else { "absent" },
                if q_present { "present" } else { "absent" },
            ),
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
}

fn xor_into(target: &mut [u8], source: &[u8]) {
    for (byte, &value) in target.iter_mut().zip(source) {
        *byte ^= value;
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
