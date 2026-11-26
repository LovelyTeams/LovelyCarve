//! Probe a VMware LVM member or a standalone VMFS filesystem descriptor.

use std::io::{self, Read, Seek, SeekFrom};

use super::volume_probe::{VolumeProbe, VolumeProbeHit};
use super::{DetectCtx, PartitionKind};
use vmfs_reader::{Error, Geometry, MemberInfo};

pub(crate) struct VmfsProbe;

pub(crate) fn same_datastore(first: &MemberInfo, second: &MemberInfo) -> bool {
    first.volume_uuid == second.volume_uuid && first.snapshot_id == second.snapshot_id
}

impl VolumeProbe for VmfsProbe {
    fn name(&self) -> &'static str {
        "vmfs"
    }

    fn probe(&self, ctx: &DetectCtx<'_>) -> anyhow::Result<Option<VolumeProbeHit>> {
        let mut source = ProbeCursor { ctx, offset: 0 };
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

struct ProbeCursor<'a> {
    ctx: &'a DetectCtx<'a>,
    offset: u64,
}

impl Read for ProbeCursor<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let count = self
            .ctx
            .size
            .saturating_sub(self.offset)
            .min(out.len() as u64) as usize;
        if count == 0 {
            return Ok(0);
        }
        let bytes = self
            .ctx
            .source
            .read(self.offset, count)
            .map_err(io::Error::other)?;
        out[..bytes.len()].copy_from_slice(&bytes);
        self.offset += bytes.len() as u64;
        Ok(bytes.len())
    }
}

impl Seek for ProbeCursor<'_> {
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        let offset = match from {
            SeekFrom::Start(value) => Some(value),
            SeekFrom::End(delta) => self.ctx.size.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.offset.checked_add_signed(delta),
        };
        self.offset =
            offset.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "VMFS probe seek"))?;
        Ok(self.offset)
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
