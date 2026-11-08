#![allow(clippy::unwrap_used, clippy::expect_used)]
/// Corpus differential tests: bytes from `VhdxReader` must match `qemu-img convert -O raw`.
///
/// These tests skip automatically if qemu-img is not installed. They verify
/// correctness against an independent authoritative reference rather than against
/// the library's own synthetic fixtures (which share the same blind spots).
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::process::Command;
use vhdx::VhdxReader;

/// Resolve a usable `qemu-img`, or `None` (the differential then skips).
///
/// `PATH` is probed first, so any install location works — including ones a
/// fixed list cannot anticipate: another package manager's prefix, or a
/// hand-built install. The absolute
/// candidates are the fallback for a stripped `PATH`: Homebrew on Apple silicon
/// and Intel, then `/usr/bin`, where the Linux CI runner's `qemu-utils` package
/// lands it. `QEMU_IMG_BIN` overrides both.
///
/// The hardcoded Homebrew path this replaces resolved only on a macOS-arm64 dev
/// machine, which is why the doc comment above used to concede these ran "only
/// on machines with QEMU available (the dev machine)".
fn qemu_img_bin() -> Option<String> {
    if let Ok(explicit) = std::env::var("QEMU_IMG_BIN") {
        return usable(&explicit);
    }
    [
        "qemu-img",
        "/opt/homebrew/bin/qemu-img",
        "/usr/local/bin/qemu-img",
        "/usr/bin/qemu-img",
    ]
    .into_iter()
    .find_map(usable)
}

/// A candidate counts only if it actually executes: a successful `--version`
/// proves both that the name resolved and that the binary runs on this host,
/// which a bare `Path::exists()` check does not.
fn usable(candidate: &str) -> Option<String> {
    Command::new(candidate)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|_| candidate.to_string())
}

fn corpus_vhdx_matches_qemu_raw(corpus: &Path) {
    let Some(qemu_img) = qemu_img_bin() else {
        return;
    };
    if !corpus.exists() {
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let raw_path = tmp.path().join("reference.raw");

    let ok = Command::new(&qemu_img)
        .args([
            "convert",
            "-O",
            "raw",
            corpus.to_str().unwrap(),
            raw_path.to_str().unwrap(),
        ])
        .status()
        .expect("spawn qemu-img")
        .success();
    assert!(ok, "qemu-img convert failed for {}", corpus.display());
    let ref_data = std::fs::read(&raw_path).expect("read reference raw");

    let mut reader = VhdxReader::open(corpus).expect("open vhdx");
    let vhdx_size = reader.virtual_disk_size() as usize;
    assert_eq!(
        vhdx_size,
        ref_data.len(),
        "virtual_disk_size must match qemu-img reference raw length for {}",
        corpus.display()
    );

    // Sample every 64 KiB, covering block and sector boundaries, plus near-end.
    let step = 65536usize;
    let mut offset = 0usize;
    while offset < vhdx_size {
        let len = 512.min(vhdx_size - offset);
        let mut buf = vec![0u8; len];
        reader.seek(SeekFrom::Start(offset as u64)).expect("seek");
        reader.read_exact(&mut buf).expect("read");
        assert_eq!(
            buf,
            ref_data[offset..offset + len],
            "byte mismatch at offset {offset:#x} in {}",
            corpus.display()
        );
        offset += step;
    }

    // Near-end check.
    if vhdx_size >= 512 {
        let end = vhdx_size - 512;
        let mut buf = vec![0u8; 512];
        reader
            .seek(SeekFrom::Start(end as u64))
            .expect("seek near-end");
        reader.read_exact(&mut buf).expect("read near-end");
        assert_eq!(
            buf,
            ref_data[end..end + 512],
            "byte mismatch near end of {}",
            corpus.display()
        );
    }
}

#[test]
fn corpus_ext2_vhdx_matches_qemu_raw() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/ext2.vhdx");
    corpus_vhdx_matches_qemu_raw(&p);
}

#[test]
fn corpus_qemu_fixed_vhdx_matches_qemu_raw() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/qemu_fixed.vhdx");
    corpus_vhdx_matches_qemu_raw(&p);
}

#[test]
fn corpus_fat_parent_vhdx_matches_qemu_raw() {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/fat-parent.vhdx");
    corpus_vhdx_matches_qemu_raw(&p);
}
