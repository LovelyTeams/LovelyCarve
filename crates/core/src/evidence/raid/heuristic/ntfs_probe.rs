//! NTFS heuristic probes and MFT plausibility scoring.
use super::*;

// The NTFS counterpart to `Ext4Probe`: a wrong RAID reconstruction corrupts
// the $MFT at stripe boundaries, so the correct layout is the one whose leading
// $MFT records form an unbroken `FILE`-signed, self-numbered sequence. The
// vendored `ntfs` crate is for browsing *valid* volumes and would panic on the
// garbage a wrong candidate produces, so scoring uses this minimal walker
// instead. Every read is bounds-checked via `read_rel`; the worst a wrong
// candidate does is score 0.

/// Minimal read-only NTFS view over a partition slice for candidate scoring.
/// Locates $MFT from the boot sector and reads file records by index. Assumes
/// the $MFT's initial data run is contiguous from its LCN — true for freshly
/// created volumes (the scoring scenario); fragmented $MFTs may score lower but
/// never panic. Fixup is intentionally NOT applied: the self number at record
/// offset 0x2C lives in the first 512-byte sector, which the update-sequence
/// array never rewrites.
pub(super) struct NtfsProbe<'a> {
    pub(super) source: &'a (dyn SectorSource + Send + Sync),
    pub(super) base: u64,
    pub(super) len: u64,
    pub(super) mft_byte: u64,
    pub(super) record_size: u64,
}

impl<'a> NtfsProbe<'a> {
    pub(super) fn open(
        source: &'a (dyn SectorSource + Send + Sync),
        base: u64,
        len: u64,
    ) -> Option<Self> {
        let boot = read_rel(source, base, len, 0, 512)?;
        if boot.get(3..11)? != b"NTFS    " {
            return None;
        }
        let bytes_per_sector = le16(&boot, 0x0B)? as u64;
        let sectors_per_cluster = *boot.get(0x0D)? as u64;
        if bytes_per_sector == 0 || sectors_per_cluster == 0 {
            return None;
        }
        let cluster_size = bytes_per_sector.checked_mul(sectors_per_cluster)?;
        let mft_lcn = u64::from_le_bytes(boot.get(0x30..0x38)?.try_into().ok()?);
        let mft_byte = mft_lcn.checked_mul(cluster_size)?;
        // file-record-size: i8 at 0x40. Negative ⇒ 2^(-v) bytes; positive ⇒
        // that many clusters per record.
        let raw = *boot.get(0x40)? as i8;
        let record_size = if raw < 0 {
            1u64 << ((-(raw as i32)) as u32).min(20)
        } else {
            (raw as u64).checked_mul(cluster_size)?
        };
        if !(512..=65536).contains(&record_size) {
            return None;
        }
        Some(Self {
            source,
            base,
            len,
            mft_byte,
            record_size,
        })
    }

    /// True when MFT record `idx` carries the `FILE` signature and stores its
    /// own record number at offset 0x2C (NTFS 3.1+).
    pub(super) fn record_self_consistent(&self, idx: u64) -> bool {
        let Some(rel) = idx
            .checked_mul(self.record_size)
            .and_then(|o| self.mft_byte.checked_add(o))
        else {
            return false;
        };
        let Some(rec) = read_rel(self.source, self.base, self.len, rel, 0x30) else {
            return false;
        };
        if &rec[0..4] != b"FILE" {
            return false;
        }
        le32(&rec, 0x2C) == Some(idx as u32)
    }
}

/// Score an NTFS partition by how many leading $MFT records are
/// self-consistent (FILE-signed and self-numbered). The correct layout walks
/// the contiguous system + early-file records; a wrong layout breaks the
/// sequence at the first stripe boundary. Returns `(valid, probed)`.
pub(super) fn ntfs_mft_score(
    source: Arc<dyn SectorSource + Send + Sync>,
    base: u64,
    len: u64,
) -> (u32, usize) {
    let Some(probe) = NtfsProbe::open(source.as_ref(), base, len) else {
        return (0, 0);
    };
    let mut valid = 0u32;
    for idx in 0..NTFS_MFT_PROBE_RECORDS {
        if probe.record_self_consistent(idx) {
            valid += 1;
        }
    }
    (valid, NTFS_MFT_PROBE_RECORDS as usize)
}
