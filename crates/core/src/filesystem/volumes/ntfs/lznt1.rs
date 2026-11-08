//! Bounded LZNT1 block decoding. Each back-reference stays within its 4 KiB
//! block; output never exceeds the caller's compression-unit budget.
use anyhow::{ensure, Result};

pub(super) fn decompress(input: &[u8], limit: usize) -> Result<Vec<u8>> {
    let mut output = Vec::with_capacity(limit);
    let mut cursor = 0;
    // Cluster-aligned storage can end in a single padding byte. Once the
    // complete unit is decoded, that padding is not another block header.
    while cursor < input.len() && output.len() < limit {
        ensure!(cursor + 2 <= input.len(), "truncated LZNT1 block header");
        let header = u16::from_le_bytes([input[cursor], input[cursor + 1]]);
        cursor += 2;
        if header == 0 {
            break;
        }
        ensure!(header & 0x7000 == 0x3000, "invalid LZNT1 block signature");
        let length = usize::from(header & 0x0fff) + 1;
        let end = cursor + length;
        ensure!(end <= input.len(), "truncated LZNT1 block payload");
        let start = output.len();
        let block_limit = limit.saturating_sub(start).min(4096);
        if header & 0x8000 == 0 {
            ensure!(
                length <= block_limit,
                "LZNT1 raw block exceeds output budget"
            );
            output.extend_from_slice(&input[cursor..end]);
            cursor = end;
            continue;
        }
        while cursor < end {
            let flags = input[cursor];
            cursor += 1;
            for bit in 0..8 {
                if cursor == end {
                    ensure!(flags >> bit == 0, "truncated LZNT1 token group");
                    break;
                }
                if flags & (1 << bit) == 0 {
                    ensure!(
                        output.len() - start < block_limit,
                        "LZNT1 literal exceeds block"
                    );
                    output.push(input[cursor]);
                    cursor += 1;
                } else {
                    ensure!(cursor + 2 <= end, "truncated LZNT1 back-reference");
                    let token = usize::from(u16::from_le_bytes([input[cursor], input[cursor + 1]]));
                    cursor += 2;
                    let mut mask = 0x0fff;
                    let mut shift = 12;
                    let mut position = (output.len() - start).saturating_sub(1);
                    while position >= 16 {
                        mask >>= 1;
                        shift -= 1;
                        position >>= 1;
                    }
                    let distance = (token >> shift) + 1;
                    let count = (token & mask) + 3;
                    ensure!(
                        distance <= output.len() - start,
                        "LZNT1 back-reference precedes block"
                    );
                    ensure!(
                        count <= block_limit.saturating_sub(output.len() - start),
                        "LZNT1 match exceeds block"
                    );
                    for _ in 0..count {
                        output.push(output[output.len() - distance]);
                    }
                }
            }
        }
    }
    Ok(output)
}

#[cfg(test)]
#[path = "lznt1_tests.rs"]
mod tests;
