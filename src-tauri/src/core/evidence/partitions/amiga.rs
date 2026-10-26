//! Native Amiga RDB and OFS/FFS probes. Disk-sector extents stay separate from
//! DosEnvec filesystem-block counts, including unused trailing track sectors.
use super::{
    volume_probe::{VolumeProbe, VolumeProbeHit},
    AmigaGeometry, DetectCtx, PartParser, Partition, PartitionKind, SectorSource, TableScheme,
};
use anyhow::{anyhow, bail, ensure, Context, Result};
use lamfold::FoldError;
use std::collections::HashSet;

struct RdbSource<'a> {
    source: &'a dyn SectorSource,
    block_size: usize,
    remaining: usize,
    fault: Option<String>,
}
impl amiga_rdb::BlockSource for RdbSource<'_> {
    type Error = anyhow::Error;
    fn block_size(&self) -> usize {
        self.block_size
    }
    fn block_count(&self) -> Option<u64> {
        Some(self.source.size() / self.block_size as u64)
    }
    fn read_block(&mut self, lba: u64, bytes: &mut [u8]) -> Result<()> {
        let result = (|| {
            ensure!(self.remaining > 0, "RDB metadata read limit");
            self.remaining -= 1;
            ensure!(bytes.len() == self.block_size, "RDB block size mismatch");
            let offset = lba
                .checked_mul(self.block_size as u64)
                .context("RDB offset overflow")?;
            ensure!(
                offset
                    .checked_add(bytes.len() as u64)
                    .is_some_and(|end| end <= self.source.size()),
                "RDB block outside source"
            );
            let raw = self.source.read(offset, bytes.len())?;
            ensure!(raw.len() == bytes.len(), "RDB short source read");
            bytes.copy_from_slice(&raw);
            Ok(())
        })();
        if let Err(error) = &result {
            self.fault = Some(format!("{error:#}"));
        }
        result
    }
}
pub struct RdbParser;
impl PartParser for RdbParser {
    fn name(&self) -> &'static str {
        "rdb"
    }
    fn scheme(&self) -> TableScheme {
        TableScheme::Rdb
    }
    fn try_parse(&self, context: &DetectCtx<'_>) -> Result<Option<Vec<Partition>>> {
        let mut inspected = HashSet::new();
        let mut valid_size = None;
        let mut saw_magic = false;
        for block_size in [512usize, 1024, 2048, 4096, 8192, 16384, 32768] {
            for block in 0..16u64 {
                let offset = block * block_size as u64;
                if offset + 20 > context.size || !inspected.insert(offset) {
                    continue;
                }
                let prefix =
                    if let Some(prefix) = context.head.get(offset as usize..offset as usize + 20) {
                        prefix.to_vec()
                    } else {
                        context.source.read(offset, 20)?
                    };
                if prefix.len() < 20 {
                    // A short nonmatching signature probe does not claim an
                    // unrelated volume. Once RDSK is present, truncation is
                    // corruption rather than permission to try another parser.
                    ensure!(!prefix.starts_with(b"RDSK"), "RDB short probe read");
                    continue;
                }
                if &prefix[..4] != b"RDSK" {
                    continue;
                }
                let declared = amiga_rdb::be32(&prefix, 16) as usize;
                if !amiga_rdb::block_size_ok(declared)
                    || !offset.is_multiple_of(declared as u64)
                    || offset / declared as u64 >= 16
                    || offset + declared as u64 > context.size
                {
                    continue;
                }
                saw_magic = true;
                let bytes = context.source.read(offset, declared)?;
                if bytes.len() != declared || !amiga_rdb::checksum_ok(&bytes) {
                    continue;
                }
                ensure!(
                    valid_size.is_none_or(|previous| previous == declared),
                    "RDB conflicting device block geometries"
                );
                valid_size = Some(declared);
            }
        }
        let Some(block_size) = valid_size else {
            if saw_magic {
                bail!("RDB header magic present but no valid checksum/geometry");
            }
            return Ok(None);
        };
        let mut source = RdbSource {
            source: context.source,
            block_size,
            remaining: 4096,
            fault: None,
        };
        let parsed = amiga_rdb::Rdb::parse(&mut source);
        if let Some(fault) = source.fault {
            bail!("RDB source read: {fault}");
        }
        let rdb = parsed.map_err(|error| anyhow!("RDB parse: {error}"))?;
        ensure!(
            rdb.partitions.len() <= 256 && rdb.filesystems.len() <= 256,
            "RDB partition/driver count limit"
        );
        ensure!(
            rdb.bad_blocks.is_empty(),
            "RDB BADB remapping is not supported; refusing unmapped source bytes"
        );
        let issues = rdb.validate();
        ensure!(issues.is_empty(), "RDB invalid table: {issues:?}");
        rdb.partitions
            .iter()
            .enumerate()
            .map(|(index, part)| {
                let env = &part.envec_raw;
                ensure!(env.len() > 16, "RDB truncated DosEnvec");
                ensure!(env[2] == 0, "RDB nonzero sector origin is unsupported");
                ensure!(
                    env[1].checked_mul(4) == Some(rdb.block_bytes),
                    "RDB DosEnvec device block size mismatch"
                );
                ensure!(
                    env[3] > 0 && env[4] > 0 && env[5] > 0 && env[10] >= env[9],
                    "RDB invalid partition geometry"
                );
                let cylinder_blocks = u64::from(env[3])
                    .checked_mul(u64::from(env[5]))
                    .context("RDB cylinder overflow")?;
                let cylinders = u64::from(env[10]) - u64::from(env[9]) + 1;
                let start = u64::from(env[9])
                    .checked_mul(cylinder_blocks)
                    .and_then(|value| value.checked_mul(block_size as u64))
                    .context("RDB start overflow")?;
                let length = cylinders
                    .checked_mul(cylinder_blocks)
                    .and_then(|value| value.checked_mul(block_size as u64))
                    .context("RDB length overflow")?;
                ensure!(
                    length > 0
                        && start
                            .checked_add(length)
                            .is_some_and(|end| end <= context.size),
                    "RDB partition outside source"
                );
                ensure!(
                    start.is_multiple_of(context.sector_size as u64)
                        && length.is_multiple_of(context.sector_size as u64),
                    "RDB partition not aligned to source sectors"
                );
                let block_size = rdb
                    .block_bytes
                    .checked_mul(env[4])
                    .context("RDB filesystem block size overflow")?;
                let blocks = cylinders
                    .checked_mul(u64::from(env[3]))
                    .and_then(|value| value.checked_mul(u64::from(env[5] / env[4])))
                    .context("RDB filesystem block count overflow")?;
                let native = is_ofs_ffs(part.dos_type);
                if native {
                    ensure!(
                        blocks <= u64::from(u32::MAX)
                            && block_size.is_power_of_two()
                            && (512..=32768).contains(&block_size)
                            && blocks > 0,
                        "RDB unsupported OFS/FFS block geometry"
                    );
                }
                let amiga_geometry = Some(AmigaGeometry {
                    block_size,
                    blocks,
                    reserved: env[6],
                    dos_type: part.dos_type,
                });
                Ok(Partition {
                    id: u8::try_from(index).context("RDB partition ID overflow")?,
                    kind: if native {
                        PartitionKind::Amiga
                    } else {
                        PartitionKind::Unknown
                    },
                    start_lba: start / context.sector_size as u64,
                    length_lba: length / context.sector_size as u64,
                    label: Some(part.name.clone()),
                    unique_guid: None,
                    amiga_geometry,
                })
            })
            .collect::<Result<Vec<_>>>()
            .map(Some)
    }
}
pub(super) fn is_ofs_ffs(dos_type: u32) -> bool {
    (0x444f5300..=0x444f5307).contains(&dos_type)
}
pub struct AmigaProbe;
struct ProbeSource<'a> {
    source: &'a dyn SectorSource,
    length: u64,
}
impl lamfold::BlockSource for ProbeSource<'_> {
    fn len(&self) -> u64 {
        self.length
    }
    fn read_at(&mut self, offset: u64, bytes: &mut [u8]) -> lamfold::Result<()> {
        if offset
            .checked_add(bytes.len() as u64)
            .is_none_or(|end| end > self.length)
        {
            return Err(FoldError::Corrupt("amiga: probe range"));
        }
        let raw = self
            .source
            .read(offset, bytes.len())
            .map_err(|error| FoldError::Source(format!("{error:#}")))?;
        if raw.len() != bytes.len() {
            return Err(FoldError::Corrupt("amiga: short probe source"));
        }
        bytes.copy_from_slice(&raw);
        Ok(())
    }
}
impl VolumeProbe for AmigaProbe {
    fn name(&self) -> &'static str {
        "amiga"
    }
    fn probe(&self, context: &DetectCtx<'_>) -> Result<Option<VolumeProbeHit>> {
        let Some(magic) = context.head.get(..4) else {
            return Ok(None);
        };
        if !is_ofs_ffs(u32::from_be_bytes([magic[0], magic[1], magic[2], magic[3]])) {
            return Ok(None);
        }
        let mut source = ProbeSource {
            source: context.source,
            length: context.size,
        };
        let geometry = match amiga_reader::Geometry::inspect(&mut source, None) {
            Ok(geometry) => geometry,
            Err(FoldError::Source(error)) => return Err(anyhow!("Amiga probe source: {error}")),
            Err(_) => return Ok(None),
        };
        Ok(Some(VolumeProbeHit {
            probe: "amiga".into(),
            kind: PartitionKind::Amiga,
            confidence: 100,
            relative_offset: 0,
            evidence: vec![format!(
                "Amiga {:?}, {}-byte blocks, root {}, clean={}",
                geometry.variant, geometry.block_size, geometry.root, geometry.clean
            )],
        }))
    }
}

pub(super) fn inspect_rdb_volume(
    source: &dyn SectorSource,
    geometry: AmigaGeometry,
) -> Result<bool> {
    let hints = amiga_reader::Hints {
        block_size: geometry.block_size,
        blocks: geometry.blocks,
        reserved: geometry.reserved,
        dos_type: geometry.dos_type,
    };
    let mut reader = ProbeSource {
        source,
        length: source.size(),
    };
    match amiga_reader::Geometry::inspect(&mut reader, Some(hints)) {
        Ok(_) => Ok(true),
        Err(FoldError::Source(error)) => Err(anyhow!("RDB Amiga volume source: {error}")),
        Err(_) => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::evidence::partitions::test_support::BufSource;
    use std::io::Read;

    fn checksum(bytes: &mut [u8]) {
        let words = amiga_rdb::be32(bytes, 4) as usize;
        bytes[8..12].fill(0);
        let sum = bytes[..words * 4].chunks_exact(4).fold(0u32, |sum, word| {
            sum.wrapping_add(u32::from_be_bytes(word.try_into().unwrap()))
        });
        bytes[8..12].copy_from_slice(&sum.wrapping_neg().to_be_bytes());
    }
    #[test]
    fn amiga_rdb_refuses_bad_checksum_geometry_cycles_and_unmapped_bad_blocks() {
        let mut original = Vec::new();
        flate2::read::ZlibDecoder::new(
            include_bytes!("../../../../vendor/amiga-reader/tests/fixtures/rdb-multiblock.zlib")
                .as_slice(),
        )
        .read_to_end(&mut original)
        .unwrap();
        let part = amiga_rdb::be32(&original, 28) as usize * 512;
        for mutation in 0..4 {
            let mut bytes = original.clone();
            match mutation {
                0 => bytes[8] ^= 1,
                1 => {
                    bytes[part + 144..part + 148].fill(0);
                    checksum(&mut bytes[part..part + 512]);
                }
                2 => {
                    let lba = (part / 512) as u32;
                    bytes[part + 16..part + 20].copy_from_slice(&lba.to_be_bytes());
                    checksum(&mut bytes[part..part + 512]);
                }
                3 => {
                    // A valid BADB containing one mapping must never be ignored.
                    let bad = 100usize;
                    bytes[24..28].copy_from_slice(&(bad as u32).to_be_bytes());
                    checksum(&mut bytes[..512]);
                    let block = &mut bytes[bad * 512..(bad + 1) * 512];
                    block.fill(0);
                    block[..4].copy_from_slice(b"BADB");
                    block[4..8].copy_from_slice(&8u32.to_be_bytes());
                    block[12..16].copy_from_slice(&7u32.to_be_bytes());
                    block[16..20].copy_from_slice(&u32::MAX.to_be_bytes());
                    block[24..28].copy_from_slice(&300u32.to_be_bytes());
                    block[28..32].copy_from_slice(&301u32.to_be_bytes());
                    checksum(block);
                }
                _ => unreachable!(),
            }
            let source = BufSource(bytes);
            let context = DetectCtx::for_source(&source, &source.0[..8192], 512);
            let error = RdbParser.try_parse(&context).unwrap_err();
            if mutation == 3 {
                assert!(format!("{error:#}").contains("BADB"));
            }
        }
    }
}
