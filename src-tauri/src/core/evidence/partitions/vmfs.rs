//! Probe a VMware LVM member or a standalone VMFS filesystem descriptor.

use super::partition_cursor::SourceCursor;
use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind, SectorSource};
use vmfs_reader::{Error, Geometry, MemberInfo};

pub(crate) struct VmfsProbe;

pub(crate) fn same_datastore(first: &MemberInfo, second: &MemberInfo) -> bool {
    first.volume_uuid == second.volume_uuid && first.snapshot_id == second.snapshot_id
}

/// Datastore identity of an extent member of a datastore spanning several
/// devices: its volume UUID and snapshot ID, as [`same_datastore`] matches
/// them. `None` for a single-extent datastore or a bare filesystem.
pub(crate) fn spanned_datastore(
    source: &dyn SectorSource,
) -> anyhow::Result<Option<([u8; 16], u32)>> {
    Ok(vmfs_reader::probe_member(&mut SourceCursor::new(source))?
        .filter(|member| member.member_count > 1)
        .map(|member| (member.volume_uuid, member.snapshot_id)))
}

impl VolumeProbe for VmfsProbe {
    fn name(&self) -> &'static str {
        "vmfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let mut source = SourceCursor::new(ctx.source);
        let evidence = match vmfs_reader::probe_member(&mut source)? {
            Some(member) => format!(
                "VMware LVM {}, {} extent members, volume {}",
                member.version,
                member.member_count,
                hex::encode(member.volume_uuid)
            ),
            None if ctx.size >= 0x200200 => match Geometry::read(&mut source) {
                Ok(geometry) => format!(
                    "{:?} filesystem descriptor, {}-byte blocks, volume {}",
                    geometry.version,
                    geometry.file_block_size,
                    hex::encode(geometry.uuid)
                ),
                Err(Error::Signature) => return Ok(None),
                Err(error) => return Err(error.into()),
            },
            None => return Ok(None),
        };
        Ok(Some(VolumeProbeHit {
            probe: self.name().into(),
            kind: PartitionKind::Vmfs,
            confidence: 100,
            evidence: vec![evidence],
            relative_offset: 0,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::partitions::{
        detect_partitions, refine_partition_kinds, SectorSource,
    };

    struct Image(Vec<u8>);

    impl SectorSource for Image {
        fn size(&self) -> u64 {
            self.0.len() as u64
        }

        fn read(&self, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
            let start = (offset as usize).min(self.0.len());
            let end = start.saturating_add(length).min(self.0.len());
            Ok(self.0[start..end].to_vec())
        }
    }

    #[test]
    fn vmfs_descriptor_refines_the_partition_at_its_actual_base() {
        let base = 8 * 512;
        let mut bytes = vec![0; base + 0x200200];
        let sectors = ((bytes.len() - base) / 512) as u32;
        let slot = &mut bytes[446..462];
        slot[4] = 0xfb;
        slot[8..12].copy_from_slice(&8u32.to_le_bytes());
        slot[12..16].copy_from_slice(&sectors.to_le_bytes());
        bytes[510..512].copy_from_slice(&[0x55, 0xaa]);
        let header = &mut bytes[base + 0x200000..];
        header[..4].copy_from_slice(&0x2fabf15eu32.to_le_bytes());
        header[4..8].copy_from_slice(&14u32.to_le_bytes());
        header[9..25].fill(1);
        header[161..169].copy_from_slice(&(1u64 << 20).to_le_bytes());
        header[217..221].copy_from_slice(&(1u32 << 13).to_le_bytes());
        let image = Image(bytes);
        let mut parts = detect_partitions(&image, 512).unwrap();
        refine_partition_kinds(&image, &mut parts, 512).unwrap();
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].start_lba, 8);
        assert_eq!(parts[0].kind, PartitionKind::Vmfs);
    }
}
