//! Physical runs for the classic ext2/3 direct and three-level indirect map.

use super::ext4_raw::RawInode;
use crate::filesystem::tree::fs_tree::FsDataRun;

pub(super) fn data_runs<F>(
    inode: &RawInode,
    base_offset: u64,
    block_size: u64,
    load_block: &mut F,
) -> anyhow::Result<Vec<FsDataRun>>
where
    F: FnMut(u64) -> anyhow::Result<Vec<u8>>,
{
    if !(1024..=65536).contains(&block_size) || !block_size.is_power_of_two() {
        anyhow::bail!("invalid ext block size {block_size}");
    }
    let pointers = block_size / 4;
    let capacity = 12 + pointers + pointers.pow(2) + pointers.pow(3);
    if inode.size.div_ceil(block_size) > capacity {
        anyhow::bail!("ext inode {} exceeds indirect map capacity", inode.number);
    }
    let mut runs = Vec::new();
    let mut logical_block = 0;
    for (index, pointer) in inode.block.as_chunks::<4>().0.iter().enumerate() {
        let pointer = u32::from_le_bytes(*pointer);
        let depth = index.saturating_sub(11) as u32;
        walk(
            pointer,
            depth,
            inode.size,
            base_offset,
            block_size,
            &mut logical_block,
            load_block,
            &mut runs,
        )?;
        if logical_block >= inode.size.div_ceil(block_size) {
            break;
        }
    }
    Ok(runs)
}

#[allow(clippy::too_many_arguments)]
fn walk<F>(
    pointer: u32,
    depth: u32,
    size: u64,
    base: u64,
    block_size: u64,
    logical: &mut u64,
    load: &mut F,
    runs: &mut Vec<FsDataRun>,
) -> anyhow::Result<()>
where
    F: FnMut(u64) -> anyhow::Result<Vec<u8>>,
{
    let total = size.div_ceil(block_size);
    if *logical >= total {
        return Ok(());
    }
    if pointer == 0 {
        // A missing indirect pointer represents the entire subtree of holes.
        *logical += (block_size / 4).pow(depth).min(total - *logical);
    } else if depth == 0 {
        let physical = base
            .checked_add(u64::from(pointer) * block_size)
            .ok_or_else(|| anyhow::anyhow!("ext block offset overflow"))?;
        let logical_offset = *logical * block_size;
        let length = block_size.min(size - logical_offset);
        if let Some(previous) = runs.last_mut().filter(|previous| {
            previous.logical_offset + previous.length == logical_offset
                && previous.physical_offset + previous.length == physical
        }) {
            previous.length += length;
        } else {
            runs.push(FsDataRun {
                logical_offset,
                physical_offset: physical,
                length,
            });
        }
        *logical += 1;
    } else {
        let bytes = load(u64::from(pointer) * block_size)?;
        if bytes.len() as u64 != block_size {
            anyhow::bail!("truncated ext indirect block {pointer}");
        }
        for entry in bytes.as_chunks::<4>().0 {
            walk(
                u32::from_le_bytes(*entry),
                depth - 1,
                size,
                base,
                block_size,
                logical,
                load,
                runs,
            )?;
            if *logical >= total {
                break;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indirect_runs_preserve_holes_and_merge_adjacent_blocks() {
        let mut bytes = vec![0; 128];
        bytes[0..2].copy_from_slice(&0x8000u16.to_le_bytes());
        bytes[4..8].copy_from_slice(&(15 * 1024u32).to_le_bytes());
        bytes[88..92].copy_from_slice(&20u32.to_le_bytes());
        let inode = RawInode::parse(12, &bytes).unwrap();
        let runs = data_runs(&inode, 4096, 1024, &mut |offset| {
            assert_eq!(offset, 20 * 1024);
            let mut block = vec![0; 1024];
            block[..4].copy_from_slice(&30u32.to_le_bytes());
            block[8..12].copy_from_slice(&31u32.to_le_bytes());
            Ok(block)
        })
        .unwrap();
        assert_eq!(
            runs.into_iter()
                .map(|run| (run.logical_offset, run.physical_offset, run.length))
                .collect::<Vec<_>>(),
            vec![
                (12 * 1024, 4096 + 30 * 1024, 1024),
                (14 * 1024, 4096 + 31 * 1024, 1024)
            ]
        );
    }
}
