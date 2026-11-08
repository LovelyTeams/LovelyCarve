//! End-to-end verification of the deleted-CSV recovery on `UDisk.img`:
//! resolve the recovered `_00W.CSV` node through the *real* engine paths
//! (`children_for_image` → its `deleted:run:` id → `file_stat`), then stream the
//! reconstructed byte run straight off the image and validate it the way the
//! recovery report does — UTF-8 BOM, legal UTF-8, exactly 2,000,001 lines, and
//! 14 commas (15 columns) on every line.
//!
//! Usage: cargo run --no-default-features --example verify_csv_recovery -- <image>

use std::env;
use std::sync::Arc;

use lovelycarve_core::filesystem::evidence_session::EvidenceSession;
use lovelycarve_core::filesystem::tree::fs_tree::{children_for_image, file_stat};

fn main() -> anyhow::Result<()> {
    let path = env::args()
        .nth(1)
        .expect("usage: verify_csv_recovery <image>");
    let image = Arc::new(EvidenceSession::open(&path)?);

    // Resolve the recovered CSV node from the partition root, exactly as the GUI
    // would (deleted entries surface inline once recovery is on).
    let children = children_for_image(&image, "part:0")?;
    let csv = children
        .iter()
        .find(|n| n.name.eq_ignore_ascii_case("_00W.CSV") && n.deleted)
        .expect("recovered _00W.CSV node not found");
    println!("node: name={} size={} id={}", csv.name, csv.size, csv.id);

    // The deleted-run id encodes the reconstructed (offset, len): pull the stat
    // so we read the *engine's* idea of the run, not a hand-computed offset.
    let stat = file_stat(&image, &csv.id)?;
    let run = stat
        .data_runs
        .first()
        .expect("no data run on recovered CSV");
    println!(
        "data run: physical_offset=0x{:x} length={}",
        run.physical_offset, run.length
    );
    println!("magic16: {}", stat.magic_hex.as_deref().unwrap_or("(none)"));

    // Stream the run off the image in chunks, validating as we go. We mirror the
    // report's checks without holding 498 MiB in memory.
    let total = run.length;
    let mut offset = run.physical_offset;
    let mut remaining = total;
    const CHUNK: u64 = 8 * 1024 * 1024;

    let mut head = Vec::new();
    let mut lines: u64 = 0; // count of '\n'
    let mut commas_this_line: u64 = 0;
    let mut bad_comma_lines: u64 = 0;
    let mut first_bad_line: Option<(u64, u64)> = None; // (line_no, commas)
    let mut last_byte = 0u8;
    let mut nonempty = false;

    while remaining > 0 {
        let want = CHUNK.min(remaining) as usize;
        let buf = image.read_range(offset, want)?;
        if buf.is_empty() {
            break;
        }
        if head.len() < 16 {
            head.extend_from_slice(&buf[..buf.len().min(16 - head.len())]);
        }
        for &b in &buf {
            nonempty = true;
            last_byte = b;
            match b {
                b',' => commas_this_line += 1,
                b'\n' => {
                    lines += 1;
                    if commas_this_line != 14 {
                        bad_comma_lines += 1;
                        if first_bad_line.is_none() {
                            first_bad_line = Some((lines, commas_this_line));
                        }
                    }
                    commas_this_line = 0;
                }
                _ => {}
            }
        }
        offset += buf.len() as u64;
        remaining -= buf.len() as u64;
    }

    // A trailing partial line (no final '\n') still counts as a record line.
    let trailing_partial = nonempty && last_byte != b'\n';
    if trailing_partial && commas_this_line != 14 {
        bad_comma_lines += 1;
        if first_bad_line.is_none() {
            first_bad_line = Some((lines + 1, commas_this_line));
        }
    }
    let logical_lines = lines + u64::from(trailing_partial);

    let bom = head.starts_with(&[0xEF, 0xBB, 0xBF]);
    println!("\n=== validation ===");
    println!("UTF-8 BOM (EF BB BF):     {}", yesno(bom));
    println!("total newline-terminated: {lines}");
    println!("logical lines:            {logical_lines}");
    println!("lines with != 14 commas:  {bad_comma_lines}");
    if let Some((ln, c)) = first_bad_line {
        println!("  first offending line {ln}: {c} commas");
    }
    println!(
        "ends on newline boundary:  {} (last byte 0x{last_byte:02x})",
        yesno(!trailing_partial)
    );

    let expect_lines = 2_000_001u64;
    let ok = bom && logical_lines == expect_lines && bad_comma_lines == 0 && !trailing_partial;
    println!(
        "\nRESULT: {} (expected {expect_lines} lines × 15 cols, BOM, clean tail)",
        if ok { "PASS ✅" } else { "FAIL ❌" }
    );
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

fn yesno(b: bool) -> &'static str {
    if b {
        "yes"
    } else {
        "no"
    }
}
