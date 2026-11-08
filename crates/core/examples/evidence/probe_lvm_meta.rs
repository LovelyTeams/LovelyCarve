//! Dump every VG metadata snapshot inside an LVM PV partition's mda area.
//!
//! Usage: cargo run --example probe_lvm_meta -- <image-path>
//!
//! Reads the first 4 MiB of every Lvm partition, then finds every `<vg> {`
//! block that contains `physical_volumes` + `logical_volumes`, prints its
//! `seqno` and the names of its `logical_volumes` children. Used to
//! diagnose "only one LV detected" cases where the parser is grabbing the
//! oldest snapshot instead of the active one.

use std::env;

use lovelycarve_core::evidence::image::DiskImage;
use lovelycarve_core::evidence::partitions::PartitionKind;

fn main() -> anyhow::Result<()> {
    let path = env::args()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("usage: probe_lvm_meta <image>"))?;
    let image = DiskImage::open(&path)?;

    for partition in &image.info().partitions {
        if partition.kind != PartitionKind::Lvm {
            continue;
        }
        let off = partition.start_lba * image.sector_size as u64;
        let len = (partition.length_lba * image.sector_size as u64).min(4 * 1024 * 1024) as usize;
        let bytes = image.read_range(off, len)?;
        let text = sanitize(&bytes);

        println!(
            "== partition {} (start_lba={}) ==",
            partition.id, partition.start_lba
        );
        let mut snapshot_idx = 0usize;
        for (name, block) in named_blocks(&text) {
            if !block.contains("physical_volumes") || !block.contains("logical_volumes") {
                continue;
            }
            snapshot_idx += 1;
            let seqno = first_assignment_u64(&block, "seqno");
            let lvs_block = extract_block(&block, "logical_volumes").unwrap_or_default();
            let lv_names: Vec<String> = top_level_named_blocks(&lvs_block)
                .into_iter()
                .map(|(n, _)| n)
                .collect();
            println!(
                "  snapshot #{snapshot_idx}: vg={name} seqno={seqno:?} lvs={}",
                lv_names.join(",")
            );
        }
    }
    Ok(())
}

fn sanitize(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        if b == b'\n' || b == b'\r' || b == b'\t' || (0x20..=0x7e).contains(&b) {
            out.push(b as char);
        } else {
            out.push('\n');
        }
    }
    out
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.' || b == b'-'
}

fn matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (idx, ch) in text[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(open + idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn named_blocks(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let name_start = cursor;
        while cursor < bytes.len() && is_word_char(bytes[cursor]) {
            cursor += 1;
        }
        if cursor == name_start {
            cursor += 1;
            continue;
        }
        let name = text[name_start..cursor].to_string();
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'{' {
            while cursor < bytes.len() && bytes[cursor] != b'{' && bytes[cursor] != b'\n' {
                cursor += 1;
            }
            continue;
        }
        if let Some(close) = matching_brace(text, cursor) {
            out.push((name, text[cursor + 1..close].to_string()));
            cursor = close + 1;
        } else {
            break;
        }
    }
    out
}

fn top_level_named_blocks(text: &str) -> Vec<(String, String)> {
    named_blocks(text)
}

fn extract_block(text: &str, key: &str) -> Option<String> {
    let pos = find_key(text, key)?;
    let open_rel = text[pos..].find('{')?;
    let open = pos + open_rel;
    let close = matching_brace(text, open)?;
    Some(text[open + 1..close].to_string())
}

fn find_key(text: &str, key: &str) -> Option<usize> {
    let mut cursor = 0;
    while cursor < text.len() {
        let pos = text[cursor..].find(key)?;
        let absolute = cursor + pos;
        let prev = absolute
            .checked_sub(1)
            .and_then(|idx| text.as_bytes().get(idx).copied());
        let next = text.as_bytes().get(absolute + key.len()).copied();
        let prev_ok = prev.is_none_or(|b| !is_word_char(b));
        let next_ok = next.is_none_or(|b| !is_word_char(b));
        if prev_ok && next_ok {
            return Some(absolute);
        }
        cursor = absolute + key.len();
    }
    None
}

fn first_assignment_u64(text: &str, key: &str) -> Option<u64> {
    let pos = find_key(text, key)?;
    let after = &text[pos + key.len()..];
    let eq = after.find('=')?;
    let value = after[eq + 1..].trim_start();
    value
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}
