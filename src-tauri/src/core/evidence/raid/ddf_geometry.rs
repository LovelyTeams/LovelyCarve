//! How a DDF configuration record maps to an md level and layout, and which
//! records of one virtual disk can be read together.
use super::ddf::DdfConfig;
use super::source::{ArrayLevel, Raid10Layout, Raid5Layout, Raid6Layout};

/// Primary RAID levels (`prl`).
const PRL_RAID0: u8 = 0x00;
const PRL_RAID1: u8 = 0x01;
const PRL_RAID3: u8 = 0x03;
const PRL_RAID4: u8 = 0x04;
const PRL_RAID5: u8 = 0x05;
const PRL_RAID6: u8 = 0x06;
const PRL_JBOD: u8 = 0x0F;
const PRL_RAID1E: u8 = 0x11;
const PRL_RAID5E: u8 = 0x15;
const PRL_CONCAT: u8 = 0x1F;
const PRL_RAID5EE: u8 = 0x25;
/// Secondary RAID levels (`srl`) md reads as RAID10.
const SRL_STRIPED: u8 = 0x00;
const SRL_SPANNED: u8 = 0x03;

/// md level of a virtual disk's primary configuration record.
pub(super) fn ddf_level(config: &DdfConfig) -> Result<ArrayLevel, String> {
    let prim = config.prim_elmnt_count;
    let unsupported = || {
        format!(
            "DDF level {:#04x} qualifier {:#04x} over {prim} disk(s) is not supported",
            config.prl, config.rlq
        )
    };
    if config.sec_elmnt_count > 1 {
        // Mirrored BVDs striped together: md RAID10 with one near copy per
        // BVD member.
        let near = match (config.prl, config.rlq, prim) {
            (PRL_RAID1, 0x00, 2) => 0x102,
            (PRL_RAID1, 0x01, 3) => 0x103,
            _ => {
                return Err(format!(
                    "DDF secondary level {:#04x} over level {:#04x} with {prim} disk(s) per BVD is not supported",
                    config.srl, config.prl
                ))
            }
        };
        if !matches!(config.srl, SRL_STRIPED | SRL_SPANNED) {
            return Err(format!(
                "DDF secondary level {:#04x} is not supported",
                config.srl
            ));
        }
        return Raid10Layout::from_raw(near)
            .map(ArrayLevel::Raid10)
            .ok_or_else(unsupported);
    }
    let level = match (config.prl, config.rlq) {
        (PRL_RAID0, 0x00) => ArrayLevel::Raid0,
        (PRL_RAID1, 0x00) if prim == 2 => ArrayLevel::Raid1,
        (PRL_RAID1, 0x01) if prim == 3 => ArrayLevel::Raid1,
        // Parity in the last (0x01) or first (0x00) extent.
        (PRL_RAID4, 0x01) => ArrayLevel::Raid4,
        (PRL_RAID4, 0x00) => ArrayLevel::Raid5(Raid5Layout::Parity0),
        // Rotating parity from extent 0 or N, data restarting or continuing.
        (PRL_RAID5, 0x00) => ArrayLevel::Raid5(Raid5Layout::RightAsymmetric),
        (PRL_RAID5, 0x02) => ArrayLevel::Raid5(Raid5Layout::LeftAsymmetric),
        (PRL_RAID5, 0x03) => ArrayLevel::Raid5(Raid5Layout::LeftSymmetric),
        (PRL_RAID6, 0x01) => ArrayLevel::Raid6(Raid6Layout::RotatingZeroRestart),
        (PRL_RAID6, 0x02) => ArrayLevel::Raid6(Raid6Layout::RotatingNRestart),
        (PRL_RAID6, 0x03) => ArrayLevel::Raid6(Raid6Layout::RotatingNContinue),
        // Integrated adjacent stripe mirroring is md's near-2 over any count.
        (PRL_RAID1E, 0x00) => Raid10Layout::from_raw(0x102)
            .map(ArrayLevel::Raid10)
            .ok_or_else(unsupported)?,
        (PRL_CONCAT, _) => ArrayLevel::Linear,
        _ => return Err(unsupported()),
    };
    if usize::from(prim) < level.min_members() {
        return Err(unsupported());
    }
    Ok(level)
}

/// md-style level number shown for a virtual disk whose layout cannot be
/// read: the RAID family of its primary level, ×10 for a secondary level.
pub(super) fn display_level(config: &DdfConfig) -> i32 {
    let family = match config.prl {
        PRL_RAID0 => 0,
        PRL_RAID1 => 1,
        PRL_RAID3 => 3,
        PRL_RAID4 => 4,
        PRL_RAID5 | PRL_RAID5E | PRL_RAID5EE => 5,
        PRL_RAID6 => 6,
        PRL_RAID1E => 10,
        PRL_CONCAT | PRL_JBOD => -1,
        other => i32::from(other),
    };
    if config.sec_elmnt_count > 1 && family >= 0 {
        family * 10
    } else {
        family
    }
}

/// Why a BVD record cannot be combined with the primary one, if it cannot.
pub(super) fn bvd_mismatch(primary: &DdfConfig, bvd: &DdfConfig, sec_count: u8) -> Option<String> {
    if bvd.sec_elmnt_seq >= sec_count {
        return Some(format!(
            "BVD {} lies outside its {sec_count} secondary elements",
            bvd.sec_elmnt_seq
        ));
    }
    let consistent = bvd.prl == primary.prl
        && bvd.rlq == primary.rlq
        && bvd.srl == primary.srl
        && bvd.sec_elmnt_count == primary.sec_elmnt_count
        && bvd.prim_elmnt_count == primary.prim_elmnt_count
        && bvd.chunk_shift == primary.chunk_shift
        && bvd.blocks == primary.blocks
        && bvd.array_blocks == primary.array_blocks;
    (!consistent).then(|| "its BVD records disagree on level, size or strip".to_string())
}

/// Volume size the metadata records. For a virtual disk with a secondary
/// level the field may hold either the whole size or one BVD's; the latter
/// (equal to the BVD's extent) is not a limit.
pub(super) fn recorded_bytes(primary: &DdfConfig, sec_count: u8) -> Option<u64> {
    if primary.array_blocks == 0 || (sec_count > 1 && primary.array_blocks == primary.blocks) {
        return None;
    }
    Some(primary.array_blocks.saturating_mul(512))
}
