//! Translate chunk-tree addresses into bounded source runs.
use super::*;
use crate::core::evidence::partitions::lvm2::layout::LinearLayout;
use crate::core::filesystem::volumes::lvm::read_backing_into;
use anyhow::ensure;

const CHUNK_HEADER: usize = 48;
const STRIPE_SIZE: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ChunkMapping {
    pub(super) logical: u64,
    pub(super) length: u64,
    stripe_len: u64,
    profile: Profile,
    stripes: Vec<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Profile {
    Copies,
    Striped { copies: usize },
    Parity { columns: usize },
}

impl Profile {
    fn parse(flags: u64, count: usize, sub_stripes: usize) -> anyhow::Result<Self> {
        let profile = match (flags & !7, count) {
            (0, 1) | (16 | 32, 2) | (512, 3) | (1024, 4) => Self::Copies,
            (8, 2..) => Self::Striped { copies: 1 },
            (64, 4..) if sub_stripes >= 2 && count.is_multiple_of(sub_stripes) => Self::Striped {
                copies: sub_stripes,
            },
            (128, 2..) => Self::Parity { columns: 1 },
            (256, 3..) => Self::Parity { columns: 2 },
            _ => bail!(Failure::unsupported(format!(
                "btrfs: chunk flags {flags:#x}, {count} stripes and {sub_stripes} copies"
            ))),
        };
        Ok(profile)
    }

    fn data_columns(self, count: usize) -> usize {
        match self {
            Self::Copies => 1,
            Self::Striped { copies } => count / copies,
            Self::Parity { columns } => count - columns,
        }
    }
}

impl ChunkMapping {
    pub(super) fn parse(
        logical: u64,
        data: &[u8],
        devices: &super::devices::DeviceSet,
    ) -> anyhow::Result<(Self, usize)> {
        let (chunk_bytes, stripes) = data
            .split_at_checked(CHUNK_HEADER)
            .context(Failure::corrupt("btrfs: truncated chunk header"))?;
        let chunk = Chunk::ref_from_bytes(chunk_bytes)
            .map_err(|_| anyhow!(Failure::corrupt("btrfs: invalid chunk header")))?;
        let count = usize::from(chunk.num_stripes.get());
        let stripe_bytes = count * STRIPE_SIZE;
        let records = stripes
            .get(..stripe_bytes)
            .context(Failure::corrupt("btrfs: truncated chunk stripe table"))?;
        let length = chunk.length.get();
        let stripe_len = chunk.stripe_len.get();
        ensure!(
            length > 0
                && logical.checked_add(length).is_some()
                && (4096..=65536).contains(&stripe_len)
                && stripe_len.is_power_of_two(),
            Failure::corrupt("btrfs: invalid chunk geometry")
        );
        let profile = Profile::parse(
            chunk.chunk_type.get(),
            count,
            usize::from(chunk.sub_stripes.get()),
        )?;
        let columns = profile.data_columns(count) as u64;
        let bytes_per_stripe = if columns == 1 {
            length
        } else {
            let row = stripe_len * columns;
            ensure!(
                length.is_multiple_of(row),
                Failure::corrupt("btrfs: partial chunk stripe row")
            );
            length / columns
        };
        let mut mapped = Vec::with_capacity(count);
        for record in records.as_chunks::<STRIPE_SIZE>().0 {
            let stripe = Stripe::ref_from_bytes(record)
                .map_err(|_| anyhow!(Failure::corrupt("btrfs: invalid chunk stripe")))?;
            mapped.push(devices.resolve(stripe, bytes_per_stripe)?);
        }
        Ok((
            Self {
                logical,
                length,
                stripe_len,
                profile,
                stripes: mapped,
            },
            CHUNK_HEADER + stripe_bytes,
        ))
    }

    pub(super) fn from_system_array(
        mut bytes: &[u8],
        devices: &super::devices::DeviceSet,
    ) -> anyhow::Result<Vec<Self>> {
        let mut out = Vec::new();
        while !bytes.is_empty() {
            let (key, payload) = bytes
                .split_at_checked(KEY_RECORD_SIZE)
                .context(Failure::corrupt("btrfs: truncated system chunk key"))?;
            let key = Key::ref_from_bytes(key)
                .map_err(|_| anyhow!(Failure::corrupt("btrfs: invalid system chunk key")))?;
            ensure!(
                key.key_type == KEY_CHUNK_ITEM,
                Failure::corrupt("btrfs: unexpected system chunk key")
            );
            let (map, used) = Self::parse(key.offset.get(), payload, devices)?;
            out.push(map);
            bytes = &payload[used..];
        }
        sort_chunks(&mut out)?;
        Ok(out)
    }

    pub(super) fn end(&self) -> u64 {
        self.logical + self.length
    }

    fn run(&self, logical: u64) -> ChunkMappingResult {
        let delta = logical - self.logical;
        let remaining = self.length - delta;
        if self.profile == Profile::Copies {
            return ChunkMappingResult {
                physical: self.stripes[0] + delta,
                length: remaining,
            };
        }
        let count = self.stripes.len();
        let columns = self.profile.data_columns(count) as u64;
        let (unit, within) = (delta / self.stripe_len, delta % self.stripe_len);
        let row = unit / columns;
        let column = (unit % columns) as usize;
        let slot = match self.profile {
            Profile::Striped { copies } => column * copies,
            Profile::Parity { .. } => (row % count as u64) as usize + column,
            Profile::Copies => unreachable!("copies were handled before stripe routing"),
        } % count;
        ChunkMappingResult {
            physical: self.stripes[slot] + row * self.stripe_len + within,
            length: remaining.min(self.stripe_len - within),
        }
    }

    #[cfg(test)]
    pub(super) fn linear(logical: u64, physical: u64, length: u64) -> Self {
        Self {
            logical,
            length,
            stripe_len: 65536,
            profile: Profile::Copies,
            stripes: vec![physical],
        }
    }
}

pub(super) fn sort_chunks(chunks: &mut [ChunkMapping]) -> anyhow::Result<()> {
    chunks.sort_unstable_by_key(|chunk| chunk.logical);
    ensure!(
        chunks
            .windows(2)
            .all(|pair| pair[0].end() <= pair[1].logical),
        Failure::corrupt("btrfs: overlapping chunk mappings")
    );
    Ok(())
}

pub(super) fn map_run(chunks: &[ChunkMapping], logical: u64) -> anyhow::Result<ChunkMappingResult> {
    let index = chunks.partition_point(|chunk| chunk.end() <= logical);
    let chunk = chunks
        .get(index)
        .filter(|chunk| chunk.logical <= logical)
        .with_context(|| {
            Failure::corrupt(format!("btrfs: logical address {logical:#x} is unmapped"))
        })?;
    Ok(chunk.run(logical))
}

pub(super) fn read_logical(
    image: &DiskImage,
    layout: Option<&LinearLayout>,
    chunks: &[ChunkMapping],
    mut logical: u64,
    mut target: &mut [u8],
) -> anyhow::Result<()> {
    logical
        .checked_add(target.len() as u64)
        .context(Failure::corrupt("btrfs: logical read overflow"))?;
    while !target.is_empty() {
        let run = map_run(chunks, logical)?;
        let count = run.length.min(target.len() as u64) as usize;
        let (output, rest) = target.split_at_mut(count);
        let read = read_backing_into(image, layout, run.physical, output)?;
        ensure!(read == count, Failure::corrupt("btrfs: short stripe read"));
        logical += count as u64;
        target = rest;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn btrfs_rotates_raid56_data_columns() {
        let stripes = vec![0x100000, 0x200000, 0x300000, 0x400000];
        let stripe_len = 0x10000;
        for (flags, rows) in [
            (
                128,
                &[
                    [0x100000, 0x200000, 0x300000],
                    [0x210000, 0x310000, 0x410000],
                    [0x320000, 0x420000, 0x120000],
                ][..],
            ),
            (
                256,
                &[
                    [0x100000, 0x200000, 0],
                    [0x210000, 0x310000, 0],
                    [0x320000, 0x420000, 0],
                ][..],
            ),
        ] {
            let profile = Profile::parse(flags, stripes.len(), 1).unwrap();
            let columns = profile.data_columns(stripes.len());
            let mapping = ChunkMapping {
                logical: 0x800000,
                length: stripe_len * (rows.len() * columns) as u64,
                stripe_len,
                profile,
                stripes: stripes.clone(),
            };
            for (row, addresses) in rows.iter().enumerate() {
                for (column, address) in addresses[..columns].iter().enumerate() {
                    let logical = mapping.logical + (row * columns + column) as u64 * stripe_len;
                    let run = mapping.run(logical + 17);
                    assert_eq!(run.physical, address + 17);
                    assert_eq!(run.length, stripe_len - 17);
                }
            }
        }
    }
}
