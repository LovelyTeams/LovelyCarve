//! RAID1: every present member holds the whole array.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::bail;

use super::traced::{RepairLog, TracedBytes};
use super::StripedMember;
use crate::core::evidence::image::BadRegion;
use crate::core::evidence::partitions::SectorSource;

/// Mirror reader. Reads come from the lowest present slot; another mirror is
/// used only when that read fails (a truncated or unreadable member), and the
/// first such fallback per mirror is logged. Bytes the serving mirror
/// synthesized are taken from the other mirrors where they hold them
/// authentically.
pub struct MirrorVolume {
    members: Vec<Option<StripedMember>>,
    size_bytes: u64,
    fallback_logged: Vec<AtomicBool>,
    repair_log: RepairLog,
}

impl MirrorVolume {
    /// `size_bytes` is the array size (md's per-device `size`). At least one
    /// present member must declare that much data; a shorter mirror only
    /// serves the reads it can satisfy.
    pub fn new(members: Vec<Option<StripedMember>>, size_bytes: u64) -> anyhow::Result<Self> {
        if members.iter().all(Option::is_none) {
            bail!("RAID1 needs at least one present mirror");
        }
        if !members
            .iter()
            .flatten()
            .any(|member| member.data_size_bytes >= size_bytes)
        {
            bail!("no RAID1 mirror holds the array's {size_bytes} bytes");
        }
        let fallback_logged = members.iter().map(|_| AtomicBool::new(false)).collect();
        Ok(Self {
            members,
            size_bytes,
            fallback_logged,
            repair_log: RepairLog::default(),
        })
    }

    pub fn read_at(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_traced_at(offset, len).map(|(bytes, _)| bytes)
    }

    /// [`Self::read_at`] plus the spans no mirror holds authentically.
    pub fn read_traced_at(
        &self,
        offset: u64,
        len: usize,
    ) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        if len == 0 || offset >= self.size_bytes {
            return Ok((Vec::new(), Vec::new()));
        }
        let take = (self.size_bytes - offset).min(len as u64) as usize;
        let mut failure: Option<(usize, anyhow::Error)> = None;
        for (slot, member) in self.members.iter().enumerate() {
            let Some(member) = member else {
                continue;
            };
            match member.read_exact_traced(offset, take) {
                Ok(mut piece) => {
                    if let Some((failed_slot, error)) = &failure {
                        let first = self
                            .fallback_logged
                            .get(*failed_slot)
                            .is_some_and(|logged| !logged.swap(true, Ordering::Relaxed));
                        if first {
                            tracing::warn!(
                                failed_slot,
                                slot,
                                offset,
                                "RAID1 mirror read failed ({error:#}); served from another mirror"
                            );
                        }
                    }
                    if !piece.is_authentic() {
                        self.repair(slot, offset, &mut piece);
                    }
                    return Ok(piece.into_read(offset));
                }
                Err(error) => {
                    if failure.is_none() {
                        failure = Some((slot, error));
                    }
                }
            }
        }
        match failure {
            Some((slot, error)) => Err(error.context(format!(
                "every RAID1 mirror failed at offset {offset} (first failure in slot {slot})"
            ))),
            None => bail!("RAID1 has no present mirror"),
        }
    }

    /// Take the bytes `slot` synthesized in `piece` (read at array `offset`)
    /// from the other mirrors, in slot order.
    fn repair(&self, slot: usize, offset: u64, piece: &mut TracedBytes) {
        let mut repaired = 0;
        for (other, member) in self.members.iter().enumerate() {
            if piece.is_authentic() {
                break;
            }
            let Some(member) = member.as_ref().filter(|_| other != slot) else {
                continue;
            };
            repaired += piece.repair_from(|range| {
                member.read_exact_traced(offset + range.start as u64, range.len())
            });
        }
        if repaired > 0 {
            self.repair_log.note("RAID1", slot, offset, repaired);
        }
    }
}

impl SectorSource for MirrorVolume {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_at(offset, len)
    }
    fn size(&self) -> u64 {
        self.size_bytes
    }
    fn read_traced(&self, offset: u64, len: usize) -> anyhow::Result<(Vec<u8>, Vec<BadRegion>)> {
        self.read_traced_at(offset, len)
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;
    use std::sync::Arc;

    use super::*;
    use crate::core::evidence::raid::source::test_support::{
        assert_audited, faulty_member, pattern, regions_where, vec_member, Lcg, VecMember,
    };

    #[test]
    fn synthesized_bytes_come_from_another_mirror() {
        let data = pattern(64, 4);
        let mirror = MirrorVolume::new(
            vec![
                Some(faulty_member(data.clone(), &[8..16])),
                None,
                Some(faulty_member(data.clone(), &[12..20, 40..48])),
            ],
            64,
        )
        .expect("mirror");
        let (bytes, faults) = mirror.read_traced_at(0, 64).expect("read");
        // Only 12..16 is unreadable on both present mirrors.
        assert_eq!(faults, vec![BadRegion { offset: 12, len: 4 }]);
        assert_audited(&bytes, &faults, &data, "RAID1");
        assert_eq!(mirror.read_at(0, 64).expect("plain read"), bytes);

        let (_, faults) = mirror.read_traced_at(14, 10).expect("partial read");
        assert_eq!(faults, vec![BadRegion { offset: 14, len: 2 }]);
    }

    #[test]
    fn a_byte_is_synthesized_only_where_every_mirror_is() {
        let mut rng = Lcg(7);
        for case in 0..200u8 {
            let data = pattern(96, case);
            let mirrors = 2 + rng.below(2) as usize;
            let bad: Vec<Vec<Range<u64>>> = (0..mirrors).map(|_| rng.spans(96, 3, 40)).collect();
            let members = bad
                .iter()
                .map(|spans| Some(faulty_member(data.clone(), spans)))
                .collect();
            let mirror = MirrorVolume::new(members, 96).expect("mirror");
            let (bytes, faults) = mirror.read_traced_at(0, 96).expect("read");
            let expected = regions_where(96, |index| {
                bad.iter()
                    .all(|spans| spans.iter().any(|span| span.contains(&(index as u64))))
            });
            assert_eq!(faults, expected, "case {case}: {bad:?}");
            assert_audited(&bytes, &faults, &data, &format!("case {case}"));
        }
    }

    #[test]
    fn reads_the_first_present_mirror() {
        let data = pattern(64, 4);
        let mut stale = data.clone();
        stale.fill(0xEE);
        let mirror = MirrorVolume::new(
            vec![
                None,
                Some(vec_member(data.clone())),
                Some(vec_member(stale)),
            ],
            64,
        )
        .expect("mirror");
        assert_eq!(mirror.read_at(0, 128).expect("read"), data);
        assert_eq!(mirror.read_at(60, 8).expect("tail"), data[60..]);
    }

    #[test]
    fn falls_back_when_a_mirror_is_truncated() {
        let data = pattern(64, 6);
        let truncated = StripedMember {
            reader: Arc::new(VecMember(data[..16].to_vec())),
            data_offset_bytes: 0,
            data_size_bytes: 64,
        };
        let mirror = MirrorVolume::new(vec![Some(truncated), Some(vec_member(data.clone()))], 64)
            .expect("mirror");
        assert_eq!(mirror.read_at(0, 64).expect("fallback"), data);
        assert_eq!(mirror.read_at(32, 32).expect("fallback again"), data[32..]);
    }

    #[test]
    fn rejects_mirrors_smaller_than_the_array() {
        assert!(MirrorVolume::new(vec![Some(vec_member(vec![0; 32]))], 64).is_err());
        assert!(MirrorVolume::new(vec![None, None], 64).is_err());
    }
}
