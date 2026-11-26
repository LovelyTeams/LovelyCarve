//! Bind physical pointers to the member named by their on-disk device index.

use crate::superblock::{Member, Superblock, FIELD_CLEAN};
use crate::{Error, Result};
use fs_core::BlockRead;
use std::collections::{BTreeMap, BTreeSet};

pub fn member_geometry(sb: &Superblock) -> Result<(Member, u64)> {
    let member = sb
        .members()?
        .into_iter()
        .nth(usize::from(sb.dev_idx))
        .ok_or_else(|| Error::Corrupt("no member entry for this device".into()))?;
    let length = member
        .nbuckets
        .checked_mul(u64::from(member.bucket_size) * 512)
        .filter(|bytes| *bytes != 0 && u64::from(member.first_bucket) < member.nbuckets)
        .ok_or_else(|| Error::Corrupt("invalid member geometry".into()))?;
    Ok((member, length))
}

/// Select the newest shared state after checking every configured member.
/// Removed slots have an empty UUID and are not member devices.
pub fn select_superblock(headers: &[Superblock]) -> Result<usize> {
    let newest = headers
        .iter()
        .enumerate()
        .max_by_key(|(_, sb)| (sb.seq, std::cmp::Reverse(sb.dev_idx)))
        .ok_or_else(|| Error::NotFound("no filesystem members".into()))?;
    let authoritative = newest.1;
    if authoritative.nr_devices == 1 && headers.len() == 1 && authoritative.dev_idx == 0 {
        return Ok(newest.0);
    }
    let members = authoritative.members()?;
    let expected = members
        .iter()
        .filter(|member| member.uuid != [0; 16])
        .count();
    let mut seen = BTreeSet::new();
    for sb in headers {
        if sb.uuid != authoritative.uuid
            || sb.user_uuid != authoritative.user_uuid
            || sb.version != authoritative.version
            || sb.block_size != authoritative.block_size
            || sb.btree_node_size() != authoritative.btree_node_size()
            || !same_topology(&sb.members()?, &members)
        {
            return Err(Error::Corrupt(
                "members disagree on filesystem identity or geometry".into(),
            ));
        }
        let member = members
            .get(usize::from(sb.dev_idx))
            .filter(|member| member.uuid != [0; 16] && seen.insert(sb.dev_idx));
        let Some(member) = member else {
            return Err(Error::Corrupt(format!(
                "duplicate or inactive device {}",
                sb.dev_idx
            )));
        };
        if sb.seq != member.last_sb_seq.unwrap_or(authoritative.seq) {
            return Err(Error::Corrupt(format!(
                "unexpected superblock sequence {} on device {}",
                sb.seq, sb.dev_idx
            )));
        }
        if sb.seq == authoritative.seq
            && (sb.is_clean() != authoritative.is_clean()
                || sb.field(FIELD_CLEAN) != authoritative.field(FIELD_CLEAN))
        {
            return Err(Error::Corrupt(
                "members disagree on the clean filesystem state".into(),
            ));
        }
    }
    let unique = members
        .iter()
        .map(|member| member.uuid)
        .filter(|uuid| *uuid != [0; 16])
        .collect::<BTreeSet<_>>();
    if unique.len() != expected {
        return Err(Error::Corrupt(
            "filesystem member UUIDs are not unique".into(),
        ));
    }
    if headers.len() != expected {
        return Err(Error::Unsupported(format!(
            "filesystem needs all {expected} members; {} available",
            headers.len()
        )));
    }
    Ok(newest.0)
}

fn same_topology(left: &[Member], right: &[Member]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            left.uuid == right.uuid
                && left.nbuckets == right.nbuckets
                && left.first_bucket == right.first_bucket
                && left.bucket_size == right.bucket_size
        })
}

pub(crate) trait MemberRead {
    fn resolve(&self, index: u8) -> Result<(&dyn BlockRead, &Member, u64)>;
}

pub(crate) struct DeviceSet<D> {
    entries: BTreeMap<u8, (D, Superblock, Member, u64)>,
}

impl<D: BlockRead> DeviceSet<D> {
    pub(crate) fn open(sources: Vec<D>) -> Result<(Self, Superblock)> {
        let headers = sources
            .iter()
            .map(|source| Superblock::read(source))
            .collect::<Result<Vec<_>>>()?;
        let selected = headers[select_superblock(&headers)?].clone();
        let mut entries = BTreeMap::new();
        for (source, header) in sources.into_iter().zip(headers) {
            let (member, length) = member_geometry(&header)?;
            if length > source.size_bytes() {
                return Err(Error::Corrupt(format!(
                    "device {} exceeds source",
                    header.dev_idx
                )));
            }
            entries.insert(header.dev_idx, (source, header, member, length));
        }
        Ok((Self { entries }, selected))
    }

    pub(crate) fn sources(&self) -> impl Iterator<Item = (&dyn BlockRead, &Superblock)> {
        self.entries
            .values()
            .map(|(source, sb, _, _)| (source as &dyn BlockRead, sb))
    }

    pub(crate) fn total_bytes(&self) -> u64 {
        self.entries
            .values()
            .fold(0u64, |total, (_, _, _, length)| {
                total.saturating_add(*length)
            })
    }
}

impl<D: BlockRead> MemberRead for DeviceSet<D> {
    fn resolve(&self, index: u8) -> Result<(&dyn BlockRead, &Member, u64)> {
        self.entries
            .get(&index)
            .map(|(source, _, member, length)| (source as &dyn BlockRead, member, *length))
            .ok_or_else(|| Error::Corrupt(format!("pointer references unavailable device {index}")))
    }
}
