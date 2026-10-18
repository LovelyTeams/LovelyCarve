//! RAID1: every present member holds the whole array.

use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::bail;

use super::StripedMember;
use crate::core::evidence::partitions::SectorSource;

/// Mirror reader. Reads come from the lowest present slot; another mirror is
/// used only when that read fails (a truncated or unreadable member), and the
/// first such fallback per mirror is logged.
pub struct MirrorVolume {
    members: Vec<Option<StripedMember>>,
    size_bytes: u64,
    fallback_logged: Vec<AtomicBool>,
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
        })
    }

    pub fn read_at(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        if len == 0 || offset >= self.size_bytes {
            return Ok(Vec::new());
        }
        let take = (self.size_bytes - offset).min(len as u64) as usize;
        let mut failure: Option<(usize, anyhow::Error)> = None;
        for (slot, member) in self.members.iter().enumerate() {
            let Some(member) = member else {
                continue;
            };
            match member.read_exact(offset, take) {
                Ok(bytes) => {
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
                    return Ok(bytes);
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
}

impl SectorSource for MirrorVolume {
    fn read(&self, offset: u64, len: usize) -> anyhow::Result<Vec<u8>> {
        self.read_at(offset, len)
    }
    fn size(&self) -> u64 {
        self.size_bytes
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::core::evidence::raid::source::test_support::{pattern, vec_member, VecMember};

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
